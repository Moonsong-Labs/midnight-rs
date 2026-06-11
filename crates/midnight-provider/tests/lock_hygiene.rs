//! Provider lock hygiene and sync-task cancellation, against a local mock
//! indexer (no devnet required):
//!
//! - `resync_wallet` must not hold the wallet lock across the replay I/O:
//!   reads (`balance`) complete while a stalled resync replay is in flight.
//! - `SyncWalletBuilder::stream` must not leak the spawned sync task:
//!   dropping the progress receiver or the `SyncHandle` tears down the
//!   indexer WebSocket subscriptions promptly.
//!
//! The mock serves both indexer protocols on one port: GraphQL-over-HTTP
//! (`get_block`) and `graphql-transport-ws` subscriptions (via
//! `midnight_indexer_client::testutil`). In *fast* mode every subscription
//! completes immediately (empty chain), so a full wallet sync takes
//! milliseconds; in *stall* mode subscriptions are accepted and then held
//! silent, pinning the replay phase mid-flight.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use midnight_helpers::INITIAL_PARAMETERS;
use midnight_helpers::midnight_serialize::tagged_serialize;
use midnight_indexer_client::testutil::{ServerWs, next_json, send_next, subscriber_handshake};
use midnight_provider::{MidnightProvider, ProviderError};
use midnight_wallet::{Network, WalletSeed};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// ---------------------------------------------------------------------------
// Mock indexer
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MockState {
    /// When set, subscriptions are held open without events instead of
    /// completing immediately.
    stall: AtomicBool,
    /// Number of currently-open stalled subscription sockets. Incremented
    /// after the `subscribe` handshake, decremented when the client tears
    /// the connection down — the observable for "subscriptions got cleaned
    /// up".
    stalled_subs: AtomicUsize,
}

async fn spawn_mock() -> (String, Arc<MockState>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(MockState::default());
    let conn_state = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(handle_conn(stream, Arc::clone(&conn_state)));
        }
    });
    (url, state)
}

async fn handle_conn(stream: TcpStream, state: Arc<MockState>) {
    // Route by request method without consuming bytes: WS upgrades arrive as
    // GET, GraphQL queries as POST. `peek` leaves the stream intact for the
    // WebSocket handshake.
    let mut head = [0u8; 4];
    loop {
        match stream.peek(&mut head).await {
            Ok(n) if n >= 4 => break,
            Ok(0) | Err(_) => return,
            Ok(_) => tokio::task::yield_now().await,
        }
    }
    if &head == b"GET " {
        handle_ws(stream, state).await;
    } else {
        handle_http(stream).await;
    }
}

/// Serve one subscription: handshake, then either complete it immediately
/// (fast mode) or hold it silent until the client disconnects (stall mode).
async fn handle_ws(stream: TcpStream, state: Arc<MockState>) {
    let (mut ws, sub) = subscriber_handshake(stream).await;
    if state.stall.load(Ordering::SeqCst) {
        state.stalled_subs.fetch_add(1, Ordering::SeqCst);
        // Hold the subscription open without events, answering client
        // frames (keepalive pings), until the client closes the connection.
        drain(&mut ws).await;
        state.stalled_subs.fetch_sub(1, Ordering::SeqCst);
    } else {
        let query = sub["payload"]["query"].as_str().unwrap_or("");
        // "Empty chain" replies: each replay loop completes immediately.
        let data = if query.contains("zswapLedgerEvents") {
            json!({"zswapLedgerEvents": {"id": 0, "raw": "", "maxId": 0}})
        } else if query.contains("dustLedgerEvents") {
            json!({"dustLedgerEvents": {"id": 0, "raw": "", "maxId": 0}})
        } else {
            json!({"unshieldedTransactions": {
                "__typename": "UnshieldedTransactionsProgress",
                "highestTransactionId": 0,
            }})
        };
        send_next(&mut ws, &sub, data).await;
        drain(&mut ws).await;
    }
}

/// Consume client frames (answering pings) until the connection closes.
async fn drain(ws: &mut ServerWs) {
    while next_json(ws).await.is_some() {}
}

/// Serve one GraphQL HTTP request. Every query in the exercised paths is
/// `get_block(None)`, so the response is always the same post-genesis block
/// carrying valid ledger parameters.
async fn handle_http(mut stream: TcpStream) {
    // Read the request head plus its content-length body.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        let Ok(n) = stream.read(&mut tmp).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            return;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let content_length = head
        .lines()
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let Ok(n) = stream.read(&mut tmp).await else {
            return;
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }

    let body = block_response();
    let resp = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn block_response() -> String {
    let mut params = Vec::new();
    tagged_serialize(&INITIAL_PARAMETERS, &mut params).unwrap();
    json!({
        "data": {
            "block": {
                "hash": "00".repeat(32),
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

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn seed() -> WalletSeed {
    WalletSeed::try_from_hex_str(&"11".repeat(32)).unwrap()
}

fn provider(url: &str) -> MidnightProvider {
    // The node URL is never dialed in these paths.
    MidnightProvider::new("ws://127.0.0.1:1", url).unwrap()
}

async fn wait_until(what: &str, f: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// `resync_wallet` snapshots under a brief read lock, replays lock-free, and
/// commits under a brief write lock — so a read (`balance`) completes while
/// the replay is stalled mid-flight. Before the fix the wallet write lock was
/// held across the whole replay and this read blocked until the resync ended.
#[tokio::test]
async fn balance_completes_while_resync_replay_is_in_flight() {
    let (url, state) = spawn_mock().await;

    // Fast mode: attach a synced wallet in milliseconds.
    let provider = provider(&url)
        .sync_wallet(seed(), Network::Undeployed)
        .await
        .expect("initial sync against the mock indexer");

    // Stall mode: the next resync's replay phase hangs on silent
    // subscriptions.
    state.stall.store(true, Ordering::SeqCst);
    let provider = Arc::new(provider);
    let resync_provider = Arc::clone(&provider);
    let resync = tokio::spawn(async move { resync_provider.resync_wallet().await });

    // Wait until the replay phase is live: all three resync subscriptions
    // are connected and stalled.
    wait_until("resync replay subscriptions", || {
        state.stalled_subs.load(Ordering::SeqCst) >= 3
    })
    .await;

    // The replay is mid-flight; a read must complete promptly.
    tokio::time::timeout(Duration::from_secs(2), provider.balance())
        .await
        .expect("balance() must not block while a resync replay is in flight")
        .expect("balance() must succeed");

    resync.abort();
}

/// Dropping the progress receiver cancels the streamed sync: the handle
/// resolves to `SyncCancelled` and the mock observes all subscription
/// sockets closing — no orphaned subscription tasks.
#[tokio::test]
async fn dropping_receiver_cancels_streamed_sync_and_closes_subscriptions() {
    let (url, state) = spawn_mock().await;
    state.stall.store(true, Ordering::SeqCst);

    let (rx, handle) = provider(&url)
        .sync_wallet(seed(), Network::Undeployed)
        .stream();

    // Initial sync runs zswap + unshielded first (dust starts after both
    // complete), so a stalled initial sync holds two subscriptions.
    wait_until("stalled sync subscriptions", || {
        state.stalled_subs.load(Ordering::SeqCst) >= 2
    })
    .await;

    drop(rx);

    let result = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("handle must resolve promptly after the receiver is dropped");
    match result {
        Err(ProviderError::SyncCancelled) => {}
        Err(other) => panic!("expected SyncCancelled, got {other:?}"),
        Ok(_) => panic!("a cancelled sync must surface an error"),
    }

    wait_until("subscription sockets to close", || {
        state.stalled_subs.load(Ordering::SeqCst) == 0
    })
    .await;
}

/// Dropping the `SyncHandle` aborts the sync task: the progress channel
/// closes and the mock observes all subscription sockets closing.
#[tokio::test]
async fn dropping_sync_handle_aborts_sync_and_closes_subscriptions() {
    let (url, state) = spawn_mock().await;
    state.stall.store(true, Ordering::SeqCst);

    let (mut rx, handle) = provider(&url)
        .sync_wallet(seed(), Network::Undeployed)
        .stream();

    wait_until("stalled sync subscriptions", || {
        state.stalled_subs.load(Ordering::SeqCst) >= 2
    })
    .await;

    drop(handle);

    // The aborted task drops its progress senders; recv() ends promptly.
    let next = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("recv must resolve promptly after the handle is dropped");
    assert!(next.is_none(), "channel must close, got {next:?}");

    wait_until("subscription sockets to close", || {
        state.stalled_subs.load(Ordering::SeqCst) == 0
    })
    .await;
}
