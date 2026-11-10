//! What a build spends ([`SpentInputs`]), in the form a reservation takes.

use midnight_helpers::{HashOutput, Timestamp};

use crate::coin::{DustNullifier, Nullifier};

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
