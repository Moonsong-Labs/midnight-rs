//! HTTP plumbing shared by the mock indexers in this crate's integration
//! tests (`tx_result_wait.rs`, `lock_hygiene.rs`). Each test file keeps its
//! own routing and response bodies; this module only owns the
//! protocol-neutral parts: reading one HTTP request (head plus
//! content-length body) and writing one JSON response.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Read one HTTP request (head plus its content-length body) off the stream
/// and return exactly the declared body. Returns `None` if the peer
/// disconnects or errors before the head completes or before the whole body
/// arrives, or if the head exceeds 64 KiB — the caller should bail without
/// responding. A caller parses the result, so a short read must not reach it
/// as a request in its own right.
pub async fn read_http_request_body(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        let Ok(n) = stream.read(&mut tmp).await else {
            return None;
        };
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            return None;
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
            return None;
        };
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = &buf[header_end..header_end + content_length];
    Some(String::from_utf8_lossy(body).to_string())
}

/// Write one `200 OK` JSON response and close the connection.
pub async fn write_json_response(stream: &mut TcpStream, body: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}
