use midnight_helpers::WalletSeedError;

/// Errors that can occur with wallet operations.
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    /// The provided seed could not be parsed.
    #[error("invalid wallet seed: {0}")]
    Seed(#[from] WalletSeedError),

    /// Sync with node failed.
    #[error("sync failed: {0}")]
    Sync(String),

    /// A streamed sync was cancelled because its progress receiver was
    /// dropped before the sync completed. See `WalletSyncBuilder::stream` in
    /// `midnight-wallet` for the cancellation contract: dropping the receiver
    /// (or the `SyncHandle`) tears the sync down.
    #[error("sync cancelled: progress receiver dropped before sync completed")]
    SyncCancelled,

    /// The background sync task panicked or was cancelled before completing.
    #[error("sync task join: {0}")]
    SyncTaskJoin(String),

    /// The persisted snapshot belongs to a chain the node no longer has. Its
    /// event cursors are counts, so a resume would climb the new chain to the
    /// same counts and report the old chain's balance as current.
    ///
    /// The snapshot is left alone. Remove the directory named here and sync
    /// again, which replays from genesis.
    #[error(
        "wallet snapshot belongs to another chain: it pinned finalized block {pinned_height} \
         as {pinned_hash}, and the node reports {found} at that height. \
         Remove {path} and sync again."
    )]
    ChainMismatch {
        path: String,
        pinned_height: u64,
        pinned_hash: String,
        found: String,
    },

    /// The indexer delivered an event id lower than one already delivered
    /// on the same subscription connection. Re-delivering already-applied
    /// events at the start of a (re)connection is legal (and deduped);
    /// going backwards mid-stream is not, and indicates a corrupt or
    /// hostile indexer.
    #[error("{kind} event stream went backwards: id {id} after {prev} on the same connection")]
    EventOrder {
        /// Which replay stream observed the regression (`zswap`, `dust`,
        /// or `unshielded`).
        kind: &'static str,
        /// The offending event id.
        id: i64,
        /// The highest id the same connection had already delivered.
        prev: i64,
    },

    /// The indexer sent an unshielded UTXO with a field the wallet cannot
    /// parse. The event carrying it was rejected as a whole; no part of it
    /// was applied to the wallet.
    #[error("malformed unshielded UTXO from indexer (tx {tx_id:?}): {field} = {value:?}: {reason}")]
    MalformedUtxo {
        /// The offending UTXO field.
        field: &'static str,
        /// The raw value the indexer sent.
        value: String,
        /// Why it failed to parse.
        reason: String,
        /// The indexer transaction id of the event that carried the UTXO,
        /// when the event had one. Identifies the offending event without
        /// digging through debug logs.
        tx_id: Option<i64>,
    },

    /// Ledger parameters decoded from an indexer block failed sanity
    /// checks; fee and TTL math would compute nonsense from them.
    #[error("corrupt ledger parameters from indexer: {field} = {value}")]
    CorruptParameters {
        /// The offending parameter field.
        field: &'static str,
        /// The decoded value that failed the check.
        value: String,
    },

    /// Indexer client error (HTTP / GraphQL / deserialization).
    #[error("indexer: {0}")]
    Indexer(#[from] midnight_indexer_client::IndexerError),

    /// Transfer transaction failed.
    #[error("transfer failed: {0}")]
    Transfer(String),

    /// A build named an input that another build already holds.
    #[error("{held} is reserved by a build that has not confirmed yet")]
    InputsReserved {
        /// The first held input the build named.
        held: String,
    },

    /// ZK proving failed.
    ///
    /// The ledger's `ProofProvider::prove` returns a bare transaction with no
    /// error channel, so a proof backend signals failure by panicking. The
    /// proving call site catches that unwind and reports it here instead, so a
    /// long-running caller sees an error rather than losing its task.
    #[error("proving failed: {0}")]
    Proving(String),

    /// State persistence failed.
    #[error("storage: {0}")]
    Storage(String),

    /// The recipient address could not be parsed.
    #[error("invalid address: {0}")]
    InvalidAddress(String),

    /// The recipient address encodes a different network than the wallet is
    /// synced to. The keys in such an address are unusable on this chain, so
    /// building a transfer against it would send funds nowhere recoverable.
    #[error("address is for network `{actual}`, but this wallet is on `{expected}`")]
    AddressNetworkMismatch {
        /// The network the wallet is synced to.
        expected: String,
        /// The network named by the address's bech32 HRP.
        actual: String,
    },
}
