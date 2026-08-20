//! Plain value types on the provider's public surface: things a caller reads,
//! passes, or formats, with no I/O of their own. The modules that produce them
//! stay about their own job.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub node_connected: bool,
    pub indexer_connected: bool,
    pub block_height: Option<u64>,
    pub peers: Option<u64>,
    pub is_syncing: Option<bool>,
}

// Re-export state query types from the pallet RPC crate — single source of truth.
pub use midnight_rpc_api::{RpcStateQuery as StateQuery, RpcStateQueryResult as StateQueryResult};

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

impl From<TransactionHash> for [u8; 32] {
    fn from(hash: TransactionHash) -> Self {
        hash.0
    }
}

impl AsRef<[u8]> for TransactionHash {
    fn as_ref(&self) -> &[u8] {
        &self.0
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
        assert_eq!(hash.to_string().len(), 64);
        assert_eq!(
            format!("{hash:?}"),
            format!("TransactionHash({})", "ab".repeat(32))
        );
    }

    #[test]
    fn a_transaction_hash_round_trips_through_its_bytes() {
        let bytes = [7u8; 32];
        let hash = TransactionHash::from(bytes);
        assert_eq!(hash.as_bytes(), &bytes);
        assert_eq!(<[u8; 32]>::from(hash), bytes);
        assert_eq!(hex::encode(hash), hash.to_string());
    }
}
