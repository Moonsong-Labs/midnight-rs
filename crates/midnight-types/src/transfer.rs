//! The vocabulary of a transfer build: the request that names one
//! ([`TransferRequest`]), what it spends ([`SpentInputs`]), and its result
//! ([`TransferResult`]).
//!
//! The builds themselves are per ledger generation, in
//! [`crate::ledger_8`] and [`crate::ledger_9`].

use midnight_helpers::{CoinSelectionStrategy, Timestamp};

use crate::coin::{DustNullifier, Nullifier, ShieldedTokenType, UnshieldedTokenType};
use crate::ledger_version::LedgerVersion;

/// A proven transaction a build made, and the inputs the build reserved for
/// it.
pub struct TransferResult {
    /// The proven transaction, tagged with its generation's encoding.
    pub tx_bytes: Vec<u8>,
    /// The generation the transaction is built for.
    pub ledger_version: LedgerVersion,
    /// The unshielded UTXOs this transaction spends. Its build reserved them,
    /// and `WalletFacade::release` names them by these keys if the
    /// transaction never reaches the chain.
    pub spent_unshielded_inputs: Vec<SpentUtxoKey>,
    /// The shielded coins this transaction spends, by nullifier. Reserved and
    /// released the same way.
    pub spent_shielded_inputs: Vec<Nullifier>,
    /// The Dust spends that fund this transaction's fee, by nullifier.
    /// Reserved and released the same way.
    pub spent_dust: Vec<DustNullifier>,
    /// Deterministic Dust fee the chain will charge for this transaction, in
    /// SPECK (`1 DUST = 10^15 SPECK`). Computed via
    /// `Transaction::fees(&ledger.parameters, false)` against the parameters
    /// the build pipeline saw — matches what the node's own estimation RPC
    /// returns and what the indexer later reports as `paidFees` for an
    /// accepted, included transaction.
    pub fee_speck: u128,
    /// The chain time this build selected against, which its reservation is
    /// stamped with. A release names it, so handing this result back drops
    /// this build's own entry, not one a later build made in a later block.
    pub reserved_at: Timestamp,
}

impl std::fmt::Debug for TransferResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferResult")
            .field("tx_bytes", &format_args!("{} bytes", self.tx_bytes.len()))
            .field("ledger_version", &self.ledger_version)
            .field("spent_unshielded_inputs", &self.spent_unshielded_inputs)
            .field("spent_shielded_inputs", &self.spent_shielded_inputs)
            .field("spent_dust", &self.spent_dust)
            .field("fee_speck", &self.fee_speck)
            .field("reserved_at", &self.reserved_at)
            .finish()
    }
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
///
/// `reserved_at` travels with the inputs because it is half of a
/// reservation's identity. A release names both, so a build hands back its
/// own entry, not one a later build made over the same input in a later
/// block. Two reservations in one block share the stamp, which is why a
/// reservation is released once.
#[derive(Debug, Default, Clone)]
pub struct SpentInputs {
    /// The Dust spends that fund the fee, by nullifier.
    pub dust: Vec<DustNullifier>,
    /// The unshielded UTXOs the build spends.
    pub unshielded: Vec<SpentUtxoKey>,
    /// The shielded coins the build spends, by nullifier.
    pub shielded: Vec<Nullifier>,
    /// The chain time the build read when it selected these inputs, which is
    /// what the reservation is stamped with.
    pub reserved_at: Timestamp,
}

impl SpentInputs {
    /// The shielded coins a build pinned, for one that spends nothing else.
    pub fn from_shielded(shielded: Vec<Nullifier>, reserved_at: Timestamp) -> Self {
        Self {
            shielded,
            reserved_at,
            ..Self::default()
        }
    }

    /// Whether this build drew nothing at all, so there is nothing to reserve
    /// and nothing to hand back.
    pub fn is_empty(&self) -> bool {
        self.dust.is_empty() && self.unshielded.is_empty() && self.shielded.is_empty()
    }
}

impl From<&TransferResult> for SpentInputs {
    fn from(result: &TransferResult) -> Self {
        Self {
            dust: result.spent_dust.clone(),
            unshielded: result.spent_unshielded_inputs.clone(),
            shielded: result.spent_shielded_inputs.clone(),
            reserved_at: result.reserved_at,
        }
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

/// One build, as data: what to make, and how to choose the inputs that make
/// it.
#[derive(Debug)]
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
