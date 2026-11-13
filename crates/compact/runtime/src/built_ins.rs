//! Compact runtime builtin circuits: hashes, commitments, elliptic-curve
//! operations, and field/bytes casts invoked by name during interpretation.

use midnight_base_crypto::hash::HashOutput;
use midnight_transient_crypto::curve::{EmbeddedGroupAffine, Fr};
use midnight_typed_state::AlignedValue;

use crate::compact_types::encode_typed;
use crate::conversions::{value_to_embedded_group, value_to_fr, value_to_hash_output};
use crate::error::InterpreterError;
use crate::value::Value;
use compact_codegen::ir::{FieldType, Type};

/// Encode argument `i` for hashing or committing: at its declared parameter
/// type, or with the type-free encoding when no type is declared.
///
/// A struct has no type-free encoding, so it is an error without a type. A
/// fallback to the empty value would give a commitment that binds to nothing
/// and still looks like a valid digest.
fn encode_argument(
    arg_types: &[Type],
    i: usize,
    v: &Value,
) -> Result<AlignedValue, InterpreterError> {
    match arg_types.get(i) {
        Some(ty) => encode_typed(v, ty),
        None => v.try_to_aligned_value(),
    }
}

/// The bytes that `persistentHash`, `keccak256` and `sha512` digest: the
/// binary representation of each argument, in order.
///
/// ZKIR v3 builds the same bytes for its `PersistentHash`, `Keccak256` and
/// `Sha512` instructions and changes only the digest, so these natives encode
/// here.
fn hash_preimage(args: &[Value], arg_types: &[Type]) -> Result<Vec<u8>, InterpreterError> {
    use midnight_base_crypto::repr::BinaryHashRepr;
    use midnight_transient_crypto::fab::ValueReprAlignedValue;

    let mut bytes = Vec::new();
    for (i, value) in args.iter().enumerate() {
        let av = match (value, arg_types.get(i)) {
            (Value::Integer(_), None) => encode_typed(value, &Type::Field(FieldType::Native))?,
            _ => encode_argument(arg_types, i, value)?,
        };
        ValueReprAlignedValue(av).binary_repr(&mut bytes);
    }
    Ok(bytes)
}

fn arg<'a>(args: &'a [Value], i: usize, native: &str) -> Result<&'a Value, InterpreterError> {
    args.get(i)
        .ok_or_else(|| InterpreterError::TypeError(format!("{native}: missing argument {}", i + 1)))
}

fn field_arg(args: &[Value], i: usize, native: &str) -> Result<Fr, InterpreterError> {
    value_to_fr(arg(args, i, native)?).ok_or_else(|| {
        InterpreterError::TypeError(format!("{native}: argument {} is not a Field", i + 1))
    })
}

fn point_arg(
    args: &[Value],
    i: usize,
    native: &str,
) -> Result<EmbeddedGroupAffine, InterpreterError> {
    value_to_embedded_group(arg(args, i, native)?).ok_or_else(|| {
        InterpreterError::TypeError(format!("{native}: argument {} is not a JubjubPoint", i + 1))
    })
}

fn hash_arg(args: &[Value], i: usize, native: &str) -> Result<HashOutput, InterpreterError> {
    value_to_hash_output(arg(args, i, native)?).map_err(|_| {
        InterpreterError::TypeError(format!("{native}: argument {} is not a Bytes<32>", i + 1))
    })
}

fn arity(args: &[Value], n: usize, native: &str) -> Result<(), InterpreterError> {
    if args.len() != n {
        return Err(InterpreterError::TypeError(format!(
            "{native} expects {n} arguments, got {}",
            args.len()
        )));
    }
    Ok(())
}

/// Run the Compact runtime builtin `name` on `args`.
///
/// `arg_types` holds the declared parameter types of the callee, in argument
/// order. Returns `Some(Ok(value))` when `name` is a builtin, `Some(Err(..))`
/// when the builtin fails, and `None` when `name` is not a builtin.
///
/// A hash or commitment encodes each argument at its type, as the chain does:
/// a `Uint<64>` argument is an 8-byte atom, and a struct is the concatenation
/// of its fields at their declared widths. An argument with no type takes the
/// type-free encoding, except that `persistentHash`, `keccak256` and `sha512`
/// take a bare integer as a field element.
pub fn try_builtin_typed(
    name: &str,
    args: &[Value],
    arg_types: &[Type],
) -> Option<Result<Value, InterpreterError>> {
    builtin(name, args, arg_types).transpose()
}

fn builtin(
    name: &str,
    args: &[Value],
    arg_types: &[Type],
) -> Result<Option<Value>, InterpreterError> {
    let encode_arg = |i: usize, v: &Value| encode_argument(arg_types, i, v);
    let av = match name {
        "persistentCommit" => {
            // persistentCommit(value, opening) = persistent_commit(value, opening):
            // a domain-separated commitment. The opening is written to the
            // hasher first, then the value (see base-crypto `persistent_commit`).
            // Used to derive a contract's custom shielded token type:
            // `tokenType(domain_sep, self()) = persistentCommit((domain_sep,
            // self().bytes), "midnight:derive_token\0..")`. Matching the
            // on-chain derivation exactly is what lets a minted coin's color
            // line up with the recipient's wallet sync.
            use midnight_base_crypto::hash::persistent_commit;
            use midnight_transient_crypto::fab::ValueReprAlignedValue;

            let value = arg(args, 0, name)?;
            let opening = hash_arg(args, 1, name)?;
            // Flatten the value into a single AlignedValue and commit. A
            // `Value::Tuple` concatenates its elements in order; a struct is
            // encoded field-by-field at its declared widths when its type is in
            // scope, and is an error when it is not.
            let wrapped = ValueReprAlignedValue(encode_arg(0, value)?);
            let hash: HashOutput = persistent_commit(&wrapped, opening);
            AlignedValue::from(hash.0)
        }
        "transientCommit" => {
            // transientCommit(value, opening): the Poseidon (transient-field)
            // counterpart of persistentCommit. Binds to transient-crypto's
            // `transient_commit`, so the value matches what the zkir/prover
            // computes rather than being reimplemented here.
            use midnight_transient_crypto::fab::ValueReprAlignedValue;
            use midnight_transient_crypto::hash::transient_commit;

            let value = arg(args, 0, name)?;
            let opening = field_arg(args, 1, name)?;
            let wrapped = ValueReprAlignedValue(encode_arg(0, value)?);
            AlignedValue::from(transient_commit(&wrapped, opening))
        }
        "persistentHash" => {
            use midnight_base_crypto::hash::persistent_hash;
            AlignedValue::from(persistent_hash(&hash_preimage(args, arg_types)?).0)
        }
        "keccak256" => {
            use sha3::{Digest, Keccak256};
            let hash: [u8; 32] = Keccak256::digest(hash_preimage(args, arg_types)?).into();
            AlignedValue::from(hash)
        }
        "sha512" => {
            use sha2::{Digest, Sha512};
            let hash: [u8; 64] = Sha512::digest(hash_preimage(args, arg_types)?).into();
            AlignedValue::from(hash)
        }
        // EC scalar multiplication: G * scalar
        "ecMulGenerator" => {
            AlignedValue::from(EmbeddedGroupAffine::generator() * field_arg(args, 0, name)?)
        }
        // EC scalar multiplication: point * scalar
        "ecMul" => {
            arity(args, 2, name)?;
            AlignedValue::from(point_arg(args, 0, name)? * field_arg(args, 1, name)?)
        }
        // EC point addition: p1 + p2
        "ecAdd" => {
            arity(args, 2, name)?;
            AlignedValue::from(point_arg(args, 0, name)? + point_arg(args, 1, name)?)
        }
        "ecNeg" => AlignedValue::from(-point_arg(args, 0, name)?),
        "hashToCurve" => {
            // hashToCurve(value) -> JubjubPoint. Binds to transient-crypto's
            // `hash_to_curve` so the embedded-curve point matches the prover.
            use midnight_transient_crypto::fab::ValueReprAlignedValue;
            use midnight_transient_crypto::hash::hash_to_curve;
            let value = arg(args, 0, name)?;
            AlignedValue::from(hash_to_curve(&ValueReprAlignedValue(encode_arg(0, value)?)))
        }
        "jubjubPointX" => {
            AlignedValue::from(point_arg(args, 0, name)?.x().unwrap_or(Fr::from(0u64)))
        }
        "jubjubPointY" => {
            AlignedValue::from(point_arg(args, 0, name)?.y().unwrap_or(Fr::from(0u64)))
        }
        "constructJubjubPoint" => {
            // constructJubjubPoint(x, y) -> JubjubPoint. Binds to
            // EmbeddedGroupAffine::new, which returns None for an off-curve
            // (x, y) pair.
            arity(args, 2, name)?;
            let x = field_arg(args, 0, name)?;
            let y = field_arg(args, 1, name)?;
            let point = EmbeddedGroupAffine::new(x, y).ok_or_else(|| {
                InterpreterError::TypeError(
                    "constructJubjubPoint: (x, y) is not on the embedded curve".to_string(),
                )
            })?;
            AlignedValue::from(point)
        }
        "transientHash" => {
            // transientHash(value): Poseidon over the value's field elements.
            // Binds to transient-crypto's `field_vec`, the same decomposition
            // `transientCommit` above hands to `transient_commit`, so the digest
            // matches the circuit rather than being reimplemented here. That
            // matters for anything wider than one field element: a struct
            // literal arrives as one multi-atom value, and a `Bytes<N>` atom
            // splits every 31 bytes.
            use midnight_transient_crypto::fab::ValueReprAlignedValue;
            use midnight_transient_crypto::hash::transient_hash;
            use midnight_transient_crypto::repr::FieldRepr;

            let mut field_inputs: Vec<Fr> = Vec::with_capacity(args.len());
            for (i, value) in args.iter().enumerate() {
                // Each argument is encoded whole: a Tuple concatenates its
                // elements, and a declared Vector or Struct type recurses into
                // them, so a struct nested in either keeps its own type.
                field_inputs.extend(ValueReprAlignedValue(encode_arg(i, value)?).field_vec());
            }
            AlignedValue::from(transient_hash(&field_inputs))
        }
        "degradeToTransient" => {
            // Maps a persistent-field value (a 32-byte hash / Field) into the
            // transient field. This is the library `degrade_to_transient`, i.e.
            // `HashOutput::field_vec()[1]` — the low `FR_BYTES_STORED` (31) bytes
            // decoded as an `Fr`, dropping the top byte. It is deliberately *not*
            // a little-endian decode of all 32 bytes: those differ whenever the
            // 32nd byte is non-zero, and the on-chain circuit computes the former.
            use midnight_transient_crypto::hash::degrade_to_transient;
            AlignedValue::from(degrade_to_transient(hash_arg(args, 0, name)?))
        }
        "upgradeFromTransient" => {
            // Field -> Bytes<32>: the inverse-direction companion of
            // degradeToTransient. Binds to transient-crypto's
            // `upgrade_from_transient`.
            use midnight_transient_crypto::hash::upgrade_from_transient;
            AlignedValue::from(upgrade_from_transient(field_arg(args, 0, name)?).0)
        }
        _ => match crate::foreign::native(name, args) {
            Some(result) => result?,
            None => return Ok(None), // Not a builtin
        },
    };
    Ok(Some(Value::AlignedValue(av)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_base_crypto::fab::{Alignment, AlignmentAtom};

    fn label_value_av() -> AlignedValue {
        crate::compact_types::bytes_aligned_value(vec![0x11; 32], 32).unwrap()
    }

    fn label_value() -> Value {
        Value::AlignedValue(label_value_av())
    }

    /// The named spelling of a `Point`. This is the shape #119 is about:
    /// a `HashMap`, so it carries no field order of its own and the encoding has
    /// to take order from the declaration.
    fn a_point_struct() -> Value {
        Value::Struct(
            [
                ("x".to_string(), Value::Integer(0x1234_5678)),
                ("flag".to_string(), Value::Bool(true)),
                ("label".to_string(), label_value()),
            ]
            .into_iter()
            .collect(),
        )
    }

    /// `struct Point { x: Uint<32>, flag: Boolean, label: Bytes<32> }`.
    fn point_ty() -> Type {
        Type::Struct {
            name: "Point".to_string(),
            fields: vec![
                (
                    "x".to_string(),
                    Type::Unsigned("4294967295".parse().unwrap()),
                ),
                ("flag".to_string(), Type::Boolean),
                ("label".to_string(), Type::Bytes(32)),
            ],
        }
    }

    /// A struct that does not match its declaration is an error, never a
    /// partial or empty encoding.
    #[test]
    fn malformed_structs_are_rejected() {
        let missing_field = Value::Struct(
            [
                ("x".to_string(), Value::Integer(1)),
                ("flag".to_string(), Value::Bool(true)),
                ("nope".to_string(), label_value()),
            ]
            .into_iter()
            .collect(),
        );
        assert!(encode_typed(&missing_field, &point_ty()).is_err());

        let short_tuple = Value::Tuple(vec![Value::Integer(1), Value::Bool(true)]);
        assert!(encode_typed(&short_tuple, &point_ty()).is_err());

        let extra_field = Value::Struct(
            [
                ("x".to_string(), Value::Integer(1)),
                ("flag".to_string(), Value::Bool(true)),
                ("label".to_string(), label_value()),
                ("extra".to_string(), Value::Integer(0)),
            ]
            .into_iter()
            .collect(),
        );
        assert!(encode_typed(&extra_field, &point_ty()).is_err());
    }

    /// Without a declared type there is no way to know a struct's field widths,
    /// so the builtins must say so rather than fall back to the empty encoding.
    #[test]
    fn an_untyped_struct_is_an_error_not_an_empty_encoding() {
        assert!(a_point_struct().try_to_aligned_value().is_err());
        assert!(
            Value::Tuple(vec![Value::Integer(1), a_point_struct()])
                .try_to_aligned_value()
                .is_err(),
            "including when nested in a tuple"
        );

        for name in [
            "persistentHash",
            "keccak256",
            "sha512",
            "persistentCommit",
            "transientCommit",
            "transientHash",
        ] {
            let args = [a_point_struct(), Value::Integer(0)];
            assert!(
                matches!(try_builtin_typed(name, &args, &[]), Some(Err(_))),
                "{name} must reject an untyped struct"
            );
        }
    }

    /// A `Uint` bound encodes at the compiler's `ceil(bits/8)` width, which is
    /// not always a primitive size. Pinned to what the canonical runtime emits
    /// for `CompactTypeUnsignedInteger(maxval, width).toValue(v)`.
    #[test]
    fn uint_fields_use_the_compilers_exact_byte_width() {
        // One above u64::MAX needs 65 bits, so 9 bytes, not the 16 of a u128.
        let av = encode_typed(
            &Value::Integer(7),
            &Type::Unsigned("18446744073709551616".parse().unwrap()),
        )
        .expect("in range");
        assert_eq!(
            av.alignment,
            Alignment(vec![midnight_base_crypto::fab::AlignmentSegment::Atom(
                AlignmentAtom::Bytes { length: 9 }
            )])
        );
    }

    #[test]
    fn each_point_native_reads_its_own_coordinate() {
        use midnight_transient_crypto::curve::{EmbeddedGroupAffine, Fr};
        let p = EmbeddedGroupAffine::generator() * Fr::from(11u64);
        let coordinate = |name: &str| match try_builtin_typed(
            name,
            &[Value::AlignedValue(AlignedValue::from(p))],
            &[],
        )
        .expect("builtin known")
        .expect("ok")
        {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("{name} returned {other:?}"),
        };
        assert_eq!(coordinate("jubjubPointX"), p.x().unwrap());
        assert_eq!(coordinate("jubjubPointY"), p.y().unwrap());
    }
}
