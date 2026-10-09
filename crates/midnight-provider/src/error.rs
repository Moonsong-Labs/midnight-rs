use std::time::Duration;

use crate::TransactionHash;
use crate::submit::{NotApplied, SubmitError};
use midnight_indexer_client::IndexerError;
use midnight_types::WalletError;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("indexer error: {0}")]
    Indexer(#[from] IndexerError),

    #[error("RPC error: {0}")]
    Rpc(String),

    /// An operation that needs a synced wallet ran on a provider without one.
    ///
    /// Sync a wallet with `Wallet::sync` from `midnight-wallet`. Then attach
    /// it with [`MidnightProvider::with_wallet`], as
    /// `.with_wallet(LocalWallet::new(wallet))`.
    ///
    /// [`MidnightProvider::with_wallet`]: crate::MidnightProvider::with_wallet
    #[error(
        "provider has no wallet; sync one with `Wallet::sync` and attach it with \
         `.with_wallet(LocalWallet::new(wallet))`"
    )]
    NoWallet,

    /// An error surfaced from the wallet (sync/resync/transaction building).
    /// Match the inner [`WalletError`], whose `Display` and `source()` this
    /// variant forwards.
    #[error(transparent)]
    Wallet(#[from] WalletError),

    /// The submission of a transaction, or the wait for it, failed.
    ///
    /// Match the inner [`SubmitError`], whose variant docs give the retry rule
    /// for each failure.
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

    /// An effect that a provider call waited for did not show before the timeout.
    ///
    /// [`MidnightProvider::wait_observed`] returns it with the transaction
    /// whose spends the wallet did not see, and
    /// [`MidnightProvider::resync_until`] with no transaction.
    /// [`MidnightProvider::register_all_night`] also returns it. Its docs say
    /// what `transaction_hash` names there.
    ///
    /// The wallet keeps the state of its last resync. The timeout cancels
    /// nothing: the transaction can still land, and the indexer can still
    /// serve it. Wait again with a longer timeout. After a `PartialSuccess`
    /// or `Failure` verdict, an input that the chain did not spend never
    /// reads as spent. A wait for it always ends here.
    ///
    /// [`MidnightProvider::wait_observed`]: crate::MidnightProvider::wait_observed
    /// [`MidnightProvider::resync_until`]: crate::MidnightProvider::resync_until
    /// [`MidnightProvider::register_all_night`]: crate::MidnightProvider::register_all_night
    #[error(
        "{} did not show within {waited:?}",
        match transaction_hash {
            Some(hash) => format!("the effect of transaction {hash}"),
            None => "the effect that the call waited for".to_string(),
        }
    )]
    EffectTimeout {
        /// How long the wait ran before it found that the timeout passed.
        waited: Duration,
        /// The transaction whose effect the wait was for: its spends in the
        /// wallet, or its finality. `None` is a wait on a balance predicate,
        /// or a deadline that passed with no transaction in flight.
        transaction_hash: Option<TransactionHash>,
    },
}

impl From<NotApplied> for ProviderError {
    fn from(not_applied: NotApplied) -> Self {
        Self::NotApplied(Box::new(not_applied))
    }
}
