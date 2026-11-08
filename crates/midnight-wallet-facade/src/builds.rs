//! A wallet's builds.

use std::sync::Arc;

use async_trait::async_trait;
use midnight_helpers::{
    BuildContext, DefaultDB, FinalizedTransaction, ProofProvider, StandardTransactionInfo,
};
use midnight_types::{Cover, PreparedTransfer, TransferRequest, WalletError};

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
    /// handing the inputs back if the build never reaches the chain. Once the
    /// build is proven, that owner calls [`WalletFacade::persist_reservation`]
    /// with what the build spent.
    pub fn into_prepared(self) -> PreparedTransfer {
        self.0
    }
}

/// A wallet's builds: the selection, funding and reservation steps that name
/// the ledger's transaction types.
#[async_trait]
pub trait WalletBuilds: WalletFacade {
    /// The half of a [`BuildContext`] a transaction executes against. It
    /// carries no key material and no coin state.
    async fn execution_context(&self) -> Result<Arc<BuildContext>, WalletError>;

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

    /// Cover what `cover` names in a transaction someone else finished, and
    /// reserve what that spends, as one transition.
    ///
    /// The cover rides a part of its own, merged in after proving, so
    /// `external` and its proofs stay as they are. `tx_info` supplies the
    /// context the part is priced against and the prover that proves it; it
    /// carries no intents of its own.
    ///
    /// Checking that a pinned coin is still free, selecting the other coins,
    /// and reserving them all happen under one hold, so two builds cannot
    /// spend the same coin. An implementation refuses a pinned coin that
    /// another build holds with [`WalletError::InputsReserved`], and one that
    /// `cover` names twice with [`WalletError::DuplicateInput`], before it
    /// spends anything.
    ///
    /// `None` when there is nothing to add, so nothing to reserve: `external`
    /// has no deficit that `cover` takes, `cover` pins no coin, and no Dust
    /// is due.
    async fn prepare_fees(
        &self,
        tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
        external: &FinalizedTransaction<DefaultDB>,
        cover: Cover,
    ) -> Result<Option<ReservedBuild>, WalletError>;
}

/// A shared wallet builds as the wallet inside does.
#[async_trait]
impl<T: WalletBuilds + ?Sized> WalletBuilds for Arc<T> {
    async fn execution_context(&self) -> Result<Arc<BuildContext>, WalletError> {
        (**self).execution_context().await
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

    async fn prepare_fees(
        &self,
        tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
        external: &FinalizedTransaction<DefaultDB>,
        cover: Cover,
    ) -> Result<Option<ReservedBuild>, WalletError> {
        (**self).prepare_fees(tx_info, external, cover).await
    }
}
