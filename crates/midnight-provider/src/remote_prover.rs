//! Remote proof-server client.
//!
//! [`RemoteProofServer`] implements each ledger generation's `ProofProvider`
//! (see the generation modules) by delegating ZK proving to an HTTP proof
//! server (e.g. midnightntwrk/proof-server) over its `/check`
//! and `/prove` endpoints, with retry / exponential backoff for transient
//! network errors.
//!
//! Each ledger's `ProofServerProvider` builds the request bodies: the
//! tagged-serialized preimage, plus the circuit's IR or keys when the server
//! has no built-in key for it. This client does not use the `ProvingProvider`
//! impl of that type. That impl prints with `println!`, opens a new HTTP client
//! for each request, and returns an untyped error for a failed status, which
//! the retry loop cannot classify.

use std::time::Duration;

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
/// A non-2xx status becomes [`ProofServerError::Http`]. A transport failure
/// passes through as the `reqwest::Error` that [`is_transient`] reads.
pub(crate) async fn post(
    http: &reqwest::Client,
    base_url: &str,
    endpoint: &'static str,
    body: Vec<u8>,
) -> Result<Vec<u8>, anyhow::Error> {
    let resp = http
        .post(format!("{base_url}{endpoint}"))
        .body(body)
        .send()
        .await?;
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
/// retrying it just delays the report by the whole budget and then blames the
/// network.
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

/// Total wall-clock budget for proving (including retries).
pub(crate) const PROOF_SERVER_TIMEOUT: Duration = Duration::from_secs(30);
/// Initial backoff delay between retries.
pub(crate) const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
/// Cap on the per-retry backoff delay.
pub(crate) const MAX_BACKOFF: Duration = Duration::from_secs(5);

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
}

impl RemoteProofServer {
    /// Create a client for the proof server reachable at `url` (its base URL,
    /// e.g. `http://localhost:6300`; the `/check` and `/prove` paths are
    /// appended per request).
    pub fn new(url: String) -> Self {
        Self { url }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
