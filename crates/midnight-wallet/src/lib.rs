//! Wallet credentials and state management for the Midnight SDK.
//!
//! [`Wallet`] wraps a [`WalletSeed`] and a network identifier, exposing
//! shielded / unshielded address derivation. The seed is validated at
//! construction so downstream code (deploy / call paths) does not need
//! to re-parse it.
//!
//! [`WalletState`] provides wallet state backed by the Midnight indexer
//! for real-time balance tracking via subscriptions. Transaction building
//! syncs from the node on-demand.
//!
//! ```rust,ignore
//! use midnight_wallet::{Wallet, WalletBuilder};
//!
//! let wallet = Wallet::from_seed_hex(
//!     "0000000000000000000000000000000000000000000000000000000000000001",
//!     "undeployed",
//! )?;
//!
//! // Build a live wallet with indexer-based balance tracking
//! let live = WalletBuilder::new(wallet, "ws://localhost:9944")
//!     .indexer_url("http://localhost:8088")
//!     .build()
//!     .await?;
//!
//! let balance = live.balance().await;
//! ```

pub mod background;
pub mod balance;
pub mod builder;
pub mod state;
pub mod transfer;

pub use background::WalletSync;
pub use balance::{
    DustBalance, ShieldedBalance, ShieldedCoinBalance, UnshieldedUtxoInfo, WalletBalance,
};
pub use builder::{LiveWallet, TransferGuard, WalletBuilder};
pub use state::{SyncResult, TrackedUtxo, WalletState};
pub use transfer::{TransferBuilder, TransferResult};

use midnight_node_ledger_helpers::{
    DefaultDB, IntoWalletAddress, ShieldedWallet, UnshieldedWallet, WalletSeed, WalletSeedError,
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

    /// Transfer transaction failed.
    #[error("transfer failed: {0}")]
    Transfer(String),

    /// Transaction submission failed.
    #[error("submission failed: {0}")]
    Submission(String),
}

/// A validated wallet handle.
///
/// Holds a [`WalletSeed`] and the network identifier (e.g. `"undeployed"`,
/// `"testnet"`, `"mainnet"`) used when rendering addresses.
#[derive(Debug, Clone)]
pub struct Wallet {
    seed: WalletSeed,
    network: String,
}

impl Wallet {
    /// Create a wallet from a hex-encoded seed (16, 32, or 64 bytes after decoding).
    pub fn from_seed_hex(seed: &str, network: impl Into<String>) -> Result<Self, WalletError> {
        let seed = WalletSeed::try_from_hex_str(seed)?;
        Ok(Self {
            seed,
            network: network.into(),
        })
    }

    /// Create a wallet from a 32-byte seed.
    pub fn from_seed_bytes(seed: [u8; 32], network: impl Into<String>) -> Self {
        Self {
            seed: WalletSeed::from(seed),
            network: network.into(),
        }
    }

    /// Create a wallet from a BIP-39 mnemonic phrase.
    pub fn from_mnemonic(phrase: &str, network: impl Into<String>) -> Result<Self, WalletError> {
        let seed = WalletSeed::try_from_mnemonic(phrase)?;
        Ok(Self {
            seed,
            network: network.into(),
        })
    }

    /// The validated seed.
    ///
    /// Internal callers (deploy, call) use this to feed into the helpers'
    /// transaction-building APIs without re-parsing.
    pub fn seed(&self) -> &WalletSeed {
        &self.seed
    }

    /// The network identifier this wallet derives addresses for.
    pub fn network(&self) -> &str {
        &self.network
    }

    /// The unshielded receiving address, e.g. `mn_addr_undeployed1...`.
    pub fn unshielded_address(&self) -> String {
        UnshieldedWallet::default(self.seed)
            .address(&self.network)
            .to_bech32()
    }

    /// The shielded receiving address, e.g. `mn_shield-addr_undeployed1...`.
    pub fn shielded_address(&self) -> String {
        ShieldedWallet::<DefaultDB>::default(self.seed)
            .address(&self.network)
            .to_bech32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    #[test]
    fn from_seed_hex_validates_length() {
        let err = Wallet::from_seed_hex("00", "undeployed").unwrap_err();
        assert!(matches!(err, WalletError::Seed(_)));
    }

    #[test]
    fn from_seed_hex_rejects_invalid_hex() {
        let err = Wallet::from_seed_hex("zz", "undeployed").unwrap_err();
        assert!(matches!(err, WalletError::Seed(_)));
    }

    #[test]
    fn unshielded_address_uses_network_suffix() {
        let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
        let addr = wallet.unshielded_address();
        assert!(addr.starts_with("mn_addr_undeployed"), "address was {addr}");
    }

    #[test]
    fn shielded_address_uses_network_suffix() {
        let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
        let addr = wallet.shielded_address();
        assert!(
            addr.starts_with("mn_shield-addr_undeployed"),
            "address was {addr}"
        );
    }

    #[test]
    fn from_seed_bytes_yields_same_address_as_from_seed_hex() {
        let wallet_a = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
        let mut bytes = [0u8; 32];
        bytes[31] = 1;
        let wallet_b = Wallet::from_seed_bytes(bytes, "undeployed");
        assert_eq!(wallet_a.unshielded_address(), wallet_b.unshielded_address());
    }

    #[test]
    fn network_is_preserved() {
        let wallet = Wallet::from_seed_hex(DEV_SEED, "testnet").unwrap();
        assert_eq!(wallet.network(), "testnet");
    }
}
