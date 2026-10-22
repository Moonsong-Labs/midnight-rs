//! The secp256k1 builtins: field arithmetic, casts to and from bytes, and the
//! curve natives.
//!
//! They follow the Compact runtime. The arithmetic and the natives match the
//! functions of the same name in `built-ins.ts`. The casts match
//! `convertBigintToBytes` and `convertBytesToField` in `casts.ts`.

use midnight_curves::k256::{Fp, Fq, K256, K256Affine};
use midnight_curves::pairing::group::ff::{Field, PrimeField};
use midnight_typed_state::{AlignedValue, InvalidBuiltinDecode, ValueSlice};

use compact_codegen::ir::{Curve, FieldType};

use crate::error::InterpreterError;
use crate::value::Value;

mod values;

pub use values::{Secp256k1Base, Secp256k1Point, Secp256k1Scalar};

/// A binary operator of field arithmetic.
#[derive(Clone, Copy, Debug)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
}

impl ArithOp {
    fn apply<F: Field>(self, left: F, right: F) -> F {
        match self {
            ArithOp::Add => left + right,
            ArithOp::Sub => left - right,
            ArithOp::Mul => left * right,
        }
    }
}

/// A secp256k1 field, which does not compute as the native field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignField {
    /// `Secp256k1Base`.
    Base,
    /// `Secp256k1Scalar`.
    Scalar,
}

impl ForeignField {
    /// The foreign field that `field` names, or `None` for a native field.
    pub fn of(field: &FieldType) -> Option<Self> {
        match field {
            FieldType::Base(Curve::Secp256k1) => Some(Self::Base),
            FieldType::Scalar(Curve::Secp256k1) => Some(Self::Scalar),
            FieldType::Native
            | FieldType::Base(Curve::Jubjub)
            | FieldType::Scalar(Curve::Jubjub) => None,
        }
    }

    /// `left <op> right` in this field.
    ///
    /// # Errors
    ///
    /// [`InterpreterError::TypeError`] if an operand is not an element of
    /// this field.
    pub fn arith(
        self,
        op: ArithOp,
        left: &Value,
        right: &Value,
    ) -> Result<Value, InterpreterError> {
        let what = format!("{op:?}");
        let av = match self {
            Self::Base => AlignedValue::from(Secp256k1Base::from(
                op.apply(base(left, &what)?, base(right, &what)?),
            )),
            Self::Scalar => AlignedValue::from(Secp256k1Scalar::from(
                op.apply(scalar(left, &what)?, scalar(right, &what)?),
            )),
        };
        Ok(Value::AlignedValue(av))
    }

    /// The little-endian bytes of `value`, an element of this field. The cast
    /// to `Bytes<32>` gives these bytes.
    ///
    /// # Errors
    ///
    /// [`InterpreterError::TypeError`] if `value` is not an element of this
    /// field.
    pub fn to_le_bytes(self, value: &Value) -> Result<[u8; 32], InterpreterError> {
        let what = "a cast to Bytes";
        let mut bytes = match self {
            Self::Base => Secp256k1Base::from(base(value, what)?).to_be_bytes(),
            Self::Scalar => Secp256k1Scalar::from(scalar(value, what)?).to_be_bytes(),
        };
        bytes.reverse();
        Ok(bytes)
    }

    /// The element of this field that the little-endian `bytes` encode, modulo
    /// the field order. The cast from `Bytes<32>` gives this element.
    pub fn from_le_bytes(self, bytes: &[u8]) -> Value {
        let av = match self {
            Self::Base => AlignedValue::from(Secp256k1Base::from(reduce_le_bytes::<Fp>(bytes))),
            Self::Scalar => AlignedValue::from(Secp256k1Scalar::from(reduce_le_bytes::<Fq>(bytes))),
        };
        Value::AlignedValue(av)
    }

    /// `value` re-encoded as an element of this field, which checks that it
    /// is one.
    pub(crate) fn encode(self, value: &Value) -> Result<AlignedValue, InterpreterError> {
        let what = "an encoding";
        Ok(match self {
            Self::Base => AlignedValue::from(Secp256k1Base::from(base(value, what)?)),
            Self::Scalar => AlignedValue::from(Secp256k1Scalar::from(scalar(value, what)?)),
        })
    }
}

/// `value` re-encoded as a `Secp256k1Point`, which checks that it is one.
pub(crate) fn encode_point(value: &Value) -> Result<AlignedValue, InterpreterError> {
    Ok(AlignedValue::from(decode::<Secp256k1Point>(
        value,
        "an encoding",
        "Secp256k1Point",
    )?))
}

/// The secp256k1 native `name` applied to `args`, or `None` for another name.
pub(crate) fn native(name: &str, args: &[Value]) -> Option<Result<AlignedValue, InterpreterError>> {
    let result = match name {
        "secp256k1BaseNeg" => unary(name, args).and_then(|x| {
            let x = base(x, name)?;
            Ok(AlignedValue::from(Secp256k1Base::from(-x)))
        }),
        "secp256k1ScalarNeg" => unary(name, args).and_then(|x| {
            let x = scalar(x, name)?;
            Ok(AlignedValue::from(Secp256k1Scalar::from(-x)))
        }),
        "secp256k1BaseInv" => unary(name, args).and_then(|x| {
            let inverse = base(x, name)?.invert().into_option().ok_or_else(|| {
                InterpreterError::TypeError("secp256k1 base field has no inverse for 0".into())
            })?;
            Ok(AlignedValue::from(Secp256k1Base::from(inverse)))
        }),
        "secp256k1ScalarInv" => unary(name, args).and_then(|x| {
            let inverse = scalar(x, name)?.invert().into_option().ok_or_else(|| {
                InterpreterError::TypeError("secp256k1 scalar field has no inverse for 0".into())
            })?;
            Ok(AlignedValue::from(Secp256k1Scalar::from(inverse)))
        }),
        "secp256k1PointX" => coordinate(name, args, "x").map(|(x, _)| AlignedValue::from(x)),
        "secp256k1PointY" => coordinate(name, args, "y").map(|(_, y)| AlignedValue::from(y)),
        "secp256k1Add" => {
            binary(name, args).and_then(|(a, b)| Ok(to_point(point(a, name)? + point(b, name)?)))
        }
        "secp256k1Mul" => {
            binary(name, args).and_then(|(a, b)| Ok(to_point(point(a, name)? * scalar(b, name)?)))
        }
        "secp256k1MulGenerator" => {
            unary(name, args).and_then(|b| Ok(to_point(K256::generator() * scalar(b, name)?)))
        }
        _ => return None,
    };
    Some(result)
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

/// The affine coordinates of the point in `args`. The identity has none.
fn coordinate(
    name: &str,
    args: &[Value],
    axis: &str,
) -> Result<(Secp256k1Base, Secp256k1Base), InterpreterError> {
    decode::<Secp256k1Point>(unary(name, args)?, name, "Secp256k1Point")?
        .coordinates()
        .ok_or_else(|| {
            InterpreterError::TypeError(format!(
                "cannot extract the {axis}-coordinate of the secp256k1 identity point"
            ))
        })
}

/// Decode `value` as a `T`, the Compact type `ty`, for the operation `what`.
fn decode<T>(value: &Value, what: &str, ty: &str) -> Result<T, InterpreterError>
where
    T: for<'a> TryFrom<&'a ValueSlice, Error = InvalidBuiltinDecode>,
{
    let wrong = || InterpreterError::TypeError(format!("{what}: expected a {ty}, got {value:?}"));
    match value {
        Value::AlignedValue(av) => T::try_from(&*av.value).map_err(|_| wrong()),
        _ => Err(wrong()),
    }
}

fn base(value: &Value, what: &str) -> Result<Fp, InterpreterError> {
    decode::<Secp256k1Base>(value, what, "Secp256k1Base").map(Fp::from)
}

fn scalar(value: &Value, what: &str) -> Result<Fq, InterpreterError> {
    decode::<Secp256k1Scalar>(value, what, "Secp256k1Scalar").map(Fq::from)
}

fn point(value: &Value, what: &str) -> Result<K256, InterpreterError> {
    decode::<Secp256k1Point>(value, what, "Secp256k1Point").map(|p| K256::from(K256Affine::from(p)))
}

fn to_point(p: K256) -> AlignedValue {
    AlignedValue::from(Secp256k1Point::from(K256Affine::from(p)))
}

fn reduce_le_bytes<F: PrimeField>(bytes: &[u8]) -> F {
    let radix = F::from(256);
    bytes
        .iter()
        .rev()
        .fold(F::ZERO, |acc, &b| acc * radix + F::from(u64::from(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Compact runtime throws where these natives have no value. A zero in
    /// its place, as `jubjubPointX` gives for the Jubjub identity, would let a
    /// circuit run that the runtime refuses.
    #[test]
    fn natives_without_a_value_fail() {
        let zero_base = Value::AlignedValue(AlignedValue::from(Secp256k1Base::default()));
        let zero_scalar = Value::AlignedValue(AlignedValue::from(Secp256k1Scalar::default()));
        let identity = Value::AlignedValue(AlignedValue::from(Secp256k1Point::default()));
        for (name, arg) in [
            ("secp256k1BaseInv", zero_base),
            ("secp256k1ScalarInv", zero_scalar),
            ("secp256k1PointX", identity.clone()),
            ("secp256k1PointY", identity),
        ] {
            assert!(
                matches!(native(name, &[arg]), Some(Err(_))),
                "{name} must fail"
            );
        }
    }
}
