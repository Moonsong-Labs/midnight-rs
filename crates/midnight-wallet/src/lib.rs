//! Wallet state and address derivation for the Midnight SDK.
//!
//! [`Wallet`] holds the keys of each role, the synced ledger state (shielded
//! coins, dust UTXOs, unshielded UTXOs), the ledger parameters, and the latest
//! block context. It holds no seed: its sync derives the keys and drops the
//! seed. It reads its balance and addresses, and it
//! syncs itself. It locks its own state, so a `&Wallet` serves every call and
//! a caller never holds a lock.
//!
//! `midnight_provider::MidnightProvider::with_wallet` attaches a wallet, which
//! the provider funds and signs through. [`Wallet`] implements
//! [`MidnightWallet`], and so do `&Wallet` and `Arc<Wallet>`. Each funding
//! syncs the wallet first. [`Wallet::sync_in_background`] syncs it in a
//! background task. The wallet runs each replay against the indexer that it
//! synced from.
//!
//! For callers that only need an address (no synced state), use
//! [`Seed::unshielded_address`], [`Seed::shielded_address`] and
//! [`Seed::dust_address`].
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
//! ([`WalletError::CorruptParameters`]). Sync keeps the owner address of a
//! UTXO as it comes, because the subscription asks for the UTXOs of one
//! address. These checks catch corruption and protocol violations, not
//! dishonesty. Actively cross-checking indexer
//! answers against the node (e.g. `midnight_queryUnshielded`) is
//! explicitly out of scope; revisit if a threat model requires operating
//! against an untrusted indexer.
//!
//! ```rust,no_run
//! use midnight_provider::MidnightProvider;
//! use midnight_wallet::{Seed, Wallet};
//!
//! # async fn example(phrase: &str) -> Result<(), Box<dyn std::error::Error>> {
//! // The wallet syncs on its own (zswap + dust + unshielded subscriptions)
//! // and is then attached to the provider.
//! let provider = MidnightProvider::new("ws://localhost:9944");
//! let wallet = Wallet::from_seed(Seed::from_mnemonic(phrase)?)
//!     .sync(&provider, "http://localhost:8088")
//!     .await?;
//! let provider = provider.with_wallet(&wallet);
//!
//! let balance = provider.wallet().balance().await;
//! # Ok(())
//! # }
//! ```

mod background;
mod balance;
mod builds;
mod generation;
mod keys;
mod local;
mod pending;
mod pre_fork;
mod replay;
mod snapshot;
mod state;
mod storage;
mod sync;
mod wallet;

// The vocabulary this crate's own signatures name, so a consumer of the
// implementation needs no second dependency for it.
pub use midnight_types::address::{
    parse_dust_address, parse_shielded_recipient, parse_unshielded_recipient,
};
pub use midnight_types::{
    CoinInfo, CoinPublicKey, DustBalance, DustPublicKey, EncryptionPublicKey, FilledTx, FundedTx,
    HashOutput, InvalidNetworkName, MidnightWallet, NIGHT, Network, NetworkName, Nonce, Nullifier,
    ProvenTx, ShieldedBalance, ShieldedCoinBalance, ShieldedRecipient, ShieldedTokenType,
    SignatureRequest, SpendableShieldedCoin, TxEnvelope, UnshieldedRecipient, UnshieldedTokenType,
    UnshieldedUtxoInfo, WalletBalance, WalletError, chain_pin,
};

pub use background::BackgroundSync;
pub use local::Wallet;
pub use midnight_types::hd::{self, AccountKey, Role, RoleKey, Seed, SeedError, mnemonic};
pub use sync::{SyncHandle, SyncProgress, WalletBuilder};
