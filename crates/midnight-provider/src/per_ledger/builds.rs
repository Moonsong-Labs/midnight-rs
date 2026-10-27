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
use super::types::{PreparedInput, fee_speck};
use crate::provider::Balance;
use crate::{HeldInputs, MidnightProvider, ProviderError};

/// The attached wallet's builds on this generation.
///
/// [`MidnightProvider::builds`] hands one out after a resync, for the
/// generation the wallet's state is in. Each method that uses the wallet
/// reads its state when the method runs. A background sync of the wallet can
/// commit while a `Builds` is in use, so two calls can read different states.
/// A commit that crosses a hard fork makes each of those methods fail with
/// [`WalletError::LedgerMismatch`](midnight_types::WalletError::LedgerMismatch)
/// in [`ProviderError::Wallet`]. [`MidnightProvider::builds`] resyncs first
/// and picks the wallet's generation. So take a fresh one for a build that
/// starts later.
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
    /// forward.
    ///
    /// Each coin is named by a nullifier the wallet already knows, so this
    /// selects nothing; the caller decides what to spend.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Wallet`] with
    /// [`WalletError::DuplicateInput`](midnight_types::WalletError::DuplicateInput) when
    /// `coins` names a coin twice. The provider checks this before it asks
    /// the wallet, so a custom wallet that breaks the
    /// [`WalletBuilds::spend_shielded`] contract cannot spend a coin twice.
    /// Any other error comes from the wallet's spend.
    pub async fn prepare_shielded_inputs(
        &self,
        context: &Arc<BuildContext>,
        coins: &[SpendableShieldedCoin],
        rng: &mut StdRng,
    ) -> Result<(Vec<PreparedInput>, HeldInputs), ProviderError> {
        let nullifiers: Vec<_> = coins.iter().map(|c| c.nullifier).collect();
        midnight_types::refuse_repeated_nullifiers(&nullifiers)?;
        let (prepared, spent) = self.wallet.spend_shielded(context, nullifiers, rng).await?;
        Ok((prepared, HeldInputs::of(spent, self.provider.facade())))
    }

    /// Pay the fees for an external party's proven, fee-less transaction from
    /// the attached wallet. See [`MidnightProvider::balance_transaction`].
    pub async fn balance_transaction(
        &self,
        tx_bytes: &[u8],
    ) -> Result<TransferResult, ProviderError> {
        // Boxed so a caller's future stays small. See the frame-size note on
        // `MidnightProvider::resync_wallet`.
        Box::pin(self.balance_external(tx_bytes, Balance::Fees)).await
    }

    pub(crate) async fn balance_external(
        &self,
        tx_bytes: &[u8],
        balance: Balance,
    ) -> Result<TransferResult, ProviderError> {
        let external = deserialize_transaction(tx_bytes)?;

        // A fee-only balance adds only Dust, so a shortfall in any other
        // token would fail at submit.
        if balance == Balance::Fees {
            let imbalance = external
                .balance(None)
                .map_err(|e| ProviderError::Transaction(format!("compute balance: {e:?}")))?;
            if imbalance
                .iter()
                .any(|((tt, _seg), val)| !matches!(tt, TokenType::Dust) && *val < 0)
            {
                return Err(ProviderError::Transaction(
                    "balance_transaction covers fees only; the transaction has a token deficit, \
                     which balance_transaction_with_tokens covers from this wallet"
                        .into(),
                ));
            }
        }

        let context = self.execution_context().await?;
        let tx_info =
            StandardTransactionInfo::new_from_context(context.clone(), self.prover.clone(), None);
        let reserved = match balance {
            Balance::Fees => self.wallet.prepare_fees(tx_info, &external).await?,
            Balance::FeesAndTokens => self.wallet.prepare_balance(tx_info, &external).await?,
        };
        let Some(reserved) = reserved else {
            return Ok(TransferResult {
                tx_bytes: tx_bytes.to_vec(),
                ledger_version: super::LEDGER,
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
    /// strand them until their TTL elapses. For a balance of an external
    /// transaction, the proof includes the merge into it and its price.
    async fn prove_reserved(
        &self,
        reserved: ReservedBuild,
    ) -> Result<TransferResult, ProviderError> {
        let prepared = reserved.into_prepared();
        let held = HeldInputs::of(prepared.spent_inputs(), self.provider.facade());
        held.keep_on_ok(prepared.prove()).await.map_err(Into::into)
    }
}

/// Merge proven transactions of this generation into one. See
/// [`MidnightProvider::merge_transactions`].
pub fn merge_transactions(txs: &[Vec<u8>]) -> Result<Vec<u8>, ProviderError> {
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

fn deserialize_transaction(bytes: &[u8]) -> Result<FinalizedTransaction<DefaultDB>, ProviderError> {
    tagged_deserialize(&mut &bytes[..])
        .map_err(|e| ProviderError::Transaction(format!("deserialize transaction: {e}")))
}
