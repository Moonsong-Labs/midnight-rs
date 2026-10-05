//! The attached wallet's builds on this generation, with this generation's
//! prover.

use std::sync::Arc;

use midnight_types::{SpendableShieldedCoin, SpentInputs, TransferRequest, TransferResult};

use super::facade::{ReservedBuild, WalletBuilds};
use super::helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use super::helpers::{
    BuildContext, DefaultDB, FinalizedTransaction, FromContext, ProofProvider,
    StandardTransactionInfo, StdRng, TokenType,
};
use super::types::PreparedInput;
use crate::{HeldInputs, MidnightProvider, ProviderError};

/// The attached wallet's builds on this generation.
///
/// [`MidnightProvider::builds`] hands one out after a resync, for the
/// generation the wallet's state is in. It reads the wallet's view as that
/// resync left it, so take a fresh one for a build that starts later.
pub struct Builds<'a> {
    provider: &'a MidnightProvider,
    wallet: Arc<dyn WalletBuilds>,
    prover: Arc<dyn ProofProvider<DefaultDB>>,
}

impl std::fmt::Debug for Builds<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Builds")
            .field("ledger", &super::LEDGER)
            .finish_non_exhaustive()
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

    /// Build a [`BuildContext`] the attached wallet both executes against and
    /// pays from.
    ///
    /// [`Self::execution_context`] followed by [`Self::add_funding`], for the
    /// builds that fund from the wallet that builds them. Use the two
    /// separately when a circuit has to run before the payer is known.
    pub async fn build_context(&self) -> Result<Arc<BuildContext>, ProviderError> {
        let context = self.execution_context().await?;
        self.add_funding(&context).await?;
        Ok(context)
    }

    /// Build the half of a [`BuildContext`] a transaction executes against:
    /// chain parameters, genesis settings, the resolver, and the latest block
    /// context.
    ///
    /// The result holds no key material and no coin state, so a caller can
    /// run a circuit against it and only then decide who pays. Add a payer
    /// with [`Self::add_funding`]; a context that never gets that call funds
    /// nothing.
    pub async fn execution_context(&self) -> Result<Arc<BuildContext>, ProviderError> {
        Ok(self.wallet.execution_context().await?)
    }

    /// Put the attached wallet's spendable view into `context`, so a build can
    /// fund itself from it.
    ///
    /// Mutates the wallet: its `add_funding` evicts TTL-expired pending
    /// entries against the refreshed `block_context`.
    pub async fn add_funding(&self, context: &BuildContext) -> Result<(), ProviderError> {
        Ok(self.wallet.add_funding(context).await?)
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

    /// Spend the given coins, returning inputs an offer builder can hold.
    ///
    /// The wallet performs the spends, so what comes back carries no key
    /// material and the builder never sees a seed. `context` must be the one
    /// the caller is building against: the spends roll its wallet state
    /// forward, which is what stops a coin being spent twice in one offer.
    ///
    /// Each coin is named by a nullifier the wallet already knows, so this
    /// selects nothing; the caller decides what to spend.
    pub async fn prepare_shielded_inputs(
        &self,
        context: &Arc<BuildContext>,
        coins: &[SpendableShieldedCoin],
        rng: &mut StdRng,
    ) -> Result<(Vec<PreparedInput>, HeldInputs), ProviderError> {
        let nullifiers: Vec<_> = coins.iter().map(|c| c.nullifier).collect();
        let (prepared, spent) = self.wallet.spend_shielded(context, nullifiers, rng).await?;
        Ok((prepared, HeldInputs::of(spent, self.provider.facade())))
    }

    /// Pay the fees for an external party's proven, fee-less transaction from
    /// the attached wallet. See [`MidnightProvider::balance_transaction`].
    pub async fn balance_transaction(
        &self,
        tx_bytes: &[u8],
    ) -> Result<TransferResult, ProviderError> {
        let external: FinalizedTransaction<DefaultDB> = tagged_deserialize(&mut &tx_bytes[..])
            .map_err(|e| ProviderError::Transaction(format!("deserialize transaction: {e}")))?;

        // Refuse any non-fee token deficit: this path only adds Dust, so a
        // shortfall in any other token (an unfunded swap side) would just fail
        // at submit. Dust itself is what we are here to supply, so skip it.
        let imbalance = external
            .balance(None)
            .map_err(|e| ProviderError::Transaction(format!("compute balance: {e:?}")))?;
        if imbalance
            .iter()
            .any(|((tt, _seg), val)| !matches!(tt, TokenType::Dust) && *val < 0)
        {
            return Err(ProviderError::Transaction(
                "balance_transaction covers fees only; the transaction has a token deficit \
                 (swap balancing is not supported yet)"
                    .into(),
            ));
        }

        let context = self.execution_context().await?;
        let tx_info =
            StandardTransactionInfo::new_from_context(context.clone(), self.prover.clone(), None);
        let Some(reserved) = self.wallet.prepare_fees(tx_info, &external).await? else {
            return Ok(TransferResult {
                tx_bytes: tx_bytes.to_vec(),
                ledger_version: super::LEDGER,
                spent_unshielded_inputs: Vec::new(),
                spent_shielded_inputs: Vec::new(),
                spent_dust: Vec::new(),
                fee_speck: fee_of(&context, &external)?,
                reserved_at: context.latest_block_context().tblock,
            });
        };
        let fee = self.prove_reserved(reserved).await?;
        match with_fee_merged(&context, &external, &fee.tx_bytes) {
            Ok((tx_bytes, fee_speck)) => Ok(TransferResult {
                tx_bytes,
                fee_speck,
                ..fee
            }),
            Err(err) => {
                // No transaction carries the fee Dust, so nothing can spend it.
                self.wallet.release(&SpentInputs::from(&fee)).await;
                Err(err)
            }
        }
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
    /// strand them until their TTL elapses.
    async fn prove_reserved(
        &self,
        reserved: ReservedBuild,
    ) -> Result<TransferResult, ProviderError> {
        let prepared = reserved.into_prepared();
        let mut held = HeldInputs::of(prepared.spent_inputs(), self.provider.facade());

        match prepared.prove().await {
            Ok(result) => {
                held.keep();
                Ok(result)
            }
            Err(err) => {
                // Release here rather than leaving it to `held`, so a caller
                // that observes the error also observes the inputs back.
                held.keep();
                self.wallet.release(held.spent()).await;
                Err(err.into())
            }
        }
    }
}

/// `external` with the proven fee transaction `fee` merged in, serialized,
/// and the fee the chain charges for the merge.
fn with_fee_merged(
    context: &BuildContext,
    external: &FinalizedTransaction<DefaultDB>,
    fee: &[u8],
) -> Result<(Vec<u8>, u128), ProviderError> {
    let fee: FinalizedTransaction<DefaultDB> = tagged_deserialize(&mut &fee[..])
        .map_err(|e| ProviderError::Transaction(format!("deserialize fee transaction: {e}")))?;
    let merged = external
        .merge(&fee)
        .map_err(|e| ProviderError::Transaction(format!("merge transactions: {e:?}")))?;
    let mut bytes = Vec::new();
    tagged_serialize(&merged, &mut bytes)
        .map_err(|e| ProviderError::Transaction(format!("serialize merged transaction: {e}")))?;
    Ok((bytes, fee_of(context, &merged)?))
}

/// The fee the chain charges for `tx`, priced as a build prices its own. See
/// [`TransferResult::fee_speck`].
fn fee_of(
    context: &BuildContext,
    tx: &FinalizedTransaction<DefaultDB>,
) -> Result<u128, ProviderError> {
    context
        .with_ledger_state(|s| tx.fees(&s.parameters, false))
        .map_err(|e| ProviderError::Transaction(format!("fees: {e:?}")))
}

/// Merge proven transactions of this generation into one. See
/// [`MidnightProvider::merge_transactions`].
pub fn merge_transactions(txs: &[Vec<u8>]) -> Result<Vec<u8>, ProviderError> {
    let deserialize = |bytes: &[u8]| -> Result<FinalizedTransaction<DefaultDB>, ProviderError> {
        tagged_deserialize(&mut &bytes[..])
            .map_err(|e| ProviderError::Transaction(format!("deserialize transaction: {e}")))
    };

    let mut iter = txs.iter();
    let first = iter.next().ok_or_else(|| {
        ProviderError::Transaction("merge_transactions requires at least one transaction".into())
    })?;
    let mut merged = deserialize(first)?;
    for bytes in iter {
        let other = deserialize(bytes)?;
        merged = merged
            .merge(&other)
            .map_err(|e| ProviderError::Transaction(format!("merge transactions: {e:?}")))?;
    }

    let mut out = Vec::new();
    tagged_serialize(&merged, &mut out)
        .map_err(|e| ProviderError::Transaction(format!("serialize merged transaction: {e}")))?;
    Ok(out)
}
