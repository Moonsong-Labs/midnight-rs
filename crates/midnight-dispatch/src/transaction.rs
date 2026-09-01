//! Submitting a transaction, and what came back, without naming a generation.
//!
//! A transaction crosses this boundary as bytes, which is what the provider
//! already takes and returns, so nothing is re-encoded here. What comes back
//! is hashes and an outcome, which every generation reports alike.

/// What the chain did with a transaction once it landed in a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every fallible segment succeeded and the chain state advanced fully.
    Success,
    /// The guaranteed phase committed, and at least one fallible segment did
    /// not apply.
    PartialSuccess,
    /// The transaction did not apply.
    Failure,
}

/// A transaction that reached a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    /// The block that carried it.
    pub block_hash: [u8; 32],
    /// The extrinsic that carried it, which is how the node names it.
    pub extrinsic_hash: [u8; 32],
    /// The ledger's own identity for the transaction, which the chain names
    /// in its events and the indexer keys by.
    pub transaction_hash: [u8; 32],
    /// What the chain did with it.
    pub verdict: Verdict,
}
