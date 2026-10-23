//! The points of Compact's foreign curves: [`Secp256k1Point`],
//! [`Secp256r1Point`] and [`Curve25519Point`].
//!
//! A point of secp256k1 or secp256r1 is its affine x and y coordinates, each
//! in the encoding of its base field, then a field atom. The field atom is 1
//! for the identity and 0 for any other point. The identity has the
//! coordinates (0, 0).
//!
//! A point of Curve25519 is its affine x and y coordinates on the twisted
//! Edwards curve of Ed25519, with no flag. Its identity is the affine point
//! (0, 1). Compact holds only the points of the prime-order subgroup.

use std::fmt;

use midnight_base_crypto::fab::{
    Aligned, Alignment, AlignmentAtom, InvalidBuiltinDecode, Value, ValueAtom, ValueSlice,
};
use midnight_curves::curve25519::{self, Curve25519, Curve25519Affine, Curve25519Subgroup};
use midnight_curves::k256::{self, K256Affine};
use midnight_curves::p256::{self, P256Affine};
use midnight_transient_crypto::curve::Fr;

use super::CompactValue;
use super::fields::{
    Curve25519Base, Emulated, Secp256k1Base, Secp256r1Base, decode_field, encode_field,
    field_alignment,
};

/// A point of the secp256k1 curve, the Rust image of Compact's
/// `Secp256k1Point`. The default is the identity, as for Compact's
/// `default<Secp256k1Point>`.
///
/// # Examples
///
/// ```
/// use compact_runtime::{Secp256k1Base, Secp256k1Point};
///
/// assert_eq!(Secp256k1Point::default().coordinates(), None);
///
/// // (0, 0) is the encoding of the identity, not a point of the curve.
/// let zero = Secp256k1Base::default();
/// assert!(Secp256k1Point::from_coordinates(zero, zero).is_none());
/// ```
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Secp256k1Point(K256Affine);

/// A point of the secp256r1 (NIST P-256) curve, the Rust image of Compact's
/// `Secp256r1Point`. The default is the identity, as for Compact's
/// `default<Secp256r1Point>`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Secp256r1Point(P256Affine);

/// A point of the prime-order subgroup of Curve25519, in the twisted Edwards
/// form of Ed25519. It is the Rust image of Compact's `Curve25519Point`. The
/// default is the identity, as for Compact's `default<Curve25519Point>`.
///
/// # Examples
///
/// ```
/// use compact_runtime::{Curve25519Base, Curve25519Point};
///
/// let (x, y) = Curve25519Point::default().coordinates();
/// assert_eq!(x, Curve25519Base::default());
///
/// // The identity is the affine point (0, 1).
/// let mut one = [0u8; 32];
/// one[31] = 1;
/// assert_eq!(y, Curve25519Base::from_be_bytes(one).unwrap());
/// assert_eq!(Curve25519Point::from_coordinates(x, y), Some(Curve25519Point::default()));
/// ```
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Curve25519Point(Curve25519Subgroup);

impl Secp256k1Point {
    /// The point at the affine coordinates `(x, y)`, if it is on the curve.
    pub fn from_coordinates(x: Secp256k1Base, y: Secp256k1Base) -> Option<Self> {
        K256Affine::from_xy(x.into(), y.into()).map(Self)
    }

    /// The affine coordinates `(x, y)` of this point, or `None` for the
    /// identity.
    pub fn coordinates(&self) -> Option<(Secp256k1Base, Secp256k1Base)> {
        if self.0 == K256Affine::identity() {
            None
        } else {
            Some((self.0.x().into(), self.0.y().into()))
        }
    }
}

impl Secp256r1Point {
    /// The point at the affine coordinates `(x, y)`, if it is on the curve.
    pub fn from_coordinates(x: Secp256r1Base, y: Secp256r1Base) -> Option<Self> {
        p256::affine_from_xy(x.into(), y.into()).map(Self)
    }

    /// The affine coordinates `(x, y)` of this point, or `None` for the
    /// identity.
    pub fn coordinates(&self) -> Option<(Secp256r1Base, Secp256r1Base)> {
        if bool::from(self.0.is_identity()) {
            None
        } else {
            Some((
                p256::affine_x(&self.0).into(),
                p256::affine_y(&self.0).into(),
            ))
        }
    }
}

impl Curve25519Point {
    /// The point at the affine coordinates `(x, y)`, if it is on the curve
    /// and in its prime-order subgroup.
    pub fn from_coordinates(x: Curve25519Base, y: Curve25519Base) -> Option<Self> {
        let affine = Curve25519Affine::from_xy(x.into(), y.into())?;
        Curve25519Subgroup::from_edwards(affine.to_edwards()).map(Self)
    }

    /// The affine coordinates `(x, y)` of this point. The identity has the
    /// coordinates (0, 1).
    pub fn coordinates(&self) -> (Curve25519Base, Curve25519Base) {
        let affine = Curve25519Affine::from(Curve25519::from(self.0));
        ((*affine.x()).into(), (*affine.y()).into())
    }
}

// --- midnight-curves conversions ---

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

impl From<P256Affine> for Secp256r1Point {
    fn from(value: P256Affine) -> Self {
        Self(value)
    }
}

impl From<Secp256r1Point> for P256Affine {
    fn from(value: Secp256r1Point) -> Self {
        value.0
    }
}

impl From<Curve25519Subgroup> for Curve25519Point {
    fn from(value: Curve25519Subgroup) -> Self {
        Self(value)
    }
}

impl From<Curve25519Point> for Curve25519Subgroup {
    fn from(value: Curve25519Point) -> Self {
        value.0
    }
}

// --- Formatting ---

impl fmt::Debug for Secp256k1Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.coordinates() {
            None => f.write_str("Secp256k1Point::identity"),
            Some((x, y)) => write!(f, "Secp256k1Point({x}, {y})"),
        }
    }
}

impl fmt::Debug for Secp256r1Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.coordinates() {
            None => f.write_str("Secp256r1Point::identity"),
            Some((x, y)) => write!(f, "Secp256r1Point({x}, {y})"),
        }
    }
}

impl fmt::Debug for Curve25519Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (x, y) = self.coordinates();
        write!(f, "Curve25519Point({x}, {y})")
    }
}

// --- Compact values ---

impl CompactValue for Secp256k1Point {
    const NAME: &'static str = "Secp256k1Point";
}

impl CompactValue for Secp256r1Point {
    const NAME: &'static str = "Secp256r1Point";
}

impl CompactValue for Curve25519Point {
    const NAME: &'static str = "Curve25519Point";
}

impl Aligned for Secp256k1Point {
    fn alignment() -> Alignment {
        flagged_alignment::<k256::Fp>()
    }
}

impl Aligned for Secp256r1Point {
    fn alignment() -> Alignment {
        flagged_alignment::<p256::Fp>()
    }
}

impl Aligned for Curve25519Point {
    fn alignment() -> Alignment {
        let base = field_alignment::<curve25519::Fp>();
        Alignment::concat([&base, &base])
    }
}

impl From<Secp256k1Point> for Value {
    fn from(value: Secp256k1Point) -> Value {
        encode_flagged::<k256::Fp>(value.coordinates().map(|(x, y)| (x.into(), y.into())))
    }
}

impl From<Secp256r1Point> for Value {
    fn from(value: Secp256r1Point) -> Value {
        encode_flagged::<p256::Fp>(value.coordinates().map(|(x, y)| (x.into(), y.into())))
    }
}

impl From<Curve25519Point> for Value {
    fn from(value: Curve25519Point) -> Value {
        let (x, y) = value.coordinates();
        let mut atoms = Vec::with_capacity(4);
        atoms.extend(encode_field::<curve25519::Fp>(x.into()));
        atoms.extend(encode_field::<curve25519::Fp>(y.into()));
        Value(atoms)
    }
}

impl TryFrom<&ValueSlice> for Secp256k1Point {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        match decode_flagged(&value.0, Self::NAME)? {
            None => Ok(Self::default()),
            Some((x, y)) => K256Affine::from_xy(x, y)
                .map(Self)
                .ok_or(InvalidBuiltinDecode(Self::NAME)),
        }
    }
}

impl TryFrom<&ValueSlice> for Secp256r1Point {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        match decode_flagged(&value.0, Self::NAME)? {
            None => Ok(Self::default()),
            Some((x, y)) => p256::affine_from_xy(x, y)
                .map(Self)
                .ok_or(InvalidBuiltinDecode(Self::NAME)),
        }
    }
}

impl TryFrom<&ValueSlice> for Curve25519Point {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        let err = || InvalidBuiltinDecode(Self::NAME);
        let atoms = &value.0;
        if atoms.len() != 4 {
            return Err(err());
        }
        let x = decode_field::<curve25519::Fp>(&atoms[0..2]).ok_or_else(err)?;
        let y = decode_field::<curve25519::Fp>(&atoms[2..4]).ok_or_else(err)?;
        Self::from_coordinates(x.into(), y.into()).ok_or_else(err)
    }
}

// --- The flagged encoding of secp256k1 and secp256r1 ---

fn flagged_alignment<F: Emulated>() -> Alignment {
    let base = field_alignment::<F>();
    Alignment::concat([&base, &base, &Alignment::singleton(AlignmentAtom::Field)])
}

/// The atoms of a point with the affine `coordinates`, or of the identity for
/// `None`.
fn encode_flagged<F: Emulated>(coordinates: Option<(F, F)>) -> Value {
    let (x, y) = coordinates.unwrap_or((F::ZERO, F::ZERO));
    let identity = Fr::from(u64::from(coordinates.is_none()));
    let mut atoms = Vec::with_capacity(5);
    atoms.extend(encode_field(x));
    atoms.extend(encode_field(y));
    atoms.push(ValueAtom::from(identity));
    Value(atoms)
}

/// The affine coordinates that `atoms` encode, or `None` for the identity.
///
/// # Errors
///
/// [`InvalidBuiltinDecode`] for the Compact type `name` if `atoms` are not a
/// flagged point.
fn decode_flagged<F: Emulated>(
    atoms: &[ValueAtom],
    name: &'static str,
) -> Result<Option<(F, F)>, InvalidBuiltinDecode> {
    let err = || InvalidBuiltinDecode(name);
    if atoms.len() != 5 {
        return Err(err());
    }
    let x = decode_field(&atoms[0..2]).ok_or_else(err)?;
    let y = decode_field(&atoms[2..4]).ok_or_else(err)?;
    let flag = Fr::try_from(&atoms[4])?;
    // The prover reads a flag of 1 as the identity, whatever the coordinates
    // hold.
    if flag == Fr::from(1u64) {
        Ok(None)
    } else if flag == Fr::from(0u64) {
        Ok(Some((x, y)))
    } else {
        Err(err())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_curves::k256::K256;
    use midnight_curves::p256::P256;
    use midnight_curves::pairing::group::Group;
    use midnight_curves::pairing::group::ff::Field;
    use midnight_zkir_v3::ir_types::IrValue;

    use crate::foreign::test_support::{atoms_as_fields, decode_fields, ir_fields};

    #[test]
    fn points_take_the_provers_encoding() {
        let k1 = K256::generator();
        for p in [K256::identity(), k1, k1 * k256::Fq::from(7u64)] {
            let point = Secp256k1Point(K256Affine::from(p));
            let expected = ir_fields(&IrValue::Secp256k1Point(p));
            assert_eq!(atoms_as_fields(point), expected);
            assert_eq!(decode_fields::<Secp256k1Point>(&expected), point);
        }
        let r1 = P256::generator();
        for p in [P256::identity(), r1, r1 * p256::Fq::from(7u64)] {
            let point = Secp256r1Point(P256Affine::from(p));
            let expected = ir_fields(&IrValue::Secp256r1Point(p));
            assert_eq!(atoms_as_fields(point), expected);
            assert_eq!(decode_fields::<Secp256r1Point>(&expected), point);
        }
        let ed = Curve25519Subgroup::generator();
        let seven = curve25519::Scalar::from(7u64);
        for p in [Curve25519Subgroup::identity(), ed, ed * seven] {
            let point = Curve25519Point(p);
            let expected = ir_fields(&IrValue::Curve25519Point(p));
            assert_eq!(atoms_as_fields(point), expected);
            assert_eq!(decode_fields::<Curve25519Point>(&expected), point);
        }
    }

    #[test]
    fn a_point_off_the_curve_does_not_decode() {
        let one = k256::Fp::ONE;
        let mut atoms = encode_field(one).to_vec();
        atoms.extend(encode_field(one));
        atoms.push(ValueAtom::from(Fr::from(0u64)));
        assert!(Secp256k1Point::try_from(&*Value(atoms)).is_err());
    }

    /// The prover holds only the prime-order subgroup of Curve25519. A point of
    /// small order is on the curve, so only the subgroup check refuses it.
    #[test]
    fn a_curve25519_point_outside_the_subgroup_does_not_decode() {
        // (0, -1) has order 2.
        let (x, y) = (curve25519::Fp::ZERO, -curve25519::Fp::ONE);
        assert!(Curve25519Affine::from_xy(x, y).is_some());
        let mut atoms = encode_field(x).to_vec();
        atoms.extend(encode_field(y));
        assert!(Curve25519Point::try_from(&*Value(atoms)).is_err());
    }
}
