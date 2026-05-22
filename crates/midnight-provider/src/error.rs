use midnight_indexer_client::IndexerError;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("indexer error: {0}")]
    Indexer(#[from] IndexerError),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("RPC connection timed out")]
    RpcTimeout,

    /// An operation requiring a synced wallet was invoked on a provider
    /// without one. Call `MidnightProvider::sync_wallet(...)`, or use
    /// `MidnightProvider::with_wallet(...)` if you already have a synced wallet.
    #[error(
        "provider has no wallet; call .sync_wallet(...) on the provider, or .with_wallet(...) if you already have a synced wallet"
    )]
    NoWallet,

    /// An error surfaced from the wallet (sync/resync/transaction building).
    #[error("wallet: {0}")]
    Wallet(String),

    /// Transaction submission failed (connect, build, submit, or watch).
    #[error("submission: {0}")]
    Submission(String),

    /// The chain only has the genesis block, which on dev devnets has a
    /// hardcoded `tblock` from months before wall clock. Building a
    /// transaction now would produce an `intent.ttl` that's already in the
    /// past once the chain produces its first real-time block, causing
    /// rejection at submission. Wait for the chain to advance past genesis
    /// and retry.
    #[error(
        "chain has not advanced past genesis after {0}s; refusing to build a transaction with a stale TTL"
    )]
    ChainNotReady(u64),

    /// The background sync task spawned by
    /// [`MidnightProvider::sync_wallet_with_progress`](crate::MidnightProvider::sync_wallet_with_progress)
    /// panicked or was cancelled before completing.
    #[error("sync task join: {0}")]
    SyncTaskJoin(#[from] tokio::task::JoinError),
}
