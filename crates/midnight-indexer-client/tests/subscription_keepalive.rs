//! Mock-WebSocket-server tests for the `graphql-transport-ws` subscription
//! client: keepalive pings, idle timeout, typed transport vs protocol
//! errors, and connection-drop behavior. No real indexer required.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use midnight_indexer_client::{IndexerError, SubscriptionClient};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

type ServerWs = WebSocketStream<TcpStream>;

const QUERY: &str = "subscription { events { value } }";

async fn bind() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (listener, url)
}

/// Accept one WS connection (echoing the requested subprotocol), perform the
/// `graphql-transport-ws` init/ack handshake, and return the socket plus the
/// parsed `subscribe` message.
async fn accept_subscriber(listener: &TcpListener) -> (ServerWs, serde_json::Value) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut ws = accept_ws(stream).await;
    let init = next_json(&mut ws).await.expect("connection_init");
    assert_eq!(init["type"], "connection_init");
    send_json(&mut ws, &json!({"type": "connection_ack"})).await;
    let sub = next_json(&mut ws).await.expect("subscribe");
    assert_eq!(sub["type"], "subscribe");
    (ws, sub)
}

async fn accept_ws(stream: TcpStream) -> ServerWs {
    // The Err size is fixed by tungstenite's `Callback` trait.
    #[allow(clippy::result_large_err)]
    fn echo_subprotocol(req: &Request, mut resp: Response) -> Result<Response, ErrorResponse> {
        if let Some(proto) = req.headers().get("Sec-WebSocket-Protocol") {
            resp.headers_mut()
                .insert("Sec-WebSocket-Protocol", proto.clone());
        }
        Ok(resp)
    }
    tokio_tungstenite::accept_hdr_async(stream, echo_subprotocol)
        .await
        .unwrap()
}

/// Read frames until a Text frame parses as JSON; answers WS Ping frames.
/// Returns `None` once the connection closes or errors.
async fn next_json(ws: &mut ServerWs) -> Option<serde_json::Value> {
    while let Some(msg) = ws.next().await {
        match msg.ok()? {
            Message::Text(t) => return serde_json::from_str(&t).ok(),
            Message::Ping(p) => {
                let _ = ws.send(Message::Pong(p)).await;
            }
            Message::Close(_) => return None,
            _ => {}
        }
    }
    None
}

async fn send_json(ws: &mut ServerWs, v: &serde_json::Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

fn next_msg(sub_id: &str, data: serde_json::Value) -> serde_json::Value {
    json!({"type": "next", "id": sub_id, "payload": {"data": data}})
}

async fn recv(
    sub: &mut midnight_indexer_client::Subscription<serde_json::Value>,
) -> Option<Result<serde_json::Value, IndexerError>> {
    tokio::time::timeout(Duration::from_secs(10), sub.next())
        .await
        .expect("subscription.next() must resolve within the test bound")
}

#[tokio::test]
async fn events_flow_and_client_pings_a_quiet_server() {
    let (listener, url) = bind().await;
    let server = tokio::spawn(async move {
        let (mut ws, sub) = accept_subscriber(&listener).await;
        let sub_id = sub["id"].as_str().unwrap().to_string();
        send_json(&mut ws, &next_msg(&sub_id, json!({"value": 1}))).await;
        // Go silent until the client's keepalive ping arrives; answer with
        // pong, then prove the connection survived by sending more data.
        loop {
            let msg = next_json(&mut ws).await.expect("client frame");
            if msg["type"] == "ping" {
                send_json(&mut ws, &json!({"type": "pong"})).await;
                break;
            }
        }
        send_json(&mut ws, &next_msg(&sub_id, json!({"value": 2}))).await;
        // Hold the socket open until the client disconnects.
        while next_json(&mut ws).await.is_some() {}
    });

    let client = SubscriptionClient::new(&url)
        .with_keepalive(Duration::from_millis(100), Duration::from_secs(5));
    let mut sub = client
        .subscribe::<serde_json::Value>(QUERY, json!({}))
        .await
        .unwrap();

    assert_eq!(recv(&mut sub).await.unwrap().unwrap()["value"], 1);
    assert_eq!(recv(&mut sub).await.unwrap().unwrap()["value"], 2);

    drop(sub);
    server.await.unwrap();
}

#[tokio::test]
async fn silent_server_times_out_with_retryable_transport_error() {
    let (listener, url) = bind().await;
    let server = tokio::spawn(async move {
        let (mut ws, _sub) = accept_subscriber(&listener).await;
        // Stay silent and swallow the client's keepalive pings.
        let mut got_ping = false;
        while let Some(msg) = next_json(&mut ws).await {
            if msg["type"] == "ping" {
                got_ping = true;
            }
        }
        got_ping
    });

    let client = SubscriptionClient::new(&url)
        .with_keepalive(Duration::from_millis(100), Duration::from_millis(400));
    let mut sub = client
        .subscribe::<serde_json::Value>(QUERY, json!({}))
        .await
        .unwrap();

    let err = recv(&mut sub).await.unwrap().unwrap_err();
    assert!(matches!(err, IndexerError::Transport(_)), "got: {err:?}");
    assert!(err.is_retryable());
    // The channel closes after the idle-timeout error.
    assert!(recv(&mut sub).await.is_none());

    assert!(server.await.unwrap(), "server never saw a keepalive ping");
}

#[tokio::test]
async fn abrupt_server_drop_yields_retryable_transport_error() {
    let (listener, url) = bind().await;
    let server = tokio::spawn(async move {
        let (mut ws, sub) = accept_subscriber(&listener).await;
        let sub_id = sub["id"].as_str().unwrap().to_string();
        send_json(&mut ws, &next_msg(&sub_id, json!({"value": 7}))).await;
        // Drop the socket without a close handshake.
        drop(ws);
    });

    let client = SubscriptionClient::new(&url);
    let mut sub = client
        .subscribe::<serde_json::Value>(QUERY, json!({}))
        .await
        .unwrap();

    assert_eq!(recv(&mut sub).await.unwrap().unwrap()["value"], 7);
    let err = recv(&mut sub).await.unwrap().unwrap_err();
    assert!(matches!(err, IndexerError::Transport(_)), "got: {err:?}");
    assert!(err.is_retryable());
    assert!(recv(&mut sub).await.is_none());

    server.await.unwrap();
}

#[tokio::test]
async fn graphql_error_message_is_fatal_protocol_error() {
    let (listener, url) = bind().await;
    let server = tokio::spawn(async move {
        let (mut ws, sub) = accept_subscriber(&listener).await;
        let sub_id = sub["id"].as_str().unwrap().to_string();
        send_json(
            &mut ws,
            &json!({
                "type": "error",
                "id": sub_id,
                "payload": [{"message": "unknown field"}],
            }),
        )
        .await;
        while next_json(&mut ws).await.is_some() {}
    });

    let client = SubscriptionClient::new(&url);
    let mut sub = client
        .subscribe::<serde_json::Value>(QUERY, json!({}))
        .await
        .unwrap();

    let err = recv(&mut sub).await.unwrap().unwrap_err();
    assert!(matches!(err, IndexerError::Protocol(_)), "got: {err:?}");
    assert!(!err.is_retryable());
    assert!(recv(&mut sub).await.is_none());

    server.await.unwrap();
}

#[tokio::test]
async fn connection_refused_is_retryable_transport_error() {
    // Bind to grab a free port, then drop the listener so connects fail.
    let (listener, url) = bind().await;
    drop(listener);

    let client = SubscriptionClient::new(&url);
    let err = client
        .subscribe::<serde_json::Value>(QUERY, json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, IndexerError::Transport(_)), "got: {err:?}");
    assert!(err.is_retryable());
}

#[tokio::test]
async fn missing_connection_ack_times_out_as_transport_error() {
    let (listener, url) = bind().await;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_ws(stream).await;
        // Read connection_init but never send connection_ack.
        let _ = next_json(&mut ws).await;
        while next_json(&mut ws).await.is_some() {}
    });

    let client = SubscriptionClient::new(&url).with_connect_timeout(Duration::from_millis(300));
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        client.subscribe::<serde_json::Value>(QUERY, json!({})),
    )
    .await
    .expect("subscribe must resolve within the test bound")
    .unwrap_err();
    assert!(matches!(err, IndexerError::Transport(_)), "got: {err:?}");
    assert!(err.is_retryable());

    server.abort();
}
