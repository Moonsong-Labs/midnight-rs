//! Free helpers for deriving Midnight wallet addresses from a seed.
//!
//! Address derivation is a pure function of seed + network. These helpers
//! exist for callers that only need an address and don't want to construct
//! a full [`crate::Wallet`] (which carries synced state and requires I/O at
//! construction time).
//!
//! Equivalent methods exist on [`crate::Wallet`] and call into these
//! functions, so synced wallets expose the same addresses.

use midnight_helpers::{
    DefaultDB, IntoWalletAddress, ShieldedWallet, UnshieldedWallet, WalletSeed,
};

use crate::Network;

/// Derive the unshielded receiving address for `seed` on `network`.
///
/// `network` accepts both [`Network`] and `&str` / `String` (via `Into`).
/// E.g. `mn_addr_undeployed1...`.
pub fn derive_unshielded(seed: &WalletSeed, network: impl Into<Network>) -> String {
    UnshieldedWallet::default(seed.clone())
        .address(network.into().as_str())
        .to_bech32()
}

/// Derive the shielded receiving address for `seed` on `network`.
///
/// `network` accepts both [`Network`] and `&str` / `String` (via `Into`).
/// E.g. `mn_shield-addr_undeployed1...`.
pub fn derive_shielded(seed: &WalletSeed, network: impl Into<Network>) -> String {
    ShieldedWallet::<DefaultDB>::default(seed.clone())
        .address(network.into().as_str())
        .to_bech32()
}
