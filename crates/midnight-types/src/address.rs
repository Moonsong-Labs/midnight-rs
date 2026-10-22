//! Decode a Midnight shielded address into the keys it carries.
//!
//! A [`Seed`](crate::hd::Seed) derives its own addresses and keys. This module
//! reads them back from an address string that someone else shared.

use midnight_helpers::ledger_9::{DefaultDB, ShieldedWallet};

use crate::ledger_9::convert::IntoSdk;
use crate::{Network, ShieldedRecipient, WalletError};

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
    let wallet = crate::ledger_9::shielded_destination(s, network.into().as_str())?;
    Ok(shielded_recipient(&wallet))
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
}
