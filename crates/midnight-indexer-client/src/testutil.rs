//! Mock `graphql-transport-ws` server helpers for tests.
//!
//! Compiled only with the `test-util` feature and consumed as a dev-dependency
//! by the crates that exercise subscription behavior against a local mock
//! server: this crate's keepalive tests, `midnight-wallet`'s replay-loop
//! tests, and `midnight-provider`'s lock/cancellation tests. Not a public API:
//! everything here panics on unexpected input, which is the right behavior in
//! a test server and the wrong one anywhere else.

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

/// Server side of one accepted mock WebSocket connection.
pub type ServerWs = WebSocketStream<TcpStream>;

/// Bind a listener on an ephemeral local port and return it with the
/// matching `http://` base URL (the clients normalize it to `ws://` and the
/// GraphQL paths themselves).
pub async fn bind() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (listener, url)
}

/// Upgrade an accepted TCP stream to a WebSocket, echoing the requested
/// subprotocol (the `graphql-transport-ws` clients require the server to
/// confirm it).
pub async fn accept_ws(stream: TcpStream) -> ServerWs {
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

/// Accept one WS connection, run the `graphql-transport-ws` init/ack
/// handshake, and return the socket plus the parsed `subscribe` message.
pub async fn accept_subscriber(listener: &TcpListener) -> (ServerWs, serde_json::Value) {
    let (stream, _) = listener.accept().await.unwrap();
    subscriber_handshake(stream).await
}

/// Run the `graphql-transport-ws` init/ack handshake on an already-accepted
/// TCP stream (for servers that multiplex WS with other protocols on one
/// port) and return the socket plus the parsed `subscribe` message.
pub async fn subscriber_handshake(stream: TcpStream) -> (ServerWs, serde_json::Value) {
    let mut ws = accept_ws(stream).await;
    let init = next_json(&mut ws).await.expect("connection_init");
    assert_eq!(init["type"], "connection_init");
    send_json(&mut ws, &json!({"type": "connection_ack"})).await;
    let sub = next_json(&mut ws).await.expect("subscribe");
    assert_eq!(sub["type"], "subscribe");
    (ws, sub)
}

/// Read frames until a Text frame parses as JSON; answers WS Ping frames.
/// Returns `None` once the connection closes or errors.
pub async fn next_json(ws: &mut ServerWs) -> Option<serde_json::Value> {
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

/// Send a JSON value as a Text frame.
pub async fn send_json(ws: &mut ServerWs, v: &serde_json::Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// Wrap subscription `data` in a `next` message addressed to the subscription
/// id carried by `sub` (the parsed `subscribe` message) and send it.
pub async fn send_next(ws: &mut ServerWs, sub: &serde_json::Value, data: serde_json::Value) {
    let sub_id = sub["id"].as_str().unwrap();
    send_json(
        ws,
        &json!({"type": "next", "id": sub_id, "payload": {"data": data}}),
    )
    .await;
}
