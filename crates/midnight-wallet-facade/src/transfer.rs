//! The vocabulary of a transfer build, and the proving step that consumes it.
//!
//! A wallet implementation selects inputs and assembles a [`PreparedTransfer`];
//! everything here is what its consumer needs: the request that names a build
//! ([`TransferRequest`]), what the build spends ([`SpentInputs`]), the proving
//! step ([`PreparedTransfer::prove`]), and its result ([`TransferResult`]).

use futures_util::FutureExt;

use midnight_helpers::{
    CoinSelectionStrategy, DefaultDB, DustLocalState, DustSpend, Nullifier, PedersenRandomness,
    ProofPreimageMarker, ShieldedTokenType, Signature, Sp, SplittableRng, StandardTrasactionInfo,
    Transaction, UnshieldedTokenType, WalletSeed,
};

use crate::WalletError;

type UnprovenTx = Transaction<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>;
type FinalizedTx = midnight_helpers::FinalizedTransaction<DefaultDB>;

pub struct TransferResult {
    pub tx_bytes: Vec<u8>,
    /// Unshielded UTXO inputs consumed by this transaction. Pass to
    /// `Wallet::reserve_pending` together with `dust_batches` so
    /// subsequent in-process builds don't re-select the same inputs before
    /// the indexer surfaces the spend events.
    pub spent_unshielded_inputs: Vec<SpentUtxoKey>,
    /// Shielded (Zswap) coin nullifiers consumed by this transaction. Pass to
    /// `Wallet::reserve_pending` so subsequent in-process builds don't
    /// re-select the same coins before the indexer confirms the spend.
    pub spent_shielded_inputs: Vec<Nullifier>,
    /// Dust batches that funded this transaction's fees. Each batch's
    /// `(spends, updated_state)` pair came from one `speculative_spend`
    /// call and must be kept together for the new `mark_spent` API.
    /// Same caveat as `spent_unshielded_inputs` — pass to
    /// `Wallet::reserve_pending` for double-build prevention.
    pub dust_batches: Vec<DustSpendBatch>,
    /// Deterministic Dust fee the chain will charge for this transaction, in
    /// SPECK (`1 DUST = 10^15 SPECK`). Computed via
    /// `Transaction::fees(&ledger.parameters, false)` against the parameters
    /// the build pipeline saw — matches what the node's own estimation RPC
    /// returns and what the indexer later reports as `paidFees` for an
    /// accepted, included transaction.
    pub fee_speck: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpentUtxoKey {
    pub intent_hash: String,
    pub output_index: u32,
}

/// What one build spends, in the form a reservation takes.
///
/// A build reserves its inputs so a later build in the same process does not
/// re-select them before the indexer surfaces the spend. Reserve them, and
/// hand them back if the build will never reach the chain.
#[derive(Default, Clone)]
pub struct SpentInputs {
    /// The Dust batches that fund the fee.
    pub dust_batches: Vec<DustSpendBatch>,
    /// The unshielded UTXOs the build spends.
    pub unshielded: Vec<SpentUtxoKey>,
    /// The shielded coins the build spends, by nullifier.
    pub shielded: Vec<Nullifier>,
}

impl SpentInputs {
    /// The Dust a build drew, for one that spends nothing else.
    pub fn from_dust(dust_batches: Vec<DustSpendBatch>) -> Self {
        Self {
            dust_batches,
            ..Self::default()
        }
    }

    /// The shielded coins a build pinned, for one that spends nothing else.
    pub fn from_shielded(shielded: Vec<Nullifier>) -> Self {
        Self {
            shielded,
            ..Self::default()
        }
    }

    /// The nullifier of every Dust spend here, which is how
    /// `Wallet::release_pending` names them.
    pub fn dust_nullifiers(&self) -> Vec<midnight_helpers::DustNullifier> {
        self.dust_batches
            .iter()
            .flat_map(|b| b.spends.iter().map(|s| s.old_nullifier))
            .collect()
    }
}

impl From<&TransferResult> for SpentInputs {
    fn from(result: &TransferResult) -> Self {
        Self {
            dust_batches: result.dust_batches.clone(),
            unshielded: result.spent_unshielded_inputs.clone(),
            shielded: result.spent_shielded_inputs.clone(),
        }
    }
}

/// One transfer a build can make.
///
/// Data rather than a closure over the builder, so a caller can hand a build
/// to a wallet it reaches only through a trait.
pub enum TransferKind {
    /// A shielded (Zswap) transfer. See
    /// `TransferBuilder::prepare_shielded`.
    Shielded {
        token_type: ShieldedTokenType,
        amount: u128,
        /// A bech32 shielded address.
        recipient: String,
        /// False leaves the transaction fee-less, for another wallet to fund.
        pay_fees: bool,
    },
    /// An unshielded (UTXO) transfer. See
    /// `TransferBuilder::prepare_unshielded`.
    Unshielded {
        token_type: UnshieldedTokenType,
        amount: u128,
        /// A bech32 unshielded address.
        recipient: String,
        /// False leaves the transaction fee-less, for another wallet to fund.
        pay_fees: bool,
    },
    /// One half of a two-party shielded swap, always fee-less. See
    /// `TransferBuilder::prepare_shielded_swap`.
    ShieldedSwap {
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
    },
    /// A dust-address registration. See
    /// `TransferBuilder::prepare_register_dust`.
    DustRegistration {
        /// Fallback creation time for the UTXOs whose own creation time the
        /// indexer did not report.
        utxo_ctime: Option<u64>,
    },
}

/// One build, as data: what to make, and how to choose the inputs that make
/// it.
pub struct TransferRequest {
    /// The transfer to build.
    pub kind: TransferKind,
    /// How to order the coins and UTXOs the build draws on.
    pub coin_selection: CoinSelectionStrategy,
}

impl TransferRequest {
    /// A request that orders its inputs the default way.
    pub fn new(kind: TransferKind) -> Self {
        Self {
            kind,
            coin_selection: CoinSelectionStrategy::default(),
        }
    }

    /// Order the coins and UTXOs this build draws on. See
    /// `TransferBuilder::with_coin_selection`.
    pub fn with_coin_selection(mut self, strategy: CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }
}

impl From<TransferKind> for TransferRequest {
    fn from(kind: TransferKind) -> Self {
        Self::new(kind)
    }
}

/// One funding-seed's dust contribution to a transaction.
///
/// `speculative_spend` returns both the spend records and the resulting
/// `DustLocalState`; the new helpers API requires that pair to be passed
/// together to `DustWallet::mark_spent`. We keep them grouped here so
/// callers (and the pending-reservations layer) can preserve the
/// invariant: the same `(spends, updated_state)` produced by a single
/// `speculative_spend` must be applied together.
#[derive(Clone)]
pub struct DustSpendBatch {
    pub seed: WalletSeed,
    pub spends: Vec<DustSpend<ProofPreimageMarker, DefaultDB>>,
    pub updated_state: Sp<DustLocalState<DefaultDB>, DefaultDB>,
}

/// A transfer whose inputs are selected and whose Dust fee is balanced, but
/// which is not yet proven.
///
/// The wallet is needed only to produce this. Proving reads the build context
/// alone, so a caller can reserve what this reports, release the wallet, and
/// prove afterwards.
pub struct PreparedTransfer {
    tx_info: StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
    dust_batches: Vec<DustSpendBatch>,
    spent_unshielded_inputs: Vec<SpentUtxoKey>,
    spent_shielded_inputs: Vec<Nullifier>,
}

impl PreparedTransfer {
    /// Wrap an assembled, fee-balanced, unproven transaction.
    ///
    /// For the build paths that produce one; the spends they report are added
    /// with the two setters below.
    pub fn new(
        tx_info: StandardTrasactionInfo<DefaultDB>,
        tx: UnprovenTx,
        dust_batches: Vec<DustSpendBatch>,
    ) -> Self {
        Self {
            tx_info,
            tx,
            dust_batches,
            spent_unshielded_inputs: Vec::new(),
            spent_shielded_inputs: Vec::new(),
        }
    }

    /// Record the unshielded UTXOs this build spends.
    pub fn set_spent_unshielded_inputs(&mut self, inputs: Vec<SpentUtxoKey>) {
        self.spent_unshielded_inputs = inputs;
    }

    /// Record the shielded coins this build spends, by nullifier.
    pub fn set_spent_shielded_inputs(&mut self, inputs: Vec<Nullifier>) {
        self.spent_shielded_inputs = inputs;
    }
    /// The Dust batches that will fund this build's fee.
    pub fn dust_batches(&self) -> &[DustSpendBatch] {
        &self.dust_batches
    }

    /// The unshielded UTXOs this build will spend.
    pub fn spent_unshielded_inputs(&self) -> &[SpentUtxoKey] {
        &self.spent_unshielded_inputs
    }

    /// The shielded coins this build will spend, by nullifier.
    pub fn spent_shielded_inputs(&self) -> &[Nullifier] {
        &self.spent_shielded_inputs
    }

    /// Everything this build will spend, in the form a reservation takes.
    pub fn spent_inputs(&self) -> SpentInputs {
        SpentInputs {
            dust_batches: self.dust_batches.clone(),
            unshielded: self.spent_unshielded_inputs.clone(),
            shielded: self.spent_shielded_inputs.clone(),
        }
    }

    /// Prove the transaction and serialize it. The slowest step in a build,
    /// and it touches no wallet.
    pub async fn prove(self) -> Result<TransferResult, WalletError> {
        let PreparedTransfer {
            mut tx_info,
            tx,
            dust_batches,
            spent_unshielded_inputs,
            spent_shielded_inputs,
        } = self;

        // Keep a handle to the ledger context so we can read `parameters`
        // after proving to compute the fee. `Arc::clone` is cheap and the lock
        // inside `with_ledger_state` is held only for the closure.
        let context = tx_info.context.clone();
        let finalized = prove_tx_no_validate(&mut tx_info, tx).await?;

        let mut bytes = Vec::new();
        midnight_helpers::midnight_serialize::tagged_serialize(&finalized, &mut bytes)
            .map_err(|e| WalletError::Transfer(format!("serialize: {e}")))?;

        // Mirrors the node's own estimation RPC: `enforce_time_to_dismiss =
        // false`, i.e. report the deterministic SPECK cost without the
        // chain-side mempool-eviction check. The chain charges the same number
        // at inclusion; if the tx exceeds the eviction-time bound, that
        // surfaces at submit, not here.
        let fee_speck = context
            .with_ledger_state(|s| finalized.fees(&s.parameters, false))
            .map_err(transfer_err("fees"))?;

        Ok(TransferResult {
            tx_bytes: bytes,
            spent_unshielded_inputs,
            spent_shielded_inputs,
            dust_batches,
            fee_speck,
        })
    }
}

/// Wrap a foreign error as [`WalletError::Transfer`], naming the failing
/// step.
pub fn transfer_err<E: std::fmt::Debug>(ctx: &str) -> impl FnOnce(E) -> WalletError + '_ {
    move |e| WalletError::Transfer(format!("{ctx}: {e:?}"))
}

pub async fn prove_tx_no_validate(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
) -> Result<FinalizedTx, WalletError> {
    let resolver = tx_info.context.resolver().await;
    let parameters = tx_info
        .context
        .ledger_state
        .lock()
        .map_err(|_| WalletError::Transfer("ledger state lock poisoned".into()))?
        .parameters
        .clone();
    let mut rng = tx_info.rng.split();
    // `ProofProvider::prove` returns a bare transaction, so a backend that
    // fails has nowhere to report it but the unwind. Catch it here and hand
    // the caller a typed error instead of tearing down their task.
    let proven = std::panic::AssertUnwindSafe(tx_info.prover.prove(
        tx,
        rng.split(),
        &resolver,
        &parameters.cost_model.runtime_cost_model,
    ))
    .catch_unwind()
    .await
    .map_err(|payload| WalletError::Proving(panic_message(payload)))?;
    Ok(proven.seal(rng))
}

/// Recover a printable message from a caught panic payload.
///
/// Shared with the provider's fee-paying path, which proves through the same
/// backend and needs the same treatment.
pub fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    "proof backend panicked with a non-string payload".to_string()
}
