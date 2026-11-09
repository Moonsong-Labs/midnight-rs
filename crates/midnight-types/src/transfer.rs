//! The vocabulary of a transfer build: the request that names one
//! ([`TransferRequest`]) and what it spends ([`SpentInputs`]).
//!
//! [`crate::build::TransferBuilder`] makes the builds themselves.

use midnight_helpers::{HashOutput, Timestamp};

use crate::coin::{
    DustNullifier, DustPublicKey, Nullifier, ShieldedRecipient, ShieldedTokenType,
    UnshieldedRecipient, UnshieldedTokenType,
};

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
/// its own entry, not one a later build made over the same input at a later
/// chain time. Two reservations at one chain time share the stamp, which is
/// why a reservation is released once.
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
    /// The chain time of the build context when the build selected the
    /// input, which is what the reservation is stamped with.
    pub reserved_at: Timestamp,
}

impl SpentInputs {
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
        recipient: ShieldedRecipient,
        /// False leaves the transaction fee-less, for another wallet to fund.
        pay_fees: bool,
    },
    /// An unshielded (UTXO) transfer. See
    /// `TransferBuilder::prepare_unshielded`.
    Unshielded {
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: UnshieldedRecipient,
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
        /// The Dust public key that gets the Dust of the registered tNIGHT.
        /// `None` names this wallet's own key.
        target: Option<DustPublicKey>,
    },
}

/// One build, as data: what to make, and how long the transaction stays
/// valid.
#[derive(Debug)]
pub struct TransferRequest {
    /// The transfer to build.
    pub kind: TransferKind,
    /// How long the transaction stays valid after the chain time of the
    /// build. `None` gives the chain's `global_ttl`, the longest TTL the chain
    /// accepts. A shorter TTL bounds how long a transaction that never
    /// confirms can still land, and how long the wallet holds its inputs.
    ///
    /// The build fails with
    /// [`WalletError::TtlOutOfRange`](crate::WalletError::TtlOutOfRange) when
    /// `ttl` is under one second or longer than `global_ttl`. It fails with
    /// [`WalletError::TtlWithoutIntent`](crate::WalletError::TtlWithoutIntent)
    /// for a Dustless shielded transfer or a swap half, which carry no intent
    /// to hold a TTL.
    pub ttl: Option<std::time::Duration>,
}

impl TransferRequest {
    /// A request whose transaction stays valid for the chain's `global_ttl`.
    pub fn new(kind: TransferKind) -> Self {
        Self { kind, ttl: None }
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
}
