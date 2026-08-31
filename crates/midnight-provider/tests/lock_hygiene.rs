//! Provider lock hygiene and sync-task cancellation, against a local mock
//! indexer (no devnet required):
//!
//! - `resync_wallet` must not hold the wallet lock across the replay I/O:
//!   reads (`balance`) complete while a stalled resync replay is in flight.
//! - `WalletSyncBuilder::stream` must not leak the spawned sync task:
//!   dropping the progress receiver or the `SyncHandle` tears down the
//!   indexer WebSocket subscriptions promptly.
//!
//! The mock serves both indexer protocols on one port: GraphQL-over-HTTP
//! (`get_block`) and `graphql-transport-ws` subscriptions (via
//! `midnight_indexer_client::testutil`). In *fast* mode every subscription
//! completes immediately (empty chain), so a full wallet sync takes
//! milliseconds; in *stall* mode subscriptions are accepted and then held
//! silent, pinning the replay phase mid-flight.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use midnight_indexer_client::testutil::{ServerWs, next_json, send_next, subscriber_handshake};
use midnight_provider::MidnightProvider;
use midnight_types::INITIAL_PARAMETERS;
use midnight_types::midnight_serialize::tagged_serialize;
use midnight_wallet::{LocalWallet, Network, Wallet, WalletError, WalletFacade, WalletSeed};
use serde_json::json;
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
    /// When set, the unshielded subscription alone is held open without
    /// events. Dust and zswap still complete, which is what an address with
    /// no new transactions looks like to a resync.
    quiet_unshielded: AtomicBool,
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
            // A short sleep, not yield_now: peek returns the same partial
            // bytes immediately, so yielding would hot-spin on a stalled peer.
            Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
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
    let query = sub["payload"]["query"].as_str().unwrap_or("").to_string();
    if query.contains("unshieldedTransactions") && state.quiet_unshielded.load(Ordering::SeqCst) {
        // The shape of an address with no new transactions: the stream is
        // healthy and answers pings, it simply has nothing to deliver.
        drain(&mut ws).await;
        return;
    }
    if state.stall.load(Ordering::SeqCst) {
        state.stalled_subs.fetch_add(1, Ordering::SeqCst);
        // Hold the subscription open without events, answering client
        // frames (keepalive pings), until the client closes the connection.
        drain(&mut ws).await;
        state.stalled_subs.fetch_sub(1, Ordering::SeqCst);
    } else {
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
    if !common::read_http_request(&mut stream).await {
        return;
    }
    common::write_json_response(&mut stream, &block_response()).await;
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
    // Generous: a subscription whose first connect loses a race with the mock
    // retries on a bounded exponential backoff, which can eat several seconds
    // on a loaded machine. Promptness is asserted by the short timeouts after
    // each drop, not here.
    tokio::time::timeout(Duration::from_secs(30), async {
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
    let wallet = Wallet::sync(&url, seed(), Network::Undeployed)
        .await
        .expect("initial sync against the mock indexer");
    let provider = provider(&url).with_wallet(LocalWallet::new(wallet));

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

    let (rx, handle) = Wallet::sync(&url, seed(), Network::Undeployed)
        .stream()
        .await
        .expect("stream");

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
        Err(WalletError::SyncCancelled) => {}
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

    let (mut rx, handle) = Wallet::sync(&url, seed(), Network::Undeployed)
        .stream()
        .await
        .expect("stream");

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

/// A resync survives an indexer that has nothing to say about this address.
///
/// The three replays a resync runs are all resumes: the wallet already holds
/// a cursor, so a stream that stays quiet means "you are at the tip", not
/// "the sync failed". Dust and zswap read it that way. The unshielded replay
/// was asked to read it the other way, and a quiet unshielded stream is the
/// normal state of an address with no new transactions, so a resync failed
/// with `timeout waiting for unshielded sync` whenever the indexer pushed
/// nothing inside the idle bound. That took the devnet E2E job down.
///
/// The mock here answers dust and zswap and holds the unshielded stream open
/// and silent. Reverting the resume flag on that replay makes this fail.
#[tokio::test]
async fn resync_survives_an_unshielded_stream_with_nothing_to_deliver() {
    let (url, state) = spawn_mock().await;

    // Fast mode: attach a synced wallet in milliseconds.
    let wallet = Wallet::sync(&url, seed(), Network::Undeployed)
        .await
        .expect("initial sync against the mock indexer");
    let provider = provider(&url).with_wallet(LocalWallet::new(wallet));

    state.quiet_unshielded.store(true, Ordering::SeqCst);

    // The resume path settles on its own idle bound, around ten seconds.
    // Thirty leaves room on a loaded machine while still failing fast.
    tokio::time::timeout(Duration::from_secs(30), provider.resync_wallet())
        .await
        .expect("the resync must reach a verdict inside the idle bound")
        .expect("a quiet unshielded stream means at-tip, not a failed resync");
}

/// A chain that can be replaced under the wallet, on command.
///
/// Chain A holds `0xaa<height>` and is 100 blocks tall; chain B replaced it,
/// holds `0xbb<height>` at every height, and is taller.
#[derive(Default)]
struct SwappableChain {
    replaced: AtomicBool,
}

impl SwappableChain {
    fn replace(&self) {
        self.replaced.store(true, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl midnight_wallet::chain_pin::ChainView for SwappableChain {
    async fn block_hashes_at(&self, height: u64) -> Option<Vec<String>> {
        let chain = if self.replaced.load(Ordering::SeqCst) {
            "bb"
        } else {
            "aa"
        };
        Some(vec![format!("0x{chain}{height:04}")])
    }

    async fn finalized_height(&self) -> Option<u64> {
        Some(if self.replaced.load(Ordering::SeqCst) {
            200
        } else {
            100
        })
    }
}

/// A chain replaced while a resync is in flight is caught by the next one.
///
/// The pin a resync leaves behind marks the chain its state came from, so it
/// has to be taken before the check, not after the commit. Taken after, a swap
/// during the replay gets stamped with the new chain's own block: the wallet
/// then holds chain B's pin over state rebuilt from chain A, every later check
/// passes, and the wallet serves a dead chain's balance with nothing to show
/// for it. That is the failure the pin exists to catch, so it must not be the
/// failure the pin creates.
///
/// The replay is held open for about ten seconds by the quiet unshielded
/// stream, which is the window the chain is replaced in.
#[tokio::test]
async fn a_chain_replaced_during_a_resync_is_caught_by_the_next_one() {
    let (url, state) = spawn_mock().await;
    let chain = SwappableChain::default();

    let wallet = Wallet::sync(&url, seed(), Network::Undeployed)
        .pinned_to(&chain)
        .await
        .expect("initial sync against the mock indexer");
    let wallet = LocalWallet::new(wallet);

    state.quiet_unshielded.store(true, Ordering::SeqCst);

    // The replace lands after the resync has checked the old pin and while its
    // replay is still running.
    let (resync, ()) = tokio::join!(wallet.resync(&chain), async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        chain.replace();
    });
    resync.expect("the resync itself completes; the swap is not visible to it");

    let err = wallet
        .resync(&chain)
        .await
        .expect_err("the next resync must refuse: the pin belongs to a chain that is gone");
    assert!(
        matches!(err, WalletError::ChainMismatch { .. }),
        "expected ChainMismatch, got {err:?}"
    );
}
