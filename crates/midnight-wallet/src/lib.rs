//! Wallet state and address derivation for the Midnight SDK.
//!
//! [`Wallet`] owns the seed, the secret keys, the synced ledger state
//! (shielded coins, dust UTXOs, unshielded UTXOs), the ledger parameters,
//! and the latest block context. It exposes mutation methods
//! (`set_block_context`, `set_parameters`, `reserve_pending`) plus
//! accessors for balances and addresses.
//!
//! All network I/O — initial sync, resync, indexer subscriptions, building a
//! [`midnight_helpers::LedgerContext`] — is driven by
//! [`midnight_provider::MidnightProvider`], which owns the wallet behind an
//! `Arc<RwLock<_>>`.
//!
//! For callers that only need an address (no synced state), use the free
//! helpers in [`address`].
//!
//! # Indexer trust model
//!
//! The indexer is the wallet's **sole** data source: shielded state, dust
//! state, the unshielded UTXO set, and the ledger parameters used for fee
//! and TTL math are all rebuilt from indexer subscriptions and blocks.
//! Nothing is cross-checked against a node. A hostile or compromised
//! indexer can therefore fabricate UTXOs the chain does not contain (the
//! node rejects transactions built from them) or withhold real ones (funds
//! look missing until a sync against an honest indexer), so point the
//! provider at an indexer trusted as much as the node.
//!
//! What sync does enforce is the *shape* of the data: event ids must not go
//! backwards within a subscription connection
//! ([`WalletError::EventOrder`]), an event with a malformed field rejects
//! the whole event before any part of it is applied
//! ([`WalletError::MalformedUtxo`], decode errors), and decoded ledger
//! parameters are sanity-checked before fee math consumes them
//! ([`WalletError::CorruptParameters`]). These checks catch corruption and
//! protocol violations, not dishonesty. Actively cross-checking indexer
//! answers against the node (e.g. `midnight_queryUnshielded`) is
//! explicitly out of scope; revisit if a threat model requires operating
//! against an untrusted indexer.
//!
//! ```rust,ignore
//! use midnight_provider::MidnightProvider;
//!
//! // The provider owns the URLs; sync_wallet drives the zswap + dust +
//! // unshielded sync against the provider's indexer.
//! let provider = MidnightProvider::new("ws://localhost:9944", "http://localhost:8088")?
//!     .sync_wallet(seed, Network::Undeployed, None)
//!     .await?;
//!
//! let balance = provider.balance().await?;
//! ```

pub mod address;
pub mod balance;
pub mod hd;
pub mod network;
pub mod pending;
pub mod state;
pub mod storage;
pub mod transfer;

pub use balance::{
    DustBalance, ShieldedBalance, ShieldedCoinBalance, SpendableShieldedCoin, UnshieldedUtxoInfo,
    WalletBalance,
};
pub use hd::{AccountKey, Role, RoleKey, Seed, SeedError, mnemonic};
pub use network::Network;
pub use state::{ResyncCommit, ResyncPlan, SyncProgress, TrackedUtxo, Wallet};
pub use transfer::{SpentUtxoKey, TransferBuilder, TransferResult, parse_shielded_recipient};

pub use midnight_helpers::LocalProofServer;
pub use midnight_helpers::{
    HashOutput, NIGHT, SPECKS_PER_DUST, STARS_PER_NIGHT, ShieldedTokenType, UnshieldedTokenType,
    WalletSeed, WalletSeedError,
};

/// Errors that can occur with wallet operations.
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    /// The provided seed could not be parsed.
    #[error("invalid wallet seed: {0}")]
    Seed(#[from] WalletSeedError),

    /// Sync with node failed.
    #[error("sync failed: {0}")]
    Sync(String),

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

    /// State persistence failed.
    #[error("storage: {0}")]
    Storage(String),

    /// The recipient address could not be parsed.
    #[error("invalid address: {0}")]
    InvalidAddress(String),
}

#[cfg(test)]
mod tests {
    use super::address::{derive_shielded, derive_unshielded};
    use midnight_helpers::WalletSeed;

    const DEV_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    fn dev_seed() -> WalletSeed {
        WalletSeed::try_from_hex_str(DEV_SEED).unwrap()
    }

    #[test]
    fn derive_unshielded_uses_network_suffix() {
        let addr = derive_unshielded(&dev_seed(), "undeployed");
        assert!(addr.starts_with("mn_addr_undeployed"), "address was {addr}");
    }

    #[test]
    fn derive_shielded_uses_network_suffix() {
        let addr = derive_shielded(&dev_seed(), "undeployed");
        assert!(
            addr.starts_with("mn_shield-addr_undeployed"),
            "address was {addr}"
        );
    }

    #[test]
    fn derive_unshielded_is_deterministic_for_a_seed() {
        let a = derive_unshielded(&dev_seed(), "undeployed");
        let b = derive_unshielded(&dev_seed(), "undeployed");
        assert_eq!(a, b);
    }

    #[test]
    fn derive_unshielded_differs_per_network() {
        let a = derive_unshielded(&dev_seed(), "undeployed");
        let b = derive_unshielded(&dev_seed(), "testnet");
        assert_ne!(a, b);
    }
}
