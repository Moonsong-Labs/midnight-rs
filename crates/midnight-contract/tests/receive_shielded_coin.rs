//! Live devnet E2E for a circuit call that spends the caller's own shielded
//! coin.
//!
//! The `deposit` circuit of the `receive-shielded` devnet contract does
//! `receiveShielded(coin)`. The call builds its `ShieldedCoinInfo` argument
//! from one wallet coin and attaches that coin with `with_shielded_inputs`.
//! The SDK then spends that exact coin to fund the receive. The circuit
//! receives less than the coin holds, so the wallet must get the rest back as
//! change.
//!
//! The test mints its coin of a fresh token to the dev wallet first. The
//! genesis coins of the dev wallet stay for the other tests and the examples.
//!
//! Gated on `MIDNIGHT_NODE_URL` / `MIDNIGHT_INDEXER_URL` (a running devnet +
//! indexer).

mod common;

mod receive_shielded {
    compact_bindgen::contract!(
        "../../devnet/contracts/receive-shielded/compiled/compiler/analyzed-ir.sexp"
    );
}

mod shielded_mint {
    compact_bindgen::contract!(
        "../../devnet/contracts/shielded-mint/compiled/compiler/analyzed-ir.sexp"
    );
}

use compact_bindgen::Bytes;
use midnight_contract::ExtrinsicHash;
use midnight_provider::MidnightProvider;

#[tokio::test]
async fn a_call_spends_the_callers_coin_and_returns_the_change() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);
    let (own_coin_public_key, _) = provider
        .shielded_public_keys()
        .await
        .expect("shielded keys of the dev wallet");

    // A fresh mint contract mints a fresh token: `tokenType(domain_sep, contract_addr)`.
    let mint_contract = shielded_mint::Contract::deploy(&provider)
        .with_initial_state(shielded_mint::LedgerInitialState)
        .with_zk_config(shielded_mint::ZK_CONFIG_DIR)
        .await
        .expect("deploy mint contract");
    let domain_sep = [0x11u8; 32];
    let minted: u64 = 1000;
    mint_contract
        .circuits()
        .mint(
            Bytes(domain_sep),
            minted,
            Bytes([0x55u8; 32]),
            own_coin_public_key.into(),
        )
        .await
        .expect("mint to the dev wallet");
    let token_type = midnight_contract::parse_address(mint_contract.address())
        .expect("contract address")
        .custom_shielded_token_type(midnight_base_crypto::hash::HashOutput(domain_sep));
    let coin = common::resync_until_coin(
        &provider,
        token_type,
        u128::from(minted),
        "the coin it minted to itself",
    )
    .await;

    let receiver = receive_shielded::Contract::deploy(&provider)
        .with_initial_state(receive_shielded::LedgerInitialState)
        .with_zk_config(receive_shielded::ZK_CONFIG_DIR)
        .await
        .expect("deploy receive-shielded contract");
    let received: u128 = 400;
    let expected_change = coin.value - received;
    let deposited = receive_shielded::ShieldedCoinInfo {
        nonce: Bytes(coin.nonce),
        color: Bytes(coin.token_type.0.0),
        value: received,
    };
    let outcome = receiver
        .circuits()
        .with_shielded_inputs([coin])
        .deposit(deposited)
        .await
        .expect("a deposit that spends the caller's own coin");

    assert_ne!(
        outcome.extrinsic_hash,
        ExtrinsicHash::zero(),
        "a successful call must expose its extrinsic hash"
    );
    assert!(
        provider
            .get_block_header(outcome.block_hash)
            .await
            .expect("header of the call's block")
            .is_some(),
        "a successful call must name the block it landed in"
    );

    common::resync_until_coin(
        &provider,
        token_type,
        expected_change,
        "the change of the deposit",
    )
    .await;
}
