//! The attached wallet's builds, with the provider's prover.

use std::sync::Arc;

use midnight_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use midnight_helpers::{
    BuildContext, DefaultDB, FinalizedTransaction, FromContext, ProofProvider,
    StandardTransactionInfo, Timestamp, make_block_context,
};
use midnight_types::{Cover, SpentInputs, TransferRequest, TransferResult, fee_speck};
use midnight_wallet_facade::{ReservedBuild, WalletBuilds};

use crate::{HeldInputs, MidnightProvider, ProviderError};

/// The attached wallet's builds.
///
/// [`MidnightProvider::builds`] hands one out after a resync. Each method
/// that uses the wallet reads its state when the method runs. A background
/// sync of the wallet can commit while a `Builds` is in use, so two calls can
/// read different states. [`MidnightProvider::builds`] resyncs first, so take
/// a fresh one for a build that starts later.
pub struct Builds<'a> {
    provider: &'a MidnightProvider,
    wallet: Arc<dyn WalletBuilds>,
    prover: Arc<dyn ProofProvider<DefaultDB>>,
}

impl std::fmt::Debug for Builds<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Builds").finish_non_exhaustive()
    }
}

impl<'a> Builds<'a> {
    pub(crate) fn new(
        provider: &'a MidnightProvider,
        wallet: Arc<dyn WalletBuilds>,
        prover: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Self {
        Self {
            provider,
            wallet,
            prover,
        }
    }

    /// The provider these builds submit through.
    pub fn provider(&self) -> &'a MidnightProvider {
        self.provider
    }

    /// The prover these builds prove with.
    pub fn proof_provider(&self) -> Arc<dyn ProofProvider<DefaultDB>> {
        self.prover.clone()
    }

    /// Build the half of a [`BuildContext`] a transaction executes against,
    /// from the attached wallet's view: chain parameters, genesis settings,
    /// the resolver, and the latest block context.
    ///
    /// The result holds no key material and no coin state. A build that
    /// funds itself from the wallet, such as [`Self::build_funded`], adds the
    /// wallet's funding view to it.
    pub async fn execution_context(&self) -> Result<Arc<BuildContext>, ProviderError> {
        Ok(self.wallet.execution_context().await?)
    }

    /// Build the half of a [`BuildContext`] a transaction executes against,
    /// from node queries: the node's network and ledger parameters, the
    /// resolver, and the time of the node's best block.
    ///
    /// A transaction built on it counts its TTL from that time. The result
    /// holds no wallet, so it funds nothing.
    pub async fn chain_context(&self) -> Result<Arc<BuildContext>, ProviderError> {
        let node = self.provider;
        let best_block_time = async {
            node.get_block_timestamp(node.best_block_hash().await?)
                .await
        };
        let (network, parameters, time) = tokio::try_join!(
            node.ledger_network_id(),
            node.ledger_parameters(),
            best_block_time
        )?;
        let time = Timestamp::from_secs(time.as_secs());
        Ok(midnight_types::execution_context(
            network,
            parameters,
            Some(make_block_context(time, Default::default(), time)),
        )?)
    }

    /// Fund a transaction the caller assembled, and prove it.
    ///
    /// The wallet balances the fee and records what it drew as one transition,
    /// then proving runs with the wallet free. A proof that fails hands the
    /// Dust back.
    ///
    /// One call rather than a reserve step and a prove step, so a build cannot
    /// reach a different provider between them. The reservation belongs to
    /// this provider's wallet, and only this provider can hand it back.
    pub async fn build_funded(
        &self,
        tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    ) -> Result<TransferResult, ProviderError> {
        let reserved = self.wallet.prepare_funded(tx_info).await?;
        self.prove_reserved(reserved).await
    }

    /// Cover what `cover` names in a proven transaction from the attached
    /// wallet, and prove the result. [`Cover::Fee`] is the sponsor's cover of
    /// [`MidnightProvider::balance_transaction`].
    ///
    /// The wallet covers it in a part of its own and reserves what the part
    /// spends, as one transition. Proving the part and merging it run with the
    /// wallet free, and a failure in either hands the inputs back. The result
    /// names what the part reserved. A transaction that the cover leaves as it
    /// is comes back unchanged, with nothing reserved.
    pub async fn balance_transaction(
        &self,
        tx_bytes: &[u8],
        cover: Cover,
    ) -> Result<TransferResult, ProviderError> {
        let external = deserialize_transaction(tx_bytes)?;
        // Boxed so a caller's future stays small. See the frame-size note on
        // `MidnightProvider::resync_wallet`.
        Box::pin(self.balance_decoded(tx_bytes, external, cover)).await
    }

    /// [`Self::balance_transaction`] for `tx_bytes`, which decode to
    /// `external`.
    pub(crate) async fn balance_decoded(
        &self,
        tx_bytes: &[u8],
        external: FinalizedTransaction<DefaultDB>,
        cover: Cover,
    ) -> Result<TransferResult, ProviderError> {
        let context = self.execution_context().await?;
        let tx_info =
            StandardTransactionInfo::new_from_context(context.clone(), self.prover.clone(), None);
        let Some(reserved) = self.wallet.prepare_fees(tx_info, &external, cover).await? else {
            return Ok(TransferResult {
                tx_bytes: tx_bytes.to_vec(),
                spent: SpentInputs::default(),
                fee_speck: fee_speck(&context, &external)?,
            });
        };
        self.prove_reserved(reserved).await
    }

    /// Ask the wallet to select and reserve, then prove without it.
    ///
    /// Proving is the slowest step in a build and reads only the build
    /// context, so leaving it inside the wallet's hold would make every other
    /// consumer wait on work that never needed the wallet.
    pub(crate) async fn build_then_prove(
        &self,
        request: TransferRequest,
    ) -> Result<TransferResult, ProviderError> {
        let reserved = self
            .wallet
            .prepare_transfer(request, self.prover.clone())
            .await?;
        self.prove_reserved(reserved).await
    }

    /// Prove a build whose inputs the wallet already holds, with the wallet
    /// released.
    ///
    /// Takes a [`ReservedBuild`] rather than a bare prepared build, so a build
    /// reaches the prover only through a wallet that says it reserved the
    /// inputs first. A proof that fails hands them back, because the
    /// reservation outlives the decision that made it and would otherwise
    /// strand them until their TTL elapses. For a cover, the proof includes
    /// the merge into the external transaction and its price.
    async fn prove_reserved(
        &self,
        reserved: ReservedBuild,
    ) -> Result<TransferResult, ProviderError> {
        let prepared = reserved.into_prepared();
        let held = HeldInputs::of(prepared.spent_inputs(), self.provider.facade());
        held.keep_on_ok(prepared.prove()).await.map_err(Into::into)
    }
}

/// Merge proven transactions into one. See
/// [`MidnightProvider::merge_transactions`].
pub(crate) fn merge_transactions(txs: &[Vec<u8>]) -> Result<Vec<u8>, ProviderError> {
    let mut iter = txs.iter();
    let first = iter.next().ok_or_else(|| {
        ProviderError::Transaction("merge_transactions requires at least one transaction".into())
    })?;
    let mut merged = deserialize_transaction(first)?;
    for bytes in iter {
        let other = deserialize_transaction(bytes)?;
        merged = merged
            .merge(&other)
            .map_err(|e| ProviderError::Transaction(format!("merge transactions: {e:?}")))?;
    }

    let mut out = Vec::new();
    tagged_serialize(&merged, &mut out)
        .map_err(|e| ProviderError::Transaction(format!("serialize merged transaction: {e}")))?;
    Ok(out)
}

pub(crate) fn deserialize_transaction(
    bytes: &[u8],
) -> Result<FinalizedTransaction<DefaultDB>, ProviderError> {
    tagged_deserialize(&mut &bytes[..])
        .map_err(|e| ProviderError::Transaction(format!("deserialize transaction: {e}")))
}
