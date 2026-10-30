//! [`RemoteProofServer`] as this generation's `ProofProvider`, and the proof
//! server client it proves through.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::info;

use super::helpers::midnight_serialize::tagged_deserialize;
use super::helpers::mn_ledger::structure::{ProofPreimageVersioned, ProofVersioned};
use super::helpers::transient_crypto::curve::Fr;
use super::helpers::transient_crypto::proofs::{Proof, ProofPreimage};
use super::helpers::{
    CostModel, DB, ProofProvider, ProofServerProvider, ProvenTransaction, Resolver, StdRng,
    UnprovenTransaction,
};
use crate::remote_prover::{ProofServerError, RemoteProofServer, post};

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
        let timeout = self.timeout;

        // A ledger 9 proof is not `Send`, so it runs to completion on a thread
        // of its own, as the upstream local prover does. The HTTP client lives
        // on that thread's runtime too: a pooled connection is bound to the
        // runtime that opened it, and this one ends with the proof.
        let proving = tokio::task::spawn_blocking(move || {
            let client = ProofServerClient {
                server: ProofServerProvider { base_url, resolver },
                http: reqwest::Client::new(),
                timeout,
            };
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime with I/O and timers")
                .block_on(async {
                    match tx.prove(client, &cost_model).await {
                        Ok(proven) => proven,
                        // Unwind without the panic hook: the build reports
                        // this unwind as `WalletError::Proving`, so stderr
                        // must not show a panic. `{:#}` keeps the cause, such
                        // as a timeout.
                        Err(err) => std::panic::resume_unwind(Box::new(format!("{err:#}"))),
                    }
                })
        });
        match proving.await {
            Ok(proven) => proven,
            // Re-raise on this task, so the caller's unwind handler sees the
            // prover's own message.
            Err(err) => std::panic::resume_unwind(err.into_panic()),
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
    timeout: Duration,
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
        let bytes = post(
            &self.http,
            &self.server.base_url,
            "/check",
            ser,
            self.timeout,
        )
        .await?;
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
        let bytes = post(
            &self.http,
            &self.server.base_url,
            "/prove",
            ser,
            self.timeout,
        )
        .await?;
        let proof: ProofVersioned = tagged_deserialize(bytes.as_slice())?;
        match proof {
            ProofVersioned::V2(proof) => Ok(proof),
            other => Err(ProofServerError::UnsupportedProofVersion(format!("{other:?}")).into()),
        }
    }
}
