use crate::submit::{NotApplied, SubmitError};
use midnight_indexer_client::IndexerError;
use midnight_types::WalletError;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("indexer error: {0}")]
    Indexer(#[from] IndexerError),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("RPC connection timed out")]
    RpcTimeout,

    /// An operation requiring a synced wallet was invoked on a provider
    /// without one. Sync a wallet (`Wallet::sync` in `midnight-wallet`) and
    /// attach it with `MidnightProvider::with_wallet`.
    #[error(
        "provider has no wallet; sync one (`Wallet::sync`) and attach it with .with_wallet(...)"
    )]
    NoWallet,

    /// An error surfaced from the wallet (sync/resync/transaction building).
    /// Match the inner [`WalletError`], whose `Display` and `source()` this
    /// variant forwards.
    #[error(transparent)]
    Wallet(#[from] WalletError),

    /// Transaction submission failed (connect, build, submit, or watch).
    /// Match the inner [`SubmitError`] to pick a recovery path:
    /// [`Invalid`](SubmitError::Invalid) is a definitive rejection (safe to
    /// rebuild and resubmit), [`Dropped`](SubmitError::Dropped) and
    /// [`NodeError`](SubmitError::NodeError) are not (the tx may
    /// still land; resubmitting the same inputs risks a double spend), and
    /// [`WatchStream`](SubmitError::WatchStream) /
    /// [`SubmitRpc`](SubmitError::SubmitRpc) /
    /// [`NotSubmitted`](SubmitError::NotSubmitted) are transport-level.
    #[error("submission: {0}")]
    Submission(#[from] SubmitError),

    /// The transaction landed in a block, but the chain did not apply it.
    /// The [`NotApplied`] holds the inclusion and its verdict.
    // Boxed: unboxed, it makes `ProviderError` so large that an error holding
    // one inline is larger than clippy's `result_large_err` limit.
    #[error(transparent)]
    NotApplied(Box<NotApplied>),

    /// A transaction-manipulation operation failed off-chain (e.g. deserializing
    /// or merging proven transactions for a multi-party submission via
    /// `MidnightProvider::merge_transactions`). Nothing was sent to the node.
    #[error("transaction: {0}")]
    Transaction(String),
}

impl From<NotApplied> for ProviderError {
    fn from(not_applied: NotApplied) -> Self {
        Self::NotApplied(Box::new(not_applied))
    }
}
