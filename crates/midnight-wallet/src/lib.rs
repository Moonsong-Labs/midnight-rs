//! Wallet state and address derivation for the Midnight SDK.
//!
//! [`Wallet`] owns the seed, the secret keys, the synced ledger state
//! (shielded coins, dust UTXOs, unshielded UTXOs), the ledger parameters,
//! and the latest block context, in the ledger generation the chain runs. It
//! exposes accessors for balances, addresses and cursors, and the resync and
//! rescan steps.
//!
//! The API a consumer programs against is the `WalletFacade` trait in
//! `midnight-wallet-facade`, which this crate depends on and implements:
//! [`LocalWallet`] is that role over a `Wallet` this process owns. Readings
//! return owned values and a mutation is one call, so a consumer never holds
//! a lock.
//!
//! Once a wallet is attached, `midnight_provider::MidnightProvider` drives
//! its network I/O: resyncs, the indexer subscriptions and the ledger
//! context. It holds the wallet as an `Arc<dyn WalletFacade>`.
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
//! ```rust,no_run
//! use midnight_provider::MidnightProvider;
//! use midnight_wallet::{LocalWallet, Network, Wallet};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let seed = midnight_wallet::WalletSeed::try_from_hex_str(
//! #     "0000000000000000000000000000000000000000000000000000000000000001",
//! # ).unwrap();
//! // The wallet syncs on its own (zswap + dust + unshielded subscriptions)
//! // and is then attached to the provider.
//! let provider = MidnightProvider::new("ws://localhost:9944", "http://localhost:8088")?;
//! let wallet = Wallet::sync(provider.indexer_url(), seed, Network::Undeployed).await?;
//! let provider = provider.with_wallet(LocalWallet::new(wallet));
//!
//! let balance = provider.balance().await?;
//! # Ok(())
//! # }
//! ```

pub mod hd;
mod ledger_8;
#[expect(
    clippy::duplicate_mod,
    reason = "`ledger_8` and `ledger_9` compile the same per-ledger source against each generation"
)]
mod ledger_9;
pub mod local;
mod replay;
mod storage;
pub mod sync;
mod wallet;

// The vocabulary this crate's own signatures name, so a consumer of the
// implementation needs no second dependency for it.
pub use midnight_types::address::parse_shielded_recipient;
pub use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, CoinSelectionStrategy, DustBalance, DustParameters,
    EncryptionPublicKey, HashOutput, LedgerVersion, NIGHT, Network, Nonce, Nullifier,
    ShieldedBalance, ShieldedCoinBalance, ShieldedRecipient, ShieldedTokenType,
    SpendableShieldedCoin, SpentInputs, SpentUtxoKey, SyncCursors, TrackedUtxo, TransferKind,
    TransferRequest, TransferResult, UnshieldedTokenType, UnshieldedUtxoInfo, WalletBalance,
    WalletError, WalletSeed, WalletSeedError, address, chain_pin, network, panic_message,
};
// The API this crate implements, so attaching a wallet needs no dependency on
// midnight-wallet-facade either.
pub use midnight_wallet_facade::WalletFacade;

pub use hd::{AccountKey, Role, RoleKey, Seed, SeedError, mnemonic};
pub use local::LocalWallet;
pub use sync::{SyncHandle, SyncProgress, WalletSyncBuilder};
pub use wallet::{ResyncCommit, ResyncPlan, ShieldedRescanCommit, ShieldedRescanPlan, Wallet};

#[cfg(test)]
mod tests {
    use super::WalletSeed;
    use super::address::{derive_shielded, derive_unshielded};

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
    fn derive_unshielded_differs_per_network() {
        let a = derive_unshielded(&dev_seed(), "undeployed");
        let b = derive_unshielded(&dev_seed(), "testnet");
        assert_ne!(a, b);
    }
}
