//! Compact runtime builtin circuits: hashes, commitments, elliptic-curve
//! operations, and field/bytes casts invoked by name during interpretation.

use midnight_typed_state::AlignedValue;

use crate::compact_types::encode_typed;
use crate::conversions::{value_to_embedded_group, value_to_fr, value_to_hash_output};
use crate::error::InterpreterError;
use crate::value::Value;
use compact_codegen::ir::Type;

/// Does this value need its declared type to encode at all?
///
/// Only `Value::Struct` does: its fields are written in declaration order at
/// their declared widths, neither of which the value itself carries. Everything
/// else has a correct type-free encoding via
/// [`Value::try_to_aligned_value`].
///
/// This gates how much of a builtin's argument list is encoded from inferred
/// types. The inferred type is *not* generally trustworthy: the portable IR
/// types every integer literal as `Field` regardless of its use site, and it
/// types `a + b` as `a`'s type because the widening casts Compact inserts are
/// erased before the IR is emitted. Encoding an integer through either would
/// move a digest that is correct today. A struct has no such prior encoding to
/// preserve, and the types that reach one (a circuit parameter, a witness
/// result) are declared rather than inferred, so this is the one case where
/// leaning on the inferred type is sound.
fn needs_declared_type(value: &Value) -> bool {
    match value {
        Value::Struct(_) => true,
        Value::Tuple(elements) => elements.iter().any(needs_declared_type),
        _ => false,
    }
}

/// The bytes that `persistentHash` and `keccak256` digest: the binary
/// representation of each argument, in order.
///
/// zkir-v3 builds the same bytes for its `PersistentHash` and `Keccak256`
/// instructions and changes only the digest, so both natives encode here.
fn hash_preimage(
    args: &[Value],
    encode_arg: impl Fn(usize, &Value) -> Result<AlignedValue, InterpreterError>,
) -> Result<Vec<u8>, InterpreterError> {
    use midnight_base_crypto::repr::BinaryHashRepr;
    use midnight_transient_crypto::curve::Fr;
    use midnight_transient_crypto::fab::ValueReprAlignedValue;

    let mut bytes = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        let av = match arg {
            // TODO: encode each argument at its parameter type in the native
            // that the call names (`Native::arguments`). This needs the
            // interpreter to key natives by full ident, not by source name.
            // Until then a bare integer hashes as a field element, which
            // differs from the chain for a `Uint<N>` argument.
            Value::Integer(n) => AlignedValue::from(Fr::from(*n)),
            other => encode_arg(i, other)?,
        };
        ValueReprAlignedValue(av).binary_repr(&mut bytes);
    }
    Ok(bytes)
}

/// Try to execute a Compact runtime builtin function.
/// Returns `Some(Ok(value))` if the function is a known builtin,
/// `Some(Err(..))` if it fails, or `None` if it's not a builtin.
pub fn try_builtin(name: &str, args: &[Value]) -> Option<Result<Value, InterpreterError>> {
    try_builtin_typed(name, args, &[])
}

/// Type-aware [`try_builtin`].
pub fn try_builtin_typed(
    name: &str,
    args: &[Value],
    arg_types: &[Option<Type>],
) -> Option<Result<Value, InterpreterError>> {
    // Encode one argument for hashing/committing.
    //
    // The declared type is consulted only for values that cannot be encoded
    // without it (see `needs_declared_type`); everything else keeps the
    // type-free encoding it has always had, so no digest that is correct today
    // moves. A failure propagates rather than falling back, because the
    // fallback would encode a struct as the *empty* value and the resulting
    // commitment would bind to nothing while still looking like a valid digest.
    let encode_arg = |i: usize, v: &Value| -> Result<AlignedValue, InterpreterError> {
        match arg_types.get(i).and_then(Option::as_ref) {
            Some(ty) if needs_declared_type(v) => encode_typed(v, ty),
            _ => v.try_to_aligned_value(),
        }
    };
    match name {
        "persistentCommit" => {
            // persistentCommit(value, opening) = persistent_commit(value, opening):
            // a domain-separated commitment. The opening is written to the
            // hasher first, then the value (see base-crypto `persistent_commit`).
            // Used to derive a contract's custom shielded token type:
            // `tokenType(domain_sep, self()) = persistentCommit((domain_sep,
            // self().bytes), "midnight:derive_token\0..")`. Matching the
            // on-chain derivation exactly is what lets a minted coin's color
            // line up with the recipient's wallet sync.
            use midnight_base_crypto::hash::{HashOutput, persistent_commit};
            use midnight_transient_crypto::fab::ValueReprAlignedValue;

            let value = match args.first() {
                Some(v) => v,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "persistentCommit expects (value, opening)".to_string(),
                    )));
                }
            };
            let opening = match args.get(1).map(value_to_hash_output) {
                Some(Ok(h)) => h,
                Some(Err(e)) => return Some(Err(e)),
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "persistentCommit expects an opening (domain separator) argument"
                            .to_string(),
                    )));
                }
            };
            // Flatten the value into a single AlignedValue and commit. A
            // `Value::Tuple` concatenates its elements in order; a struct is
            // encoded field-by-field at its declared widths when its type is in
            // scope, and is an error when it is not.
            let av = match encode_arg(0, value) {
                Ok(av) => av,
                Err(e) => return Some(Err(e)),
            };
            let wrapped = ValueReprAlignedValue(av);
            let hash: HashOutput = persistent_commit(&wrapped, opening);
            Some(Ok(Value::AlignedValue(AlignedValue::from(hash.0))))
        }
        "transientCommit" => {
            // transientCommit(value, opening): the Poseidon (transient-field)
            // counterpart of persistentCommit. Binds to transient-crypto's
            // `transient_commit`, so the value matches what the zkir/prover
            // computes rather than being reimplemented here.
            use midnight_transient_crypto::curve::Fr;
            use midnight_transient_crypto::fab::ValueReprAlignedValue;
            use midnight_transient_crypto::hash::transient_commit;

            let value = match args.first() {
                Some(v) => v,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "transientCommit expects (value, opening)".to_string(),
                    )));
                }
            };
            let opening = match args.get(1).and_then(value_to_fr) {
                Some(fr) => fr,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "transientCommit expects a Field opening argument".to_string(),
                    )));
                }
            };
            let av = match encode_arg(0, value) {
                Ok(av) => av,
                Err(e) => return Some(Err(e)),
            };
            let wrapped = ValueReprAlignedValue(av);
            let fr: Fr = transient_commit(&wrapped, opening);
            Some(Ok(Value::AlignedValue(AlignedValue::from(fr))))
        }
        "persistentHash" => {
            use midnight_base_crypto::hash::persistent_hash;
            Some(hash_preimage(args, encode_arg).map(|bytes| {
                let hash = persistent_hash(&bytes).0;
                Value::AlignedValue(AlignedValue::from(hash))
            }))
        }
        "keccak256" => {
            use sha3::{Digest, Keccak256};
            Some(hash_preimage(args, encode_arg).map(|bytes| {
                let hash: [u8; 32] = Keccak256::digest(&bytes).into();
                Value::AlignedValue(AlignedValue::from(hash))
            }))
        }
        "leafHash" => {
            let av = match args.first() {
                Some(Value::AlignedValue(av)) => av.clone(),
                Some(Value::Integer(n)) => {
                    use midnight_transient_crypto::curve::Fr;
                    // Exact u128 conversion — see `value_to_fr`.
                    AlignedValue::from(Fr::from(*n))
                }
                _ => {
                    return Some(Err(InterpreterError::TypeError(
                        "leafHash requires an AlignedValue or Integer argument".to_string(),
                    )));
                }
            };
            Some(Ok(Value::AlignedValue(crate::merkle_leaf_hash(av))))
        }
        "ecMulGenerator" | "__builtin_ec_mul_generator" => {
            // EC scalar multiplication: G * scalar
            use midnight_transient_crypto::curve::EmbeddedGroupAffine;
            if let Some(scalar) = args.first() {
                let fr_val = match value_to_fr(scalar) {
                    Some(fr) => fr,
                    None => {
                        return Some(Err(InterpreterError::TypeError(
                            "ecMulGenerator: scalar argument is not a Field/Integer".to_string(),
                        )));
                    }
                };
                let generator = EmbeddedGroupAffine::generator();
                let result = generator * fr_val;
                Some(Ok(Value::AlignedValue(AlignedValue::from(result))))
            } else {
                Some(Err(InterpreterError::TypeError(
                    "ecMulGenerator requires a scalar argument".to_string(),
                )))
            }
        }
        "ecMul" => {
            // EC scalar multiplication: point * scalar
            if args.len() != 2 {
                return Some(Err(InterpreterError::TypeError(format!(
                    "ecMul expects 2 arguments, got {}",
                    args.len()
                ))));
            }
            let point = match value_to_embedded_group(&args[0]) {
                Some(p) => p,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "ecMul: first argument is not a JubjubPoint".to_string(),
                    )));
                }
            };
            let scalar = match value_to_fr(&args[1]) {
                Some(s) => s,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "ecMul: second argument is not a Field/Integer".to_string(),
                    )));
                }
            };
            let result = point * scalar;
            Some(Ok(Value::AlignedValue(AlignedValue::from(result))))
        }
        "ecAdd" => {
            // EC point addition: p1 + p2
            if args.len() != 2 {
                return Some(Err(InterpreterError::TypeError(format!(
                    "ecAdd expects 2 arguments, got {}",
                    args.len()
                ))));
            }
            let p1 = match value_to_embedded_group(&args[0]) {
                Some(p) => p,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "ecAdd: first argument is not a JubjubPoint".to_string(),
                    )));
                }
            };
            let p2 = match value_to_embedded_group(&args[1]) {
                Some(p) => p,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "ecAdd: second argument is not a JubjubPoint".to_string(),
                    )));
                }
            };
            Some(Ok(Value::AlignedValue(AlignedValue::from(p1 + p2))))
        }
        "hashToCurve" => {
            // hashToCurve(value) -> JubjubPoint. Binds to transient-crypto's
            // `hash_to_curve` so the embedded-curve point matches the prover.
            use midnight_transient_crypto::fab::ValueReprAlignedValue;
            use midnight_transient_crypto::hash::hash_to_curve;
            let value = match args.first() {
                Some(v) => v,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "hashToCurve requires an argument".to_string(),
                    )));
                }
            };
            let av = match encode_arg(0, value) {
                Ok(av) => av,
                Err(e) => return Some(Err(e)),
            };
            let wrapped = ValueReprAlignedValue(av);
            let point = hash_to_curve(&wrapped);
            Some(Ok(Value::AlignedValue(AlignedValue::from(point))))
        }
        "jubjubPointX" => {
            // JubjubPoint -> Field (x coordinate)
            let point = match args.first().and_then(value_to_embedded_group) {
                Some(p) => p,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "jubjubPointX: argument is not a JubjubPoint".to_string(),
                    )));
                }
            };
            use midnight_transient_crypto::curve::Fr;
            let x = point.x().unwrap_or(Fr::from(0u64));
            Some(Ok(Value::AlignedValue(AlignedValue::from(x))))
        }
        "jubjubPointY" => {
            // JubjubPoint -> Field (y coordinate)
            let point = match args.first().and_then(value_to_embedded_group) {
                Some(p) => p,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "jubjubPointY: argument is not a JubjubPoint".to_string(),
                    )));
                }
            };
            use midnight_transient_crypto::curve::Fr;
            let y = point.y().unwrap_or(Fr::from(0u64));
            Some(Ok(Value::AlignedValue(AlignedValue::from(y))))
        }
        "constructJubjubPoint" => {
            // constructJubjubPoint(x, y) -> JubjubPoint. Binds to
            // EmbeddedGroupAffine::new, which returns None for an off-curve
            // (x, y) pair.
            use midnight_transient_crypto::curve::EmbeddedGroupAffine;
            if args.len() != 2 {
                return Some(Err(InterpreterError::TypeError(format!(
                    "constructJubjubPoint expects 2 arguments, got {}",
                    args.len()
                ))));
            }
            let x = match value_to_fr(&args[0]) {
                Some(fr) => fr,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "constructJubjubPoint: x is not a Field".to_string(),
                    )));
                }
            };
            let y = match value_to_fr(&args[1]) {
                Some(fr) => fr,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "constructJubjubPoint: y is not a Field".to_string(),
                    )));
                }
            };
            match EmbeddedGroupAffine::new(x, y) {
                Some(point) => Some(Ok(Value::AlignedValue(AlignedValue::from(point)))),
                None => Some(Err(InterpreterError::TypeError(
                    "constructJubjubPoint: (x, y) is not on the embedded curve".to_string(),
                ))),
            }
        }
        "transientHash" => {
            // transientHash(value): Poseidon over the value's field elements.
            // Binds to transient-crypto's `field_vec`, the same decomposition
            // `transientCommit` above hands to `transient_commit`, so the digest
            // matches the circuit rather than being reimplemented here. That
            // matters for anything wider than one field element: a struct
            // literal arrives as one multi-atom value, and a `Bytes<N>` atom
            // splits every 31 bytes.
            use midnight_transient_crypto::curve::Fr;
            use midnight_transient_crypto::fab::ValueReprAlignedValue;
            use midnight_transient_crypto::hash::transient_hash;
            use midnight_transient_crypto::repr::FieldRepr;

            let mut field_inputs: Vec<Fr> = Vec::with_capacity(args.len());
            for (i, arg) in args.iter().enumerate() {
                // Each argument is encoded whole: a Tuple concatenates its
                // elements, and a declared Vector or Struct type recurses into
                // them, so a struct nested in either keeps its own type.
                let av = match encode_arg(i, arg) {
                    Ok(av) => av,
                    Err(e) => return Some(Err(e)),
                };
                field_inputs.extend(ValueReprAlignedValue(av).field_vec());
            }
            let hash = transient_hash(&field_inputs);
            Some(Ok(Value::AlignedValue(AlignedValue::from(hash))))
        }
        "degradeToTransient" => {
            // Maps a persistent-field value (a 32-byte hash / Field) into the
            // transient field. This is the library `degrade_to_transient`, i.e.
            // `HashOutput::field_vec()[1]` — the low `FR_BYTES_STORED` (31) bytes
            // decoded as an `Fr`, dropping the top byte. It is deliberately *not*
            // a little-endian decode of all 32 bytes: those differ whenever the
            // 32nd byte is non-zero, and the on-chain circuit computes the former.
            use midnight_base_crypto::hash::HashOutput;
            use midnight_transient_crypto::hash::degrade_to_transient;
            let arg = match args.first() {
                Some(a) => a,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "degradeToTransient requires an argument".to_string(),
                    )));
                }
            };
            let bytes = match arg {
                Value::AlignedValue(av) => {
                    // Concatenate all atoms; for Bytes<N> this is a single atom.
                    let mut buf = Vec::new();
                    for atom in &av.value.0 {
                        buf.extend_from_slice(&atom.0);
                    }
                    buf
                }
                _ => {
                    return Some(Err(InterpreterError::TypeError(
                        "degradeToTransient: argument is not Bytes".to_string(),
                    )));
                }
            };
            let mut buf = [0u8; 32];
            let n = bytes.len().min(32);
            buf[..n].copy_from_slice(&bytes[..n]);
            let fr = degrade_to_transient(HashOutput(buf));
            Some(Ok(Value::AlignedValue(AlignedValue::from(fr))))
        }
        "upgradeFromTransient" => {
            // Field -> Bytes<32>: the inverse-direction companion of
            // degradeToTransient. Binds to transient-crypto's
            // `upgrade_from_transient`.
            use midnight_transient_crypto::hash::upgrade_from_transient;
            let fr = match args.first().and_then(value_to_fr) {
                Some(fr) => fr,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "upgradeFromTransient expects a Field argument".to_string(),
                    )));
                }
            };
            let hash = upgrade_from_transient(fr);
            Some(Ok(Value::AlignedValue(AlignedValue::from(hash.0))))
        }
        "pad" => {
            // pad(len, string) — pad a string to `len` bytes
            // Return as-is for now
            if args.len() >= 2 {
                Some(Ok(args[1].clone()))
            } else {
                Some(Ok(Value::Void))
            }
        }
        // Note: "disclose" is handled directly in eval_expr for CallWitness
        // and CallPure (before try_builtin is called) so that the disclosed
        // value is recorded in ctx.communication_outputs. This case is
        // unreachable from those paths but kept as a safety fallback for any
        // other call path that might invoke try_builtin with "disclose".
        "disclose" => {
            if let Some(arg) = args.first() {
                Some(Ok(arg.clone()))
            } else {
                Some(Ok(Value::Void))
            }
        }
        _ => None, // Not a builtin
    }
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

    /// The positional spelling of a `Point`.
    fn a_point() -> Value {
        Value::Tuple(vec![
            Value::Integer(0x1234_5678),
            Value::Bool(true),
            label_value(),
        ])
    }

    /// The named spelling of the same `Point`. This is the shape #119 is about:
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

    /// `struct Point { x: Uint<32>, flag: Boolean, label: Bytes<32> }`. The
    /// field widths are chosen so a wrong order or a wrong width shows up in
    /// the alignment alone.
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

    /// The `Value::Struct` path, pinned against the atoms the canonical runtime
    /// emits for this exact input (`tests/conformance/expected/structs/`):
    /// value `["78563412", "01", "1122..1122"]` at alignment
    /// `[Bytes{4}, Bytes{1}, Bytes{32}]`.
    #[test]
    fn named_struct_encodes_to_the_canonical_atoms() {
        let encoded = encode_typed(&a_point_struct(), &point_ty()).expect("Point encodes");

        assert_eq!(encoded.value.0.len(), 3, "one atom per field, no nesting");
        assert_eq!(encoded.value.0[0].0, vec![0x78, 0x56, 0x34, 0x12]);
        assert_eq!(encoded.value.0[1].0, vec![0x01]);
        assert_eq!(encoded.value.0[2].0, vec![0x11; 32]);
        assert_eq!(
            encoded.alignment,
            Alignment(vec![
                midnight_base_crypto::fab::AlignmentSegment::Atom(AlignmentAtom::Bytes {
                    length: 4
                }),
                midnight_base_crypto::fab::AlignmentSegment::Atom(AlignmentAtom::Bytes {
                    length: 1
                }),
                midnight_base_crypto::fab::AlignmentSegment::Atom(AlignmentAtom::Bytes {
                    length: 32
                }),
            ]),
        );
    }

    /// Field order comes from the declaration, so the two spellings of the same
    /// struct have to agree. A `HashMap` iterating in some other order would
    /// break this.
    #[test]
    fn named_and_positional_struct_spellings_agree() {
        assert_eq!(
            encode_typed(&a_point_struct(), &point_ty()).unwrap(),
            encode_typed(&a_point(), &point_ty()).unwrap(),
        );
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

        let wrong_arity =
            Value::Struct([("x".to_string(), Value::Integer(1))].into_iter().collect());
        assert!(encode_typed(&wrong_arity, &point_ty()).is_err());
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
            "persistentCommit",
            "transientCommit",
            "transientHash",
        ] {
            let args = [a_point_struct(), Value::Integer(0)];
            assert!(
                matches!(try_builtin(name, &args), Some(Err(_))),
                "{name} must reject an untyped struct"
            );
        }
    }

    /// A `Uint` bound encodes at the compiler's `ceil(bits/8)` width, which is
    /// not always a primitive size. Pinned to what the canonical runtime emits
    /// for `CompactTypeUnsignedInteger(maxval, width).toValue(v)`.
    #[test]
    fn uint_fields_use_the_compilers_exact_byte_width() {
        let enc = |maxval: &str, n: u128| {
            encode_typed(&Value::Integer(n), &Type::Unsigned(maxval.parse().unwrap()))
                .expect("in range")
        };

        // Uint<24>: 3 bytes, not the 4 a u8/u16/u32 ladder would pick.
        let av = enc("16777215", 0x0012_3456);
        assert_eq!(av.value.0[0].0, vec![0x56, 0x34, 0x12]);
        assert_eq!(
            av.alignment,
            Alignment(vec![midnight_base_crypto::fab::AlignmentSegment::Atom(
                AlignmentAtom::Bytes { length: 3 }
            )])
        );

        // Uint<48>: 6 bytes, not 8.
        let av = enc("281474976710655", 0x0000_1234_5678_9abc);
        assert_eq!(av.value.0[0].0, vec![0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12]);

        // A range bound rather than a power of two: Uint<0..1000000> is 3 bytes.
        let av = enc("999999", 999_999);
        assert_eq!(av.value.0[0].0, vec![0x3f, 0x42, 0x0f]);

        // The alignment is the exact width, on or off a primitive size. One
        // above u64::MAX needs 65 bits, so 9 bytes, not the 16 of a u128.
        for (maxval, n, want) in [
            ("255", 7u128, 1usize),
            ("65535", 7, 2),
            ("4294967295", 7, 4),
            ("18446744073709551615", 7, 8),
            ("18446744073709551616", 7, 9),
        ] {
            let av = enc(maxval, n);
            assert_eq!(
                av.alignment,
                Alignment(vec![midnight_base_crypto::fab::AlignmentSegment::Atom(
                    AlignmentAtom::Bytes {
                        length: want as u32
                    }
                )]),
                "Uint maxval {maxval} should be {want} bytes"
            );
        }
    }

    /// A composite-typed value can arrive already flattened, e.g. sliced out of
    /// a struct receiver. That spelling encodes correctly and must not be
    /// rejected just because the declared type is a `Vector` or `Tuple`.
    #[test]
    fn composite_types_accept_an_already_flat_value() {
        let flat = AlignedValue::concat([AlignedValue::from([1u8; 32]), label_value_av()].iter());

        let vector_ty = Type::Vector {
            len: 2,
            ty: Box::new(Type::Bytes(32)),
        };
        assert_eq!(
            encode_typed(&Value::AlignedValue(flat.clone()), &vector_ty).unwrap(),
            flat
        );

        let tuple_ty = Type::Tuple(vec![Type::Bytes(32), Type::Bytes(32)]);
        assert_eq!(
            encode_typed(&Value::AlignedValue(flat.clone()), &tuple_ty).unwrap(),
            flat
        );

        // And it still reaches the builtins rather than aborting the circuit.
        let types = vec![Some(vector_ty)];
        assert!(matches!(
            try_builtin_typed("persistentHash", &[Value::AlignedValue(flat)], &types,),
            Some(Ok(_))
        ));
    }

    /// Only a struct is encoded through the inferred type. Integers keep the
    /// field-element encoding they have always had, because the portable IR
    /// types every literal `Field` and erases arithmetic widening, so trusting
    /// the inferred type would move digests that are correct today.
    #[test]
    fn only_structs_are_encoded_through_the_inferred_type() {
        let uint_ty = vec![Some(Type::Unsigned("65535".parse().unwrap()))];

        let typed = try_builtin_typed("persistentHash", &[Value::Integer(7)], &uint_ty);
        let untyped = try_builtin("persistentHash", &[Value::Integer(7)]);
        match (typed, untyped) {
            (Some(Ok(Value::AlignedValue(a))), Some(Ok(Value::AlignedValue(b)))) => {
                assert_eq!(a, b, "an integer's digest must not depend on the arg type")
            }
            other => panic!("unexpected: {other:?}"),
        }

        assert!(!needs_declared_type(&Value::Integer(7)));
        assert!(!needs_declared_type(&a_point()));
        assert!(needs_declared_type(&a_point_struct()));
        assert!(needs_declared_type(&Value::Tuple(vec![
            Value::Integer(1),
            a_point_struct()
        ])));
    }

    /// `StateValue::Cell` wraps exactly one `AlignedValue`, so unwrapping it is
    /// the encoding. The container variants have none.
    #[test]
    fn state_values_encode_only_as_cells() {
        use midnight_typed_state::StateValue;

        let inner = AlignedValue::from(7u64);
        let cell = Value::StateValue(StateValue::from(inner.clone()));
        assert_eq!(
            cell.try_to_aligned_value().unwrap(),
            inner,
            "no longer discarded"
        );

        let null = Value::StateValue(StateValue::Null);
        assert!(null.try_to_aligned_value().is_err());
        assert!(
            encode_typed(&null, &Type::Field(compact_codegen::ir::FieldType::Native),).is_err(),
            "a container state value has no aligned encoding at any type"
        );
    }

    #[test]
    fn each_point_native_reads_its_own_coordinate() {
        use midnight_transient_crypto::curve::{EmbeddedGroupAffine, Fr};
        let p = EmbeddedGroupAffine::generator() * Fr::from(11u64);
        let coordinate =
            |name: &str| match try_builtin(name, &[Value::AlignedValue(AlignedValue::from(p))])
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
