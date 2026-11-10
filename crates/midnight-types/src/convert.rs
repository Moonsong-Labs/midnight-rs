//! Conversions between the SDK's own values and the ledger's.

use midnight_helpers::{Deserializable, Serializable};

use crate::hd::{Seed, SeedBytes};
use crate::{
    CoinInfo, CoinPublicKey, ContractAddress, DustNullifier, DustPublicKey, EncryptionPublicKey,
    Nonce, Nullifier, ShieldedTokenType, UnshieldedRecipient, UnshieldedTokenType,
};

/// An SDK value that has a counterpart in the ledger.
pub trait IntoLedger {
    /// The ledger's counterpart.
    type Ledger;

    /// This value as the ledger's counterpart.
    fn into_ledger(self) -> Self::Ledger;
}

/// A ledger value that has a counterpart among the SDK's own.
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
            type Ledger = midnight_helpers::$ledger;
            fn into_ledger(self) -> Self::Ledger {
                midnight_helpers::$ledger(self.0)
            }
        }

        impl IntoSdk for midnight_helpers::$ledger {
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
    UnshieldedRecipient => UserAddress,
}

impl IntoLedger for CoinInfo {
    type Ledger = midnight_helpers::CoinInfo;
    fn into_ledger(self) -> Self::Ledger {
        midnight_helpers::CoinInfo {
            nonce: self.nonce.into_ledger(),
            type_: self.type_.into_ledger(),
            value: self.value,
        }
    }
}

impl IntoSdk for midnight_helpers::CoinInfo {
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
    type Ledger = midnight_helpers::EncryptionPublicKey;
    fn into_ledger(self) -> Self::Ledger {
        midnight_helpers::EncryptionPublicKey::deserialize(&mut self.0.as_slice(), 0)
            .expect("the SDK holds only encryption keys that the ledger encoded")
    }
}

impl IntoSdk for midnight_helpers::EncryptionPublicKey {
    type Sdk = EncryptionPublicKey;
    fn into_sdk(self) -> Self::Sdk {
        EncryptionPublicKey(encoding(&self))
    }
}

impl IntoLedger for DustNullifier {
    type Ledger = midnight_helpers::DustNullifier;
    fn into_ledger(self) -> Self::Ledger {
        midnight_helpers::DustNullifier(
            midnight_helpers::Fr::from_le_bytes(&self.0)
                .expect("the SDK holds only Dust nullifiers that the ledger encoded"),
        )
    }
}

impl IntoSdk for midnight_helpers::DustNullifier {
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

impl IntoLedger for DustPublicKey {
    type Ledger = midnight_helpers::DustPublicKey;
    fn into_ledger(self) -> Self::Ledger {
        midnight_helpers::DustPublicKey(
            midnight_helpers::Fr::from_le_bytes(&self.0)
                .expect("the SDK holds only Dust public keys that the ledger encoded"),
        )
    }
}

impl IntoSdk for midnight_helpers::DustPublicKey {
    type Sdk = DustPublicKey;
    fn into_sdk(self) -> Self::Sdk {
        DustPublicKey(
            self.0
                .as_le_bytes()
                .try_into()
                .expect("a field element is 32 bytes"),
        )
    }
}

impl IntoLedger for &Seed {
    type Ledger = midnight_helpers::WalletSeed;
    fn into_ledger(self) -> Self::Ledger {
        match &self.0 {
            SeedBytes::Short(bytes) => midnight_helpers::WalletSeed::Short(*bytes),
            SeedBytes::Medium(bytes) => midnight_helpers::WalletSeed::Medium(*bytes),
            SeedBytes::Long(bytes) => midnight_helpers::WalletSeed::Long(*bytes),
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
