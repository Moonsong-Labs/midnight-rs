//! The coin, key and address values the SDK's API carries, one type for every
//! ledger generation.
//!
//! Each mirrors the coin-structure type of the same name, field for field.
//! The generation modules convert them (see [`crate::ledger_8::convert`] and
//! [`crate::ledger_9::convert`]).

use std::fmt;

use midnight_helpers::HashOutput;

/// The type of a shielded token, its "color".
///
/// Treat shielded token ids as opaque: the zero id is not NIGHT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShieldedTokenType(pub HashOutput);

/// The type of an unshielded token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnshieldedTokenType(pub HashOutput);

/// NIGHT, the native unshielded token.
pub const NIGHT: UnshieldedTokenType = UnshieldedTokenType(HashOutput([0; 32]));

// `1 DUST = 10^15 SPECK` and `1 NIGHT = 10^6 STAR`, the same in every
// generation.
pub use midnight_helpers::ledger_9::{SPECKS_PER_DUST, STARS_PER_NIGHT};

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
/// A curve point, held in the 32-byte encoding that every ledger generation
/// shares.
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

/// The nullifier of a Dust spend.
///
/// A field element, held in its 32-byte little-endian form, which every
/// ledger generation shares.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DustNullifier(pub(crate) [u8; 32]);

impl fmt::Debug for DustNullifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DustNullifier({})", hex::encode(self.0))
    }
}

/// The address of a deployed contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContractAddress(pub HashOutput);

impl ContractAddress {
    /// The shielded token this contract mints under `domain_sep`.
    pub fn custom_shielded_token_type(&self, domain_sep: HashOutput) -> ShieldedTokenType {
        let address = midnight_helpers::ledger_9::ContractAddress(self.0);
        ShieldedTokenType(address.custom_shielded_token_type(domain_sep).0)
    }

    /// The unshielded token this contract mints under `domain_sep`.
    pub fn custom_unshielded_token_type(&self, domain_sep: HashOutput) -> UnshieldedTokenType {
        let address = midnight_helpers::ledger_9::ContractAddress(self.0);
        UnshieldedTokenType(address.custom_unshielded_token_type(domain_sep).0)
    }
}

#[cfg(test)]
mod tests {
    use midnight_helpers::{ledger_8, ledger_9};

    use crate::ledger_8::convert::IntoSdk as _;
    use crate::ledger_9::convert::IntoLedger as _;

    /// A field element's own serialization is variable-length, so a nullifier
    /// that went through it would not fit the SDK's 32 bytes, or would come
    /// back as another element.
    #[test]
    fn a_dust_nullifier_crosses_generations_unchanged() {
        for value in [ledger_8::Fr::from(7u64), -ledger_8::Fr::from(1u64)] {
            let nullifier = ledger_8::DustNullifier(value).into_sdk();
            let crossed: ledger_9::DustNullifier = nullifier.into_ledger();
            assert_eq!(crossed.0.as_le_bytes(), value.as_le_bytes());
        }
    }

    /// The SDK derives a contract's token types through ledger 9. A ledger 8
    /// chain names the same tokens only if both derive them alike.
    #[test]
    fn both_generations_derive_the_same_contract_token_types() {
        let address = super::ContractAddress(super::HashOutput([5; 32]));
        let domain = super::HashOutput([9; 32]);
        let address8 = ledger_8::ContractAddress(address.0);
        assert_eq!(
            address.custom_shielded_token_type(domain).0,
            address8.custom_shielded_token_type(domain).0
        );
        assert_eq!(
            address.custom_unshielded_token_type(domain).0,
            address8.custom_unshielded_token_type(domain).0
        );
    }
}
