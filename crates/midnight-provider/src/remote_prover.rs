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
                Err(err) if start.elapsed() + delay < PROOF_SERVER_TIMEOUT => {
                    warn!(?err, retry_in = ?delay, "remote proving failed, retrying");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(MAX_BACKOFF);
                }
                Err(err) => {
                    panic!("remote proving exhausted {PROOF_SERVER_TIMEOUT:?} budget: {err:?}");
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
                    anyhow::anyhow!("failed to find key '{}'", preimage.key_location().0)
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
            anyhow::bail!(
                "proof server /check error ({}): {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )
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
                    anyhow::bail!("proof server returned unsupported proof version: {other:?}")
                }
            }
        } else {
            anyhow::bail!(
                "proof server /prove error ({}): {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )
        }
    }

    fn split(&mut self) -> Self {
        self.clone()
    }
}
