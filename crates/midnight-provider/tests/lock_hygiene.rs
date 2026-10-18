//! Provider lock hygiene and sync-task cancellation, against a local mock
//! indexer (no devnet required):
//!
//! - `resync_wallet` must not hold the wallet lock across the replay I/O:
//!   reads (`balance`) complete while a stalled resync replay is in flight.
//! - `LocalWallet` serializes its own sync methods: a forget waits for a
//!   resync in flight.
//! - `WalletBuilder::sync_with_progress` must not leak the spawned sync task:
//!   dropping the progress receiver or the `SyncHandle` tears down the
//!   indexer WebSocket subscriptions promptly.
//! - `LocalWallet::sync_in_background` keeps the error of a failed pass until
//!   a pass commits, and reports a task that a panic ended. Dropping its
//!   handle closes the subscriptions of the pass in flight, and no later pass
//!   starts.
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

use midnight_helpers::ledger_8::INITIAL_PARAMETERS;
use midnight_helpers::midnight_serialize::tagged_serialize;
use midnight_indexer_client::testutil::{
    ServerWs, bind, ledger_event, next_json, read_http_request, send_next, subscriber_handshake,
    unshielded_progress, write_json_response,
};
use midnight_provider::{BlockHash, MidnightProvider};
use midnight_wallet::{
    CoinInfo, HashOutput, LocalWallet, Network, Nonce, ShieldedTokenType, SyncHandle, SyncProgress,
    Wallet, WalletBuilder, WalletError, WalletFacade,
};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

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
    /// The number of subscriptions that completed the handshake, in every
    /// mode. It only grows, so it shows a pass that starts after a sync
    /// stopped.
    opened_subs: AtomicUsize,
    /// When set, the block query answers with a GraphQL error. The client
    /// does not retry that error, so a resync fails at once.
    fail_block: AtomicBool,
}

async fn spawn_mock() -> (String, Arc<MockState>) {
    let (listener, url) = bind().await;
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
        handle_http(stream, &state).await;
    }
}

/// Serve one subscription: handshake, then either complete it immediately
/// (fast mode) or hold it silent until the client disconnects (stall mode).
async fn handle_ws(stream: TcpStream, state: Arc<MockState>) {
    let (mut ws, sub) = subscriber_handshake(stream).await;
    state.opened_subs.fetch_add(1, Ordering::SeqCst);
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
            ledger_event("zswapLedgerEvents", 0, "", 0)
        } else if query.contains("dustLedgerEvents") {
            ledger_event("dustLedgerEvents", 0, "", 0)
        } else {
            unshielded_progress(0)
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
/// `get_block(None)`, so the response is the same post-genesis block carrying
/// valid ledger parameters, or a GraphQL error when `fail_block` is set.
async fn handle_http(mut stream: TcpStream, state: &MockState) {
    if !read_http_request(&mut stream).await {
        return;
    }
    let body = if state.fail_block.load(Ordering::SeqCst) {
        json!({ "errors": [{ "message": "the mock refuses the block query" }] }).to_string()
    } else {
        block_response()
    };
    write_json_response(&mut stream, "200 OK", &body).await;
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

/// The pause of every background sync in these tests.
const PAUSE: Duration = Duration::from_millis(50);

fn builder() -> WalletBuilder {
    // No sync source here reports a network, so the wallet names one.
    Wallet::from_seed(&"11".repeat(32))
        .unwrap()
        .with_network(Network::Undeployed)
}

fn provider(url: &str) -> MidnightProvider {
    // The node URL is never dialed in these paths.
    MidnightProvider::new("ws://127.0.0.1:1", url)
}

/// Fast mode: sync an unpinned wallet in milliseconds.
async fn synced_wallet(url: &str) -> Wallet {
    builder()
        .unpinned()
        .sync(&provider(url))
        .await
        .expect("initial sync against the mock indexer")
}

/// Fast mode: attach a synced wallet in milliseconds.
async fn synced_provider(url: &str) -> MidnightProvider {
    provider(url).with_wallet(synced_wallet(url).await)
}

/// Stall mode: start a streamed sync, then wait until the mock holds its
/// subscriptions.
async fn stalled_streamed_sync(
    url: &str,
    state: &MockState,
) -> (mpsc::Receiver<SyncProgress>, SyncHandle) {
    state.stall.store(true, Ordering::SeqCst);
    let sync = builder()
        .unpinned()
        .sync_with_progress(&provider(url))
        .await
        .expect("streamed sync");

    // Initial sync runs zswap + unshielded first (dust starts after both
    // complete), so a stalled initial sync holds two subscriptions.
    wait_until("stalled sync subscriptions", || {
        state.stalled_subs.load(Ordering::SeqCst) >= 2
    })
    .await;
    sync
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
    let provider = synced_provider(&url).await;

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

/// A forget waits for a resync in flight. Otherwise the commit of that resync
/// brings the forgotten registration back, and nothing reports it.
#[tokio::test]
async fn forget_coins_waits_for_a_resync_in_flight() {
    let (url, state) = spawn_mock().await;
    let wallet = Arc::new(LocalWallet::new(synced_wallet(&url).await));

    state.stall.store(true, Ordering::SeqCst);
    let resync = tokio::spawn({
        let wallet = Arc::clone(&wallet);
        let chain = provider(&url);
        async move { wallet.resync(&chain).await }
    });
    wait_until("resync replay subscriptions", || {
        state.stalled_subs.load(Ordering::SeqCst) >= 3
    })
    .await;

    let coin = CoinInfo {
        nonce: Nonce(HashOutput([1; 32])),
        type_: ShieldedTokenType(HashOutput([2; 32])),
        value: 1,
    };
    // Keep this under the resume idle bound, around ten seconds. At that bound
    // the stalled replay ends on its own and the resync commits.
    let forget = tokio::time::timeout(Duration::from_millis(500), wallet.forget_coins(vec![coin]));
    assert!(
        forget.await.is_err(),
        "forget_coins must wait for the resync in flight"
    );

    resync.abort();
}

/// Dropping the progress receiver cancels the streamed sync: the handle
/// resolves to `SyncCancelled` and the mock observes all subscription
/// sockets closing — no orphaned subscription tasks.
#[tokio::test]
async fn dropping_receiver_cancels_streamed_sync_and_closes_subscriptions() {
    let (url, state) = spawn_mock().await;
    let (rx, handle) = stalled_streamed_sync(&url, &state).await;

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
    let (mut rx, handle) = stalled_streamed_sync(&url, &state).await;

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
    let provider = synced_provider(&url).await;

    state.quiet_unshielded.store(true, Ordering::SeqCst);

    // The resume path settles on its own idle bound, around ten seconds.
    // Thirty leaves room on a loaded machine while still failing fast.
    tokio::time::timeout(Duration::from_secs(30), provider.resync_wallet())
        .await
        .expect("the resync must reach a verdict inside the idle bound")
        .expect("a quiet unshielded stream means at-tip, not a failed resync");
}

/// A chain that can be replaced under the wallet, on command, served by the
/// mock indexer at `indexer_url`.
///
/// Chain A's hashes start with `0xaa` and end with the height, and it is 100
/// blocks tall. Chain B replaced it, its hashes start with `0xbb`, and it is
/// taller.
struct SwappableChain {
    indexer_url: String,
    replaced: AtomicBool,
}

impl SwappableChain {
    fn new(indexer_url: &str) -> Self {
        Self {
            indexer_url: indexer_url.to_string(),
            replaced: AtomicBool::new(false),
        }
    }

    fn replace(&self) {
        self.replaced.store(true, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl midnight_wallet::chain_pin::ChainView for SwappableChain {
    async fn block_hashes_at(&self, height: u64) -> Option<Vec<BlockHash>> {
        let mut hash = BlockHash::from_low_u64_be(height);
        hash.0[0] = if self.replaced.load(Ordering::SeqCst) {
            0xbb
        } else {
            0xaa
        };
        Some(vec![hash])
    }

    async fn finalized_height(&self) -> Option<u64> {
        Some(if self.replaced.load(Ordering::SeqCst) {
            200
        } else {
            100
        })
    }
}

#[async_trait::async_trait]
impl midnight_wallet::chain_pin::SyncSource for SwappableChain {
    fn indexer_url(&self) -> &str {
        &self.indexer_url
    }

    async fn network(&self) -> Option<Network> {
        None
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
    let chain = SwappableChain::new(&url);

    let wallet = builder()
        .sync(&chain)
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

/// Dropping the `BackgroundSync` aborts its task. The pass in flight closes
/// its subscriptions, and no later pass starts.
///
/// A handle that only detaches the task leaves the stalled pass open until
/// the resume idle bound, around ten seconds, and the loop runs on after it.
#[tokio::test]
async fn dropping_the_background_sync_closes_its_subscriptions() {
    let (url, state) = spawn_mock().await;
    let wallet = Arc::new(LocalWallet::new(synced_wallet(&url).await));

    state.stall.store(true, Ordering::SeqCst);
    let sync = wallet.sync_in_background(Arc::new(provider(&url)), PAUSE);
    wait_until("background resync subscriptions", || {
        state.stalled_subs.load(Ordering::SeqCst) >= 3
    })
    .await;

    drop(sync);

    // Keep this bound under the resume idle bound. At that bound the stalled
    // pass ends by itself and closes its subscriptions.
    tokio::time::timeout(
        Duration::from_secs(2),
        wait_until("subscription sockets to close", || {
            state.stalled_subs.load(Ordering::SeqCst) == 0
        }),
    )
    .await
    .expect("dropping the handle must close the subscriptions of the pass in flight");

    let opened = state.opened_subs.load(Ordering::SeqCst);
    tokio::time::sleep(PAUSE * 5).await;
    assert_eq!(
        state.opened_subs.load(Ordering::SeqCst),
        opened,
        "no pass may start after the handle drops"
    );
}

/// A failed pass shows in `last_error`, the sync goes on, and the next pass
/// that commits clears the error.
///
/// Three wrong loops fail it: one that ends at its first error, one that only
/// logs the error, and one whose commit keeps the old error. With the second,
/// the wallet goes stale and only a log line shows it. The first shows
/// `SyncTaskJoin` in place of the error of the pass.
#[tokio::test]
async fn a_failed_pass_is_recorded_and_the_next_pass_clears_it() {
    let (url, state) = spawn_mock().await;
    let wallet = Arc::new(LocalWallet::new(synced_wallet(&url).await));

    state.fail_block.store(true, Ordering::SeqCst);
    let sync = wallet.sync_in_background(Arc::new(provider(&url)), PAUSE);
    wait_until("a failed pass", || {
        matches!(sync.last_error().as_deref(), Some(WalletError::Indexer(_)))
    })
    .await;

    state.fail_block.store(false, Ordering::SeqCst);
    wait_until("a pass that commits", || sync.last_error().is_none()).await;
}

/// A node view whose finalized-height question panics.
struct PanickingChain;

#[async_trait::async_trait]
impl midnight_wallet::chain_pin::ChainView for PanickingChain {
    async fn block_hashes_at(&self, _height: u64) -> Option<Vec<BlockHash>> {
        None
    }

    async fn finalized_height(&self) -> Option<u64> {
        panic!("the node view panics");
    }
}

/// A pass that panics ends the background sync, and `last_error` says so.
///
/// A panicked pass sends nothing on the status channel. So a `last_error`
/// that reads only that channel shows a healthy wallet while nothing syncs.
#[tokio::test]
async fn a_panicked_pass_ends_the_sync_and_last_error_names_it() {
    let (url, _state) = spawn_mock().await;
    // A pinned wallet: each resync asks the chain for a fresh pin first.
    let wallet = builder()
        .sync(&SwappableChain::new(&url))
        .await
        .expect("initial sync against the mock indexer");
    let wallet = Arc::new(LocalWallet::new(wallet));

    let sync = wallet.sync_in_background(Arc::new(PanickingChain), PAUSE);
    wait_until("the background sync to stop", || {
        matches!(
            sync.last_error().as_deref(),
            Some(WalletError::SyncTaskJoin(_))
        )
    })
    .await;
}
