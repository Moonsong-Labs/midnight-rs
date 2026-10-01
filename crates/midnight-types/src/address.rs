//! Free helpers for deriving Midnight wallet addresses from a seed.
//!
//! Address derivation is a pure function of seed + network. These helpers
//! exist for callers that only need an address and don't want to construct
//! a full `Wallet` (which carries synced state and requires I/O at
//! construction time).
//!
//! Equivalent methods exist on the implementing wallet and call into these
//! functions, so synced wallets expose the same addresses.
//!
//! Every ledger generation derives the same keys and addresses from a seed,
//! so these run through ledger 9's derivation.

use midnight_helpers::ledger_9::{DefaultDB, IntoWalletAddress, ShieldedWallet, UnshieldedWallet};

use crate::ledger_9::convert::IntoSdk;
use crate::{Network, ShieldedRecipient, WalletError, WalletSeed};

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

/// The shielded keys `seed` derives: what a shielded address carries.
pub fn shielded_recipient(seed: &WalletSeed) -> ShieldedRecipient {
    (&ShieldedWallet::<DefaultDB>::default(seed.clone())).into_sdk()
}

/// Decode a `mn_shield-addr_*` bech32 string into the keys it carries.
///
/// `network` is the network the address must belong to — normally the one the
/// spending wallet is synced to. An address for any other network is rejected
/// with [`WalletError::AddressNetworkMismatch`].
pub fn parse_shielded_recipient(
    s: &str,
    network: impl Into<Network>,
) -> Result<ShieldedRecipient, WalletError> {
    Ok((&crate::ledger_9::shielded_destination(s, network)?).into_sdk())
}

#[cfg(test)]
mod tests {
    use midnight_helpers::ledger_8;
    use midnight_helpers::ledger_8::IntoWalletAddress as _;

    use super::*;
    use crate::ledger_8::convert::{IntoLedger, IntoSdk as _};

    fn seed() -> WalletSeed {
        WalletSeed::from([7; 32])
    }

    /// The helpers above derive through ledger 9 alone. If ledger 8 derived
    /// other keys from the same seed, a wallet on a ledger 8 chain would
    /// receive at addresses its owner never sees.
    #[test]
    fn both_generations_derive_the_same_addresses_and_keys() {
        let seed8 = (&seed()).into_ledger();
        let unshielded8 = ledger_8::UnshieldedWallet::default(seed8.clone())
            .address(Network::Undeployed.as_str())
            .to_bech32();
        let shielded8 = ledger_8::ShieldedWallet::<ledger_8::DefaultDB>::default(seed8);
        let address8 = shielded8.address(Network::Undeployed.as_str()).to_bech32();

        assert_eq!(derive_unshielded(&seed(), Network::Undeployed), unshielded8);
        assert_eq!(derive_shielded(&seed(), Network::Undeployed), address8);
        assert_eq!(shielded_recipient(&seed()), (&shielded8).into_sdk());
    }

    /// The encryption key crosses generations as bytes. A wrong encoding
    /// would hand a circuit call a key that encrypts to nobody.
    #[test]
    fn an_encryption_key_crosses_generations_unchanged() {
        use crate::ledger_9::convert::IntoLedger as IntoLedger9;

        let seed8 = IntoLedger::into_ledger(&seed());
        let key8 = ledger_8::ShieldedWallet::<ledger_8::DefaultDB>::default(seed8).enc_public_key;
        let key9 = ShieldedWallet::<DefaultDB>::default(seed()).enc_public_key;

        assert_eq!(IntoLedger9::into_ledger(key8.into_sdk()), key9);
    }
}
