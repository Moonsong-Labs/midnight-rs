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
    Aligned, Alignment, AlignmentAtom, InvalidBuiltinDecode, Value, ValueSlice,
};
use midnight_curves::curve25519::{self, Curve25519, Curve25519Affine, Curve25519Subgroup};
use midnight_curves::k256::{self, K256, K256Affine};
use midnight_curves::p256::{self, P256, P256Affine};
use midnight_zkir_v3::ir_types::IrType;

use super::CompactValue;
use super::fields::{
    Curve25519Base, Emulated, Secp256k1Base, Secp256r1Base, field_alignment, from_atoms, to_atoms,
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
///
/// // (0, -1) is on the curve, but its order is 2, so it is outside the
/// // subgroup.
/// let mut minus_one = [0xff; 32];
/// minus_one[0] = 0x7f;
/// minus_one[31] = 0xec;
/// let minus_one = Curve25519Base::from_be_bytes(minus_one).unwrap();
/// assert_eq!(Curve25519Point::from_coordinates(x, minus_one), None);
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
    /// The point at the affine coordinates `(x, y)`, if it is in the subgroup.
    ///
    /// A point of small order is on the curve, but not in the prime-order
    /// subgroup. This function gives `None` for it.
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
        Value(to_atoms(K256::from(value.0)))
    }
}

impl From<Secp256r1Point> for Value {
    fn from(value: Secp256r1Point) -> Value {
        Value(to_atoms(P256::from(value.0)))
    }
}

impl From<Curve25519Point> for Value {
    fn from(value: Curve25519Point) -> Value {
        Value(to_atoms(value.0))
    }
}

impl TryFrom<&ValueSlice> for Secp256k1Point {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        from_atoms::<K256>(&value.0, &IrType::Secp256k1Point)
            .map(|point| Self(K256Affine::from(point)))
            .ok_or(InvalidBuiltinDecode(Self::NAME))
    }
}

impl TryFrom<&ValueSlice> for Secp256r1Point {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        from_atoms::<P256>(&value.0, &IrType::Secp256r1Point)
            .map(|point| Self(P256Affine::from(point)))
            .ok_or(InvalidBuiltinDecode(Self::NAME))
    }
}

impl TryFrom<&ValueSlice> for Curve25519Point {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        from_atoms(&value.0, &IrType::Curve25519Point)
            .map(Self)
            .ok_or(InvalidBuiltinDecode(Self::NAME))
    }
}

fn flagged_alignment<F: Emulated>() -> Alignment {
    let base = field_alignment::<F>();
    Alignment::concat([&base, &base, &Alignment::singleton(AlignmentAtom::Field)])
}
