//! Implementation-free vocabulary and toolkit for Midnight wallets.
//!
//! Everything here is a function of `midnight-helpers` and the indexer
//! client's response types: the network and address model, the balance
//! readings, the transfer requests and results, and [`WalletError`]. No
//! wallet implementation lives here, and none is depended on, so a wallet, a
//! provider, and a contract builder can all share this vocabulary without
//! sharing an implementation.
//!
//! The vocabulary is the same for every ledger generation. The transfer
//! machinery is per generation: [`ledger_8`] and [`ledger_9`] compile one
//! source against each generation's types, and [`LedgerVersion`] names the
//! generation a chain runs.
//!
//! The API a consumer programs a wallet against is the `WalletFacade` trait in
//! `midnight-wallet-facade`, which speaks in this crate's types.

pub mod address;
pub mod balance;
pub mod chain_pin;
pub mod ledger_8;
#[expect(
    clippy::duplicate_mod,
    reason = "`ledger_8` and `ledger_9` compile the same per-ledger source against each generation"
)]
pub mod ledger_9;
pub mod network;

mod coin;
mod error;
mod ledger_version;
mod parameters;
mod sync;
mod transfer;

pub use balance::{
    DustBalance, ShieldedBalance, ShieldedCoinBalance, SpendableShieldedCoin, UnshieldedUtxoInfo,
    WalletBalance,
};
pub use coin::{
    CoinInfo, CoinPublicKey, ContractAddress, DustNullifier, EncryptionPublicKey, NIGHT, Nonce,
    Nullifier, SPECKS_PER_DUST, STARS_PER_NIGHT, ShieldedRecipient, ShieldedTokenType,
    UnshieldedTokenType,
};
pub use error::WalletError;
pub use ledger_version::{LedgerVersion, UnknownLedger};
// The seed is the same in every generation. Ledger 9's definition serves as
// the SDK's, and a generation module converts it (`convert::IntoLedger`).
pub use midnight_helpers::ledger_9::{WalletSeed, WalletSeedError};
pub use midnight_helpers::{CoinSelectionStrategy, HashOutput, Timestamp};
pub use network::Network;
pub use parameters::{ChainParameters, DustParameters};
pub use sync::{SyncCursors, TrackedUtxo, parse_intent_hash_hex};
pub use transfer::{
    SpentInputs, SpentUtxoKey, TransferKind, TransferRequest, TransferResult, panic_message,
};
