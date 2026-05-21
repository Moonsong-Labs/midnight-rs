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
}
