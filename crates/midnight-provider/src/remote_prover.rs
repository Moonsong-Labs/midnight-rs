//! Remote proof-server client.
//!
//! [`RemoteProofServer`] implements [`ProofProvider`] by delegating ZK proving
//! to an HTTP proof server (e.g. midnightntwrk/proof-server) over its `/check`
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

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use midnight_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use midnight_helpers::mn_ledger::error::TransactionProvingError;
use midnight_helpers::mn_ledger::structure::{ProofPreimageVersioned, ProofVersioned};
use midnight_helpers::transient_crypto::curve::Fr;
use midnight_helpers::transient_crypto::proofs::{
    KeyLocation, Proof, ProofPreimage, ProvingProvider, Resolver as ResolverTrait, WrappedIr,
};
use midnight_helpers::{
    CostModel, DB, PedersenRandomness, ProofMarker, ProofPreimageMarker, ProofProvider, Resolver,
    Signature, StdRng, Transaction,
};
use tracing::{info, warn};

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
/// and rebuild it as [`WalletError::Proving`](midnight_wallet::WalletError).
///
/// They convert **any** panic from the proving future, not only ones carrying
/// this prefix, and that is on purpose: the default backend is the local
/// prover, which panics through an upstream `.expect(...)` whose message this
/// crate does not control. Filtering on the prefix would leave the default
/// backend uncovered.
pub const PROVING_PANIC_PREFIX: &str = "midnight-rs proving failed";

/// Whether a proving attempt failed for a reason another attempt could fix.
///
/// Only the inner proof-server exchange and transport-level I/O can be
/// transient. A transcript that does not match the circuit, or a keyset the
/// resolver cannot supply, fails identically every time.
fn is_transient_attempt<D: DB>(err: &TransactionProvingError<D>) -> bool {
    use std::io::ErrorKind;
    match err {
        TransactionProvingError::Proving(e) => is_transient(e),
        TransactionProvingError::Tokio(io) => matches!(
            io.kind(),
            ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::TimedOut
                | ErrorKind::Interrupted
        ),
        _ => false,
    }
}

/// Total wall-clock budget for proving (including retries).
const PROOF_SERVER_TIMEOUT: Duration = Duration::from_secs(30);
/// Initial backoff delay between retries.
const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
/// Cap on the per-retry backoff delay.
const MAX_BACKOFF: Duration = Duration::from_secs(5);

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
    url: String,
    client: reqwest::Client,
}

impl RemoteProofServer {
    /// Create a client for the proof server reachable at `url` (its base URL,
    /// e.g. `http://localhost:6300`; the `/check` and `/prove` paths are
    /// appended per request).
    pub fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl<D: DB + Clone> ProofProvider<D> for RemoteProofServer {
    async fn prove(
        &self,
        tx: Transaction<Signature, ProofPreimageMarker, PedersenRandomness, D>,
        _rng: StdRng,
        resolver: &Resolver,
        cost_model: &CostModel,
    ) -> Transaction<Signature, ProofMarker, PedersenRandomness, D> {
        info!(url = %self.url, "remote proving");

        let start = Instant::now();
        let mut delay = INITIAL_BACKOFF;
        loop {
            let client = ProofServerClient {
                base_url: self.url.clone(),
                resolver,
                http: self.client.clone(),
            };
            match tx.clone().prove(client, cost_model).await {
                Ok(proven) => return proven,
                Err(err)
                    if is_transient_attempt(&err)
                        && start.elapsed() + delay < PROOF_SERVER_TIMEOUT =>
                {
                    warn!(?err, retry_in = ?delay, "remote proving failed, retrying");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(MAX_BACKOFF);
                }
                // `ProofProvider::prove` returns a bare transaction, so there is
                // no error channel to return through here. Panicking with a
                // recognisable prefix is the only way out; the wallet's proving
                // call site catches it and rebuilds a typed error, so callers
                // never see the unwind. See `PROVING_PANIC_PREFIX`.
                Err(err) => {
                    let waited = start.elapsed();
                    if is_transient_attempt(&err) {
                        panic!(
                            "{PROVING_PANIC_PREFIX}: still failing after {waited:?} \
                             (budget {PROOF_SERVER_TIMEOUT:?}): {err}"
                        );
                    }
                    panic!("{PROVING_PANIC_PREFIX}: {err}");
                }
            }
        }
    }
}

/// One-shot [`ProvingProvider`] that speaks the proof server's `/check` and
/// `/prove` HTTP protocol. Holds a borrow of the [`Resolver`] so it can attach
/// each circuit's IR to requests for non-builtin keys.
#[derive(Clone)]
struct ProofServerClient<'a> {
    base_url: String,
    resolver: &'a Resolver,
    http: reqwest::Client,
}

impl ProofServerClient<'_> {
    /// Keys the proof server already has built in: no circuit IR is sent for
    /// these, the server resolves them itself.
    fn is_builtin_key(loc: &KeyLocation) -> bool {
        [
            "midnight/zswap/spend",
            "midnight/zswap/output",
            "midnight/zswap/sign",
            "midnight/dust/spend",
        ]
        .contains(&loc.0.as_ref())
    }

    /// Serialize the `/check` request body: the preimage, plus the circuit's IR
    /// for non-builtin keys.
    async fn check_request_body(
        &self,
        preimage: &ProofPreimageVersioned,
    ) -> Result<Vec<u8>, anyhow::Error> {
        let ir = if Self::is_builtin_key(preimage.key_location()) {
            None
        } else {
            let data = self
                .resolver
                .resolve_key(preimage.key_location().clone())
                .await?
                .ok_or_else(|| {
                    ProofServerError::MissingKey(preimage.key_location().0.to_string())
                })?;
            Some(WrappedIr(data.ir_source))
        };
        let mut res = Vec::new();
        tagged_serialize(&(preimage.clone(), ir), &mut res)?;
        Ok(res)
    }

    /// Serialize the `/prove` request body: the preimage, the resolved key
    /// material for non-builtin keys, and the optional binding-input override.
    async fn proving_request_body(
        &self,
        preimage: &ProofPreimageVersioned,
        overwrite_binding_input: Option<Fr>,
    ) -> Result<Vec<u8>, anyhow::Error> {
        let data = if Self::is_builtin_key(preimage.key_location()) {
            None
        } else {
            self.resolver
                .resolve_key(preimage.key_location().clone())
                .await?
        };
        let mut res = Vec::new();
        tagged_serialize(&(preimage.clone(), data, overwrite_binding_input), &mut res)?;
        Ok(res)
    }
}

impl ProvingProvider for ProofServerClient<'_> {
    async fn check(&self, preimage: &ProofPreimage) -> Result<Vec<Option<usize>>, anyhow::Error> {
        let ser = self
            .check_request_body(&ProofPreimageVersioned::V2(Arc::new(preimage.clone())))
            .await?;
        let resp = self
            .http
            .post(format!("{}/check", self.base_url))
            .body(ser)
            .send()
            .await?;
        if resp.status().is_success() {
            let bytes = resp.bytes().await?;
            let res: Vec<Option<u64>> = tagged_deserialize(&mut bytes.to_vec().as_slice())?;
            Ok(res.into_iter().map(|i| i.map(|i| i as usize)).collect())
        } else {
            Err(ProofServerError::Http {
                endpoint: "/check",
                status: resp.status().as_u16(),
                body: resp.text().await.unwrap_or_default(),
            }
            .into())
        }
    }

    async fn prove(
        self,
        preimage: &ProofPreimage,
        overwrite_binding_input: Option<Fr>,
    ) -> Result<Proof, anyhow::Error> {
        let ser = self
            .proving_request_body(
                &ProofPreimageVersioned::V2(Arc::new(preimage.clone())),
                overwrite_binding_input,
            )
            .await?;
        let resp = self
            .http
            .post(format!("{}/prove", self.base_url))
            .body(ser)
            .send()
            .await?;
        if resp.status().is_success() {
            let bytes = resp.bytes().await?;
            let proof: ProofVersioned = tagged_deserialize(&mut bytes.to_vec().as_slice())?;
            match proof {
                ProofVersioned::V2(proof) => Ok(proof),
                other => {
                    Err(ProofServerError::UnsupportedProofVersion(format!("{other:?}")).into())
                }
            }
        } else {
            Err(ProofServerError::Http {
                endpoint: "/prove",
                status: resp.status().as_u16(),
                body: resp.text().await.unwrap_or_default(),
            }
            .into())
        }
    }

    fn split(&mut self) -> Self {
        self.clone()
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
