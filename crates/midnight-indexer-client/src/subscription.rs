use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, warn};

use crate::error::IndexerError;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_subscription_id() -> String {
    NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string()
}

/// A handle to a running GraphQL subscription.
///
/// Receives deserialized `T` values from the `data` field of each `next` message.
/// Dropping the handle cancels the subscription.
pub struct Subscription<T> {
    rx: mpsc::Receiver<Result<T, IndexerError>>,
    _cancel: tokio::sync::oneshot::Sender<()>,
}

impl<T> Subscription<T> {
    /// Receive the next event from the subscription.
    ///
    /// Returns `None` when the server completes the subscription or the
    /// connection drops.
    pub async fn next(&mut self) -> Option<Result<T, IndexerError>> {
        self.rx.recv().await
    }
}

/// A WebSocket connection to the indexer's GraphQL subscription endpoint.
///
/// Supports the `graphql-transport-ws` protocol (used by modern GraphQL servers).
pub struct SubscriptionClient {
    ws_url: String,
}

impl SubscriptionClient {
    /// Create a new subscription client.
    ///
    /// `ws_url` should be the base indexer URL (e.g. `http://127.0.0.1:8088`)
    /// or the full WebSocket subscription path. The client will normalize the
    /// URL to the subscription endpoint at `/api/v3/graphql/ws`.
    pub fn new(ws_url: impl Into<String>) -> Self {
        let raw: String = ws_url.into();
        let base = raw.trim_end_matches('/');
        let mut url = if base.ends_with("/graphql/ws") {
            base.to_string()
        } else if base.ends_with("/graphql") {
            format!("{base}/ws")
        } else {
            format!("{base}/api/v3/graphql/ws")
        };
        // Ensure ws:// or wss:// scheme
        if url.starts_with("http://") {
            url = format!("ws://{}", &url[7..]);
        } else if url.starts_with("https://") {
            url = format!("wss://{}", &url[8..]);
        }
        Self { ws_url: url }
    }

    pub fn url(&self) -> &str {
        &self.ws_url
    }

    /// Subscribe to a GraphQL subscription query.
    ///
    /// Returns a [`Subscription`] handle that yields deserialized events.
    /// The subscription is cancelled when the handle is dropped.
    pub async fn subscribe<T: DeserializeOwned + Send + 'static>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<Subscription<T>, IndexerError> {
        self.subscribe_with_protocol(query, variables, "graphql-transport-ws")
            .await
    }

    async fn subscribe_with_protocol<T: DeserializeOwned + Send + 'static>(
        &self,
        query: &str,
        variables: serde_json::Value,
        protocol: &str,
    ) -> Result<Subscription<T>, IndexerError> {
        use tokio_tungstenite::tungstenite::http::Request;

        let request = Request::builder()
            .uri(&self.ws_url)
            .header("Sec-WebSocket-Protocol", protocol)
            .header("Host", host_from_url(&self.ws_url))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .map_err(|e| IndexerError::Config(format!("build WS request: {e}")))?;

        let (ws_stream, _response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| IndexerError::Config(format!("WS connect to {}: {e}", self.ws_url)))?;

        let (mut sink, mut stream) = ws_stream.split();

        // connection_init
        let init = serde_json::json!({"type": "connection_init"});
        sink.send(Message::Text(init.to_string().into()))
            .await
            .map_err(|e| IndexerError::Config(format!("send connection_init: {e}")))?;

        // Wait for connection_ack (handle Ping frames during handshake)
        let ack_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let msg = tokio::time::timeout_at(ack_deadline, stream.next())
                .await
                .map_err(|_| IndexerError::Config("timeout waiting for connection_ack".into()))?
                .ok_or_else(|| IndexerError::Config("WS closed before connection_ack".into()))?
                .map_err(|e| IndexerError::Config(format!("read connection_ack: {e}")))?;

            match msg {
                Message::Ping(payload) => {
                    let _ = sink.send(Message::Pong(payload)).await;
                    continue;
                }
                Message::Text(text) => {
                    let ack_msg: serde_json::Value =
                        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
                    if ack_msg.get("type").and_then(|v| v.as_str()) == Some("connection_ack") {
                        break;
                    }
                    return Err(IndexerError::Config(format!(
                        "expected connection_ack, got: {text}"
                    )));
                }
                Message::Close(_) => {
                    return Err(IndexerError::Config(
                        "WS closed before connection_ack".into(),
                    ));
                }
                _ => continue,
            }
        }

        debug!("WS connection_ack received");

        // Send subscribe
        let sub_id = next_subscription_id();
        let subscribe_msg = serde_json::json!({
            "type": "subscribe",
            "id": sub_id,
            "payload": {
                "query": query,
                "variables": variables,
            }
        });
        sink.send(Message::Text(subscribe_msg.to_string().into()))
            .await
            .map_err(|e| IndexerError::Config(format!("send subscribe: {e}")))?;

        // Spawn a task to read messages and forward them
        let (tx, rx) = mpsc::channel(64);
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let expected_id = sub_id.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        // Send complete to server
                        let stop = serde_json::json!({
                            "type": "complete",
                            "id": expected_id,
                        });
                        let _ = sink.send(Message::Text(stop.to_string().into())).await;
                        break;
                    }
                    msg = stream.next() => {
                        let Some(msg) = msg else { break };
                        let Ok(msg) = msg else {
                            warn!("WS read error, closing subscription");
                            break;
                        };
                        let text = match msg {
                            Message::Text(t) => t,
                            Message::Ping(payload) => {
                                let _ = sink.send(Message::Pong(payload)).await;
                                continue;
                            }
                            Message::Close(_) => break,
                            _ => continue,
                        };
                        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) else {
                            continue;
                        };
                        let msg_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match msg_type {
                            "next" => {
                                if parsed.get("id").and_then(|v| v.as_str()) != Some(&expected_id) {
                                    continue;
                                }
                                if let Some(payload) = parsed.get("payload").and_then(|p| p.get("data")).filter(|d| !d.is_null()) {
                                    match serde_json::from_value::<T>(payload.clone()) {
                                        Ok(val) => {
                                            if tx.send(Ok(val)).await.is_err() {
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            let _ = tx.send(Err(IndexerError::Deserialization(
                                                format!("subscription event: {e}")
                                            ))).await;
                                        }
                                    }
                                }
                            }
                            "error" => {
                                let err_msg = parsed
                                    .get("payload")
                                    .map(|p| p.to_string())
                                    .unwrap_or_else(|| "unknown error".into());
                                let _ = tx.send(Err(IndexerError::Config(
                                    format!("subscription error: {err_msg}")
                                ))).await;
                                break;
                            }
                            "complete" => break,
                            _ => {}
                        }
                    }
                }
            }
        });

        Ok(Subscription {
            rx,
            _cancel: cancel_tx,
        })
    }
}

fn host_from_url(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))
        .unwrap_or(url);
    without_scheme
        .split('/')
        .next()
        .unwrap_or("localhost")
        .to_string()
}

/// GraphQL subscription queries for the Midnight indexer.
pub mod queries {
    pub const BLOCKS_SUBSCRIPTION: &str = r#"
        subscription Blocks($offset: BlockOffset) {
            blocks(offset: $offset) {
                hash
                height
                protocolVersion
                timestamp
                transactions {
                    __typename
                    ... on RegularTransaction {
                        id
                        hash
                        unshieldedCreatedOutputs {
                            owner
                            tokenType
                            value
                            intentHash
                            outputIndex
                        }
                        unshieldedSpentOutputs {
                            owner
                            tokenType
                            value
                            intentHash
                            outputIndex
                        }
                    }
                    ... on SystemTransaction {
                        id
                        hash
                    }
                }
            }
        }
    "#;

    pub const UNSHIELDED_TRANSACTIONS_SUBSCRIPTION: &str = r#"
        subscription UnshieldedTransactions($address: UnshieldedAddress!, $transactionId: Int) {
            unshieldedTransactions(address: $address, transactionId: $transactionId) {
                __typename
                ... on UnshieldedTransaction {
                    transaction {
                        id
                        hash
                        block { height }
                    }
                    createdUtxos {
                        owner
                        tokenType
                        value
                        intentHash
                        outputIndex
                    }
                    spentUtxos {
                        owner
                        tokenType
                        value
                        intentHash
                        outputIndex
                    }
                }
                ... on UnshieldedTransactionsProgress {
                    highestTransactionId
                }
            }
        }
    "#;
}
