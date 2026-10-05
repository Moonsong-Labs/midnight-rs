use std::time::Duration;

use crate::TransactionHash;
use crate::submit::SubmitError;
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

    /// A transaction-manipulation operation failed off-chain (e.g. deserializing
    /// or merging proven transactions for a multi-party submission via
    /// `MidnightProvider::merge_transactions`). Nothing was sent to the node.
    #[error("transaction: {0}")]
    Transaction(String),

    /// The wallet did not see the effect it waited for before the timeout.
    ///
    /// [`MidnightProvider::wait_observed`] returns it with the transaction
    /// whose spends the wallet did not see, and
    /// [`MidnightProvider::resync_until`] with no transaction. The wallet keeps
    /// the state of its last resync. The timeout cancels nothing: the
    /// transaction can still land, and the indexer can still serve it. Wait
    /// again with a longer timeout. After a `PartialSuccess` or `Failure`
    /// verdict, an input that the chain did not spend never reads as spent.
    /// A wait for it always ends here.
    ///
    /// [`MidnightProvider::wait_observed`]: crate::MidnightProvider::wait_observed
    /// [`MidnightProvider::resync_until`]: crate::MidnightProvider::resync_until
    #[error(
        "the wallet did not see {} within {waited:?}",
        match transaction_hash {
            Some(hash) => format!("the spends of transaction {hash}"),
            None => "the effect it waited for".to_string(),
        }
    )]
    EffectTimeout {
        /// How long the wait ran, from its first resync to the check that
        /// found the timeout passed.
        waited: Duration,
        /// The transaction whose spends the wallet waited for, or `None` for
        /// a wait on a balance predicate.
        transaction_hash: Option<TransactionHash>,
    },
}
