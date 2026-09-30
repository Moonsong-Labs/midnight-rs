//! A wallet's builds on one ledger generation.

use std::sync::Arc;

use async_trait::async_trait;
use midnight_types::{Nullifier, SpentInputs, TransferRequest, WalletError};

use super::helpers::{
    BuildContext, DefaultDB, FinalizedTransaction, ProofProvider, StandardTransactionInfo, StdRng,
};
use super::types::{PreparedInput, PreparedTransfer};
use crate::WalletFacade;

/// A prepared build whose inputs the wallet already holds.
///
/// A `prepare_*` method on [`WalletBuilds`] is what makes one, and proving
/// is what consumes one, so a build cannot reach the prover before the
/// reservation that protects its inputs.
pub struct ReservedBuild(PreparedTransfer);

impl ReservedBuild {
    /// Wrap a build whose inputs are reserved.
    ///
    /// Calling this is the implementation's statement that it has recorded the
    /// reservation.
    pub fn reserved(prepared: PreparedTransfer) -> Self {
        Self(prepared)
    }

    /// The build, out of the reservation's custody. Whoever takes it owns
    /// handing the inputs back if the build never reaches the chain.
    pub fn into_prepared(self) -> PreparedTransfer {
        self.0
    }
}

/// A wallet's builds on this generation: the selection, funding and
/// reservation steps that name its transaction types.
///
/// A wallet whose state is on another generation fails each with
/// [`WalletError::LedgerMismatch`]. That happens only between a chain's hard
/// fork and the wallet's next resync, and a build after that resync succeeds.
#[async_trait]
pub trait WalletBuilds: WalletFacade {
    /// The half of a [`BuildContext`] a transaction executes against. It
    /// carries no key material and no coin state.
    async fn execution_context(&self) -> Result<Arc<BuildContext>, WalletError>;

    /// Put this wallet's spendable view into `context`, so a build can fund
    /// itself from it.
    async fn add_funding(&self, context: &BuildContext) -> Result<(), WalletError>;

    /// Select the inputs a request draws on and reserve them, as one
    /// transition.
    ///
    /// Selection reads the reserved set and the reservation writes it, so an
    /// implementation that shares its state must cover both with one hold.
    /// Proving is not part of it: it is the slowest step in a build and reads
    /// only the build context.
    async fn prepare_transfer(
        &self,
        request: TransferRequest,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<ReservedBuild, WalletError>;

    /// Fund a transaction the caller assembled, and reserve what it drew, as
    /// one transition.
    ///
    /// [`Self::prepare_transfer`] is the same shape for a transfer this wallet
    /// selects itself. This one is for a caller that built its own
    /// transaction, a contract deploy or a maintenance update, and needs this
    /// wallet to pay its fee.
    ///
    /// The actions in `tx_info` run under this wallet's lock, so they must not
    /// call back into it. Proving is not part of it.
    ///
    /// `tx_info` must already ask for mock fee proofs. Balancing without them
    /// proves on every round, which is the cost preparing exists to avoid, and
    /// a transaction carrying a user circuit cannot mock-prove at all.
    async fn prepare_funded(
        &self,
        tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    ) -> Result<ReservedBuild, WalletError>;

    /// Spend coins this wallet owns into `context`, and reserve them, as one
    /// transition.
    ///
    /// The caller names each coin by a nullifier, so nothing is selected here.
    /// Checking that a coin is still free, spending it, and reserving it all
    /// happen under one hold, so two builds cannot pin the same coin.
    ///
    /// Call this after the funding view is in `context`. Reserving first would
    /// hide these very coins from it, because a funding view drops what a
    /// pending build already spent.
    ///
    /// The reservation stands until the caller releases it. A build that never
    /// reaches the chain must hand the coins back.
    async fn spend_shielded(
        &self,
        context: &Arc<BuildContext>,
        nullifiers: Vec<Nullifier>,
        rng: &mut StdRng,
    ) -> Result<(Vec<PreparedInput>, SpentInputs), WalletError>;

    /// Pay the fee of a transaction someone else finished, and reserve what
    /// that draws, as one transition.
    ///
    /// The fee rides an intent of its own, merged in after proving, so
    /// `external` and its proofs stay as they are. `tx_info` supplies the
    /// context the fee is priced against and the prover that proves it; it
    /// carries no intents of its own.
    ///
    /// `None` when `external` already balances its Dust, so there is nothing
    /// to draw and nothing to reserve.
    async fn prepare_fees(
        &self,
        tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
        external: &FinalizedTransaction<DefaultDB>,
    ) -> Result<Option<ReservedBuild>, WalletError>;
}

/// A shared wallet builds as the wallet inside does.
#[async_trait]
impl<T: WalletBuilds + ?Sized> WalletBuilds for Arc<T> {
    async fn execution_context(&self) -> Result<Arc<BuildContext>, WalletError> {
        (**self).execution_context().await
    }

    async fn add_funding(&self, context: &BuildContext) -> Result<(), WalletError> {
        (**self).add_funding(context).await
    }

    async fn prepare_transfer(
        &self,
        request: TransferRequest,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<ReservedBuild, WalletError> {
        (**self).prepare_transfer(request, proof_provider).await
    }

    async fn prepare_funded(
        &self,
        tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    ) -> Result<ReservedBuild, WalletError> {
        (**self).prepare_funded(tx_info).await
    }

    async fn spend_shielded(
        &self,
        context: &Arc<BuildContext>,
        nullifiers: Vec<Nullifier>,
        rng: &mut StdRng,
    ) -> Result<(Vec<PreparedInput>, SpentInputs), WalletError> {
        (**self).spend_shielded(context, nullifiers, rng).await
    }

    async fn prepare_fees(
        &self,
        tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
        external: &FinalizedTransaction<DefaultDB>,
    ) -> Result<Option<ReservedBuild>, WalletError> {
        (**self).prepare_fees(tx_info, external).await
    }
}
