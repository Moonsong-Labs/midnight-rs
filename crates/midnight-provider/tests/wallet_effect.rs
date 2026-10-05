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

use std::time::Duration;

use midnight_provider::{MidnightProvider, Network, ProviderError, WalletSeed};
use midnight_wallet::{LocalWallet, Seed, Wallet};

/// Funded with shielded tokens and Dust at genesis on the local devnet.
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::test]
async fn the_wait_ends_when_the_wallet_sees_the_spends_of_a_transaction() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        if std::env::var_os("MIDNIGHT_E2E").is_some() {
            panic!("MIDNIGHT_NODE_URL or MIDNIGHT_INDEXER_URL is missing under make test-e2e");
        }
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };

    let seed = Seed::from_hex(DEV_WALLET_SEED).expect("dev seed");
    let network = Network::Undeployed;
    let recipient = seed.shielded_address(&network);
    let provider = MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(&provider, WalletSeed::from(seed), network)
        .await
        .expect("sync");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

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

    let (finalized, pending) = pending.wait_finalized().await.expect("finalized");
    finalized
        .ensure_applied()
        .expect("the self-transfer applies");
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
