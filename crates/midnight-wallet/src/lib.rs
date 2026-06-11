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
    DustBalance, ShieldedBalance, ShieldedCoinBalance, UnshieldedUtxoInfo, WalletBalance,
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
