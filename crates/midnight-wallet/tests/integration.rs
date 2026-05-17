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
// Indexer-based sync (zswap + dust + unshielded events)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_from_indexer_replays_events() {
    let (node, indexer) = require_devnet!();
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
    let address = wallet.unshielded_address();

    let state =
        WalletState::sync_from_indexer(&node, &indexer, *wallet.seed(), &address, wallet.network())
            .await
            .expect("indexer sync should succeed");

    eprintln!(
        "synced: height={}, utxos={}, zswap_event_id={}, dust_event_id={}",
        state.last_synced_height(),
        state.unshielded_utxos().len(),
        state.zswap_event_id(),
        state.dust_event_id(),
    );

    // After sync, last_tx_id should be populated (from the Progress event)
    assert!(
        state.last_tx_id().is_some(),
        "expected last_tx_id to be set after sync"
    );

    // Zswap and dust event replay should have processed events
    assert!(
        state.zswap_event_id() > 0,
        "expected zswap events to have been replayed"
    );
    assert!(
        state.dust_event_id() > 0,
        "expected dust events to have been replayed"
    );
}

// ---------------------------------------------------------------------------
// Build context from indexed state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_context_from_indexed_state() {
    let (node, indexer) = require_devnet!();
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
    let address = wallet.unshielded_address();

    let state =
        WalletState::sync_from_indexer(&node, &indexer, *wallet.seed(), &address, wallet.network())
            .await
            .expect("indexer sync should succeed");

    // build_context should succeed when parameters are available
    let context = state.build_context().expect("build_context should succeed");

    // The context should have our wallet registered
    let wallets = context.wallets.lock().unwrap();
    assert!(
        wallets.contains_key(wallet.seed()),
        "context should contain our wallet"
    );
}

// ---------------------------------------------------------------------------
// Live wallet (background sync)
// ---------------------------------------------------------------------------

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
    eprintln!(
        "balance: dust={}, unshielded={}, shielded={}",
        balance.dust.spendable_utxos,
        balance.unshielded.len(),
        balance.shielded.total_count,
    );

    live.shutdown().await;
}

// ---------------------------------------------------------------------------
// Transfer transaction building
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_shielded_transfer() {
    let (node, indexer) = require_devnet!();
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();

    let live = WalletBuilder::new(wallet.clone(), &node, &indexer)
        .build()
        .await
        .expect("build should succeed");

    let balance = live.balance().await;
    eprintln!(
        "pre-transfer balance: dust={}, shielded={}",
        balance.dust.spendable_utxos, balance.shielded.total_count,
    );

    // Build a self-transfer (send 1 tNIGHT to ourselves)
    let proof_provider: std::sync::Arc<
        dyn midnight_node_ledger_helpers::ProofProvider<midnight_node_ledger_helpers::DefaultDB>,
    > = std::sync::Arc::new(midnight_node_ledger_helpers::LocalProofServer::new());

    let transfer_guard = live
        .transfer(proof_provider)
        .await
        .expect("transfer guard should succeed");

    let result = transfer_guard
        .builder()
        .shielded(
            midnight_node_ledger_helpers::ShieldedTokenType(
                midnight_node_ledger_helpers::HashOutput([0u8; 32]),
            ),
            1,
            *wallet.seed(),
        )
        .await;

    match &result {
        Ok(tx_result) => {
            eprintln!("transfer built successfully, tx_bytes={}", tx_result.tx_bytes.len());
        }
        Err(e) => {
            eprintln!("transfer failed: {e}");
        }
    }

    assert!(result.is_ok(), "shielded transfer should build successfully");

    // Submit the transaction to the node
    let tx_result = result.unwrap();
    let hash = tx_result
        .submit(&node)
        .await
        .expect("transaction submission should succeed");
    eprintln!("transaction submitted: {hash}");

    drop(transfer_guard);
    live.shutdown().await;
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
