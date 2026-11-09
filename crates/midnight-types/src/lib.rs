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
//! The transfer machinery names the ledger's transaction types:
//! [`TransferBuilder`] builds a [`PreparedTransfer`], and [`convert`] converts
//! between the SDK's values and the ledger's.
//!
//! The API a consumer programs a wallet against is the `WalletFacade` trait in
//! `midnight-wallet-facade`, which speaks in this crate's types.

pub mod address;
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
mod sync;
mod transfer;

pub use balance::{
    DustBalance, ShieldedBalance, ShieldedCoinBalance, SpendableShieldedCoin, UnshieldedUtxoInfo,
    WalletBalance,
};
pub use builder::{
    BuildInputs, Cover, DustSpendBatch, PreparedTransfer, TransferBuilder, balance_external,
    chain_time, execution_context, fee_speck, longest_ttl, prepare_no_validate,
};
pub use coin::{
    CoinInfo, CoinPublicKey, ContractAddress, DustNullifier, DustPublicKey, EncryptionPublicKey,
    NIGHT, Nonce, Nullifier, SPECKS_PER_DUST, STARS_PER_NIGHT, ShieldedRecipient,
    ShieldedTokenType, UnshieldedRecipient, UnshieldedTokenType,
};
pub use error::WalletError;
pub use hash::{BlockHash, ExtrinsicHash};
pub use midnight_helpers::{CoinSelectionStrategy, HashOutput, Timestamp};
pub use network::{InvalidNetworkName, Network, NetworkName};
/// The chain's 32-byte hash, under the names [`BlockHash`] and [`ExtrinsicHash`].
///
/// `Display` abbreviates it to `0x1234…cdef`. Format it with `{:x}`, or with
/// `{:#x}` for a `0x` prefix, to get the whole digest.
pub use primitive_types::H256;
pub use sync::TrackedUtxo;
pub use transfer::{
    ReservedInput, SpentInputs, SpentUtxoKey, TransferKind, TransferRequest, TransferResult,
};
