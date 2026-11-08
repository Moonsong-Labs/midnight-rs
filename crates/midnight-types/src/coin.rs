//! The coin, key and address values the SDK's API carries.
//!
//! Each mirrors the coin-structure type of the same name, field for field.
//! [`crate::convert`] converts them.

use std::fmt;

use midnight_helpers::HashOutput;

/// The type of a shielded token, its "color".
///
/// Treat shielded token ids as opaque: the zero id is not NIGHT.
///
/// [`Display`](fmt::Display) writes the id as 64-char lowercase hex, with no
/// `0x` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShieldedTokenType(pub HashOutput);

impl fmt::Display for ShieldedTokenType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&hex::encode(self.0.0))
    }
}

/// The type of an unshielded token.
///
/// [`Display`](fmt::Display) writes the id as 64-char lowercase hex, with no
/// `0x` prefix, which is the form the indexer reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnshieldedTokenType(pub HashOutput);

impl fmt::Display for UnshieldedTokenType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&hex::encode(self.0.0))
    }
}

/// NIGHT, the native unshielded token.
pub const NIGHT: UnshieldedTokenType = UnshieldedTokenType(HashOutput([0; 32]));

// `1 DUST = 10^15 SPECK` and `1 NIGHT = 10^6 STAR`.
pub use midnight_helpers::{SPECKS_PER_DUST, STARS_PER_NIGHT};

/// The nullifier that marks a shielded coin as spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Nullifier(pub HashOutput);

/// The nonce that makes a shielded coin unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Nonce(pub HashOutput);

/// A shielded coin as its owner knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CoinInfo {
    /// The nonce that makes the coin unique.
    pub nonce: Nonce,
    /// The token the coin holds.
    pub type_: ShieldedTokenType,
    /// The amount of the token, in its smallest unit.
    pub value: u128,
}

/// The public key that owns a shielded coin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CoinPublicKey(pub HashOutput);

/// The key a shielded output is encrypted to, so that its recipient can
/// discover the coin.
///
/// A curve point, held in the ledger's 32-byte encoding.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EncryptionPublicKey(pub(crate) [u8; 32]);

impl EncryptionPublicKey {
    /// The key's 32-byte encoding.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for EncryptionPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EncryptionPublicKey({})", hex::encode(self.0))
    }
}

/// The two public keys a shielded address carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShieldedRecipient {
    /// The key that owns the coins sent to the address.
    pub coin_public_key: CoinPublicKey,
    /// The key those coins are encrypted to.
    pub enc_public_key: EncryptionPublicKey,
}

/// The user address that an unshielded address carries: the hash of the
/// verifying key that owns the UTXOs sent to it.
///
/// [`crate::address::parse_unshielded_recipient`] reads one from a
/// `mn_addr_*` address, and [`crate::hd::Seed::unshielded_recipient`] derives
/// the one of a seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnshieldedRecipient(pub(crate) HashOutput);

/// The nullifier of a Dust spend.
///
/// A field element, held in the ledger's 32-byte little-endian form.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DustNullifier(pub(crate) [u8; 32]);

impl fmt::Debug for DustNullifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DustNullifier({})", hex::encode(self.0))
    }
}

/// The public key that a Dust address carries. It owns the Dust that the
/// tNIGHT registered to this address generates.
///
/// A field element, held in the ledger's 32-byte little-endian form.
/// [`crate::address::parse_dust_address`] reads one from a `mn_dust_*`
/// address.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DustPublicKey(pub(crate) [u8; 32]);

impl fmt::Debug for DustPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DustPublicKey({})", hex::encode(self.0))
    }
}

/// The address of a deployed contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContractAddress(pub HashOutput);

impl ContractAddress {
    /// The shielded token this contract mints under `domain_sep`.
    pub fn custom_shielded_token_type(&self, domain_sep: HashOutput) -> ShieldedTokenType {
        let address = midnight_helpers::ContractAddress(self.0);
        ShieldedTokenType(address.custom_shielded_token_type(domain_sep).0)
    }

    /// The unshielded token this contract mints under `domain_sep`.
    pub fn custom_unshielded_token_type(&self, domain_sep: HashOutput) -> UnshieldedTokenType {
        let address = midnight_helpers::ContractAddress(self.0);
        UnshieldedTokenType(address.custom_unshielded_token_type(domain_sep).0)
    }
}

#[cfg(test)]
mod tests {
    use crate::convert::{IntoLedger as _, IntoSdk as _};

    /// The `Display` docs promise the form the indexer reports, and
    /// `UnshieldedUtxoInfo::token_type_hex` gives it to callers. The inner
    /// `HashOutput`'s own `Display` cuts the id to ten characters.
    #[test]
    fn a_token_type_displays_as_the_indexer_hex() {
        let token = super::UnshieldedTokenType(super::HashOutput([0xab; 32]));
        assert_eq!(token.to_string(), "ab".repeat(32));
    }

    /// A field element's own serialization is variable-length, so a nullifier
    /// that went through it would not fit the SDK's 32 bytes, or would come
    /// back as another element.
    #[test]
    fn a_dust_nullifier_round_trips_through_the_sdk() {
        for value in [
            midnight_helpers::Fr::from(7u64),
            -midnight_helpers::Fr::from(1u64),
        ] {
            let nullifier = midnight_helpers::DustNullifier(value).into_sdk();
            let back: midnight_helpers::DustNullifier = nullifier.into_ledger();
            assert_eq!(back.0, value);
        }
    }
}
