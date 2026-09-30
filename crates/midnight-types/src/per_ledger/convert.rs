//! Conversions between the SDK's own values and this generation's.
//!
//! Two traits, not `From` impls, let each generation module own its
//! conversions. A later generation that shares this one's companion crates
//! then gets its own traits, not a second impl of the same `From`.

use super::helpers;
use helpers::{DefaultDB, Deserializable, Serializable};

use crate::{
    ChainParameters, CoinInfo, CoinPublicKey, ContractAddress, DustNullifier, DustParameters,
    EncryptionPublicKey, Nonce, Nullifier, ShieldedRecipient, ShieldedTokenType,
    UnshieldedTokenType, WalletSeed,
};

/// An SDK value that has a counterpart in this generation.
pub trait IntoLedger {
    /// This generation's counterpart.
    type Ledger;

    /// This value as this generation's counterpart.
    fn into_ledger(self) -> Self::Ledger;
}

/// A value of this generation that has a counterpart among the SDK's own.
pub trait IntoSdk {
    /// The SDK's counterpart.
    type Sdk;

    /// This value as the SDK's counterpart.
    fn into_sdk(self) -> Self::Sdk;
}

/// Newtypes over a shared `HashOutput`, which convert field for field.
macro_rules! hash_newtype {
    ($($sdk:ident => $ledger:ident),* $(,)?) => {$(
        impl IntoLedger for $sdk {
            type Ledger = helpers::$ledger;
            fn into_ledger(self) -> Self::Ledger {
                helpers::$ledger(self.0)
            }
        }

        impl IntoSdk for helpers::$ledger {
            type Sdk = $sdk;
            fn into_sdk(self) -> Self::Sdk {
                $sdk(self.0)
            }
        }
    )*};
}

hash_newtype! {
    ShieldedTokenType => ShieldedTokenType,
    UnshieldedTokenType => UnshieldedTokenType,
    Nullifier => Nullifier,
    Nonce => Nonce,
    CoinPublicKey => CoinPublicKey,
    ContractAddress => ContractAddress,
}

impl IntoLedger for CoinInfo {
    type Ledger = helpers::CoinInfo;
    fn into_ledger(self) -> Self::Ledger {
        helpers::CoinInfo {
            nonce: self.nonce.into_ledger(),
            type_: self.type_.into_ledger(),
            value: self.value,
        }
    }
}

impl IntoSdk for helpers::CoinInfo {
    type Sdk = CoinInfo;
    fn into_sdk(self) -> Self::Sdk {
        CoinInfo {
            nonce: self.nonce.into_sdk(),
            type_: self.type_.into_sdk(),
            value: self.value,
        }
    }
}

impl IntoLedger for EncryptionPublicKey {
    type Ledger = helpers::EncryptionPublicKey;
    fn into_ledger(self) -> Self::Ledger {
        helpers::EncryptionPublicKey::deserialize(&mut self.0.as_slice(), 0)
            .expect("every generation decodes the encryption key encoding it shares")
    }
}

impl IntoSdk for helpers::EncryptionPublicKey {
    type Sdk = EncryptionPublicKey;
    fn into_sdk(self) -> Self::Sdk {
        EncryptionPublicKey(encoding(&self))
    }
}

impl IntoLedger for DustNullifier {
    type Ledger = helpers::DustNullifier;
    fn into_ledger(self) -> Self::Ledger {
        helpers::DustNullifier(
            helpers::Fr::from_le_bytes(&self.0)
                .expect("every generation takes the field elements the SDK's nullifier holds"),
        )
    }
}

impl IntoSdk for helpers::DustNullifier {
    type Sdk = DustNullifier;
    fn into_sdk(self) -> Self::Sdk {
        DustNullifier(
            self.0
                .as_le_bytes()
                .try_into()
                .expect("a field element is 32 bytes"),
        )
    }
}

impl IntoLedger for &WalletSeed {
    type Ledger = helpers::WalletSeed;
    fn into_ledger(self) -> Self::Ledger {
        helpers::WalletSeed::try_from(self.as_bytes())
            .expect("every generation takes the seed lengths the SDK's seed holds")
    }
}

impl IntoSdk for &helpers::LedgerParameters {
    type Sdk = ChainParameters;
    fn into_sdk(self) -> Self::Sdk {
        ChainParameters {
            dust: DustParameters {
                night_dust_ratio: self.dust.night_dust_ratio,
                generation_decay_rate: self.dust.generation_decay_rate,
                dust_grace_period: self.dust.dust_grace_period,
            },
            global_ttl: self.global_ttl,
        }
    }
}

impl IntoSdk for &helpers::ShieldedWallet<DefaultDB> {
    type Sdk = ShieldedRecipient;
    fn into_sdk(self) -> Self::Sdk {
        ShieldedRecipient {
            coin_public_key: self.coin_public_key.into_sdk(),
            enc_public_key: self.enc_public_key.into_sdk(),
        }
    }
}

/// The untagged encoding of a curve point: its 32-byte compressed form.
fn encoding<T: Serializable>(value: &T) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(32);
    value
        .serialize(&mut bytes)
        .expect("serializing into a Vec cannot fail");
    bytes.try_into().expect("a curve point encodes to 32 bytes")
}
