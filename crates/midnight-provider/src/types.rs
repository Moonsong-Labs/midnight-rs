#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub node_connected: bool,
    pub indexer_connected: bool,
    pub block_height: Option<i64>,
    pub peers: Option<u64>,
    pub is_syncing: Option<bool>,
}

// Re-export state query types from the pallet RPC crate — single source of truth.
pub use midnight_rpc_api::{RpcStateQuery as StateQuery, RpcStateQueryResult as StateQueryResult};
