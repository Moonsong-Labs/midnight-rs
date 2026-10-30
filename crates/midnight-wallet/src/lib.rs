//! Wallet state and address derivation for the Midnight SDK.
//!
//! [`Wallet`] owns the seed, the secret keys, the synced ledger state
//! (shielded coins, dust UTXOs, unshielded UTXOs), the ledger parameters,
//! and the latest block context, in the ledger generation the chain runs. It
//! exposes accessors for balances, addresses and cursors. Its resyncs and
//! rescans run through [`LocalWallet`].
//!
//! The API a consumer programs against is the `WalletFacade` trait in
//! `midnight-wallet-facade`, which this crate depends on and implements:
//! [`LocalWallet`] is that role over a `Wallet` this process owns. Readings
//! return owned values and a mutation is one call, so a consumer never holds
//! a lock.
//!
//! Once a wallet is attached, `midnight_provider::MidnightProvider` starts
//! its resyncs and rescans and builds its transactions. Every build resyncs
//! the wallet first. When asked, the wallet also resyncs itself in a
//! background task ([`LocalWallet::sync_in_background`]). The wallet runs
//! each replay against the indexer that it synced from. The provider holds
//! the wallet as an `Arc<dyn WalletFacade>`.
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
//! use midnight_wallet::Wallet;
//!
//! # async fn example(phrase: &str) -> Result<(), Box<dyn std::error::Error>> {
//! // The wallet syncs on its own (zswap + dust + unshielded subscriptions)
//! // and is then attached to the provider.
//! let provider = MidnightProvider::new("ws://localhost:9944");
//! let wallet = Wallet::from_mnemonic(phrase)?
//!     .sync(&provider, "http://localhost:8088")
//!     .await?;
//! let provider = provider.with_wallet(wallet);
//!
//! let balance = provider.balance().await?;
//! # Ok(())
//! # }
//! ```

mod background;
mod ledger_8;
#[expect(
    clippy::duplicate_mod,
    reason = "`ledger_8` and `ledger_9` compile the same per-ledger source against each generation"
)]
mod ledger_9;
mod local;
mod replay;
mod storage;
mod sync;
mod wallet;

// The vocabulary this crate's own signatures name, so a consumer of the
// implementation needs no second dependency for it.
pub use midnight_types::address::{parse_dust_address, parse_shielded_recipient};
pub use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, CoinSelectionStrategy, DustBalance, DustParameters,
    DustPublicKey, EncryptionPublicKey, HashOutput, InvalidNetworkName, LedgerVersion, NIGHT,
    Network, NetworkName, Nonce, Nullifier, Payment, ReservedInput, ShieldedBalance,
    ShieldedCoinBalance, ShieldedRecipient, ShieldedTokenType, SpendableShieldedCoin, SpentInputs,
    SpentUtxoKey, SyncCursors, TrackedUtxo, TransferKind, TransferRequest, TransferResult,
    UnshieldedTokenType, UnshieldedUtxoInfo, WalletBalance, WalletError, chain_pin,
};
// The API this crate implements, so attaching a wallet needs no dependency on
// midnight-wallet-facade either.
pub use midnight_wallet_facade::WalletFacade;

pub use background::BackgroundSync;
pub use local::LocalWallet;
pub use midnight_types::hd::{self, AccountKey, Role, RoleKey, Seed, SeedError, mnemonic};
pub use sync::{SyncHandle, SyncProgress, WalletBuilder};
pub use wallet::Wallet;
