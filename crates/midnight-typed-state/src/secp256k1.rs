//! The secp256k1 values of Compact: [`Secp256k1Base`], [`Secp256k1Scalar`] and
//! [`Secp256k1Point`].
//!
//! The compiler encodes a foreign field element `v` as two atoms. They hold the
//! little-endian bytes of `v - 1` modulo the field order. The first atom has 24
//! bytes and holds the low 192 bits, and the second has the high 64 bits in 8
//! bytes. This is the public-input encoding of ZKIR v3, so zero encodes as the
//! largest element.
//!
//! A point is its x and y coordinates in that encoding, then a field atom. The
//! field atom is 1 for the identity and 0 for any other point. The identity
//! has the coordinates (0, 0).

use midnight_base_crypto::fab::{
    Aligned, Alignment, AlignmentAtom, AlignmentSegment, InvalidBuiltinDecode, Value, ValueAtom,
    ValueSlice,
};
use midnight_curves::k256::{Fp, Fq, K256Affine};
use midnight_curves::pairing::group::ff::PrimeField;
use midnight_transient_crypto::curve::Fr;

const LOW_BYTES: usize = 24;
const HIGH_BYTES: usize = 8;

/// An element of the secp256k1 base field, the Rust image of Compact's
/// `Secp256k1Base`. The default is zero.
///
/// # Examples
///
/// ```
/// use midnight_typed_state::Secp256k1Base;
///
/// let mut bytes = [0u8; 32];
/// bytes[31] = 7;
/// let seven = Secp256k1Base::from_be_bytes(bytes).expect("7 is below the modulus");
/// assert_eq!(seven.to_be_bytes(), bytes);
/// assert!(Secp256k1Base::from_be_bytes([0xff; 32]).is_none());
/// ```
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Secp256k1Base(Fp);

/// An element of the secp256k1 scalar field, the Rust image of Compact's
/// `Secp256k1Scalar`. The default is zero.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Secp256k1Scalar(Fq);

/// A point of the secp256k1 curve, the Rust image of Compact's
/// `Secp256k1Point`. The default is the identity, as for Compact's
/// `default<Secp256k1Point>`.
///
/// # Examples
///
/// ```
/// use midnight_typed_state::{Secp256k1Base, Secp256k1Point};
///
/// assert_eq!(Secp256k1Point::default().coordinates(), None);
///
/// // (0, 0) is the encoding of the identity, not a point of the curve.
/// let zero = Secp256k1Base::default();
/// assert!(Secp256k1Point::from_coordinates(zero, zero).is_none());
/// ```
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Secp256k1Point(K256Affine);

impl Secp256k1Base {
    /// The element with the big-endian encoding `bytes`, if it is below the
    /// field order.
    pub fn from_be_bytes(bytes: [u8; 32]) -> Option<Self> {
        from_be_bytes(bytes).map(Self)
    }

    /// The big-endian encoding of this element.
    pub fn to_be_bytes(self) -> [u8; 32] {
        to_be_bytes(self.0)
    }
}

impl Secp256k1Scalar {
    /// The element with the big-endian encoding `bytes`, if it is below the
    /// group order.
    pub fn from_be_bytes(bytes: [u8; 32]) -> Option<Self> {
        from_be_bytes(bytes).map(Self)
    }

    /// The big-endian encoding of this element.
    pub fn to_be_bytes(self) -> [u8; 32] {
        to_be_bytes(self.0)
    }
}

impl Secp256k1Point {
    /// The point at the affine coordinates `(x, y)`, if it is on the curve.
    pub fn from_coordinates(x: Secp256k1Base, y: Secp256k1Base) -> Option<Self> {
        K256Affine::from_xy(x.0, y.0).map(Self)
    }

    /// The affine coordinates `(x, y)` of this point, or `None` for the
    /// identity.
    pub fn coordinates(&self) -> Option<(Secp256k1Base, Secp256k1Base)> {
        if self.is_identity() {
            None
        } else {
            Some((Secp256k1Base(self.0.x()), Secp256k1Base(self.0.y())))
        }
    }

    fn is_identity(&self) -> bool {
        self.0 == K256Affine::identity()
    }
}

// --- midnight-curves conversions ---

impl From<Fp> for Secp256k1Base {
    fn from(value: Fp) -> Self {
        Self(value)
    }
}

impl From<Secp256k1Base> for Fp {
    fn from(value: Secp256k1Base) -> Self {
        value.0
    }
}

impl From<Fq> for Secp256k1Scalar {
    fn from(value: Fq) -> Self {
        Self(value)
    }
}

impl From<Secp256k1Scalar> for Fq {
    fn from(value: Secp256k1Scalar) -> Self {
        value.0
    }
}

impl From<K256Affine> for Secp256k1Point {
    fn from(value: K256Affine) -> Self {
        Self(value)
    }
}

impl From<Secp256k1Point> for K256Affine {
    fn from(value: Secp256k1Point) -> Self {
        value.0
    }
}

// --- Formatting ---

impl std::fmt::Display for Secp256k1Base {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{}", hex::encode(self.to_be_bytes()))
    }
}

impl std::fmt::Debug for Secp256k1Base {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secp256k1Base({self})")
    }
}

impl std::fmt::Display for Secp256k1Scalar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{}", hex::encode(self.to_be_bytes()))
    }
}

impl std::fmt::Debug for Secp256k1Scalar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secp256k1Scalar({self})")
    }
}

impl std::fmt::Debug for Secp256k1Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.coordinates() {
            None => f.write_str("Secp256k1Point::identity"),
            Some((x, y)) => write!(f, "Secp256k1Point({x}, {y})"),
        }
    }
}

// --- Aligned ---

fn field_alignment() -> Alignment {
    Alignment(vec![
        AlignmentSegment::Atom(AlignmentAtom::Bytes {
            length: LOW_BYTES as u32,
        }),
        AlignmentSegment::Atom(AlignmentAtom::Bytes {
            length: HIGH_BYTES as u32,
        }),
    ])
}

impl Aligned for Secp256k1Base {
    fn alignment() -> Alignment {
        field_alignment()
    }
}

impl Aligned for Secp256k1Scalar {
    fn alignment() -> Alignment {
        field_alignment()
    }
}

impl Aligned for Secp256k1Point {
    fn alignment() -> Alignment {
        Alignment::concat([
            &field_alignment(),
            &field_alignment(),
            &Alignment::singleton(AlignmentAtom::Field),
        ])
    }
}

// --- Value conversions ---

impl From<Secp256k1Base> for Value {
    fn from(value: Secp256k1Base) -> Value {
        Value(encode_field(value.0).to_vec())
    }
}

impl From<Secp256k1Scalar> for Value {
    fn from(value: Secp256k1Scalar) -> Value {
        Value(encode_field(value.0).to_vec())
    }
}

impl From<Secp256k1Point> for Value {
    fn from(value: Secp256k1Point) -> Value {
        let (x, y) = value.coordinates().unwrap_or_default();
        let identity = Fr::from(u64::from(value.is_identity()));
        let mut atoms = Vec::with_capacity(5);
        atoms.extend(encode_field(x.0));
        atoms.extend(encode_field(y.0));
        atoms.push(ValueAtom::from(identity));
        Value(atoms)
    }
}

impl TryFrom<&ValueSlice> for Secp256k1Base {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        decode_field(&value.0)
            .map(Self)
            .ok_or(InvalidBuiltinDecode("Secp256k1Base"))
    }
}

impl TryFrom<&ValueSlice> for Secp256k1Scalar {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        decode_field(&value.0)
            .map(Self)
            .ok_or(InvalidBuiltinDecode("Secp256k1Scalar"))
    }
}

impl TryFrom<&ValueSlice> for Secp256k1Point {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        let err = || InvalidBuiltinDecode("Secp256k1Point");
        let atoms = &value.0;
        if atoms.len() != 5 {
            return Err(err());
        }
        let x = decode_field::<Fp>(&atoms[0..2]).ok_or_else(err)?;
        let y = decode_field::<Fp>(&atoms[2..4]).ok_or_else(err)?;
        let flag = Fr::try_from(&atoms[4])?;
        // The prover reads a flag of 1 as the identity, whatever the
        // coordinates hold.
        if flag == Fr::from(1u64) {
            Ok(Self::default())
        } else if flag == Fr::from(0u64) {
            K256Affine::from_xy(x, y).map(Self).ok_or_else(err)
        } else {
            Err(err())
        }
    }
}

// --- Encoding ---

// The `Repr` of a k256 field element holds its big-endian bytes.

fn from_be_bytes<F: PrimeField>(bytes: [u8; 32]) -> Option<F> {
    let mut repr = F::Repr::default();
    repr.as_mut().copy_from_slice(&bytes);
    F::from_repr(repr).into()
}

fn to_be_bytes<F: PrimeField>(value: F) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(value.to_repr().as_ref());
    bytes
}

fn encode_field<F: PrimeField>(value: F) -> [ValueAtom; 2] {
    let mut le = to_be_bytes(value - F::ONE);
    le.reverse();
    let (low, high) = le.split_at(LOW_BYTES);
    [
        ValueAtom(low.to_vec()).normalize(),
        ValueAtom(high.to_vec()).normalize(),
    ]
}

fn decode_field<F: PrimeField>(atoms: &[ValueAtom]) -> Option<F> {
    let [low, high] = atoms else {
        return None;
    };
    let low: [u8; LOW_BYTES] = low.clone().try_into().ok()?;
    let high: [u8; HIGH_BYTES] = high.clone().try_into().ok()?;
    let mut bytes = [0u8; 32];
    bytes[..LOW_BYTES].copy_from_slice(&low);
    bytes[LOW_BYTES..].copy_from_slice(&high);
    bytes.reverse();
    Some(from_be_bytes::<F>(bytes)? + F::ONE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_base_crypto::fab::AlignedValue;
    use midnight_curves::k256::K256;
    use midnight_zkir_v3::ir_instructions::encode::encode_offcircuit;
    use midnight_zkir_v3::ir_types::IrValue;

    /// The field elements the prover reads for `value`, one per atom.
    fn atoms_as_fields(value: impl Into<AlignedValue>) -> Vec<Fr> {
        let av: AlignedValue = value.into();
        av.value
            .0
            .iter()
            .map(|atom| Fr::try_from(atom).expect("each atom is a field element"))
            .collect()
    }

    fn ir_fields(value: &IrValue) -> Vec<Fr> {
        encode_offcircuit(value)
            .into_iter()
            .map(|v| match v {
                IrValue::Native(fr) => fr,
                other => panic!("encode gives native values, got {other:?}"),
            })
            .collect()
    }

    fn decode<T: for<'a> TryFrom<&'a ValueSlice, Error = InvalidBuiltinDecode>>(
        fields: &[Fr],
    ) -> T {
        let value = Value(fields.iter().map(|f| ValueAtom::from(*f)).collect());
        T::try_from(&value).expect("the prover's encoding decodes")
    }

    #[test]
    fn field_elements_take_the_provers_encoding() {
        let minus_one = -Fp::ONE;
        for v in [
            Fp::ZERO,
            Fp::ONE,
            minus_one,
            Fp::from(0x1234_5678_9abc_def0),
        ] {
            let expected = ir_fields(&IrValue::Secp256k1Base(v));
            assert_eq!(atoms_as_fields(Secp256k1Base(v)), expected);
            assert_eq!(decode::<Secp256k1Base>(&expected), Secp256k1Base(v));
        }
        for v in [Fq::ZERO, Fq::ONE, -Fq::ONE] {
            let expected = ir_fields(&IrValue::Secp256k1Scalar(v));
            assert_eq!(atoms_as_fields(Secp256k1Scalar(v)), expected);
            assert_eq!(decode::<Secp256k1Scalar>(&expected), Secp256k1Scalar(v));
        }
    }

    #[test]
    fn points_take_the_provers_encoding() {
        let generator = K256::generator();
        for p in [K256::identity(), generator, generator * Fq::from(7u64)] {
            let point = Secp256k1Point(K256Affine::from(p));
            let expected = ir_fields(&IrValue::Secp256k1Point(p));
            assert_eq!(atoms_as_fields(point), expected);
            assert_eq!(decode::<Secp256k1Point>(&expected), point);
        }
    }

    #[test]
    fn a_point_off_the_curve_does_not_decode() {
        let one = Secp256k1Base(Fp::ONE);
        let off_curve = Value(
            [encode_field(one.0), encode_field(one.0)]
                .concat()
                .into_iter()
                .chain([ValueAtom::from(Fr::from(0u64))])
                .collect(),
        );
        assert!(Secp256k1Point::try_from(&*off_curve).is_err());
    }
}
