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
use num_bigint::BigUint;

use super::CompactValue;

/// The width in bits of the integer that the two atoms of a field element
/// share.
const ENCODED_BITS: usize = 256;

/// A prime field that Compact emulates, and where its encoding splits.
pub(crate) trait Emulated: CircuitField<Bytes = [u8; 32]> {
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
                Value(encode_field(value.0).to_vec())
            }
        }

        impl TryFrom<&ValueSlice> for $name {
            type Error = InvalidBuiltinDecode;

            fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
                decode_field(&value.0)
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

/// The two atoms of `value`.
pub(super) fn encode_field<F: Emulated>(value: F) -> [ValueAtom; 2] {
    split::<F>((value - F::ONE).to_biguint())
}

/// The low and the high atom of `shifted`, the integer `v - 1`.
fn split<F: Emulated>(shifted: BigUint) -> [ValueAtom; 2] {
    let low_mask = (BigUint::from(1u8) << F::LOW_BITS) - 1u8;
    let atom = |n: BigUint| ValueAtom(n.to_bytes_le()).normalize();
    [atom(&shifted & low_mask), atom(shifted >> F::LOW_BITS)]
}

/// The field element of `atoms`, or `None` if they are not two atoms in the
/// encoding of an element.
pub(super) fn decode_field<F: Emulated>(atoms: &[ValueAtom]) -> Option<F> {
    let [low, high] = atoms else {
        return None;
    };
    if low.0.len() > low_bytes::<F>() || high.0.len() > high_bytes::<F>() {
        return None;
    }
    let low = BigUint::from_bytes_le(&low.0);
    if low.bits() > F::LOW_BITS as u64 {
        return None;
    }
    let shifted = (BigUint::from_bytes_le(&high.0) << F::LOW_BITS) | low;
    Some(F::from_biguint(&shifted)? + F::ONE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_curves::pairing::group::ff::{Field, PrimeField};
    use midnight_zkir_v3::ir_types::IrValue;

    use crate::foreign::test_support::{atoms_as_fields, decode_fields, ir_fields};

    /// `T` encodes a fixed set of elements as ZKIR v3 encodes `ir(value)`,
    /// and decodes that encoding back to the element.
    fn takes_the_provers_encoding<T: FieldValue + PartialEq + fmt::Debug>(
        ir: impl Fn(T::Field) -> IrValue,
    ) {
        let one = T::Field::ONE;
        let wide = T::Field::from_u128(u128::MAX).square();
        for v in [T::Field::ZERO, one, -one, wide, -wide] {
            let expected = ir_fields(&ir(v));
            assert_eq!(atoms_as_fields(T::from(v)), expected, "{}", T::NAME);
            assert_eq!(decode_fields::<T>(&expected), T::from(v), "{}", T::NAME);
        }
    }

    #[test]
    fn field_elements_take_the_provers_encoding() {
        takes_the_provers_encoding::<Secp256k1Base>(IrValue::Secp256k1Base);
        takes_the_provers_encoding::<Secp256k1Scalar>(IrValue::Secp256k1Scalar);
        takes_the_provers_encoding::<Secp256r1Base>(IrValue::Secp256r1Base);
        takes_the_provers_encoding::<Secp256r1Scalar>(IrValue::Secp256r1Scalar);
        takes_the_provers_encoding::<Curve25519Base>(IrValue::Curve25519Base);
        takes_the_provers_encoding::<Curve25519Scalar>(IrValue::Curve25519Scalar);
    }

    /// The low atom of a `Curve25519Scalar` has room for 208 bits, but holds
    /// 204. A set bit above them is an encoding that the prover and the TS
    /// runtime refuse. Without a check, it decodes as 2^204 + 1.
    #[test]
    fn a_low_atom_wider_than_its_bits_does_not_decode() {
        let mut low = vec![0u8; 26];
        low[25] = 0x10;
        let value = Value(vec![ValueAtom(low), ValueAtom(Vec::new())]);
        assert!(Curve25519Scalar::try_from(&*value).is_err());
    }

    fn the_order_does_not_decode<T: FieldValue>() {
        let value = Value(split::<T::Field>(T::Field::modulus()).to_vec());
        assert!(T::try_from(&*value).is_err(), "{}", T::NAME);
    }

    /// The atoms of the field order are no encoding: the prover and the TS
    /// runtime refuse them. A decode that reduces modulo the order, as a cast
    /// from bytes does, reads them as one.
    #[test]
    fn an_encoding_at_the_order_does_not_decode() {
        the_order_does_not_decode::<Secp256k1Base>();
        the_order_does_not_decode::<Secp256k1Scalar>();
        the_order_does_not_decode::<Secp256r1Base>();
        the_order_does_not_decode::<Secp256r1Scalar>();
        the_order_does_not_decode::<Curve25519Base>();
        the_order_does_not_decode::<Curve25519Scalar>();
    }
}
