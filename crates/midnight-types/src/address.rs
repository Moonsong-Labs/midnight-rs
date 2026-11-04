//! Decode a Midnight shielded, unshielded or Dust address.
//!
//! A [`Seed`](crate::hd::Seed) derives its own addresses and keys. This module
//! reads them back from an address string that someone else shared.

use std::str::FromStr;

use midnight_helpers::ledger_9::{
    DefaultDB, DustWallet, ShieldedWallet, UnshieldedWallet, WalletAddress,
};

use crate::ledger_9::convert::IntoSdk;
use crate::{DustPublicKey, Network, ShieldedRecipient, UnshieldedRecipient, WalletError};

/// Payload length of a shielded address: a 32-byte coin public key followed by
/// the 32-byte serialized encryption public key.
const SHIELDED_ADDRESS_DATA_LEN: usize = 64;

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
    let address = parse_wallet_address(s, &network.into())?;
    // Upstream asserts this length rather than returning its `InvalidCoinKeyLen`
    // variant, so a truncated address would abort the process.
    let len = address.data().len();
    if len != SHIELDED_ADDRESS_DATA_LEN {
        return Err(WalletError::InvalidAddress(format!(
            "shielded address payload is {len} bytes, expected {SHIELDED_ADDRESS_DATA_LEN}"
        )));
    }
    let wallet = ShieldedWallet::<DefaultDB>::try_from(&address)
        .map_err(|e| WalletError::InvalidAddress(format!("not a shielded address: {e:?}")))?;
    Ok(shielded_recipient(&wallet))
}

/// Decode a `mn_addr_*` bech32 string into the user address it carries.
///
/// `network` is the network that the address must belong to, usually the
/// network of the wallet that spends.
///
/// # Errors
///
/// [`WalletError::AddressNetworkMismatch`] for an address of any other
/// network, and [`WalletError::InvalidAddress`] for a string that is not an
/// unshielded address.
pub fn parse_unshielded_recipient(
    s: &str,
    network: impl Into<Network>,
) -> Result<UnshieldedRecipient, WalletError> {
    let address = parse_wallet_address(s, &network.into())?;
    let wallet = UnshieldedWallet::try_from(&address)
        .map_err(|e| WalletError::InvalidAddress(format!("not an unshielded address: {e:?}")))?;
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
    let address = parse_wallet_address(s, &network.into())?;
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

fn parse_wallet_address(s: &str, network: &Network) -> Result<WalletAddress, WalletError> {
    let addr = WalletAddress::from_str(s)
        .map_err(|e| WalletError::InvalidAddress(format!("bech32 decode: {e}")))?;
    check_address_network(&addr, network)?;
    Ok(addr)
}

/// Reject an address minted for a different network.
///
/// The HRP carries the network as a third `_`-separated segment, which mainnet
/// omits entirely (`mn_shield-addr1…`, per upstream `network_suffix`). The
/// upstream `TryFrom<&WalletAddress>` impls check the `mn` prefix and the
/// credential segment but never the network, so without this an address for
/// another chain decodes cleanly and builds a transfer to a key nobody here
/// controls.
fn check_address_network(addr: &WalletAddress, expected: &Network) -> Result<(), WalletError> {
    let hrp = addr.human_readable_part();
    // The network is everything after the credential segment, not just the
    // next segment: upstream appends `_{network_id}` verbatim, and a custom
    // network name may itself contain underscores.
    let actual = hrp.splitn(3, '_').nth(2);
    let want = match expected {
        Network::Mainnet => None,
        other => Some(other.as_str()),
    };
    if actual != want {
        return Err(WalletError::AddressNetworkMismatch {
            expected: expected.as_str().to_string(),
            actual: actual.unwrap_or(Network::Mainnet.as_str()).to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use midnight_helpers::ledger_8;
    use midnight_helpers::ledger_9::{DefaultDB, ShieldedWallet};

    use crate::hd::Seed;
    use crate::ledger_8::convert::{IntoLedger, IntoSdk as _};

    fn seed() -> Seed {
        Seed::from_bytes(&[7; 32]).unwrap()
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
        use crate::Network;
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

    /// A transfer pays the owner that the recipient converts to. A recipient
    /// of other bytes names a user address nobody holds, and the chain pays it
    /// without an error.
    #[test]
    fn a_parsed_unshielded_address_names_the_owner_its_wallet_tracks_on_each_generation() {
        use crate::{Network, WalletError};

        let address = seed().unshielded_address(Network::Undeployed);
        let err = super::parse_unshielded_recipient(&address, Network::Testnet).unwrap_err();
        assert!(
            matches!(err, WalletError::AddressNetworkMismatch { .. }),
            "expected AddressNetworkMismatch, got: {err}"
        );

        let recipient = super::parse_unshielded_recipient(&address, Network::Undeployed).unwrap();
        assert_eq!(recipient, seed().unshielded_recipient());
        let seed8 = IntoLedger::into_ledger(&seed());
        let wallet8 = ledger_8::UnshieldedWallet::default(seed8);
        assert_eq!(IntoLedger::into_ledger(recipient), wallet8.user_address);
    }
}
