//! Implementation-free vocabulary and toolkit for Midnight wallets.
//!
//! Everything here is a function of `midnight-helpers` types: the network and
//! address model, the balance readings, the transfer build machinery and its
//! requests and results, and [`WalletError`]. No wallet implementation lives
//! here, and none is depended on, so a wallet, a provider, and a contract
//! builder can all share this vocabulary without sharing an implementation.
//!
//! The API a consumer programs a wallet against is the `WalletFacade` trait in
//! `midnight-wallet-facade`, which speaks in this crate's types.

pub mod address;
pub mod balance;
pub mod chain_pin;
pub mod network;
pub mod prepared_input;
pub mod transfer;

mod error;
mod sync;

pub use balance::{
    DustBalance, ShieldedBalance, ShieldedCoinBalance, SpendableShieldedCoin, UnshieldedUtxoInfo,
    WalletBalance,
};
pub use error::WalletError;
// Helpers types this crate's own signatures name, so a consumer needs no
// direct midnight-helpers dependency for them.
pub use midnight_helpers::{CoinInfo, CoinSelectionStrategy, WalletSeed};
pub use network::Network;
pub use prepared_input::PreparedInput;
pub use sync::{SyncCursors, TrackedUtxo, parse_intent_hash_hex};
pub use transfer::{
    BuildInputs, DustSpendBatch, PreparedTransfer, SpentInputs, SpentUtxoKey, TransferBuilder,
    TransferKind, TransferRequest, TransferResult, panic_message, parse_shielded_recipient,
};
