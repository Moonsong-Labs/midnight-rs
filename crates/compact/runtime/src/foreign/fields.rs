//! The foreign field values of Compact: [`Secp256k1Base`], [`Secp256k1Scalar`],
//! [`Secp256r1Base`], [`Secp256r1Scalar`], [`Curve25519Base`] and
//! [`Curve25519Scalar`].
//!
//! The compiler encodes a foreign field element `v` as two atoms. They hold the
//! little-endian bytes of `v - 1` modulo the field order, split at a bit. The
//! low atom holds the bits below the split, and the high atom holds the rest.
//! Each field splits at bit 192, into 24 and 8 bytes, except
//! `Curve25519Scalar`, which splits at bit 204, into 26 and 7 bytes. This is
//! the public-input encoding of ZKIR v3, so zero encodes as the largest
//! element.

use std::fmt;

use midnight_base_crypto::fab::{
    Aligned, Alignment, AlignmentAtom, AlignmentSegment, InvalidBuiltinDecode, Value, ValueAtom,
    ValueSlice,
};
use midnight_circuits::CircuitField;
use midnight_curves::{curve25519, k256, p256};
use midnight_transient_crypto::curve::Fr;
use midnight_zkir_v3::ir_instructions::encode::{decode_offcircuit, encode_offcircuit};
use midnight_zkir_v3::ir_types::{IrType, IrValue};

use super::CompactValue;

/// The width in bits of the integer that the two atoms of a field element
/// share.
const ENCODED_BITS: usize = 256;

/// A prime field that Compact emulates, and where its encoding splits.
pub(crate) trait Emulated:
    CircuitField<Bytes = [u8; 32]> + Into<IrValue> + TryFrom<IrValue>
{
    /// The number of low bits that the first atom holds.
    const LOW_BITS: usize;
}

impl Emulated for k256::Fp {
    const LOW_BITS: usize = 192;
}

impl Emulated for k256::Fq {
    const LOW_BITS: usize = 192;
}

impl Emulated for p256::Fp {
    const LOW_BITS: usize = 192;
}

impl Emulated for p256::Fq {
    const LOW_BITS: usize = 192;
}

impl Emulated for curve25519::Fp {
    const LOW_BITS: usize = 192;
}

// The compiler aligns this field as 26 and 7 bytes, because ZKIR v3 packs four
// of its 51-bit limbs in the first field element.
impl Emulated for curve25519::Scalar {
    const LOW_BITS: usize = 204;
}

/// A Compact foreign field type and the field that it computes in.
pub(crate) trait FieldValue:
    CompactValue + Default + From<Self::Field> + Into<Self::Field>
{
    /// The field that the type computes in.
    type Field: Emulated;

    /// The ZKIR v3 type of the field.
    const IR_TYPE: IrType;
}

/// Declares the Rust image of a Compact foreign field type: a newtype over the
/// field of `midnight-curves` that computes it, with the Compact encoding.
macro_rules! foreign_field {
    ($(#[$doc:meta])* $name:ident($field:ty)) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Default, PartialEq, Eq)]
        pub struct $name($field);

        impl $name {
            /// The element with the big-endian encoding `bytes`, if it is below
            /// the field order.
            pub fn from_be_bytes(bytes: [u8; 32]) -> Option<Self> {
                <$field as CircuitField>::from_bytes_be(&bytes).map(Self)
            }

            /// The big-endian encoding of this element.
            pub fn to_be_bytes(self) -> [u8; 32] {
                self.0.to_bytes_be()
            }
        }

        impl CompactValue for $name {
            const NAME: &'static str = stringify!($name);
        }

        impl FieldValue for $name {
            type Field = $field;
            const IR_TYPE: IrType = IrType::$name;
        }

        impl From<$field> for $name {
            fn from(value: $field) -> Self {
                Self(value)
            }
        }

        impl From<$name> for $field {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "0x{}", hex::encode(self.to_be_bytes()))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self)
            }
        }

        impl Aligned for $name {
            fn alignment() -> Alignment {
                field_alignment::<$field>()
            }
        }

        impl From<$name> for Value {
            fn from(value: $name) -> Value {
                Value(to_atoms(value.0))
            }
        }

        impl TryFrom<&ValueSlice> for $name {
            type Error = InvalidBuiltinDecode;

            fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
                from_atoms(&value.0, &Self::IR_TYPE)
                    .map(Self)
                    .ok_or(InvalidBuiltinDecode(stringify!($name)))
            }
        }
    };
}

foreign_field! {
    /// An element of the secp256k1 base field, the Rust image of Compact's
    /// `Secp256k1Base`. The default is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use compact_runtime::Secp256k1Base;
    ///
    /// let mut bytes = [0u8; 32];
    /// bytes[31] = 7;
    /// let seven = Secp256k1Base::from_be_bytes(bytes).expect("7 is below the modulus");
    /// assert_eq!(seven.to_be_bytes(), bytes);
    /// assert!(Secp256k1Base::from_be_bytes([0xff; 32]).is_none());
    /// ```
    Secp256k1Base(k256::Fp)
}

foreign_field! {
    /// An element of the secp256k1 scalar field, the Rust image of Compact's
    /// `Secp256k1Scalar`. The default is zero.
    Secp256k1Scalar(k256::Fq)
}

foreign_field! {
    /// An element of the secp256r1 (NIST P-256) base field, the Rust image of
    /// Compact's `Secp256r1Base`. The default is zero.
    Secp256r1Base(p256::Fp)
}

foreign_field! {
    /// An element of the secp256r1 (NIST P-256) scalar field, the Rust image
    /// of Compact's `Secp256r1Scalar`. The default is zero.
    Secp256r1Scalar(p256::Fq)
}

foreign_field! {
    /// An element of the Curve25519 base field, the Rust image of Compact's
    /// `Curve25519Base`.
    ///
    /// The field is the integers modulo 2^255 - 19. The default is zero.
    Curve25519Base(curve25519::Fp)
}

foreign_field! {
    /// An element of the Curve25519 scalar field, the Rust image of Compact's
    /// `Curve25519Scalar`.
    ///
    /// The order of the field is the order of the prime-order subgroup of the
    /// curve. The default is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use compact_runtime::Curve25519Scalar;
    ///
    /// let mut bytes = [0u8; 32];
    /// bytes[31] = 7;
    /// let seven = Curve25519Scalar::from_be_bytes(bytes).expect("7 is below the order");
    /// assert_eq!(seven.to_be_bytes(), bytes);
    /// // The order is below 2^253.
    /// let mut too_wide = [0u8; 32];
    /// too_wide[0] = 0x20;
    /// assert!(Curve25519Scalar::from_be_bytes(too_wide).is_none());
    /// ```
    Curve25519Scalar(curve25519::Scalar)
}

fn low_bytes<F: Emulated>() -> usize {
    F::LOW_BITS.div_ceil(8)
}

fn high_bytes<F: Emulated>() -> usize {
    (ENCODED_BITS - F::LOW_BITS).div_ceil(8)
}

pub(super) fn field_alignment<F: Emulated>() -> Alignment {
    let atom = |length: usize| {
        AlignmentSegment::Atom(AlignmentAtom::Bytes {
            length: u32::try_from(length).expect("an atom of a field element is at most 32 bytes"),
        })
    };
    Alignment(vec![atom(low_bytes::<F>()), atom(high_bytes::<F>())])
}

/// The atoms of `value` in the public-input encoding of ZKIR v3, one atom per
/// native field element.
pub(super) fn to_atoms(value: impl Into<IrValue>) -> Vec<ValueAtom> {
    encode_offcircuit(&value.into())
        .into_iter()
        .map(|field| {
            ValueAtom::from(Fr::try_from(field).expect("ZKIR v3 encodes as native field elements"))
        })
        .collect()
}

/// The value of the ZKIR v3 type `ty` that `atoms` hold in its public-input
/// encoding, or `None` if they are not that encoding of a value.
pub(super) fn from_atoms<V: TryFrom<IrValue>>(atoms: &[ValueAtom], ty: &IrType) -> Option<V> {
    let fields = atoms
        .iter()
        .map(Fr::try_from)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    V::try_from(decode_offcircuit(&fields, ty).ok()?).ok()
}
