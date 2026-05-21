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
    /// without one. Call `MidnightProvider::with_wallet(...)`.
    #[error("provider has no wallet; call .with_wallet(...) on the provider")]
    NoWallet,

    /// An error surfaced from the wallet (sync/resync/transaction building).
    #[error("wallet: {0}")]
    Wallet(String),
}
