//! The provers a provider proves with, one per ledger generation.

use std::sync::Arc;

use midnight_helpers::{DefaultDB, ledger_8, ledger_9};

/// The provers a provider proves with, one per ledger generation.
///
/// Each generation's builds prove with its own prover, because a prover
/// proves one generation's transactions. [`Self::local`] proves both locally.
/// A prover that serves both, such as
/// [`RemoteProofServer`](crate::RemoteProofServer), converts from an `Arc`
/// of itself, and a prover for one generation replaces that generation's:
///
/// ```rust,ignore
/// let provers = ProofProviders::local().with_ledger_9(Arc::new(MyLedger9Prover));
/// ```
#[derive(Clone)]
pub struct ProofProviders {
    ledger_8: Arc<dyn ledger_8::ProofProvider<DefaultDB>>,
    ledger_9: Arc<dyn ledger_9::ProofProvider<DefaultDB>>,
}

impl ProofProviders {
    /// Prove locally, in this process, on either generation.
    pub fn local() -> Self {
        Self {
            ledger_8: Arc::new(ledger_8::LocalProofServer::new()),
            ledger_9: Arc::new(ledger_9::LocalProofServer::new()),
        }
    }

    /// Prove ledger 8 transactions with `prover`.
    pub fn with_ledger_8(mut self, prover: Arc<dyn ledger_8::ProofProvider<DefaultDB>>) -> Self {
        self.ledger_8 = prover;
        self
    }

    /// Prove ledger 9 transactions with `prover`.
    pub fn with_ledger_9(mut self, prover: Arc<dyn ledger_9::ProofProvider<DefaultDB>>) -> Self {
        self.ledger_9 = prover;
        self
    }

    /// The prover of ledger 8 transactions.
    pub fn ledger_8(&self) -> Arc<dyn ledger_8::ProofProvider<DefaultDB>> {
        self.ledger_8.clone()
    }

    /// The prover of ledger 9 transactions.
    pub fn ledger_9(&self) -> Arc<dyn ledger_9::ProofProvider<DefaultDB>> {
        self.ledger_9.clone()
    }
}

impl<P> From<Arc<P>> for ProofProviders
where
    P: ledger_8::ProofProvider<DefaultDB> + ledger_9::ProofProvider<DefaultDB> + 'static,
{
    fn from(prover: Arc<P>) -> Self {
        Self {
            ledger_8: prover.clone(),
            ledger_9: prover,
        }
    }
}

impl std::fmt::Debug for ProofProviders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProofProviders").finish_non_exhaustive()
    }
}
