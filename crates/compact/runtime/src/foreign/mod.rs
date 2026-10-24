//! The foreign fields and curves of Compact: secp256k1, secp256r1 and
//! Curve25519. This module holds their values, their field arithmetic, the casts
//! between their fields and bytes, and their curve natives.
//!
//! They follow the Compact runtime. The arithmetic and the natives match the
//! functions of the same name in `built-ins.ts`, and compute with the curves of
//! `midnight-curves`, as ZKIR v3 does. The casts match `convertBigintToBytes`
//! and `convertBytesToField` in `casts.ts`.

use midnight_circuits::CircuitField;
use midnight_curves::pairing::group::ff::{Field, PrimeField};
use midnight_typed_state::{Aligned, AlignedValue, InvalidBuiltinDecode, ValueSlice};

use compact_codegen::ir::{Curve, FieldType};
use midnight_base_crypto::fab::Alignment;

use crate::error::InterpreterError;
use crate::value::Value;

mod fields;
mod natives;
mod points;

pub use fields::{
    Curve25519Base, Curve25519Scalar, Secp256k1Base, Secp256k1Scalar, Secp256r1Base,
    Secp256r1Scalar,
};
pub(crate) use natives::native;
pub use points::{Curve25519Point, Secp256k1Point, Secp256r1Point};

use fields::FieldValue;

/// A Compact value type that the interpreter holds as an `AlignedValue`.
pub(crate) trait CompactValue:
    Copy + Aligned + Into<AlignedValue> + for<'a> TryFrom<&'a ValueSlice, Error = InvalidBuiltinDecode>
{
    /// The name of the Compact type.
    const NAME: &'static str;
}

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

/// A foreign field of Compact, which does not compute as the native field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignField {
    /// `Secp256k1Base`.
    Secp256k1Base,
    /// `Secp256k1Scalar`.
    Secp256k1Scalar,
    /// `Secp256r1Base`.
    Secp256r1Base,
    /// `Secp256r1Scalar`.
    Secp256r1Scalar,
    /// `Curve25519Base`.
    Curve25519Base,
    /// `Curve25519Scalar`.
    Curve25519Scalar,
}

/// `$f::<T>($arg, ...)`, for `T` the Rust image of the foreign field `$field`.
macro_rules! with_field_value {
    ($field:expr, $f:ident($($arg:expr),*)) => {
        match $field {
            ForeignField::Secp256k1Base => $f::<Secp256k1Base>($($arg),*),
            ForeignField::Secp256k1Scalar => $f::<Secp256k1Scalar>($($arg),*),
            ForeignField::Secp256r1Base => $f::<Secp256r1Base>($($arg),*),
            ForeignField::Secp256r1Scalar => $f::<Secp256r1Scalar>($($arg),*),
            ForeignField::Curve25519Base => $f::<Curve25519Base>($($arg),*),
            ForeignField::Curve25519Scalar => $f::<Curve25519Scalar>($($arg),*),
        }
    };
}

impl ForeignField {
    /// The foreign field that `field` names, or `None` for a native field.
    pub fn of(field: &FieldType) -> Option<Self> {
        match field {
            FieldType::Native
            | FieldType::Base(Curve::Jubjub)
            | FieldType::Scalar(Curve::Jubjub) => None,
            FieldType::Base(Curve::Secp256k1) => Some(Self::Secp256k1Base),
            FieldType::Scalar(Curve::Secp256k1) => Some(Self::Secp256k1Scalar),
            FieldType::Base(Curve::Secp256r1) => Some(Self::Secp256r1Base),
            FieldType::Scalar(Curve::Secp256r1) => Some(Self::Secp256r1Scalar),
            FieldType::Base(Curve::Curve25519) => Some(Self::Curve25519Base),
            FieldType::Scalar(Curve::Curve25519) => Some(Self::Curve25519Scalar),
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
        with_field_value!(self, arith(op, left, right))
    }

    /// The little-endian bytes of `value`, an element of this field. The cast
    /// to `Bytes<32>` gives these bytes.
    ///
    /// # Errors
    ///
    /// [`InterpreterError::TypeError`] if `value` is not an element of this
    /// field.
    pub fn to_le_bytes(self, value: &Value) -> Result<[u8; 32], InterpreterError> {
        with_field_value!(self, to_le_bytes(value))
    }

    /// The element of this field that the little-endian `bytes` encode, modulo
    /// the field order. The cast from `Bytes<n>` gives this element.
    pub fn from_le_bytes(self, bytes: &[u8]) -> Value {
        with_field_value!(self, from_le_bytes(bytes))
    }

    /// `value` re-encoded as an element of this field, which checks that it
    /// is one.
    pub(crate) fn encode(self, value: &Value) -> Result<AlignedValue, InterpreterError> {
        with_field_value!(self, encode_as(value))
    }

    /// The alignment of an element of this field.
    pub(crate) fn alignment(self) -> Alignment {
        with_field_value!(self, alignment())
    }

    /// Zero, the default of this field.
    pub(crate) fn zero(self) -> AlignedValue {
        with_field_value!(self, zero())
    }
}

fn arith<T: FieldValue>(
    op: ArithOp,
    left: &Value,
    right: &Value,
) -> Result<Value, InterpreterError> {
    let what = format!("{op:?}");
    let result = op.apply(field::<T>(left, &what)?, field::<T>(right, &what)?);
    Ok(Value::AlignedValue(T::from(result).into()))
}

fn to_le_bytes<T: FieldValue>(value: &Value) -> Result<[u8; 32], InterpreterError> {
    Ok(field::<T>(value, "a cast to Bytes")?.to_bytes_le())
}

fn from_le_bytes<T: FieldValue>(bytes: &[u8]) -> Value {
    Value::AlignedValue(T::from(reduce_le_bytes(bytes)).into())
}

fn alignment<T: Aligned>() -> Alignment {
    T::alignment()
}

fn zero<T: FieldValue>() -> AlignedValue {
    T::default().into()
}

/// `value` re-encoded as a `T`, which checks that it is one.
pub(crate) fn encode_as<T: CompactValue>(value: &Value) -> Result<AlignedValue, InterpreterError> {
    Ok(decode::<T>(value, "an encoding")?.into())
}

/// Decode `value` as a `T` for the operation `what`.
fn decode<T: CompactValue>(value: &Value, what: &str) -> Result<T, InterpreterError> {
    let wrong =
        || InterpreterError::TypeError(format!("{what}: expected a {}, got {value:?}", T::NAME));
    match value {
        Value::AlignedValue(av) => T::try_from(&*av.value).map_err(|_| wrong()),
        _ => Err(wrong()),
    }
}

/// The field element of `value`, a `T`, for the operation `what`.
fn field<T: FieldValue>(value: &Value, what: &str) -> Result<T::Field, InterpreterError> {
    decode::<T>(value, what).map(Into::into)
}

fn reduce_le_bytes<F: PrimeField>(bytes: &[u8]) -> F {
    let radix = F::from(256);
    bytes
        .iter()
        .rev()
        .fold(F::ZERO, |acc, &b| acc * radix + F::from(u64::from(b)))
}

#[cfg(test)]
mod test_support {
    use midnight_base_crypto::fab::{InvalidBuiltinDecode, Value, ValueAtom, ValueSlice};
    use midnight_transient_crypto::curve::Fr;
    use midnight_typed_state::AlignedValue;
    use midnight_zkir_v3::ir_instructions::encode::encode_offcircuit;
    use midnight_zkir_v3::ir_types::IrValue;

    /// The field elements the prover reads for `value`, one per atom.
    pub(super) fn atoms_as_fields(value: impl Into<AlignedValue>) -> Vec<Fr> {
        let av: AlignedValue = value.into();
        av.value
            .0
            .iter()
            .map(|atom| Fr::try_from(atom).expect("each atom is a field element"))
            .collect()
    }

    /// The field elements that ZKIR v3 encodes `value` as.
    pub(super) fn ir_fields(value: &IrValue) -> Vec<Fr> {
        encode_offcircuit(value)
            .into_iter()
            .map(|v| match v {
                IrValue::Native(fr) => fr,
                other => panic!("encode gives native values, got {other:?}"),
            })
            .collect()
    }

    /// The `T` that the prover's `fields` encode.
    pub(super) fn decode_fields<
        T: for<'a> TryFrom<&'a ValueSlice, Error = InvalidBuiltinDecode>,
    >(
        fields: &[Fr],
    ) -> T {
        let value = Value(fields.iter().map(|f| ValueAtom::from(*f)).collect());
        T::try_from(&value).expect("the prover's encoding decodes")
    }
}
