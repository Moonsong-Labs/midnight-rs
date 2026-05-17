//! Integration tests for midnight-wallet against a running devnet.
//!
//! These tests require MIDNIGHT_NODE_URL and MIDNIGHT_INDEXER_URL to be set.
//! The CI runs a devnet (node + indexer) via docker compose.
//!
//! Run locally:
//!   MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 MIDNIGHT_INDEXER_URL=http://127.0.0.1:8088 \
//!     cargo test -p midnight-wallet --test integration -- --show-output

use midnight_wallet::{Wallet, WalletBuilder, WalletState};
use std::sync::Arc;

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
                if std::env::var("MIDNIGHT_E2E").is_ok() {
                    panic!("MIDNIGHT_NODE_URL must be set in CI");
                }
                eprintln!("skipping: MIDNIGHT_NODE_URL not set");
                return;
            }
        };
        let indexer = match indexer_url() {
            Some(u) => u,
            None => {
                if std::env::var("MIDNIGHT_E2E").is_ok() {
                    panic!("MIDNIGHT_INDEXER_URL must be set in CI");
                }
                eprintln!("skipping: MIDNIGHT_INDEXER_URL not set");
                return;
            }
        };
        (node, indexer)
    }};
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

    let live = WalletBuilder::new(wallet, &node, &indexer)
        .build()
        .await
        .expect("build should succeed");

    let state = live.state().read().await;
    assert!(
        state.last_tx_id().is_some(),
        "indexer sync should set last_tx_id"
    );
    assert!(
        state.subscription_client().is_some(),
        "subscription client should be available"
    );
    drop(state);

    let balance = live.balance().await;
    eprintln!("unshielded utxos: {}", balance.unshielded.len());

    live.shutdown().await;
}

// ---------------------------------------------------------------------------
// Node context (lazy fetch for transaction building)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_context_for_tx_building() {
    let (node, indexer) = require_devnet!();
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
    let address = wallet.unshielded_address();

    let mut state = WalletState::sync_from_indexer(&node, &indexer, *wallet.seed(), &address)
        .await
        .expect("indexer sync should succeed");

    // No node context yet (indexer sync doesn't fetch from the node)
    assert!(state.context().is_none());

    // Fetch context from node on demand
    let (ctx, blocks) = state.sync_context().await.expect("sync_context");
    assert!(state.context().is_some());
    assert!(blocks > 0, "should process blocks from node");
    drop(ctx);

    // Cached on second call
    let (_, blocks) = state.sync_context().await.expect("sync_context cached");
    assert_eq!(blocks, 0, "cached context should not re-fetch");
}

#[tokio::test]
async fn sync_or_fetch_context_uses_cached_state() {
    let (node, indexer) = require_devnet!();
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
    let address = wallet.unshielded_address();

    let mut state = WalletState::sync_from_indexer(&node, &indexer, *wallet.seed(), &address)
        .await
        .expect("indexer sync should succeed");

    // Prime the cache
    let _ = state.sync_context().await.expect("sync_context");

    let cached_ctx = state
        .context()
        .expect("context should be cached after sync_context")
        .clone();

    // sync_or_fetch_context should return the cached Arc without re-fetching.
    let context = midnight_contract::sync_or_fetch_context(Some(&state), &node, *wallet.seed())
        .await
        .expect("sync_or_fetch_context with cached state");
    assert!(
        Arc::ptr_eq(&cached_ctx, &context),
        "expected sync_or_fetch_context to reuse cached context Arc"
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
