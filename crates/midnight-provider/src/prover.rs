//! [`Prover`]: where a [`MidnightProvider`](crate::MidnightProvider) proves.

use std::sync::Arc;

use midnight_helpers::{DefaultDB, ProofProvider};

use crate::local_prover::LocalProver;
use crate::remote_prover::RemoteProofServer;

/// Where proofs are made.
///
/// Set it with [`MidnightProvider::with_prover`](crate::MidnightProvider::with_prover).
/// The provider proves every transaction with it: each `TransactionRequest`,
/// and the deploys, calls and maintenance updates of a `Contract` on the
/// provider.
pub struct Prover(Backend);

enum Backend {
    Local,
    Remote(Arc<RemoteProofServer>),
    #[cfg(feature = "test-util")]
    Custom(Arc<dyn ProofProvider<DefaultDB>>),
}

impl Prover {
    /// Prove in this process. The default.
    ///
    /// The local prover proves circuits compiled for ZKIR v2 and for ZKIR v3.
    /// The first proof of each circuit size downloads its public parameters
    /// when the cache does not hold them. The first proof with a built-in
    /// Zswap or Dust key downloads that key in the same way. The source is
    /// `MIDNIGHT_PARAM_SOURCE` (default `https://srs.midnight.network/`). The
    /// cache is the directory that upstream `MidnightDataProvider` picks,
    /// `MIDNIGHT_PP` first. For offline or container use, fill that cache once
    /// with network access. Then point `MIDNIGHT_PP` at it.
    pub fn local() -> Self {
        Self(Backend::Local)
    }

    /// Prove on the HTTP proof server at `url`, its base URL such as
    /// `http://localhost:6300`.
    ///
    /// The server must prove ledger 9 transactions, as the `proof-server`
    /// image of the devnet does. It downloads none of the files of
    /// [`Self::local`]. Each `/check` or `/prove` request has a deadline of
    /// 300 s, and a transient failure is retried with exponential backoff
    /// within that deadline. A failed proof fails the fill with
    /// [`WalletError::Proving`](midnight_types::WalletError::Proving).
    ///
    /// ```rust,no_run
    /// # const NODE_URL: &str = "ws://localhost:9944";
    /// use midnight_provider::{MidnightProvider, Prover};
    ///
    /// let provider =
    ///     MidnightProvider::new(NODE_URL).with_prover(Prover::remote("http://localhost:6300"));
    /// ```
    pub fn remote(url: &str) -> Self {
        Self(Backend::Remote(Arc::new(RemoteProofServer::new(
            url.to_owned(),
        ))))
    }

    /// Prove with `prover`, such as a test prover that counts or fails
    /// proofs.
    #[cfg(feature = "test-util")]
    pub fn custom(prover: Arc<dyn ProofProvider<DefaultDB>>) -> Self {
        Self(Backend::Custom(prover))
    }

    /// The ledger's prover behind this one.
    pub(crate) fn proof_provider(&self) -> Arc<dyn ProofProvider<DefaultDB>> {
        match &self.0 {
            Backend::Local => Arc::new(LocalProver),
            Backend::Remote(server) => server.clone(),
            #[cfg(feature = "test-util")]
            Backend::Custom(prover) => prover.clone(),
        }
    }
}

impl std::fmt::Debug for Prover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Backend::Local => f.write_str("Prover::Local"),
            Backend::Remote(server) => f.debug_tuple("Prover::Remote").field(server).finish(),
            #[cfg(feature = "test-util")]
            Backend::Custom(_) => f.write_str("Prover::Custom"),
        }
    }
}

/// A test prover can wrap a [`Prover`], such as [`Prover::local`], and pass
/// the proofs that it does not change through to it.
#[cfg(feature = "test-util")]
#[async_trait::async_trait]
impl ProofProvider<DefaultDB> for Prover {
    async fn prove(
        &self,
        tx: midnight_helpers::UnprovenTransaction<DefaultDB>,
        rng: midnight_helpers::StdRng,
        resolver: &'static midnight_helpers::Resolver,
        cost_model: midnight_helpers::CostModel,
    ) -> midnight_helpers::ProvenTransaction<DefaultDB> {
        self.proof_provider()
            .prove(tx, rng, resolver, cost_model)
            .await
    }
}
