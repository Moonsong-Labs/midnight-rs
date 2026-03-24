use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub node_connected: bool,
    pub indexer_connected: bool,
    pub block_height: Option<i64>,
    pub peers: Option<u64>,
    pub is_syncing: Option<bool>,
}

/// A query into a contract's state tree.
///
/// Each element in `path` is a hex-encoded serialized `AlignedValue`.
/// Interpreted as array index, map key, or merkle tree position depending
/// on the `StateValue` variant at each level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateQuery {
    pub path: Vec<String>,
}

/// Result of a single state query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateQueryResult {
    pub query: StateQuery,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
