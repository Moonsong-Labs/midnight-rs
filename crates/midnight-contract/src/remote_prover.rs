//! Remote proof server client.
//!
//! Implements [`ProofProvider`] by delegating to the ledger's
//! [`ProofServerProvider`] (the actual HTTP work) with retry / exponential
//! backoff for transient network errors.
//!
//! This used to be provided by `midnight-node-toolkit`, but that crate
//! transitively pulled in `sidechain-domain` / `cquisitor-lib` /
//! `pallas-primitives` / `uplc` (Cardano-side deps with their own version
//! conflicts). Reimplementing in 60 lines lets us drop the whole chain.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use midnight_helpers::{
    CostModel, DB, PedersenRandomness, ProofMarker, ProofPreimageMarker, ProofProvider,
    ProofServerProvider, Resolver, Signature, StdRng, Transaction,
};
use tracing::{info, warn};

/// Total wall-clock budget for proving (including retries).
const PROOF_SERVER_TIMEOUT: Duration = Duration::from_secs(30);
/// Initial backoff delay between retries.
const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
/// Cap on the per-retry backoff delay.
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// Client for an HTTP proof server (e.g. midnightntwrk/proof-server).
pub struct RemoteProofServer {
    url: String,
}

impl RemoteProofServer {
    pub fn new(url: String) -> Self {
        Self { url }
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
            let provider = ProofServerProvider {
                base_url: self.url.clone(),
                resolver,
            };
            match tx.clone().prove(provider, cost_model).await {
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
