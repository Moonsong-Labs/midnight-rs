//! A deploy that a wallet with no Dust builds, and that a sponsor pays for,
//! applies on chain.
//!
//! The deploy proves with no fee, and the sponsor merges a fee intent of its
//! own. Only the node can say that it accepts a transaction of that shape.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics. The
//! sponsor pays from the dev seed, so the test has a binary of its own. Two
//! tests that submit from one seed in one binary can select the same Dust.

mod common;

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use midnight_onchain_runtime::state::EntryPointBuf;
use midnight_provider::MidnightProvider;

/// A seed that no other run uses, so its wallet holds nothing.
fn unused_seed() -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{nonce:064x}")
}

/// The deployer's wallet holds no Dust. The chain must apply the sponsored
/// deploy, and the contract at the returned address must hold the verifier
/// key of the counter's circuit, so a later call can prove against it.
#[tokio::test]
async fn a_wallet_with_no_dust_deploys_and_a_sponsor_pays_the_fee() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let deployer = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&deployer, &indexer_url, &unused_seed()).await;
    let deployer = deployer.with_wallet(wallet);
    assert_eq!(
        deployer
            .balance()
            .await
            .expect("balance")
            .dust
            .balance_speck,
        0,
        "the deployer must hold no Dust"
    );

    let sponsor = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&sponsor, &indexer_url, common::DEV_WALLET_SEED).await;
    let sponsor = sponsor.with_wallet(wallet);

    let dustless = counter::Contract::deploy(&deployer)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .without_dust()
        .await
        .expect("build the deploy with no Dust fee");

    let funded = sponsor
        .balance_transaction(dustless.transaction.as_bytes())
        .await
        .expect("the sponsor pays the deploy's fee");
    let (in_block, _) = sponsor
        .submit_reserved(funded)
        .await
        .expect("submit the sponsored deploy")
        .wait_finalized()
        .await
        .expect("the chain must apply the sponsored deploy");

    let address = midnight_contract::format_contract_address(&dustless.address);
    let state =
        midnight_contract::state::fetch_state(&deployer, &address, Some(in_block.block_hash))
            .await
            .expect("a contract at the address that the deploy returned");
    let increment: EntryPointBuf = b"increment"[..].into();
    assert!(
        state.operations.contains_key(&increment),
        "the deployed contract must hold the verifier key of increment"
    );
}
