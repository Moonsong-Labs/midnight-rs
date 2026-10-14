//! `balance_transaction` on a **bare** fee-less contract call.
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

use midnight_provider::{DustlessBuilder, MidnightProvider};

#[tokio::test]
async fn balancing_a_bare_contract_call_is_accepted_on_chain() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(common::compiled_dir("counter"))
        .await
        .expect("deploy");

    let round_before = contract
        .ledger()
        .await
        .expect("ledger")
        .round()
        .expect("round");

    // The bare case: one fee-less call, nothing merged into it.
    let dustless = contract
        .circuits()
        .increment()
        .without_dust()
        .await
        .expect("build fee-less call");

    let funded = provider
        .balance_transaction(dustless.as_bytes())
        .await
        .expect("balance the bare call");

    let pending = provider
        .submit(&funded)
        .await
        .expect("submit balanced call");
    let (_, pending) = pending
        .wait_best()
        .await
        .expect("the node must accept the call, and its fallible phase must succeed");

    // The wallet reads finalized state, so this spend stays invisible to any
    // other wallet on this seed until it finalizes. The next test in the suite
    // is another process on the same seed, and it would draw the same Dust.
    pending.wait_finalized().await.expect("finalized");

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
