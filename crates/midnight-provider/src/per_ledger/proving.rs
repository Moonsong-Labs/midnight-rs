//! [`RemoteProofServer`] as this generation's `ProofProvider`, and the proof
//! server client it proves through.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tracing::{info, warn};

use super::helpers::midnight_serialize::tagged_deserialize;
use super::helpers::mn_ledger::error::TransactionProvingError;
use super::helpers::mn_ledger::structure::{ProofPreimageVersioned, ProofVersioned};
use super::helpers::transient_crypto::curve::Fr;
use super::helpers::transient_crypto::proofs::{Proof, ProofPreimage};
use super::helpers::{
    CostModel, DB, ProofProvider, ProofServerProvider, ProvenTransaction, Resolver, StdRng,
    UnprovenTransaction,
};
use crate::remote_prover::{
    INITIAL_BACKOFF, MAX_BACKOFF, PROOF_SERVER_TIMEOUT, ProofServerError, RemoteProofServer,
    is_transient, post,
};

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

#[async_trait]
impl<D: DB + Clone> ProofProvider<D> for RemoteProofServer {
    async fn prove(
        &self,
        tx: UnprovenTransaction<D>,
        _rng: StdRng,
        resolver: &'static Resolver,
        cost_model: CostModel,
    ) -> ProvenTransaction<D> {
        info!(url = %self.url, "remote proving");
        let base_url = self.url.clone();

        // A ledger 9 proof is not `Send`, so it runs to completion on a thread
        // of its own, as the upstream local prover does. The HTTP client lives
        // on that thread's runtime too: a pooled connection is bound to the
        // runtime that opened it, and this one ends with the proof.
        let proving = tokio::task::spawn_blocking(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime with I/O and timers")
                .block_on(prove_with_retries(
                    tx,
                    base_url,
                    reqwest::Client::new(),
                    resolver,
                    cost_model,
                ))
        });
        match proving.await {
            Ok(proven) => proven,
            // Re-raise on this task, so the caller's unwind handler sees the
            // prover's own message.
            Err(err) => std::panic::resume_unwind(err.into_panic()),
        }
    }
}

async fn prove_with_retries<D: DB + Clone>(
    tx: UnprovenTransaction<D>,
    base_url: String,
    http: reqwest::Client,
    resolver: &Resolver,
    cost_model: CostModel,
) -> ProvenTransaction<D> {
    let start = Instant::now();
    let mut delay = INITIAL_BACKOFF;
    loop {
        let client = ProofServerClient {
            server: ProofServerProvider {
                base_url: base_url.clone(),
                resolver,
            },
            http: http.clone(),
        };
        match tx.clone().prove(client, &cost_model).await {
            Ok(proven) => return proven,
            Err(err)
                if is_transient_attempt(&err) && start.elapsed() + delay < PROOF_SERVER_TIMEOUT =>
            {
                warn!(?err, retry_in = ?delay, "remote proving failed, retrying");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(MAX_BACKOFF);
            }
            // Unwind without the panic hook: the build reports this unwind as
            // `WalletError::Proving`, so stderr must not show a panic.
            Err(err) => {
                let message = if is_transient_attempt(&err) {
                    format!(
                        "still failing after {:?} (budget {PROOF_SERVER_TIMEOUT:?}): {err}",
                        start.elapsed()
                    )
                } else {
                    err.to_string()
                };
                std::panic::resume_unwind(Box::new(message));
            }
        }
    }
}

/// One-shot `ProvingProvider` that speaks the proof server's `/check` and
/// `/prove` HTTP protocol. The ledger's `ProofServerProvider` builds each
/// request body, and this client sends it.
#[derive(Clone)]
pub(super) struct ProofServerClient<'a> {
    pub(super) server: ProofServerProvider<'a>,
    http: reqwest::Client,
}

impl ProofServerClient<'_> {
    /// `ProvingProvider::check`, over the proof server's `/check`.
    pub(super) async fn check_preimage(
        &self,
        preimage: &ProofPreimage,
    ) -> Result<Vec<Option<usize>>, anyhow::Error> {
        let ser = self
            .server
            .check_request_body(&ProofPreimageVersioned::V2(Arc::new(preimage.clone())))
            .await?;
        let bytes = post(&self.http, &self.server.base_url, "/check", ser).await?;
        let res: Vec<Option<u64>> = tagged_deserialize(bytes.as_slice())?;
        Ok(res.into_iter().map(|i| i.map(|i| i as usize)).collect())
    }

    /// `ProvingProvider::prove`, over the proof server's `/prove`.
    pub(super) async fn prove_preimage(
        self,
        preimage: &ProofPreimage,
        overwrite_binding_input: Option<Fr>,
    ) -> Result<Proof, anyhow::Error> {
        let ser = self
            .server
            .proving_request_body(
                &ProofPreimageVersioned::V2(Arc::new(preimage.clone())),
                overwrite_binding_input,
            )
            .await?;
        let bytes = post(&self.http, &self.server.base_url, "/prove", ser).await?;
        let proof: ProofVersioned = tagged_deserialize(bytes.as_slice())?;
        match proof {
            ProofVersioned::V2(proof) => Ok(proof),
            other => Err(ProofServerError::UnsupportedProofVersion(format!("{other:?}")).into()),
        }
    }
}
