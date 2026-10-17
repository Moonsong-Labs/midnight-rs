//! `MidnightProvider::wait_observed` ends once the wallet sees the spends of a
//! transaction. `MidnightProvider::resync_until` returns the balance that ends
//! its wait.
//!
//! A finalized transaction reaches the wallet only through a resync, and one
//! resync can end before it sees the transaction. The test fails on every run
//! when a transfer handle carries no spent inputs, or when `wait_observed`
//! never resyncs. A wait that ends before the wallet sees the spends leaves the
//! Dust of the fee reserved, and the test checks for that.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

#[expect(dead_code, reason = "this test needs no synced_provider")]
mod common;

use std::time::Duration;

use midnight_provider::MidnightProvider;

#[tokio::test]
async fn wait_observed_ends_once_the_wallet_sees_the_spends() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    let recipient = wallet.shielded_address();
    let provider = provider.with_wallet(wallet);

    let before = provider.balance().await.expect("balance before");
    let token = before
        .shielded
        .coins
        .first()
        .expect("the dev seed holds a shielded coin at genesis")
        .token_type;

    let pending = provider
        .transfer_shielded(token, 1, &recipient)
        .await
        .expect("submit self-transfer");
    let (_, pending) = pending
        .wait_finalized()
        .await
        .expect("the self-transfer finalizes and applies");
    let spent = pending
        .spent_inputs()
        .expect("a transfer carries its spent inputs");
    assert!(
        !spent.dust.is_empty(),
        "the transfer must pay its fee in Dust, or the Dust check below proves nothing"
    );

    tokio::time::timeout(Duration::from_secs(60), provider.wait_observed(spent))
        .await
        .expect("the wallet sees the spends within 60 s")
        .expect("resync");

    // No resync after the wait: this reads the state that the wait left.
    let dust = provider.balance().await.expect("balance after").dust;
    assert_eq!(
        dust.spendable_speck, dust.balance_speck,
        "the wallet must hold no Dust reserved for the fee it paid"
    );

    // One input and two outputs, a change coin and the 1-unit coin.
    let count = before.shielded.total_count;
    let after = tokio::time::timeout(
        Duration::from_secs(60),
        provider.resync_until(|b| b.shielded.total_count > count),
    )
    .await
    .expect("the wallet sees the outputs within 60 s")
    .expect("resync");
    assert!(
        after.shielded.total_count > count,
        "resync_until returns the balance that made `done` hold"
    );
}
