//! Two fee-less contract calls, merged into one transaction that one sponsor
//! pays for.
//!
//! The calls go to two contracts, as `MidnightProvider::merge_transactions`
//! advises.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

mod common;

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use midnight_provider::{DustlessBuilder, MidnightProvider};

#[tokio::test]
async fn two_dustless_calls_merge_into_one_sponsored_transaction() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let mut counters = Vec::new();
    for _ in 0..2 {
        let counter = counter::Contract::deploy(&provider)
            .with_initial_state(counter::LedgerInitialState::default())
            .with_zk_config(counter::ZK_CONFIG_DIR)
            .await
            .expect("deploy a counter");
        let round = counter
            .ledger()
            .await
            .expect("ledger")
            .round()
            .expect("round");
        counters.push((counter, round));
    }

    let mut calls = Vec::new();
    for (counter, _) in &counters {
        let call = counter
            .circuits()
            .increment()
            .without_dust()
            .await
            .expect("build a fee-less call");
        calls.push(call.into_bytes());
    }

    let merged = provider
        .merge_transactions(&calls)
        .expect("two calls must merge: each call's intent needs its own segment");
    let funded = provider
        .balance_transaction(&merged)
        .await
        .expect("balance the merged calls");
    let pending = provider
        .submit_reserved(funded)
        .await
        .expect("submit the merged calls");
    let (_, pending) = pending
        .wait_best()
        .await
        .expect("the node must accept the merged calls, and both fallible phases must succeed");

    // Wait for finality: the next test binary syncs the same seed, and until
    // then it would draw the same Dust.
    pending.wait_finalized().await.expect("finalized");

    for (index, (counter, round_before)) in counters.iter().enumerate() {
        let round_after = counter
            .ledger()
            .await
            .expect("ledger")
            .round()
            .expect("round");
        assert_eq!(
            round_after,
            round_before + 1,
            "the merged transaction should have advanced counter {index}"
        );
    }
}
