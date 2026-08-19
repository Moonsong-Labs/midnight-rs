//! `sync_through` orders one wallet's build behind another wallet's spend.
//!
//! Two `Wallet`s built from one seed share nothing but what the indexer
//! replays into each of them. A build reserves the Dust it draws inside the
//! wallet that drew it, so a second wallet that has not seen the first one's
//! transaction re-selects the same note and the node rejects its transaction
//! with ledger custom error 196, `DustDoubleSpend`.
//!
//! The ordering test runs against a mock indexer, which is where the
//! guarantee is pinned: the resync must not start until the indexer serves
//! the block. The other two need a devnet (`MIDNIGHT_NODE_URL`,
//! `MIDNIGHT_INDEXER_URL`) and cover the real flow and the deadline.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use midnight_provider::{MidnightProvider, NIGHT, Network, ProviderError, Seed, WalletSeed};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};

const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(1);

fn devnet_urls() -> Option<(String, String)> {
    Some((
        std::env::var("MIDNIGHT_NODE_URL").ok()?,
        std::env::var("MIDNIGHT_INDEXER_URL").ok()?,
    ))
}

async fn synced_wallet(node_url: &str, indexer_url: &str, seed: Seed) -> MidnightProvider {
    MidnightProvider::new(node_url, indexer_url)
        .expect("provider")
        .sync_wallet(WalletSeed::from(seed), Network::Undeployed)
        .await
        .expect("sync")
}

#[tokio::test]
async fn a_second_wallet_on_one_seed_builds_behind_the_first_one() {
    let Some((node_url, indexer_url)) = devnet_urls() else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };
    let seed = Seed::from_hex(DEV_WALLET_SEED).expect("dev seed");
    let recipient = seed.unshielded_address(&Network::Undeployed);

    // Both wallets sync before either spends, so neither knows what the other
    // is about to draw.
    let first = synced_wallet(&node_url, &indexer_url, seed.clone()).await;
    let second = synced_wallet(&node_url, &indexer_url, seed).await;

    let (in_block, _) = first
        .transfer_unshielded(NIGHT, 1, &recipient)
        .await
        .expect("first wallet submits")
        .wait_finalized()
        .await
        .expect("first wallet's transfer finalizes");

    second
        .sync_through(&in_block.block_hash, TIMEOUT, POLL)
        .await
        .expect("second wallet reaches the block");

    // Without the wait this build re-selects the Dust the first wallet spent
    // and the node rejects it.
    second
        .transfer_unshielded(NIGHT, 1, &recipient)
        .await
        .expect("second wallet submits")
        .wait_finalized()
        .await
        .expect("second wallet's transfer finalizes");
}

#[tokio::test]
async fn a_block_the_indexer_never_has_times_out() {
    let Some((node_url, indexer_url)) = devnet_urls() else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };
    let provider = MidnightProvider::new(&node_url, &indexer_url).expect("provider");

    let outcome = provider
        .sync_through(
            &[0xab; 32],
            Duration::from_secs(1),
            Duration::from_millis(50),
        )
        .await;

    assert!(
        matches!(outcome, Err(ProviderError::BlockNotIndexed { .. })),
        "got {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// Ordering, against a mock indexer (no devnet)
// ---------------------------------------------------------------------------

/// Counts the two query shapes `sync_through` issues: the block lookup by
/// hash, and the `get_block(None)` that the resync's readiness check makes.
struct MockState {
    /// Serve "not indexed yet" for this many hash lookups, then the block.
    lag: usize,
    lookups: AtomicUsize,
    resyncs: AtomicUsize,
    /// How many hash lookups had been served when the first resync began.
    /// `usize::MAX` until one does.
    lookups_before_resync: AtomicUsize,
}

async fn spawn_mock(lag: usize) -> (String, Arc<MockState>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(MockState {
        lag,
        lookups: AtomicUsize::new(0),
        resyncs: AtomicUsize::new(0),
        lookups_before_resync: AtomicUsize::new(usize::MAX),
    });
    let conn_state = Arc::clone(&state);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(handle_http(stream, Arc::clone(&conn_state)));
        }
    });
    (url, state)
}

async fn handle_http(mut stream: TcpStream, state: Arc<MockState>) {
    let Some(request) = common::read_http_request_body(&mut stream).await else {
        return;
    };
    let body: serde_json::Value = serde_json::from_str(&request).unwrap_or(json!({}));
    // Only `sync_through`'s lookup carries an offset; the readiness check the
    // resync makes first asks for the latest block.
    let is_lookup = !body["variables"]["offset"].is_null();
    let response = if is_lookup {
        let served = state.lookups.fetch_add(1, Ordering::SeqCst);
        if served < state.lag {
            json!({"data": {"block": null}}).to_string()
        } else {
            block_response()
        }
    } else {
        if state.resyncs.fetch_add(1, Ordering::SeqCst) == 0 {
            state
                .lookups_before_resync
                .store(state.lookups.load(Ordering::SeqCst), Ordering::SeqCst);
        }
        block_response()
    };
    common::write_json_response(&mut stream, &response).await;
}

fn block_response() -> String {
    let mut params = Vec::new();
    midnight_helpers::midnight_serialize::tagged_serialize(
        &midnight_helpers::INITIAL_PARAMETERS,
        &mut params,
    )
    .unwrap();
    json!({
        "data": {
            "block": {
                "hash": "ab".repeat(32),
                "height": 5,
                "protocolVersion": 1,
                "timestamp": 1_000_000_i64,
                "author": null,
                "ledgerParameters": hex::encode(&params),
            }
        }
    })
    .to_string()
}

/// The resync must not start until the indexer serves the block. Resyncing
/// first would replay to whatever the indexer had a moment ago, which says
/// nothing about the block the caller named.
#[tokio::test]
async fn the_resync_waits_for_the_indexer_to_serve_the_block() {
    let (url, state) = spawn_mock(3).await;
    let provider = MidnightProvider::new("ws://127.0.0.1:1", &url).expect("provider");

    // No wallet is attached, so the resync this reaches fails with `NoWallet`.
    // Reaching it at all is the assertion: the block wait let it through.
    let outcome = provider
        .sync_through(
            &[0xab; 32],
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await;

    assert!(
        matches!(outcome, Err(ProviderError::NoWallet)),
        "got {outcome:?}"
    );
    assert_eq!(
        state.lookups.load(Ordering::SeqCst),
        4,
        "the wait must poll until the indexer serves the block"
    );
    assert_eq!(state.resyncs.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.lookups_before_resync.load(Ordering::SeqCst),
        4,
        "the resync must start only after the indexer served the block"
    );
}
