use std::collections::HashSet;
use std::fmt::Debug;
use std::hash::Hash;

use crate::{LedgerVersion, Network, ShieldedTokenType, UnknownLedger, UnshieldedTokenType};

/// Errors that can occur with wallet operations.
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    /// Sync with node failed.
    #[error("sync failed: {0}")]
    Sync(String),

    /// A streamed sync was cancelled because its progress receiver was
    /// dropped before the sync completed. See
    /// `WalletBuilder::sync_with_progress` in `midnight-wallet` for the
    /// cancellation contract: dropping the receiver (or the `SyncHandle`)
    /// tears the sync down.
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

    /// The sync gave up on the indexer.
    ///
    /// [`IndexerError::is_retryable`] tells whether a later sync can succeed.
    ///
    /// [`IndexerError::is_retryable`]: midnight_indexer_client::IndexerError::is_retryable
    #[error("indexer: {0}")]
    Indexer(#[from] midnight_indexer_client::IndexerError),

    /// Transfer transaction failed.
    #[error("transfer failed: {0}")]
    Transfer(String),

    /// The wallets that fund the fee hold too little spendable Dust.
    ///
    /// A build through `MidnightProvider` resyncs the wallet first, so a later
    /// build reads the Dust that the wallet has gained since. The wallet's
    /// [`DustBalance`](crate::DustBalance) tells which next step applies: see
    /// [Dust shortfalls] in `docs/dust-and-fees.md`.
    ///
    /// [Dust shortfalls]: https://github.com/Moonsong-Labs/midnight-rs/blob/main/docs/dust-and-fees.md#dust-shortfalls
    #[error("insufficient Dust: the build asks for {required} SPECK, {available} SPECK spendable")]
    InsufficientDust {
        /// The Dust the build asks its funding wallets for, in SPECK.
        ///
        /// The build prices the transaction with a margin for price rises, so
        /// this can be more than the fee the chain charges. A Dust registration
        /// asks only for the part of its fee past the generationless Dust of
        /// the tNIGHT it spends.
        ///
        /// It is a lower bound on what the build needs: the Dust spends that
        /// pay the fee make the transaction larger, and a larger transaction
        /// costs more.
        required: u128,
        /// The Dust the funding wallets can give now, in SPECK, less what
        /// pending builds reserve.
        ///
        /// A Dust registration has no funding wallets, so it reads 0.
        available: u128,
    },

    /// The wallet holds too little of a shielded token for the transfer.
    ///
    /// When a pending build spends coins of this token, wait for its
    /// transaction to finalize, then build again. Otherwise the wallet must
    /// receive more of the token.
    #[error("insufficient shielded token {token_type}: need {required}, {available} spendable")]
    InsufficientShielded {
        /// The token the transfer spends.
        token_type: ShieldedTokenType,
        /// The amount the transfer spends.
        required: u128,
        /// The amount the wallet can spend now, less the coins that pending
        /// builds reserve.
        available: u128,
    },

    /// The wallet holds too little of an unshielded token for the transfer.
    ///
    /// When a pending build spends UTXOs of this token, wait for its
    /// transaction to finalize, then build again. Otherwise the wallet must
    /// receive more of the token.
    #[error("insufficient unshielded token {token_type}: need {required}, {available} spendable")]
    InsufficientUnshielded {
        /// The token the transfer spends.
        token_type: UnshieldedTokenType,
        /// The amount the transfer spends.
        required: u128,
        /// The amount the wallet can spend now, less the UTXOs that pending
        /// builds reserve.
        available: u128,
    },

    /// A build named an input that another build already holds.
    #[error("{held} is reserved by a build that has not confirmed yet")]
    InputsReserved {
        /// The first held input the build named.
        held: String,
    },

    /// A build named the same input more than once.
    ///
    /// The build stops before it spends or proves anything. Name each input
    /// once.
    #[error("{input} is named more than once in one build")]
    DuplicateInput {
        /// The first input the build named again.
        input: String,
    },

    /// ZK proving failed.
    ///
    /// The ledger's `ProofProvider::prove` returns a bare transaction with no
    /// error channel, so a proof backend signals failure by an unwind. The
    /// proving call site catches that unwind and reports its message here
    /// instead, so a long-running caller sees an error rather than losing its
    /// task.
    ///
    /// The catch converts any unwind of the proving future, whatever its
    /// message.
    #[error("proving failed: {0}")]
    Proving(String),

    /// State persistence failed.
    #[error("storage: {0}")]
    Storage(String),

    /// The chain's data belongs to no ledger generation this build links.
    #[error(transparent)]
    UnknownLedger(#[from] UnknownLedger),

    /// Data of one ledger generation met a wallet, or a value, of another.
    ///
    /// A build returns it between a chain's hard fork and the wallet's next
    /// resync, and a build after that resync succeeds. A shielded rescan
    /// returns it in the same window: resync, then rescan. A replay that
    /// meets an earlier generation than the wallet's state returns it too,
    /// which means the chain was replaced.
    #[error("expected {expected} data, found {found} data")]
    LedgerMismatch {
        /// The generation of the wallet, or of the value the data met.
        expected: LedgerVersion,
        /// The generation of the data.
        found: LedgerVersion,
    },

    /// The persisted snapshot holds a later ledger generation than the chain
    /// runs, so it belongs to a chain the node no longer has.
    ///
    /// The snapshot is left alone. Remove the directory named here and sync
    /// again, which replays from genesis.
    #[error(
        "wallet snapshot holds {snapshot} state, but the chain runs {chain}: \
         it belongs to another chain. Remove {path} and sync again."
    )]
    LedgerRegression {
        /// The snapshot directory to remove.
        path: String,
        /// The generation the snapshot's state is in.
        snapshot: LedgerVersion,
        /// The generation the chain runs.
        chain: LedgerVersion,
    },

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

    /// The node runs another network than the one the wallet syncs as.
    ///
    /// A sync that names its network with `WalletBuilder::with_network` in
    /// `midnight-wallet` checks it before it reads the indexer, unless the
    /// caller opts out with `WalletBuilder::unpinned`. To recover, name the
    /// network that the node runs. Or leave the network out, so that the sync
    /// takes the node's network. Or use a node of the named network.
    #[error("this wallet syncs as network {wallet}, but the node runs {chain}")]
    NetworkMismatch {
        /// The network the sync was asked to use.
        wallet: Network,
        /// The network the node's runtime reports.
        chain: Network,
    },

    /// The sync names no network, and the node reported no valid one.
    ///
    /// The node could not answer, it has no network id set, or its id is not
    /// a valid name for a [`Network`]. A wallet with no network takes the
    /// network that the node runs. To recover, check the node URL and that the
    /// node is up, or name the network with `WalletBuilder::with_network` in
    /// `midnight-wallet`.
    #[error(
        "the node did not report a valid network: check the node URL and that the node is up, \
         or name the network with `with_network`, for example `with_network(Network::Preprod)`"
    )]
    NetworkUnknown,
}

/// Refuse a list of shielded coin nullifiers that names one coin twice.
///
/// A build calls this before it spends anything, so a repeated coin cannot be
/// spent, or its value counted, twice.
///
/// # Errors
///
/// [`WalletError::DuplicateInput`] naming the first nullifier that `nullifiers`
/// repeats.
pub fn refuse_repeated_nullifiers<N: Eq + Hash + Debug>(
    nullifiers: &[N],
) -> Result<(), WalletError> {
    let mut named = HashSet::with_capacity(nullifiers.len());
    match nullifiers.iter().find(|n| !named.insert(*n)) {
        Some(repeated) => Err(WalletError::DuplicateInput {
            input: format!("shielded coin {repeated:?}"),
        }),
        None => Ok(()),
    }
}
