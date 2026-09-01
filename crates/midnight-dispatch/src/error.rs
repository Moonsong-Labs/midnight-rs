//! What can go wrong before a call reaches a generation.

/// A failure reaching or reading the chain.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The chain runs a generation this build does not carry.
    #[error(transparent)]
    Generation(#[from] crate::GenerationError),
    /// The named circuit is not one the contract declares.
    #[error("the contract declares no circuit named {0:?}")]
    UnknownCircuit(String),
    /// The node or indexer refused, or could not be reached.
    #[error("{0}")]
    Chain(String),
}
