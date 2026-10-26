//! Remote proof-server client.
//!
//! [`RemoteProofServer`] implements each ledger generation's `ProofProvider`
//! (see the generation modules) by delegating ZK proving to an HTTP proof
//! server (e.g. midnightntwrk/proof-server) over its `/check`
//! and `/prove` endpoints. Each request has a deadline, and a transient
//! failure is retried with exponential backoff before that deadline.
//!
//! Each ledger's `ProofServerProvider` builds the request bodies: the
//! tagged-serialized preimage, plus the circuit's IR or keys when the server
//! has no built-in key for it. This client does not use the `ProvingProvider`
//! impl of that type. That impl prints with `println!`, opens a new HTTP client
//! for each request, and returns an untyped error for a failed status, which
//! the retry loop cannot classify.

use std::time::{Duration, Instant};

use tracing::warn;

/// Failures this client raises itself, typed so the retry loop can tell a
/// transient outage from a permanent rejection. They travel as `anyhow::Error`
/// because that is what the ledger's `ProvingProvider` trait deals in, and are
/// recovered by downcasting.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProofServerError {
    /// The proof server rejected the request. `status` decides whether
    /// retrying can help.
    #[error("proof server {endpoint} error ({status}): {body}")]
    Http {
        endpoint: &'static str,
        status: u16,
        body: String,
    },
    /// The server answered with a proof this SDK cannot represent.
    #[error("proof server returned an unsupported proof version: {0}")]
    UnsupportedProofVersion(String),
}

/// POST `body` to the proof server's `endpoint` and return the response body.
///
/// `timeout` bounds the whole request, retries included. A transient failure
/// (see [`is_transient`]) is retried at most [`MAX_RETRIES`] times, and only
/// while the backoff ends before the deadline. A non-2xx status becomes
/// [`ProofServerError::Http`]. A transport failure, a timeout included, passes
/// through as a `reqwest::Error`.
pub(crate) async fn post(
    http: &reqwest::Client,
    base_url: &str,
    endpoint: &'static str,
    body: Vec<u8>,
    timeout: Duration,
) -> Result<Vec<u8>, anyhow::Error> {
    let url = format!("{base_url}{endpoint}");
    let start = Instant::now();
    let remaining = || timeout.saturating_sub(start.elapsed());
    let mut delay = INITIAL_BACKOFF;
    let mut retries = 0;
    loop {
        let request = http.post(&url).body(body.clone()).timeout(remaining());
        match send(request, endpoint).await {
            Ok(bytes) => return Ok(bytes),
            Err(err) if retries < MAX_RETRIES && is_transient(&err) && delay < remaining() => {
                warn!(endpoint, ?err, retry_in = ?delay, "proof server request failed, retrying");
                tokio::time::sleep(delay).await;
                delay *= 2;
                retries += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn send(
    request: reqwest::RequestBuilder,
    endpoint: &'static str,
) -> Result<Vec<u8>, anyhow::Error> {
    let resp = request.send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ProofServerError::Http {
            endpoint,
            status: status.as_u16(),
            body: resp.text().await.unwrap_or_default(),
        }
        .into());
    }
    Ok(resp.bytes().await?.to_vec())
}

/// Whether `err` is worth retrying.
///
/// Only a server-side outage or a transport failure is. Everything else is
/// deterministic: a missing key, a malformed request, an unsupported proof
/// version or a decode failure produces the same result on every attempt, so
/// retrying it only delays the report.
///
/// This client retries any HTTP 5xx, which is deliberately wider than
/// midnight-js (it retries 500 and 503 only). A proof server behind a load
/// balancer can answer 502 or 504 while it restarts, and those are outages like
/// any other.
pub(crate) fn is_transient(err: &anyhow::Error) -> bool {
    if let Some(ProofServerError::Http { status, .. }) = err.downcast_ref::<ProofServerError>() {
        return (500..600).contains(status);
    }
    // Transport-level failures never reached the server, so the request may
    // still succeed once it does. `is_request` is reqwest's `Kind::Request`,
    // raised when the client fails to send (connection refused, reset, timed
    // out); a malformed URL or header is `Kind::Builder`, which none of these
    // predicates match, so a misconfigured client still fails fast.
    if let Some(req) = err.downcast_ref::<reqwest::Error>() {
        return req.is_timeout() || req.is_connect() || req.is_request();
    }
    false
}

// The deadline, the retry count and the backoff are those of midnight-js.
/// The default deadline of one `/check` or `/prove` request, retries included.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// The most retries of one request.
const MAX_RETRIES: u32 = 3;
/// The delay before the first retry. Each retry doubles it.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Proof backend that delegates proving to a remote HTTP proof server.
///
/// Construct one with [`RemoteProofServer::new`] and hand it to
/// [`MidnightProvider::with_proof_provider`](crate::MidnightProvider::with_proof_provider):
///
/// ```rust,no_run
/// # fn f() -> anyhow::Result<()> {
/// # const NODE_URL: &str = "ws://localhost:9944";
/// use std::sync::Arc;
/// use midnight_provider::{MidnightProvider, RemoteProofServer};
///
/// let prover = Arc::new(RemoteProofServer::new("http://localhost:6300".to_string()));
/// let provider = MidnightProvider::new(NODE_URL).with_proof_provider(prover);
/// # Ok(())
/// # }
/// ```
///
/// A failed proof unwinds `prove` with a `String` message. A build through
/// `with_proof_provider` returns that message as
/// [`WalletError::Proving`](crate::WalletError::Proving). A caller of `prove`
/// outside a build must catch the unwind (for example with
/// `FutureExt::catch_unwind`) to get the message.
pub struct RemoteProofServer {
    pub(crate) url: String,
    pub(crate) timeout: Duration,
}

impl RemoteProofServer {
    /// Create a client for the proof server reachable at `url` (its base URL,
    /// e.g. `http://localhost:6300`; the `/check` and `/prove` paths are
    /// appended per request).
    pub fn new(url: String) -> Self {
        Self {
            url,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Set the deadline of each `/check` or `/prove` request (default: 300 s).
    ///
    /// The deadline covers the retries of the request. When a request does not
    /// succeed before its deadline, the proof fails.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    /// A proof server that records the body of each request and then sends
    /// `answer`, or holds the connection with no answer when `answer` is
    /// `None`. It returns its URL and the recorded bodies.
    async fn proof_server(answer: Option<&'static [u8]>) -> (String, Arc<Mutex<Vec<Vec<u8>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&bodies);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let recorded = Arc::clone(&recorded);
                tokio::spawn(async move {
                    let body = read_request_body(&mut stream).await;
                    recorded.lock().unwrap().push(body);
                    match answer {
                        Some(answer) => stream.write_all(answer).await.unwrap(),
                        None => std::future::pending().await,
                    }
                });
            }
        });
        (url, bodies)
    }

    /// Read one HTTP request to its end, so that the close after the answer
    /// does not reset the connection, and return its body.
    async fn read_request_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let read = stream.read(&mut chunk).await.unwrap();
            assert_ne!(read, 0, "the client closed before the end of the request");
            request.extend_from_slice(&chunk[..read]);
            let Some(head_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&request[..head_end]).to_ascii_lowercase();
            let body_len: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |len| len.trim().parse().unwrap());
            let body = head_end + 4..head_end + 4 + body_len;
            if request.len() >= body.end {
                return request[body].to_vec();
            }
        }
    }

    /// A timed-out attempt leaves no time for a retry.
    #[tokio::test]
    async fn a_server_that_does_not_answer_fails_the_request_at_its_deadline() {
        let (url, bodies) = proof_server(None).await;
        let http = reqwest::Client::new();

        let request = post(
            &http,
            &url,
            "/prove",
            b"preimage".to_vec(),
            Duration::from_millis(200),
        );
        let err = tokio::time::timeout(Duration::from_secs(10), request)
            .await
            .expect("the deadline must end the request")
            .unwrap_err();

        assert!(
            err.downcast_ref::<reqwest::Error>()
                .is_some_and(reqwest::Error::is_timeout),
            "{err:#}"
        );
        assert_eq!(bodies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_5xx_is_retried_with_the_same_body_until_the_retries_run_out() {
        let (url, bodies) = proof_server(Some(
            b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        ))
        .await;

        let err = post(
            &reqwest::Client::new(),
            &url,
            "/prove",
            b"preimage".to_vec(),
            Duration::from_secs(60),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(
                err.downcast_ref::<ProofServerError>(),
                Some(ProofServerError::Http { status: 503, .. })
            ),
            "{err:#}"
        );
        let attempts = MAX_RETRIES as usize + 1;
        assert_eq!(
            *bodies.lock().unwrap(),
            vec![b"preimage".to_vec(); attempts]
        );
    }

    #[test]
    fn a_5xx_is_transient_and_a_4xx_or_decode_failure_is_not() {
        let http = |status| {
            anyhow::Error::new(ProofServerError::Http {
                endpoint: "/prove",
                status,
                body: String::new(),
            })
        };
        let cases = [
            (http(400), false),
            (http(404), false),
            (http(422), false),
            (http(500), true),
            (http(502), true),
            (http(503), true),
            (anyhow::anyhow!("tagged_deserialize: unexpected tag"), false),
        ];
        for (err, transient) in cases {
            assert_eq!(is_transient(&err), transient, "{err}");
        }
    }
}
