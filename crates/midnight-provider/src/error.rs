use crate::submit::{NotApplied, SubmitError};
use midnight_types::WalletError;

/// The failure of a provider method: a node query, the wallet, the send or its
/// wait, or an off-chain transaction step.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// A node query or the connect failed, or the node gave no answer in time.
    ///
    /// [`MidnightProvider::prepare`](crate::MidnightProvider::prepare) also
    /// returns it when the node's metadata has no matching call. On a send,
    /// it means that the transaction never left this process.
    #[error("RPC error: {0}")]
    Rpc(String),

    /// An error surfaced from the wallet (its sync, its funding or its
    /// signing), from the proof, or from a fill that refuses its request,
    /// such as a sponsor of bytes that do not decode.
    /// Match the inner [`WalletError`], whose `Display` and `source()` this
    /// variant forwards.
    #[error(transparent)]
    Wallet(#[from] WalletError),

    /// The submission of a transaction, or the wait for it, failed.
    ///
    /// Match the inner [`SubmitError`], whose variant docs give the retry rule
    /// for each failure, and whose `Display` and `source()` this variant
    /// forwards.
    #[error(transparent)]
    Submission(#[from] SubmitError),

    /// The transaction landed in a block, but the chain did not apply it.
    /// The [`NotApplied`] variant tells a partial success from a failure, and
    /// its receipt names the transaction and the block.
    // Boxed: unboxed, it is the largest variant, and every `Result` that
    // carries a `ProviderError` pays its size.
    #[error(transparent)]
    NotApplied(Box<NotApplied>),

    /// A transaction-manipulation operation failed off-chain, such as a
    /// [`merge_transactions`](crate::merge_transactions), or a send refused a
    /// transaction that pays no fee. Nothing was sent to the node.
    #[error("transaction: {0}")]
    Transaction(String),
}

impl From<NotApplied> for ProviderError {
    fn from(not_applied: NotApplied) -> Self {
        Self::NotApplied(Box::new(not_applied))
    }
}
