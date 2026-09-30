//! The provider's builds on a chain that runs ledger 8.
//!
//! The same source as [`crate::ledger_9`], compiled against ledger 8's types.

use midnight_helpers::ledger_8 as helpers;
use midnight_types::ledger_8 as types;
use midnight_wallet_facade::ledger_8 as facade;

#[path = "per_ledger/builds.rs"]
mod builds;
#[path = "per_ledger/proving.rs"]
mod proving;

pub use builds::{Builds, merge_transactions};
pub use midnight_wallet_facade::ledger_8::{ReservedBuild, WalletBuilds};

/// The generation this module builds for.
const LEDGER: midnight_types::LedgerVersion = midnight_types::LedgerVersion::V8;

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
}
