//! Live devnet E2E for a payer that completes another party's one-sided
//! offer with `balance_transaction_with_tokens`.
//!
//! The maker holds a fresh token Y and no Dust. It builds an unbalanced swap
//! half that gives Y and asks for the devnet's default shielded token X, and
//! hands over the bytes. No mirror half exists. The payer, the genesis
//! wallet, covers the X deficit from its own coins, takes the Y surplus, pays
//! the fee and submits.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

mod common;

mod shielded_mint {
    compact_bindgen::contract!(
        "../../devnet/contracts/shielded-mint/compiled/compiler/analyzed-ir.sexp"
    );
}

use std::collections::BTreeMap;

use compact_bindgen::Bytes;
use midnight_base_crypto::hash::HashOutput;
use midnight_contract::ShieldedTokenType;
use midnight_provider::{MidnightProvider, TokenType, imbalances};
use midnight_wallet::Seed;

/// A seed that nothing else in the suite uses, so the maker holds only what
/// this test pays it.
const MAKER_SEED: &str = "00000000000000000000000000000000000000000000000000000000000000b1";

/// The devnet's default shielded token, which the genesis wallet holds.
const TOKEN_X: ShieldedTokenType = ShieldedTokenType(HashOutput([0; 32]));

/// The Y minted to the maker, more than it gives so the half carries change.
const MINT_Y: u64 = 1000;
/// The maker asks for `DX` of X and gives `DY` of Y. Values that no other
/// coin of the maker or of the payer carries.
const DX: u128 = 7;
const DY: u128 = 5;

#[tokio::test]
async fn a_payer_completes_a_one_sided_offer_and_takes_what_it_gives() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let payer = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&payer, &indexer_url, common::DEV_WALLET_SEED).await;
    let payer = payer.with_wallet(wallet);

    // --- Setup: mint a fresh token Y to the maker, with its encryption key so
    //     its sync finds the coin ---
    let maker_keys = Seed::from_hex(MAKER_SEED).unwrap().shielded_recipient();
    let mint = shielded_mint::Contract::deploy(&payer)
        .with_initial_state(shielded_mint::LedgerInitialState)
        .with_zk_config(shielded_mint::ZK_CONFIG_DIR)
        .await
        .expect("deploy mint contract");
    let domain_sep = [0x55u8; 32];
    mint.circuits()
        .with_coin_encryption_keys([maker_keys])
        .mint(
            Bytes(domain_sep),
            MINT_Y,
            Bytes([0x66u8; 32]),
            maker_keys.coin_public_key.into(),
        )
        .await
        .expect("mint Y to the maker");
    let token_y = midnight_contract::parse_contract_address(mint.address())
        .expect("contract address")
        .custom_shielded_token_type(HashOutput(domain_sep));

    let maker = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&maker, &indexer_url, MAKER_SEED).await;
    let maker = maker.with_wallet(wallet);
    common::resync_until_coin(&maker, token_y, u128::from(MINT_Y), "the minted Y").await;

    // --- The maker's one-sided offer: give DY of Y, ask DX of X ---
    let offer = maker
        .shielded_swap(token_y, DY, TOKEN_X, DX)
        .await
        .expect("build the one-sided offer");

    // --- The payer checks the terms, then completes it ---
    assert_eq!(
        imbalances(offer.as_bytes()).expect("read the offer's imbalance"),
        BTreeMap::from([
            ((TokenType::Shielded(TOKEN_X), 0), -(DX as i128)),
            ((TokenType::Shielded(token_y), 0), DY as i128),
        ]),
        "the offer must ask for DX of X and give DY of Y at segment 0"
    );
    let complete = payer
        .balance_transaction_with_tokens(offer.as_bytes())
        .await
        .expect("complete the offer");

    // The chain refuses only a deficit. A surplus burns, so a payer that
    // dropped its change or left the Y unclaimed would still land.
    let unbalanced: Vec<_> = imbalances(&complete.tx_bytes)
        .expect("read the completed imbalance")
        .into_iter()
        .filter(|((token, _), value)| matches!(token, TokenType::Shielded(_)) && *value != 0)
        .collect();
    assert!(
        unbalanced.is_empty(),
        "the payer must cover the X deficit, take its change back and take the Y surplus: {unbalanced:?}"
    );
    assert!(
        !complete.spent.shielded.is_empty() && !complete.spent.dust.is_empty(),
        "the result must name the payer's X coins and fee Dust, or nothing can hand them back: {:?}",
        complete.spent
    );

    let pending = payer
        .submit_reserved(complete)
        .await
        .expect("submit the completed offer");
    let (_, pending) = pending
        .wait_best()
        .await
        .expect("the node must accept the completed offer");
    // The next test in the suite is another process on the payer's seed, and
    // its wallet reads finalized state, so it would draw the same Dust.
    pending.wait_finalized().await.expect("finalized");

    common::resync_until_coin(&payer, token_y, DY, "the Y the offer gives").await;
    common::resync_until_coin(&maker, TOKEN_X, DX, "the X the offer asks for").await;
}
