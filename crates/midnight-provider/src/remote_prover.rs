//! Remote proof-server client.
//!
//! [`RemoteProofServer`] implements each ledger generation's `ProofProvider`
//! (see the generation modules) by delegating ZK proving to an HTTP proof
//! server (e.g. midnightntwrk/proof-server) over its `/check`
//! and `/prove` endpoints, with retry / exponential backoff for transient
//! network errors.
//!
//! The wire protocol (a tagged-serialized preimage plus optional circuit IR,
//! POSTed to `/check` and `/prove`, with a tagged-serialized proof in the
//! response) is implemented here directly. It deliberately does **not** reuse
//! the ledger crate's `test_utilities::ProofServerProvider`: that type is
//! test-only scaffolding (behind the crate's test utilities, logging with
//! `println!` and asserting on fee bounds) and is not meant for production
//! proving.

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
    /// No proving key for this circuit. Points at the caller's zk config, not
    /// at the network.
    #[error("no proving key for circuit `{0}`; check the directory passed to `with_zk_config`")]
    MissingKey(String),
    /// The server answered with a proof this SDK cannot represent.
    #[error("proof server returned an unsupported proof version: {0}")]
    UnsupportedProofVersion(String),
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

/// Prefix on the panic message a terminal proving failure raises, so the cause
/// is recognisable in logs and in the error the caller finally sees.
///
/// The ledger's `ProofProvider::prove` returns a bare transaction, so a failure
/// has nowhere to go but the unwind. The proving call sites catch that unwind
/// and rebuild it as [`WalletError::Proving`](midnight_types::WalletError).
///
/// They convert **any** panic from the proving future, not only ones carrying
/// this prefix, and that is on purpose: the default backend is the local
/// prover, which panics through an upstream `.expect(...)` whose message this
/// crate does not control. Filtering on the prefix would leave the default
/// backend uncovered.
pub const PROVING_PANIC_PREFIX: &str = "midnight-rs proving failed";

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
/// ```rust,ignore
/// use std::sync::Arc;
/// use midnight_provider::{MidnightProvider, RemoteProofServer};
///
/// let prover = Arc::new(RemoteProofServer::new("http://localhost:6300".to_string()));
/// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?.with_proof_provider(prover);
/// ```
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
    fn missing_key_is_permanent() {
        let err = anyhow::Error::new(ProofServerError::MissingKey("counter/increment".into()));
        assert!(
            !is_transient(&err),
            "a missing proving key never resolves by retrying"
        );
        assert!(
            err.to_string().contains("counter/increment"),
            "the error must name the key"
        );
    }

    #[test]
    fn client_errors_are_permanent() {
        for status in [400u16, 404, 422] {
            let err = anyhow::Error::new(ProofServerError::Http {
                endpoint: "/prove",
                status,
                body: "bad request".into(),
            });
            assert!(
                !is_transient(&err),
                "HTTP {status} is a permanent rejection"
            );
        }
    }

    #[test]
    fn server_errors_are_transient() {
        for status in [500u16, 502, 503] {
            let err = anyhow::Error::new(ProofServerError::Http {
                endpoint: "/prove",
                status,
                body: "upstream down".into(),
            });
            assert!(is_transient(&err), "HTTP {status} is worth retrying");
        }
    }

    #[test]
    fn unsupported_proof_version_is_permanent() {
        let err = anyhow::Error::new(ProofServerError::UnsupportedProofVersion("V1".into()));
        assert!(!is_transient(&err));
    }

    /// Anything we cannot classify (serialization failures, ledger-side errors)
    /// is treated as permanent: retrying a deterministic failure just delays
    /// the report by the whole budget.
    #[test]
    fn unclassified_errors_are_permanent() {
        let err = anyhow::anyhow!("tagged_deserialize: unexpected tag");
        assert!(!is_transient(&err));
    }
}
