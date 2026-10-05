//! [`RemoteProofServer`] as this generation's `ProofProvider`, and the proof
//! server client it proves through.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tracing::{Instrument, Span, info, warn};

use super::helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use super::helpers::mn_ledger::error::TransactionProvingError;
use super::helpers::mn_ledger::structure::{ProofPreimageVersioned, ProofVersioned};
use super::helpers::transient_crypto::curve::Fr;
use super::helpers::transient_crypto::proofs::{
    KeyLocation, Proof, ProofPreimage, Resolver as ResolverTrait, WrappedIr,
};
use super::helpers::{
    CostModel, DB, PedersenRandomness, ProofMarker, ProofPreimageMarker, ProofProvider, Resolver,
    Signature, StdRng, Transaction,
};
use crate::remote_prover::{
    INITIAL_BACKOFF, MAX_BACKOFF, PROOF_SERVER_TIMEOUT, PROVING_PANIC_PREFIX, ProofServerError,
    RemoteProofServer, is_transient,
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
        tx: Transaction<Signature, ProofPreimageMarker, PedersenRandomness, D>,
        _rng: StdRng,
        resolver: &'static Resolver,
        cost_model: CostModel,
    ) -> Transaction<Signature, ProofMarker, PedersenRandomness, D> {
        info!(url = %self.url, "remote proving");
        let base_url = self.url.clone();
        // A span does not cross `spawn_blocking`, so this instruments the
        // retries with the caller's span.
        let span = Span::current();

        // A ledger 9 proof is not `Send`, so it runs to completion on a thread
        // of its own, as the upstream local prover does. The HTTP client lives
        // on that thread's runtime too: a pooled connection is bound to the
        // runtime that opened it, and this one ends with the proof.
        let proving = tokio::task::spawn_blocking(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime with I/O and timers")
                .block_on(
                    prove_with_retries(tx, base_url, reqwest::Client::new(), resolver, cost_model)
                        .instrument(span),
                )
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
    tx: Transaction<Signature, ProofPreimageMarker, PedersenRandomness, D>,
    base_url: String,
    http: reqwest::Client,
    resolver: &Resolver,
    cost_model: CostModel,
) -> Transaction<Signature, ProofMarker, PedersenRandomness, D> {
    let start = Instant::now();
    let mut delay = INITIAL_BACKOFF;
    loop {
        let client = ProofServerClient {
            base_url: base_url.clone(),
            resolver,
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

/// One-shot `ProvingProvider` that speaks the proof server's `/check` and
/// `/prove` HTTP protocol. Holds a borrow of the [`Resolver`] so it can attach
/// each circuit's IR to requests for non-builtin keys.
#[derive(Clone)]
pub(super) struct ProofServerClient<'a> {
    base_url: String,
    pub(super) resolver: &'a Resolver,
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

impl ProofServerClient<'_> {
    /// `ProvingProvider::check`, over the proof server's `/check`.
    pub(super) async fn check_preimage(
        &self,
        preimage: &ProofPreimage,
    ) -> Result<Vec<Option<usize>>, anyhow::Error> {
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

    /// `ProvingProvider::prove`, over the proof server's `/prove`.
    pub(super) async fn prove_preimage(
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
}
