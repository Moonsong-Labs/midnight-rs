//! Decode a Midnight shielded, unshielded or Dust address.
//!
//! A [`Seed`](crate::hd::Seed) derives its own addresses and keys. This module
//! reads them back from an address string that someone else shared.

use std::str::FromStr;

use midnight_helpers::{DefaultDB, DustWallet, ShieldedWallet, UnshieldedWallet, WalletAddress};

use crate::convert::IntoSdk;
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
    use midnight_helpers::{DefaultDB, DustWallet, ShieldedWallet, UnshieldedWallet};

    use crate::Network;
    use crate::convert::{IntoLedger, IntoSdk as _};
    use crate::hd::Seed;

    fn seed() -> Seed {
        Seed::from_bytes(&[7; 32]).unwrap()
    }

    /// The SDK holds the encryption key as bytes. A wrong encoding would hand
    /// a circuit call a key that encrypts to nobody.
    #[test]
    fn an_encryption_key_round_trips_through_the_sdk() {
        let key = ShieldedWallet::<DefaultDB>::default(seed().into_ledger()).enc_public_key;

        assert_eq!(key.into_sdk().into_ledger(), key);
    }

    /// A registration puts the parsed key into the ledger's type. A key that
    /// changes on the way names a Dust address nobody holds, and the chain
    /// sends the Dust there without an error.
    #[test]
    fn a_parsed_dust_address_names_the_key_its_wallet_tracks() {
        let address = seed().dust_address(Network::Undeployed);
        let key = super::parse_dust_address(&address, Network::Undeployed).unwrap();

        let wallet = DustWallet::<DefaultDB>::default(seed().into_ledger(), None);
        assert_eq!(key.into_ledger(), wallet.public_key);
    }

    /// A transfer pays the owner that the recipient converts to. A recipient
    /// of other bytes names a user address nobody holds, and the chain pays it
    /// without an error.
    #[test]
    fn a_parsed_unshielded_address_names_the_owner_its_wallet_tracks() {
        let address = seed().unshielded_address(Network::Undeployed);
        let recipient = super::parse_unshielded_recipient(&address, Network::Undeployed).unwrap();
        assert_eq!(recipient, seed().unshielded_recipient());
        let wallet = UnshieldedWallet::default(seed().into_ledger());
        assert_eq!(recipient.into_ledger(), wallet.user_address);
    }
}
