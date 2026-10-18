//! `BackgroundSync::wait_until` ends once the wallet sees the effects of a
//! transaction: first its spends, then its outputs.
//!
//! A finalized transaction reaches the wallet only through a resync, and one
//! resync can end before it sees the transaction. The test fails when a
//! transfer handle carries no spent inputs. It also fails when
//! `Wallet::has_observed` reads a reservation. A resync does not end a
//! shielded reservation, so that wait times out. A wait that ends before the
//! commit that clears the reservation of the fee's Dust also fails it. A
//! replay that moves its cursor past the outputs and does not apply them makes
//! the last wait time out.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

#[expect(dead_code, reason = "this test needs no synced_provider")]
mod common;

use std::sync::Arc;
use std::time::Duration;

use midnight_provider::MidnightProvider;
use midnight_wallet::LocalWallet;

#[tokio::test]
async fn wait_until_ends_once_the_wallet_sees_the_spends_then_the_outputs() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    let recipient = wallet.shielded_address();
    let wallet = Arc::new(LocalWallet::new(wallet));
    let provider = Arc::new(provider.with_wallet(Arc::clone(&wallet)));
    let sync = wallet.sync_in_background(provider.clone(), Duration::from_secs(1));

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

    // Read the Dust in the same check that ends the wait. A later pass can
    // commit before a read after the wait.
    let mut dust = None;
    tokio::time::timeout(
        Duration::from_secs(60),
        sync.wait_until(|w| {
            let observed = w.has_observed(spent);
            if observed {
                dust = Some(w.balance().dust);
            }
            observed
        }),
    )
    .await
    .expect("the wallet sees the spends within 60 s")
    .expect("background sync");
    let dust = dust.expect("the wait ends only on a check that saw the spends");
    assert_eq!(
        dust.spendable_speck, dust.balance_speck,
        "the wallet must hold no Dust reserved for the fee it paid"
    );

    // One input and two outputs, a change coin and the 1-unit coin.
    let count = before.shielded.total_count;
    tokio::time::timeout(
        Duration::from_secs(60),
        sync.wait_until(|w| w.balance().shielded.total_count > count),
    )
    .await
    .expect("the wallet sees the outputs within 60 s")
    .expect("background sync");
}
