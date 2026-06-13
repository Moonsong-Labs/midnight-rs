//! `wait_transaction_result` timeout-vs-found semantics against a mock
//! indexer (no devnet required).
//!
//! The mock serves the GraphQL-over-HTTP `transactions` query only. Each
//! poll is one POST; the mock counts requests and can switch from "not
//! indexed yet" (empty `transactions` array) to "result surfaced" after a
//! configurable number of polls — simulating indexer lag behind the node.
//!
//! Covers audit finding A#31: before `TxResultWait`, a timeout and a tx
//! that genuinely never landed were both `Ok(None)`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use midnight_provider::{MidnightProvider, TransactionResultStatus, TxResultWait};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};

// ---------------------------------------------------------------------------
// Mock indexer (HTTP only — wait_transaction_result never opens a WS)
// ---------------------------------------------------------------------------

struct MockState {
    /// Number of `transactions` queries served so far.
    requests: AtomicUsize,
    /// Serve "not indexed yet" for this many requests, then surface the
    /// result. `usize::MAX` means the result never surfaces.
    lag: usize,
}

async fn spawn_mock(lag: usize) -> (String, Arc<MockState>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(MockState {
        requests: AtomicUsize::new(0),
        lag,
    });
    let conn_state = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(handle_http(stream, Arc::clone(&conn_state)));
        }
    });
    (url, state)
}

/// Serve one GraphQL HTTP request (the client sends `connection: close`
/// semantics per poll via a fresh request; we close after each response).
async fn handle_http(mut stream: TcpStream, state: Arc<MockState>) {
    if !common::read_http_request(&mut stream).await {
        return;
    }
    let served = state.requests.fetch_add(1, Ordering::SeqCst);
    let body = if served < state.lag {
        not_indexed_response()
    } else {
        result_response()
    };
    common::write_json_response(&mut stream, &body).await;
}

/// The indexer hasn't surfaced the transaction (not landed, or lagging —
/// indistinguishable from the indexer's response alone).
fn not_indexed_response() -> String {
    json!({"data": {"transactions": []}}).to_string()
}

/// The transaction is indexed with a chain-side result attached.
fn result_response() -> String {
    json!({
        "data": {
            "transactions": [{
                "__typename": "RegularTransaction",
                "id": 1,
                "hash": "ab".repeat(32),
                "transactionResult": {
                    "status": "SUCCESS",
                    "segments": [{"id": 0, "success": true}],
                },
            }]
        }
    })
    .to_string()
}

fn provider(url: &str) -> MidnightProvider {
    // The node URL is never dialed in this path.
    MidnightProvider::new("ws://127.0.0.1:1", url).unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The indexer lags two polls behind, then surfaces the result: the wait
/// keeps polling and reports `Found` with the chain-side status.
#[tokio::test]
async fn found_after_indexer_lag() {
    let (url, state) = spawn_mock(2).await;
    let provider = provider(&url);

    let outcome = provider
        .wait_transaction_result(
            &[0xab; 32],
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await
        .expect("polling the mock indexer must not error");

    let result = match outcome {
        TxResultWait::Found(result) => result,
        TxResultWait::TimedOut => panic!("expected Found, got TimedOut"),
    };
    assert_eq!(result.status, TransactionResultStatus::Success);
    assert!(
        state.requests.load(Ordering::SeqCst) >= 3,
        "the wait must have kept polling through the lag"
    );
}

/// The result never surfaces within the deadline: the wait reports
/// `TimedOut` — a provisional outcome, not evidence the tx never landed.
#[tokio::test]
async fn timeout_when_result_never_surfaces() {
    let (url, state) = spawn_mock(usize::MAX).await;
    let provider = provider(&url);

    // Generous deadline-to-poll ratio (1s / 50ms) so a single slow CI
    // round-trip can't eat the whole budget and flake the `>= 2` polls
    // assertion below.
    let outcome = provider
        .wait_transaction_result(
            &[0xcd; 32],
            Duration::from_secs(1),
            Duration::from_millis(50),
        )
        .await
        .expect("polling the mock indexer must not error");

    assert_eq!(outcome, TxResultWait::TimedOut);
    assert_eq!(outcome.found(), None);
    assert!(
        state.requests.load(Ordering::SeqCst) >= 2,
        "the wait must poll more than once before timing out"
    );
}
