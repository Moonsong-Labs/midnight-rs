use crate::submit::SubmitError;
use midnight_indexer_client::IndexerError;
use midnight_wallet::WalletError;

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
    /// Callers can match on the inner [`WalletError`] variants
    /// ([`Seed`](WalletError::Seed), [`Sync`](WalletError::Sync),
    /// [`EventOrder`](WalletError::EventOrder),
    /// [`MalformedUtxo`](WalletError::MalformedUtxo),
    /// [`CorruptParameters`](WalletError::CorruptParameters),
    /// [`Transfer`](WalletError::Transfer), [`Storage`](WalletError::Storage),
    /// [`InvalidAddress`](WalletError::InvalidAddress)) to distinguish cases
    /// without grepping the error message.
    #[error("wallet: {0}")]
    Wallet(#[from] WalletError),

    /// Transaction submission failed (connect, build, submit, or watch).
    /// Match the inner [`SubmitError`] to pick a recovery path:
    /// [`Invalid`](SubmitError::Invalid) is a definitive rejection (safe to
    /// rebuild and resubmit), [`Dropped`](SubmitError::Dropped) and
    /// [`RuntimeError`](SubmitError::RuntimeError) are not (the tx may
    /// still land; resubmitting the same inputs risks a double spend), and
    /// [`WatchStream`](SubmitError::WatchStream) /
    /// [`SubmitRpc`](SubmitError::SubmitRpc) /
    /// [`NotSubmitted`](SubmitError::NotSubmitted) are transport-level.
    #[error("submission: {0}")]
    Submission(#[from] SubmitError),

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
    /// [`SyncWalletBuilder::stream`](crate::SyncWalletBuilder::stream)
    /// panicked or was cancelled before completing.
    #[error("sync task join: {0}")]
    SyncTaskJoin(#[from] tokio::task::JoinError),

    /// A streamed sync was cancelled because its progress receiver was
    /// dropped before the sync completed. See
    /// [`SyncWalletBuilder::stream`](crate::SyncWalletBuilder::stream) for
    /// the cancellation contract: dropping the receiver (or the
    /// [`SyncHandle`](crate::SyncHandle)) tears the sync down.
    #[error("sync cancelled: progress receiver dropped before sync completed")]
    SyncCancelled,
}
