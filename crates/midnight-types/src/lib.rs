//! Implementation-free vocabulary and toolkit for Midnight wallets.
//!
//! Everything here is a function of `midnight-helpers`, the [`H256`] of
//! `primitive-types`, and the BIP-32 and BIP-39 seed derivation: the wallet
//! [`Seed`](hd::Seed) and its HD derivation, the network and address model,
//! the balance readings, [`WalletError`], and the [`MidnightWallet`] trait
//! with the stages of a transaction that it funds and signs. No wallet
//! implementation lives here, and none is depended on, so a wallet, a
//! provider, and a contract builder can all share this vocabulary without
//! sharing an implementation.
//!
//! [`convert`] converts between the SDK's values and the ledger's.

pub mod address;
#[doc(hidden)]
pub mod build;
pub mod chain_pin;
pub mod convert;
pub mod hd;

mod balance;
mod builder;
mod coin;
mod error;
mod hash;
mod inputs;
mod network;
mod stage;
mod sync;
mod transfer;
mod wallet;

pub use balance::{
    DustBalance, ShieldedBalance, ShieldedCoinBalance, SpendableShieldedCoin, UnshieldedUtxoInfo,
    WalletBalance,
};
pub use coin::{
    CoinInfo, CoinPublicKey, ContractAddress, DustNullifier, DustPublicKey, EncryptionPublicKey,
    NIGHT, Nonce, Nullifier, SPECKS_PER_DUST, STARS_PER_NIGHT, ShieldedRecipient,
    ShieldedTokenType, UnshieldedRecipient, UnshieldedTokenType,
};
pub use error::WalletError;
pub use hash::{BlockHash, ExtrinsicHash};
pub use midnight_helpers::{CoinSelectionStrategy, HashOutput};
pub use network::{InvalidNetworkName, Network, NetworkName};
/// The chain's 32-byte hash, under the names [`BlockHash`] and [`ExtrinsicHash`].
///
/// `Display` abbreviates it to `0x1234…cdef`. Format it with `{:x}`, or with
/// `{:#x}` for a `0x` prefix, to get the whole digest.
pub use primitive_types::H256;
pub use stage::{FilledTx, FundedTx, ProvenTx, TxEnvelope};
pub use wallet::MidnightWallet;
