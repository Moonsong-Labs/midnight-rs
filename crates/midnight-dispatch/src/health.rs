//! Reachability, in a shape no generation owns.

/// Node and indexer reachability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    /// Whether the node answered.
    pub node_connected: bool,
    /// Whether the indexer answered.
    pub indexer_connected: bool,
    /// The best block height the node reported.
    pub block_height: Option<u64>,
    /// How many peers the node reported.
    pub peers: Option<u64>,
    /// Whether the node reported itself still syncing.
    pub is_syncing: Option<bool>,
}
