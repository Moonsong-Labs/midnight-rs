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

use midnight_indexer_client::TransactionResult;

/// Outcome of
/// [`MidnightProvider::wait_transaction_result`](crate::MidnightProvider::wait_transaction_result).
///
/// The indexer cannot positively report "this transaction will never
/// land" — absence from its index is always provisional (the tx may not
/// have landed, or the indexer may simply lag the node). So the only two
/// observable outcomes are: a result surfaced in time, or it didn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxResultWait {
    /// The indexer surfaced the transaction's chain-side result within the
    /// deadline.
    Found(TransactionResult),
    /// The deadline elapsed before the indexer surfaced a result. This does
    /// **not** mean the transaction failed or was excluded: the indexer may
    /// be lagging the node, and the result can still appear later. Poll
    /// again, or consult the node directly, before treating the transaction
    /// as lost.
    TimedOut,
}

impl TxResultWait {
    /// The found result, or `None` if the wait timed out.
    pub fn found(self) -> Option<TransactionResult> {
        match self {
            Self::Found(result) => Some(result),
            Self::TimedOut => None,
        }
    }
}
