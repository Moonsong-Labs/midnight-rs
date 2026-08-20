//! HTTP plumbing shared by the mock indexers in this crate's integration
//! tests (`tx_result_wait.rs`, `lock_hygiene.rs`). Each test file keeps its
//! own routing and response bodies; this module only owns the
//! protocol-neutral parts: reading one HTTP request (head plus
//! content-length body) and writing one JSON response.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Read one HTTP request (head plus its content-length body) off the
/// stream, discarding it. Returns `false` if the peer disconnects before
/// the head completes, errors mid-body, or the head exceeds 64 KiB — the
/// caller should bail without responding.
pub async fn read_http_request(stream: &mut TcpStream) -> bool {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        let Ok(n) = stream.read(&mut tmp).await else {
            return false;
        };
        if n == 0 {
            return false;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            return false;
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
            return false;
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    true
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
