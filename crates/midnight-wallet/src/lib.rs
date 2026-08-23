//! Wallet state and address derivation for the Midnight SDK.
//!
//! [`Wallet`] owns the seed, the secret keys, the synced ledger state
//! (shielded coins, dust UTXOs, unshielded UTXOs), the ledger parameters,
//! and the latest block context. It exposes mutation methods
//! (`set_block_context`, `set_parameters`, `reserve_pending`) plus
//! accessors for balances and addresses.
//!
//! The API a consumer programs against is the `WalletFacade` trait in
//! `midnight-wallet-facade`, which this crate depends on and implements:
//! [`LocalWallet`] is that role over a `Wallet` this process owns. Readings
//! return owned values and a mutation is one call, so a consumer never holds
//! a lock.
//!
//! All network I/O — initial sync, resync, indexer subscriptions, building a
//! [`midnight_helpers::LedgerContext`] — is driven by
//! [`midnight_provider::MidnightProvider`], which holds the wallet as an
//! `Arc<dyn WalletFacade>`.
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

pub mod balance;
pub mod hd;
pub mod local;
pub mod pending;
pub mod state;
pub mod storage;
pub mod sync_ext;
pub mod transfer;

// The API this crate implements lives in midnight-wallet-facade; re-export it
// whole so a consumer of the implementation needs no second dependency.
pub use midnight_wallet_core::{
    DustBalance, Network, PreparedInput, ShieldedBalance, ShieldedCoinBalance,
    SpendableShieldedCoin, SyncCursors, TrackedUtxo, UnshieldedUtxoInfo, WalletBalance,
    WalletError, address, chain_pin, network, prepared_input,
};
pub use midnight_wallet_facade::{ReservedBuild, WalletFacade};

pub use hd::{AccountKey, Role, RoleKey, Seed, SeedError, mnemonic};
pub use local::LocalWallet;
pub use state::{
    ResyncCommit, ResyncPlan, ShieldedRescanCommit, ShieldedRescanPlan, SyncProgress, Wallet,
};
pub use sync_ext::{SyncHandle, SyncWalletBuilder, SyncWalletExt};
pub use transfer::{
    BuildInputs, PreparedTransfer, SpentInputs, SpentUtxoKey, TransferBuilder, TransferKind,
    TransferRequest, TransferResult, panic_message, parse_shielded_recipient,
};

pub use midnight_helpers::LocalProofServer;
pub use midnight_helpers::{
    CoinInfo, CoinSelectionStrategy, HashOutput, NIGHT, Nonce, SPECKS_PER_DUST, STARS_PER_NIGHT,
    ShieldedTokenType, UnshieldedTokenType, WalletSeed, WalletSeedError,
};

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
