//! `TransactionRequest::sponsor` on a **bare** fee-less contract call.
//!
//! The documented two-party flow is "party 1 builds fee-less, party 2 pays the
//! fees". When party 1 contributes only a contract call, with nothing merged
//! alongside it, the balanced transaction used to be rejected by the node with
//! `NotNormalized`: no Dust was drawn, yet an empty `DustActions` intent was
//! attached anyway, and the ledger treats that as non-canonical.
//!
//! `examples/combine-and-sponsor` never caught this because it merges the call
//! with a transfer before balancing.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

mod common;

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use midnight_provider::{MidnightProvider, TransactionRequest};

#[tokio::test]
async fn balancing_a_bare_contract_call_is_accepted_on_chain() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .send()
        .await
        .expect("submit deploy")
        .into_contract()
        .await
        .expect("deploy");

    let round_before = contract
        .ledger()
        .await
        .expect("ledger")
        .round()
        .expect("round");

    // The bare case: one fee-less call, nothing merged into it.
    let fee_less = contract
        .circuits()
        .increment()
        .without_fee()
        .await
        .expect("build fee-less call");

    // The wallet reads finalized state, so the receipt also keeps this spend
    // from the next test in the suite, another process on the same seed,
    // which would draw the same Dust.
    provider
        .send_transaction(TransactionRequest::sponsor(fee_less.into_bytes()))
        .await
        .expect("submit balanced call")
        .get_receipt()
        .await
        .expect("the node must accept the call, and its fallible phase must succeed");

    // The call has to have actually run, not merely landed.
    let round_after = contract
        .ledger()
        .await
        .expect("ledger")
        .round()
        .expect("round");
    assert_eq!(
        round_after,
        round_before + 1,
        "the balanced call should have advanced the counter"
    );
}
