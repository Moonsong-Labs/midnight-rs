//! Decode a Midnight shielded, unshielded or Dust address into the keys or
//! the user address it carries.
//!
//! A [`Seed`](crate::hd::Seed) derives its own addresses and keys. This module
//! reads them back from an address string that someone else shared.

use midnight_helpers::ledger_9::{DefaultDB, DustWallet, ShieldedWallet};

use crate::ledger_9::convert::IntoSdk;
use crate::{DustPublicKey, Network, ShieldedRecipient, UserAddress, WalletError};

/// Decode a `mn_shield-addr_*` bech32 string into the keys it carries.
///
/// `network` is the network that the address must belong to, usually the
/// network of the wallet that spends.
///
/// # Errors
///
/// [`WalletError::AddressNetworkMismatch`] for an address of any other
/// network, and [`WalletError::InvalidAddress`] for a string that is not a
/// shielded address.
pub fn parse_shielded_recipient(
    s: &str,
    network: impl Into<Network>,
) -> Result<ShieldedRecipient, WalletError> {
    let wallet = crate::ledger_9::shielded_destination(s, &network.into())?;
    Ok(shielded_recipient(&wallet))
}

/// Decode a `mn_addr_*` bech32 string into the [`UserAddress`] it carries.
///
/// A Compact circuit that pays a user takes this value as its `UserAddress`
/// argument.
///
/// `network` is the network that the address must belong to, usually the
/// network of the wallet that pays.
///
/// # Errors
///
/// [`WalletError::AddressNetworkMismatch`] for an address of any other
/// network, and [`WalletError::InvalidAddress`] for a string that is not an
/// unshielded address.
pub fn parse_unshielded_address(
    s: &str,
    network: impl Into<Network>,
) -> Result<UserAddress, WalletError> {
    let wallet = crate::ledger_9::parse_unshielded_recipient(s, &network.into())?;
    Ok(wallet.user_address.into_sdk())
}

/// Decode a `mn_dust_*` bech32 string into the Dust public key it carries.
///
/// `network` is the network that the address must belong to, usually the
/// network of the wallet that registers its tNIGHT to the address.
///
/// # Errors
///
/// [`WalletError::AddressNetworkMismatch`] for an address of any other
/// network, and [`WalletError::InvalidAddress`] for a string that is not a
/// Dust address.
pub fn parse_dust_address(
    s: &str,
    network: impl Into<Network>,
) -> Result<DustPublicKey, WalletError> {
    let address = crate::ledger_9::parse_wallet_address(s, &network.into())?;
    let wallet = DustWallet::<DefaultDB>::try_from(&address)
        .map_err(|e| WalletError::InvalidAddress(format!("not a Dust address: {e:?}")))?;
    Ok(wallet.public_key.into_sdk())
}

/// The two public keys of a shielded wallet.
pub(crate) fn shielded_recipient(wallet: &ShieldedWallet<DefaultDB>) -> ShieldedRecipient {
    ShieldedRecipient {
        coin_public_key: wallet.coin_public_key.into_sdk(),
        enc_public_key: wallet.enc_public_key.into_sdk(),
    }
}

#[cfg(test)]
mod tests {
    use midnight_helpers::ledger_8;
    use midnight_helpers::ledger_9::{DefaultDB, ShieldedWallet};

    use crate::Network;
    use crate::hd::Seed;
    use crate::ledger_8::convert::{IntoLedger, IntoSdk as _};

    fn seed() -> Seed {
        Seed::from_bytes(&[7; 32]).unwrap()
    }

    /// A circuit pays the bytes that a seed's user address gives. They must be
    /// the bytes that its shared address carries, or the payout goes to a key
    /// that the seed does not hold.
    #[test]
    fn the_unshielded_address_of_a_seed_carries_its_user_address() {
        let address = seed().unshielded_address(Network::Preprod);
        let parsed = super::parse_unshielded_address(&address, Network::Preprod).unwrap();
        assert_eq!(parsed, seed().user_address());
    }

    /// The encryption key crosses generations as bytes. A wrong encoding
    /// would hand a circuit call a key that encrypts to nobody.
    #[test]
    fn an_encryption_key_crosses_generations_unchanged() {
        use crate::ledger_9::convert::IntoLedger as IntoLedger9;

        let seed8 = IntoLedger::into_ledger(&seed());
        let key8 = ledger_8::ShieldedWallet::<ledger_8::DefaultDB>::default(seed8).enc_public_key;
        let seed9 = IntoLedger9::into_ledger(&seed());
        let key9 = ShieldedWallet::<DefaultDB>::default(seed9).enc_public_key;

        assert_eq!(IntoLedger9::into_ledger(key8.into_sdk()), key9);
    }

    /// A registration puts the parsed key into the chain's generation. A key
    /// that changes on the way names a Dust address nobody holds, and the
    /// chain sends the Dust there without an error.
    #[test]
    fn a_parsed_dust_address_names_the_key_its_wallet_tracks_on_each_generation() {
        use crate::ledger_9::convert::IntoLedger as IntoLedger9;

        let address = seed().dust_address(Network::Undeployed);
        let key = super::parse_dust_address(&address, Network::Undeployed).unwrap();

        let seed8 = IntoLedger::into_ledger(&seed());
        let wallet8 = ledger_8::DustWallet::<ledger_8::DefaultDB>::default(seed8, None);
        assert_eq!(IntoLedger::into_ledger(key), wallet8.public_key);
        let seed9 = IntoLedger9::into_ledger(&seed());
        let wallet9 = midnight_helpers::ledger_9::DustWallet::<DefaultDB>::default(seed9, None);
        assert_eq!(IntoLedger9::into_ledger(key), wallet9.public_key);
    }
}
