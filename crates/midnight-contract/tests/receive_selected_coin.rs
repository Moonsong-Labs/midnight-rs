//! Live devnet E2E for a circuit call that receives a shielded coin and names
//! no coin to fund it.
//!
//! The `deposit` circuit of the `receive-shielded` devnet contract does
//! `receiveShielded(coin)`. The call attaches no coin with
//! `with_shielded_inputs`, so the wallet selects one of the token to cover the
//! receive. The circuit receives less than the coin holds, so the wallet must
//! get the rest back as change.
//!
//! The test mints its coin of a fresh token to the dev wallet first, so that
//! coin is the only one the wallet can select. The genesis coins of the dev
//! wallet stay for the other tests and the examples.
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
use midnight_provider::MidnightProvider;

#[tokio::test]
async fn a_call_with_no_pinned_coin_spends_a_selected_one_and_returns_the_change() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);
    let own_coin_public_key = provider.wallet().shielded_recipient().coin_public_key;

    // A fresh mint contract mints a fresh token: `tokenType(domain_sep, contract_addr)`.
    let mint_contract = shielded_mint::Contract::deploy(&provider)
        .with_initial_state(shielded_mint::LedgerInitialState)
        .with_zk_config(shielded_mint::ZK_CONFIG_DIR)
        .send()
        .await
        .expect("submit deploy")
        .into_contract()
        .await
        .expect("deploy mint contract");
    let domain_sep = [0x22u8; 32];
    let minted: u64 = 1000;
    mint_contract
        .circuits()
        .mint(
            Bytes(domain_sep),
            minted,
            Bytes([0x66u8; 32]),
            own_coin_public_key.into(),
        )
        .send()
        .await
        .expect("submit mint")
        .get_receipt()
        .await
        .expect("mint to the dev wallet");
    let token_type = midnight_contract::parse_contract_address(mint_contract.address())
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
        .send()
        .await
        .expect("submit deploy")
        .into_contract()
        .await
        .expect("deploy receive-shielded contract");
    let received: u128 = 400;
    let deposited = receive_shielded::ShieldedCoinInfo {
        nonce: Bytes([0x77u8; 32]),
        color: Bytes(token_type.0.0),
        value: received,
    };
    receiver
        .circuits()
        .deposit(deposited)
        .send()
        .await
        .expect("submit deposit")
        .get_receipt()
        .await
        .expect("a deposit that the wallet funds with a coin it selects");

    common::resync_until_coin(
        &provider,
        token_type,
        coin.value - received,
        "the change of the deposit",
    )
    .await;
}
