//! Implementation-free vocabulary and toolkit for Midnight wallets.
//!
//! Everything here is a function of `midnight-helpers`, the [`H256`] of
//! `primitive-types`, and the BIP-32 and BIP-39 seed derivation: the wallet
//! [`Seed`](hd::Seed) and its HD derivation, the network and address model,
//! the balance readings, the transfer requests and results, and
//! [`WalletError`]. No wallet implementation lives here, and none is depended
//! on, so a wallet, a provider, and a contract builder can all share this
//! vocabulary without sharing an implementation.
//!
//! The vocabulary is the same for every ledger generation. The transfer
//! machinery is per generation: [`ledger_8`] and [`ledger_9`] compile one
//! source against each generation's types, and [`LedgerVersion`] names the
//! generation a chain runs.
//!
//! The API a consumer programs a wallet against is the `WalletFacade` trait in
//! `midnight-wallet-facade`, which speaks in this crate's types.

pub mod address;
pub mod chain_pin;
pub mod hd;
pub mod ledger_8;
#[expect(
    clippy::duplicate_mod,
    reason = "`ledger_8` and `ledger_9` compile the same per-ledger source against each generation"
)]
pub mod ledger_9;

mod balance;
mod coin;
mod error;
mod hash;
mod ledger_version;
mod network;
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
pub use error::{WalletError, refuse_repeated_nullifiers};
pub use hash::{BlockHash, ExtrinsicHash};
pub use ledger_version::{LedgerVersion, UnknownLedger};
pub use midnight_helpers::{CoinSelectionStrategy, HashOutput, Timestamp};
pub use network::{InvalidNetworkName, Network, NetworkName};
pub use parameters::{ChainParameters, DustParameters};
/// The chain's 32-byte hash, under the names [`BlockHash`] and [`ExtrinsicHash`].
///
/// `Display` abbreviates it to `0x1234…cdef`. Format it with `{:x}`, or with
/// `{:#x}` for a `0x` prefix, to get the whole digest.
pub use primitive_types::H256;
pub use sync::{SyncCursors, TrackedUtxo};
pub use transfer::{
    ReservedInput, SpentInputs, SpentUtxoKey, TransferKind, TransferRequest, TransferResult,
    panic_message,
};
