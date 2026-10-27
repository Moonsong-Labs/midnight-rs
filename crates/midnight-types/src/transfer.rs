//! The vocabulary of a transfer build: the request that names one
//! ([`TransferRequest`]), what it spends ([`SpentInputs`]), and its result
//! ([`TransferResult`]).
//!
//! The builds themselves are per ledger generation, in
//! [`crate::ledger_8`] and [`crate::ledger_9`].

use midnight_helpers::{CoinSelectionStrategy, HashOutput, Timestamp};

use crate::coin::{DustNullifier, Nullifier, ShieldedTokenType, UnshieldedTokenType};
use crate::ledger_version::LedgerVersion;

/// A proven transaction a build made, and the inputs the build reserved for
/// it.
pub struct TransferResult {
    /// The proven transaction, tagged with its generation's encoding.
    pub tx_bytes: Vec<u8>,
    /// The generation the transaction is built for.
    pub ledger_version: LedgerVersion,
    /// The inputs of this transaction that the builds of this wallet
    /// reserved. `WalletFacade::release` takes them back if the transaction
    /// never reaches the chain. A transaction that pays the fee of another
    /// party's transaction also spends that party's inputs, and this names
    /// only the Dust of the fee.
    pub spent: SpentInputs,
    /// Deterministic Dust fee the chain will charge for this transaction, in
    /// SPECK (`1 DUST = 10^15 SPECK`). Computed via
    /// `Transaction::fees(&ledger.parameters, false)` against the parameters
    /// the build pipeline saw — matches what the node's own estimation RPC
    /// returns and what the indexer later reports as `paidFees` for an
    /// accepted, included transaction.
    ///
    /// The `false` skips the time-to-dismiss check, and the node applies that
    /// check at submit. It rejects a transaction, as
    /// `FeeCalculation(OutsideTimeToDismiss)`, when validating the whole
    /// transaction and applying its guaranteed part take more time than its
    /// size allows. The allowance is `time_to_dismiss_per_byte` per byte, and
    /// never less than `min_time_to_dismiss`. The check never changes the
    /// fee, but a quote does not mean that the node accepts the transaction.
    pub fee_speck: u128,
}

impl std::fmt::Debug for TransferResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferResult")
            .field("tx_bytes", &format_args!("{} bytes", self.tx_bytes.len()))
            .field("ledger_version", &self.ledger_version)
            .field("spent", &self.spent)
            .field("fee_speck", &self.fee_speck)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpentUtxoKey {
    /// The intent hash of the transaction that created the UTXO.
    pub intent_hash: HashOutput,
    /// The number of the UTXO among that transaction's outputs.
    pub output_index: u32,
}

/// What one build spends, in the form a reservation takes.
///
/// A build reserves its inputs so a later build in the same process does not
/// re-select them before the indexer surfaces the spend. Reserve them, and
/// hand them back if the build will never reach the chain.
///
/// Each input carries the stamp of its reservation, because the stamp is half
/// of a reservation's identity. A release names both, so a build hands back
/// its own entry, not one a later build made over the same input in a later
/// block. Two reservations in one block share the stamp, which is why a
/// reservation is released once.
///
/// One transaction can hold the inputs of two reservations with different
/// stamps. A contract call or a deploy with a shielded offer reserves its
/// pinned coins in its own build context, and the Dust for its fee in a fresh
/// one. [`Self::merge`] joins the two, so one value hands both back.
#[derive(Debug, Default, Clone)]
pub struct SpentInputs {
    /// The Dust spends that fund the fee, by nullifier.
    pub dust: Vec<ReservedInput<DustNullifier>>,
    /// The unshielded UTXOs the build spends.
    pub unshielded: Vec<ReservedInput<SpentUtxoKey>>,
    /// The shielded coins the build spends, by nullifier.
    pub shielded: Vec<ReservedInput<Nullifier>>,
}

/// One input that a build reserved, and the stamp of that reservation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedInput<T> {
    /// The input: a nullifier, or the key of an unshielded UTXO.
    pub input: T,
    /// The block time of the build context when the build selected the
    /// input, which is what the reservation is stamped with.
    pub reserved_at: Timestamp,
}

impl SpentInputs {
    /// The shielded coins a build pinned, for one that spends nothing else.
    pub fn from_shielded(shielded: Vec<Nullifier>, reserved_at: Timestamp) -> Self {
        Self {
            shielded: shielded
                .into_iter()
                .map(|input| ReservedInput { input, reserved_at })
                .collect(),
            ..Self::default()
        }
    }

    /// Add the inputs of `other`. Each input keeps its own stamp.
    pub fn merge(&mut self, other: SpentInputs) {
        self.dust.extend(other.dust);
        self.unshielded.extend(other.unshielded);
        self.shielded.extend(other.shielded);
    }

    /// Whether this build drew nothing at all, so there is nothing to reserve
    /// and nothing to hand back.
    pub fn is_empty(&self) -> bool {
        self.dust.is_empty() && self.unshielded.is_empty() && self.shielded.is_empty()
    }
}

/// One transfer a build can make.
///
/// Data rather than a closure over the builder, so a caller can hand a build
/// to a wallet it reaches only through a trait.
#[derive(Debug)]
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

/// One build, as data: what to make, how to choose the inputs that make it,
/// and how long the transaction stays valid.
#[derive(Debug)]
pub struct TransferRequest {
    /// The transfer to build.
    pub kind: TransferKind,
    /// How to order the coins and UTXOs the build draws on.
    pub coin_selection: CoinSelectionStrategy,
    /// How long the transaction stays valid after the chain time of the
    /// build. `None` gives the chain's `global_ttl`. See
    /// `TransferBuilder::with_ttl`.
    pub ttl: Option<std::time::Duration>,
}

impl TransferRequest {
    /// A request that orders its inputs the default way, and whose
    /// transaction stays valid for the chain's `global_ttl`.
    pub fn new(kind: TransferKind) -> Self {
        Self {
            kind,
            coin_selection: CoinSelectionStrategy::default(),
            ttl: None,
        }
    }

    /// Order the coins and UTXOs this build draws on. See
    /// `TransferBuilder::with_coin_selection`.
    pub fn with_coin_selection(mut self, strategy: CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }

    /// Make the transaction expire `ttl` after the chain time of the build.
    /// See `TransferBuilder::with_ttl`.
    pub fn with_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }
}

impl From<TransferKind> for TransferRequest {
    fn from(kind: TransferKind) -> Self {
        Self::new(kind)
    }
}

/// Recover a printable message from a caught panic payload.
pub(crate) fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    "proof backend panicked with a non-string payload".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_message_handles_both_payload_shapes() {
        assert_eq!(
            panic_message(Box::new("static str".to_string())),
            "static str"
        );
        assert_eq!(panic_message(Box::new("literal")), "literal");
        assert!(panic_message(Box::new(42u8)).contains("non-string payload"));
    }

    #[test]
    fn the_default_coin_selection_is_largest_first() {
        assert_eq!(
            TransferRequest::new(TransferKind::DustRegistration { utxo_ctime: None })
                .coin_selection,
            CoinSelectionStrategy::LargestFirst,
            "every shielded input carries its own proof, so the default \
             spends the fewest inputs"
        );
    }
}
