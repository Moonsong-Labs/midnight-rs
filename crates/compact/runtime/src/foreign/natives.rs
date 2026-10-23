//! The natives of the foreign curves: `neg` and `inv` of each field, the
//! coordinates of a point, and `ecAdd`, `ecMul` and `ecMulGenerator`.
//!
//! Each curve has the same nine natives. The symbol of a native is the prefix
//! of its curve and the name of the operation, such as `secp256r1BaseInv`.

use midnight_curves::curve25519::Curve25519Subgroup;
use midnight_curves::k256::{K256, K256Affine};
use midnight_curves::p256::{P256, P256Affine};
use midnight_curves::pairing::group::Group;
use midnight_curves::pairing::group::ff::Field;
use midnight_typed_state::AlignedValue;

use super::fields::FieldValue;
use super::{
    CompactValue, Curve25519Base, Curve25519Point, Curve25519Scalar, Secp256k1Base, Secp256k1Point,
    Secp256k1Scalar, Secp256r1Base, Secp256r1Point, Secp256r1Scalar, decode, field,
};
use crate::error::InterpreterError;
use crate::value::Value;

/// The foreign-curve native `name` applied to `args`, or `None` for another
/// name.
pub(crate) fn native(name: &str, args: &[Value]) -> Option<Result<AlignedValue, InterpreterError>> {
    if let Some(op) = name.strip_prefix(Secp256k1::PREFIX) {
        curve_native::<Secp256k1>(op, name, args)
    } else if let Some(op) = name.strip_prefix(Secp256r1::PREFIX) {
        curve_native::<Secp256r1>(op, name, args)
    } else if let Some(op) = name.strip_prefix(Curve25519::PREFIX) {
        curve_native::<Curve25519>(op, name, args)
    } else {
        None
    }
}

/// A foreign curve: the Rust images of its Compact types, and the group that
/// its natives compute in.
trait ForeignCurve {
    /// The prefix of the symbols of the curve's natives.
    const PREFIX: &'static str;
    /// The name of the curve in the errors of the Compact runtime.
    const NAME: &'static str;

    type Base: FieldValue;
    type Scalar: FieldValue;
    type Point: CompactValue;
    type Group: Group<Scalar = <Self::Scalar as FieldValue>::Field>;

    fn to_group(point: Self::Point) -> Self::Group;

    fn from_group(group: Self::Group) -> Self::Point;

    /// The affine coordinates of `point`, or `None` if it has none.
    fn coordinates(point: Self::Point) -> Option<(Self::Base, Self::Base)>;
}

struct Secp256k1;

impl ForeignCurve for Secp256k1 {
    const PREFIX: &'static str = "secp256k1";
    const NAME: &'static str = "secp256k1";

    type Base = Secp256k1Base;
    type Scalar = Secp256k1Scalar;
    type Point = Secp256k1Point;
    type Group = K256;

    fn to_group(point: Secp256k1Point) -> K256 {
        K256::from(K256Affine::from(point))
    }

    fn from_group(group: K256) -> Secp256k1Point {
        Secp256k1Point::from(K256Affine::from(group))
    }

    fn coordinates(point: Secp256k1Point) -> Option<(Secp256k1Base, Secp256k1Base)> {
        point.coordinates()
    }
}

struct Secp256r1;

impl ForeignCurve for Secp256r1 {
    const PREFIX: &'static str = "secp256r1";
    const NAME: &'static str = "secp256r1";

    type Base = Secp256r1Base;
    type Scalar = Secp256r1Scalar;
    type Point = Secp256r1Point;
    type Group = P256;

    fn to_group(point: Secp256r1Point) -> P256 {
        P256::from(P256Affine::from(point))
    }

    fn from_group(group: P256) -> Secp256r1Point {
        Secp256r1Point::from(P256Affine::from(group))
    }

    fn coordinates(point: Secp256r1Point) -> Option<(Secp256r1Base, Secp256r1Base)> {
        point.coordinates()
    }
}

struct Curve25519;

impl ForeignCurve for Curve25519 {
    const PREFIX: &'static str = "curve25519";
    const NAME: &'static str = "Curve25519";

    type Base = Curve25519Base;
    type Scalar = Curve25519Scalar;
    type Point = Curve25519Point;
    type Group = Curve25519Subgroup;

    fn to_group(point: Curve25519Point) -> Curve25519Subgroup {
        point.into()
    }

    fn from_group(group: Curve25519Subgroup) -> Curve25519Point {
        group.into()
    }

    fn coordinates(point: Curve25519Point) -> Option<(Curve25519Base, Curve25519Base)> {
        Some(point.coordinates())
    }
}

/// The native `op` of the curve `C`, which `name` spells, applied to `args`.
fn curve_native<C: ForeignCurve>(
    op: &str,
    name: &str,
    args: &[Value],
) -> Option<Result<AlignedValue, InterpreterError>> {
    let result = match op {
        "BaseNeg" => neg::<C::Base>(name, args),
        "ScalarNeg" => neg::<C::Scalar>(name, args),
        "BaseInv" => inv::<C::Base>(name, args, &format!("{} base field", C::NAME)),
        "ScalarInv" => inv::<C::Scalar>(name, args, &format!("{} scalar field", C::NAME)),
        "PointX" => coordinates::<C>(name, args, "x").map(|(x, _)| x.into()),
        "PointY" => coordinates::<C>(name, args, "y").map(|(_, y)| y.into()),
        "Add" => binary(name, args).and_then(|(a, b)| {
            let sum = C::to_group(decode(a, name)?) + C::to_group(decode(b, name)?);
            Ok(C::from_group(sum).into())
        }),
        "Mul" => binary(name, args).and_then(|(a, k)| {
            let product = C::to_group(decode(a, name)?) * field::<C::Scalar>(k, name)?;
            Ok(C::from_group(product).into())
        }),
        "MulGenerator" => unary(name, args).and_then(|k| {
            let product = C::Group::generator() * field::<C::Scalar>(k, name)?;
            Ok(C::from_group(product).into())
        }),
        _ => return None,
    };
    Some(result)
}

fn neg<T: FieldValue>(name: &str, args: &[Value]) -> Result<AlignedValue, InterpreterError> {
    let x = field::<T>(unary(name, args)?, name)?;
    Ok(T::from(-x).into())
}

/// The inverse of the element in `args`. `field_name` names its field in the
/// error for zero.
fn inv<T: FieldValue>(
    name: &str,
    args: &[Value],
    field_name: &str,
) -> Result<AlignedValue, InterpreterError> {
    let x = field::<T>(unary(name, args)?, name)?;
    // `Field::invert`, not an inherent `invert`: the one of curve25519-dalek's
    // `Scalar` maps zero to zero.
    let inverse = Field::invert(&x)
        .into_option()
        .ok_or_else(|| InterpreterError::TypeError(format!("{field_name} has no inverse for 0")))?;
    Ok(T::from(inverse).into())
}

/// The affine coordinates of the point in `args`. The identity of a curve in
/// Weierstrass form has none.
fn coordinates<C: ForeignCurve>(
    name: &str,
    args: &[Value],
    axis: &str,
) -> Result<(C::Base, C::Base), InterpreterError> {
    C::coordinates(decode(unary(name, args)?, name)?).ok_or_else(|| {
        InterpreterError::TypeError(format!(
            "cannot extract the {axis}-coordinate of the {} identity point",
            C::NAME
        ))
    })
}

fn unary<'a>(name: &str, args: &'a [Value]) -> Result<&'a Value, InterpreterError> {
    match args {
        [x] => Ok(x),
        _ => Err(arity_error(name, 1, args)),
    }
}

fn binary<'a>(name: &str, args: &'a [Value]) -> Result<(&'a Value, &'a Value), InterpreterError> {
    match args {
        [a, b] => Ok((a, b)),
        _ => Err(arity_error(name, 2, args)),
    }
}

fn arity_error(name: &str, n: usize, args: &[Value]) -> InterpreterError {
    InterpreterError::TypeError(format!("{name} expects {n} arguments, got {}", args.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(av: impl Into<AlignedValue>) -> Value {
        Value::AlignedValue(av.into())
    }

    /// The Compact runtime throws where these natives have no value. A zero in
    /// its place, as `jubjubPointX` gives for the Jubjub identity, would let a
    /// circuit run that the runtime refuses.
    #[test]
    fn natives_without_a_value_fail() {
        for (name, arg) in [
            ("secp256k1BaseInv", value(Secp256k1Base::default())),
            ("secp256k1ScalarInv", value(Secp256k1Scalar::default())),
            ("secp256k1PointX", value(Secp256k1Point::default())),
            ("secp256k1PointY", value(Secp256k1Point::default())),
            ("secp256r1BaseInv", value(Secp256r1Base::default())),
            ("secp256r1ScalarInv", value(Secp256r1Scalar::default())),
            ("secp256r1PointX", value(Secp256r1Point::default())),
            ("secp256r1PointY", value(Secp256r1Point::default())),
            ("curve25519BaseInv", value(Curve25519Base::default())),
            ("curve25519ScalarInv", value(Curve25519Scalar::default())),
        ] {
            assert!(
                matches!(native(name, &[arg]), Some(Err(_))),
                "{name} must fail"
            );
        }
    }
}
