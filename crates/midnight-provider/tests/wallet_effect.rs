//! `MidnightProvider::wait_observed` returns once the wallet sees the spends of
//! a transaction, and not before.
//!
//! After finality, the wallet sees a spend only when a resync replays the
//! indexer's events, and the indexer serves them a moment later. A wait that
//! gives up with `Ok` leaves the caller with a stale coin set and with Dust that
//! still reads as reserved.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

#[expect(dead_code, reason = "this test needs no synced_provider")]
mod common;

use std::time::Duration;

use midnight_provider::{MidnightProvider, ProviderError};

#[tokio::test]
async fn the_wait_ends_when_the_wallet_sees_the_spends_of_a_transaction() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    let recipient = wallet.shielded_address();
    let provider = provider.with_wallet(wallet);

    let coins_before = provider
        .spendable_shielded_coins()
        .await
        .expect("coins before");
    let token = coins_before
        .first()
        .expect("the dev seed holds a shielded coin at genesis")
        .token_type;

    let pending = provider
        .transfer_shielded(token, 1, &recipient)
        .await
        .expect("submit self-transfer");
    let transaction_hash = pending.transaction_hash();

    // The submit has just returned, so no block holds the transaction, and
    // the one round that a zero timeout runs cannot see its spends.
    match provider
        .wait_observed(transaction_hash, pending.spent_inputs(), Duration::ZERO)
        .await
    {
        Err(ProviderError::EffectTimeout {
            transaction_hash: Some(hash),
            ..
        }) => assert_eq!(
            hash, transaction_hash,
            "the timeout must name this transaction"
        ),
        other => panic!("a wait past its deadline must time out, got {other:?}"),
    }

    let (_, pending) = pending
        .wait_finalized()
        .await
        .expect("the self-transfer finalizes and applies");
    provider
        .wait_observed(
            transaction_hash,
            pending.spent_inputs(),
            Duration::from_secs(60),
        )
        .await
        .expect("the wallet sees the spends");

    // No resync after the wait: these read the state that the wait left.
    let coins_after = provider
        .spendable_shielded_coins()
        .await
        .expect("coins after");
    assert!(
        coins_after.iter().any(|after| coins_before
            .iter()
            .all(|before| before.nullifier != after.nullifier)),
        "the wallet must hold a coin that the self-transfer made"
    );
    let dust = provider.balance().await.expect("balance").dust;
    assert_eq!(
        dust.spendable_speck, dust.balance_speck,
        "the wallet must hold no Dust reserved for the fee it paid"
    );
}
