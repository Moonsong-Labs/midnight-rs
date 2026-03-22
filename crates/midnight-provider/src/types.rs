use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub node_connected: bool,
    pub indexer_connected: bool,
    pub block_height: Option<i64>,
    pub peers: Option<u64>,
    pub is_syncing: Option<bool>,
}

/// A query for a specific field/key in a contract's state tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateQuery {
    pub field_path: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Result of a single state query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateQueryResult {
    pub field_path: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
