//! The provider's builds on a chain that runs ledger 9.

use midnight_helpers::ledger_9 as helpers;
use midnight_types::ledger_9 as types;
use midnight_wallet_facade::ledger_9 as facade;

#[path = "per_ledger/builds.rs"]
mod builds;
#[path = "per_ledger/proving.rs"]
mod proving;

pub(crate) use builds::deserialize_transaction;
pub use builds::{Builds, merge_transactions};
pub use midnight_wallet_facade::ledger_9::{ReservedBuild, WalletBuilds};

impl helpers::transient_crypto::proofs::ProvingProvider for proving::ProofServerClient<'_> {
    async fn check(
        &self,
        preimage: &helpers::ProofPreimage,
    ) -> Result<Vec<Option<usize>>, anyhow::Error> {
        self.check_preimage(preimage).await
    }

    async fn prove(
        self,
        preimage: &helpers::ProofPreimage,
        overwrite_binding_input: Option<helpers::Fr>,
    ) -> Result<helpers::transient_crypto::proofs::Proof, anyhow::Error> {
        self.prove_preimage(preimage, overwrite_binding_input).await
    }

    fn split(&mut self) -> Self {
        self.clone()
    }

    fn resolver(&self) -> &impl helpers::ResolverTrait {
        self.server.resolver
    }
}
