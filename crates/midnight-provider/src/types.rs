//! Plain value types on the provider's public surface: things a caller reads,
//! passes, or formats, with no I/O of their own. The modules that produce them
//! stay about their own job.

/// The Midnight ledger's identity for a transaction: SHA-256 over its tagged
/// serialization.
///
/// Distinct from the substrate extrinsic hash the same handles carry, which
/// covers the SCALE-encoded extrinsic wrapping this transaction and is hashed
/// with blake2-256. This is the one the chain names in its `TxApplied` /
/// `TxPartialSuccess` events, an explorer shows, and an indexer query keyed by
/// transaction hash takes; the extrinsic hash is not recorded off-node at all.
///
/// [`Display`](std::fmt::Display) writes the unprefixed lowercase hex those
/// expect, so `to_string()` produces the form a query takes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransactionHash([u8; 32]);

impl TransactionHash {
    /// The raw digest.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for TransactionHash {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Display for TransactionHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for TransactionHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TransactionHash({self})")
    }
}

/// A Midnight transaction that a block ran, with its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockTransaction {
    /// The ledger's `transaction_hash()` of the transaction, which is the
    /// hash that its outcome event names.
    pub transaction_hash: TransactionHash,
    /// What the chain reports for the transaction.
    pub outcome: TxOutcome,
    /// The tagged transaction bytes, the form of
    /// [`TxEnvelope::into_bytes`](midnight_types::TxEnvelope::into_bytes).
    /// `None` when the block ran the transaction from storage (a collective
    /// `close`, a federated `motion_close`, the Scheduler) and its body
    /// holds no copy. The content of that transaction is then unknown.
    pub bytes: Option<Vec<u8>>,
}

/// What the chain reports for a transaction of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxOutcome {
    /// `Midnight.TxApplied`: every segment applied.
    Applied,
    /// `Midnight.TxPartialSuccess`: segment 0 applied and at least one
    /// fallible segment failed. The event does not say which.
    PartialSuccess,
    /// A transaction in the body of block 0. The node does not run block 0
    /// and records no outcome for it.
    Genesis,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Display` writes the whole digest. The ledger's own hash types print a
    /// 10-character prefix, and a truncated hash silently matches nothing in a
    /// query keyed by transaction hash.
    #[test]
    fn display_writes_the_whole_digest_as_hex() {
        let hash = TransactionHash::from([0xab; 32]);
        assert_eq!(hash.to_string(), "ab".repeat(32));
    }
}
