//! Integration tests for midnight-wallet against a running devnet.
//!
//! These tests require MIDNIGHT_NODE_URL and MIDNIGHT_INDEXER_URL to be set.
//! The CI runs a devnet (node + indexer) via docker compose.
//!
//! Run locally:
//!   MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 MIDNIGHT_INDEXER_URL=http://127.0.0.1:8088 \
//!     cargo test -p midnight-wallet --test integration -- --show-output

use midnight_wallet::{Wallet, WalletBuilder, WalletState};

const DEV_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn node_url() -> Option<String> {
    std::env::var("MIDNIGHT_NODE_URL").ok()
}

fn indexer_url() -> Option<String> {
    std::env::var("MIDNIGHT_INDEXER_URL").ok()
}

macro_rules! require_devnet {
    () => {{
        let node = match node_url() {
            Some(u) => u,
            None => {
                eprintln!("skipping: MIDNIGHT_NODE_URL not set");
                return;
            }
        };
        let indexer = match indexer_url() {
            Some(u) => u,
            None => {
                eprintln!("skipping: MIDNIGHT_INDEXER_URL not set");
                return;
            }
        };
        (node, indexer)
    }};
}

macro_rules! require_node {
    () => {
        match node_url() {
            Some(u) => u,
            None => {
                eprintln!("skipping: MIDNIGHT_NODE_URL not set");
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Indexer-based sync
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_from_indexer_tracks_utxos() {
    let (node, indexer) = require_devnet!();
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
    let address = wallet.unshielded_address();

    let state = WalletState::sync_from_indexer(&node, &indexer, *wallet.seed(), &address)
        .await
        .expect("indexer sync should succeed");

    eprintln!(
        "synced: height={}, utxos={}",
        state.last_synced_height(),
        state.unshielded_utxos().len()
    );

    // sync_from_indexer must receive the Progress event to succeed, so
    // last_tx_id should always be populated after a successful sync.
    assert!(
        state.last_tx_id().is_some(),
        "expected last_tx_id to be set after sync"
    );
}

#[tokio::test]
async fn live_wallet_with_indexer() {
    let (node, indexer) = require_devnet!();
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();

    let live = WalletBuilder::new(wallet, &node)
        .indexer_url(&indexer)
        .build()
        .await
        .expect("build should succeed");

    let balance = live.balance().await;
    eprintln!("unshielded utxos: {}", balance.unshielded.len());

    live.shutdown().await;
}

// ---------------------------------------------------------------------------
// Node-based sync (fallback, no indexer required)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_from_node_and_check_dust() {
    let node = require_node!();
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();

    let state = WalletState::sync_from_node(&node, *wallet.seed())
        .await
        .expect("node sync should succeed");

    let balance = state.balance();
    assert!(
        balance.dust.spendable_utxos > 0,
        "devnet faucet should have funded this wallet with DUST"
    );
    assert!(state.last_synced_height() > 0);
    eprintln!(
        "dust utxos: {}, height: {}",
        balance.dust.spendable_utxos,
        state.last_synced_height()
    );
}

#[tokio::test]
async fn sync_context_for_tx_building() {
    let node = require_node!();
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();

    let mut state = WalletState::sync_from_node(&node, *wallet.seed())
        .await
        .expect("sync should succeed");

    // Context should already be cached after sync_from_node
    assert!(state.context().is_some());

    // Invalidate and re-sync
    state.invalidate_context();
    assert!(state.context().is_none());

    let ctx = state.sync_context().await.expect("sync_context");
    assert!(state.context().is_some());
    drop(ctx);
}

#[tokio::test]
async fn sync_or_fetch_context_with_cached_state() {
    let node = require_node!();
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();

    // Full sync (baseline)
    let full_start = std::time::Instant::now();
    let state = WalletState::sync_from_node(&node, *wallet.seed())
        .await
        .expect("sync");
    let full_sync_time = full_start.elapsed();

    // Cached path (should skip network entirely)
    let cached_start = std::time::Instant::now();
    let context = midnight_contract::sync_or_fetch_context(Some(&state), &node, *wallet.seed())
        .await
        .expect("sync_or_fetch_context with cached state");
    let cached_time = cached_start.elapsed();

    eprintln!("full sync: {full_sync_time:?}, cached: {cached_time:?}");
    // The cached path does no network I/O, so it should be orders of magnitude
    // faster than the full sync. Use a relative comparison to avoid CI flakiness.
    assert!(
        cached_time < full_sync_time / 2,
        "cached path ({cached_time:?}) should be much faster than full sync ({full_sync_time:?})"
    );
    drop(context);
}

// ---------------------------------------------------------------------------
// Subscription client connectivity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_client_connects() {
    let (_node, indexer) = require_devnet!();

    let sub_client = midnight_indexer_client::SubscriptionClient::new(&indexer);

    // Subscribe to blocks from offset 0
    let variables = serde_json::json!({ "offset": { "height": 0 } });
    let mut subscription = sub_client
        .subscribe::<serde_json::Value>(
            midnight_indexer_client::subscription::queries::BLOCKS_SUBSCRIPTION,
            variables,
        )
        .await
        .expect("blocks subscription should connect");

    // We should receive at least one block event
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), subscription.next())
        .await
        .expect("should receive block within 10s");

    assert!(
        event.is_some(),
        "subscription should yield at least one event"
    );
    let event = event.unwrap().expect("event should be Ok");
    eprintln!("received block event: {event}");
}
