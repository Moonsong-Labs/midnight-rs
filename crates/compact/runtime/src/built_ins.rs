//! Compact runtime builtin circuits: hashes, commitments, elliptic-curve
//! operations, and field/bytes casts invoked by name during interpretation.

use midnight_typed_state::AlignedValue;

use crate::compact_types::encode_typed_with_defs;
use crate::conversions::{value_to_embedded_group, value_to_fr, value_to_hash_output};
use crate::error::InterpreterError;
use crate::value::Value;
use compact_codegen::ir::{StructDef, TypeRef};

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

/// Try to execute a Compact runtime builtin function.
/// Returns `Some(Ok(value))` if the function is a known builtin,
/// `Some(Err(..))` if it fails, or `None` if it's not a builtin.
pub fn try_builtin(name: &str, args: &[Value]) -> Option<Result<Value, InterpreterError>> {
    try_builtin_typed(name, args, &[], &std::collections::HashMap::new())
}

/// Type-aware [`try_builtin`].
pub fn try_builtin_typed(
    name: &str,
    args: &[Value],
    arg_types: &[Option<TypeRef>],
    struct_defs: &std::collections::HashMap<String, StructDef>,
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
            Some(ty) if needs_declared_type(v) => encode_typed_with_defs(v, ty, struct_defs),
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
            // persistentHash hashes an AlignedValue using midnight-ledger's
            // PersistentHashWriter with proper binary_repr.
            use midnight_base_crypto::hash::PersistentHashWriter;
            use midnight_base_crypto::repr::BinaryHashRepr;
            use midnight_transient_crypto::fab::ValueReprAlignedValue;

            let mut hasher = PersistentHashWriter::default();
            for (i, arg) in args.iter().enumerate() {
                // A struct is the one shape that needs its declared type; every
                // other variant keeps the encoding it already had.
                if needs_declared_type(arg) {
                    let av = match encode_arg(i, arg) {
                        Ok(av) => av,
                        Err(e) => return Some(Err(e)),
                    };
                    ValueReprAlignedValue(av).binary_repr(&mut hasher);
                    continue;
                }
                let av = match arg {
                    // Hash integers as a field element rather than at the
                    // width-preserving fallback. This does not match what the
                    // chain computes for a declared `Uint<N>` argument, but the
                    // portable IR carries no type argument for a builtin call
                    // and its inferred alternatives are unreliable, so changing
                    // it would trade one wrong digest for another. Tracked
                    // separately from the struct encoding.
                    Value::Integer(n) => {
                        use midnight_transient_crypto::curve::Fr;
                        AlignedValue::from(Fr::from(*n))
                    }
                    // Flattens a tuple's elements in order, which is the rule
                    // the bindgen-emitted `Into<AlignedValue>` impls use, so it
                    // matches what the on-chain persistent_hash circuit produces
                    // for the same typed input.
                    other => match other.try_to_aligned_value() {
                        Ok(av) => av,
                        Err(e) => return Some(Err(e)),
                    },
                };
                ValueReprAlignedValue(av).binary_repr(&mut hasher);
            }
            let hash = hasher.finalize();
            Some(Ok(Value::AlignedValue(AlignedValue::from(hash.0))))
        }
        "leafHash" => {
            // leafHash uses midnight-ledger's merkle tree leaf hashing
            use midnight_transient_crypto::fab::ValueReprAlignedValue;
            match args.first() {
                Some(Value::AlignedValue(av)) => {
                    let wrapped = ValueReprAlignedValue(av.clone());
                    let hash = midnight_transient_crypto::merkle_tree::leaf_hash(&wrapped);
                    Some(Ok(Value::AlignedValue(AlignedValue::from(hash.0))))
                }
                Some(Value::Integer(n)) => {
                    use midnight_transient_crypto::curve::Fr;
                    // Exact u128 conversion — see `value_to_fr`.
                    let av = AlignedValue::from(Fr::from(*n));
                    let wrapped = ValueReprAlignedValue(av);
                    let hash = midnight_transient_crypto::merkle_tree::leaf_hash(&wrapped);
                    Some(Ok(Value::AlignedValue(AlignedValue::from(hash.0))))
                }
                _ => Some(Err(InterpreterError::TypeError(
                    "leafHash requires an AlignedValue or Integer argument".to_string(),
                ))),
            }
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
            // Poseidon hash: transientHash<Vector<N, Field>>([fields...]) -> Field
            use midnight_transient_crypto::curve::Fr;
            use midnight_transient_crypto::hash::transient_hash;
            let mut field_inputs: Vec<Fr> = Vec::with_capacity(args.len());
            for (i, arg) in args.iter().enumerate() {
                // The IR sometimes passes a single Tuple wrapping all the fields.
                // Flatten one level so callers can pass either a flat arg list or
                // a single Tuple.
                if let Value::Tuple(elems) = arg {
                    for (j, e) in elems.iter().enumerate() {
                        match value_to_fr(e) {
                            Some(fr) => field_inputs.push(fr),
                            None => {
                                return Some(Err(InterpreterError::TypeError(format!(
                                    "transientHash: tuple arg {i} elem {j} is not a Field"
                                ))));
                            }
                        }
                    }
                } else {
                    match value_to_fr(arg) {
                        Some(fr) => field_inputs.push(fr),
                        None => {
                            return Some(Err(InterpreterError::TypeError(format!(
                                "transientHash: arg {i} is not a Field"
                            ))));
                        }
                    }
                }
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
    use compact_codegen::ir::StructField;
    use midnight_base_crypto::fab::{Alignment, AlignmentAtom};

    /// `struct Point { x: Uint<32>, flag: Boolean, label: Bytes<32> }`. The
    /// field widths are chosen so a wrong order or a wrong width shows up in
    /// the alignment alone.
    fn point_defs() -> std::collections::HashMap<String, StructDef> {
        let def = StructDef {
            name: "Point".to_string(),
            fields: vec![
                StructField {
                    name: "x".to_string(),
                    ty: TypeRef::Uint {
                        maxval: "4294967295".to_string(),
                    },
                },
                StructField {
                    name: "flag".to_string(),
                    ty: TypeRef::Boolean,
                },
                StructField {
                    name: "label".to_string(),
                    ty: TypeRef::Bytes { length: 32 },
                },
            ],
        };
        std::iter::once((def.name.clone(), def)).collect()
    }

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

    fn point_ty() -> TypeRef {
        TypeRef::Struct {
            name: "Point".to_string(),
        }
    }

    /// A struct reaching a commit/hash builtin must be flattened as the concat
    /// of its fields' encodings in declaration order, each at its declared
    /// width. That is the rule the canonical runtime's generated per-struct
    /// descriptor applies (`toValue`/`alignment` concat the field descriptors
    /// in order).
    /// It used to encode as the empty value, so the commitment bound to
    /// nothing. See #119.
    #[test]
    fn struct_flattens_in_declaration_order_at_declared_widths() {
        let defs = point_defs();
        let ty = Some(TypeRef::Struct {
            name: "Point".to_string(),
        });
        let encoded = encode_typed_with_defs(&a_point(), ty.as_ref().unwrap(), &defs).unwrap();

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
            "declaration order x:Uint<32>, flag:Boolean, label:Bytes<32>"
        );
        assert_eq!(encoded.value.0.len(), 3, "one atom per field");
        assert_eq!(
            encoded.value.0[0].0,
            vec![0x78, 0x56, 0x34, 0x12],
            "x is a 4-byte little-endian atom, not the 8-byte untyped fallback"
        );
    }

    /// The commit/hash builtins must consume that flattening rather than the
    /// untyped `to_aligned_value` fallback. `persistentCommit` is the
    /// observable case: `persistent_hash` zero-pads each atom to its declared
    /// width, so a `Uint<32>` field encoded at the untyped 8-byte fallback
    /// width commits to a different digest. (`transientCommit` reduces each
    /// atom to a field element, which is width-insensitive, so it is pinned
    /// positively against the canonical encoding instead.)
    #[test]
    fn commit_builtins_bind_to_the_typed_struct_encoding() {
        use midnight_transient_crypto::fab::ValueReprAlignedValue;

        let defs = point_defs();
        let point_ty = TypeRef::Struct {
            name: "Point".to_string(),
        };
        let types = vec![Some(point_ty.clone()), None];

        let typed = try_builtin_typed(
            "persistentCommit",
            &[a_point_struct(), Value::Integer(0)],
            &types,
            &defs,
        )
        .unwrap()
        .unwrap();
        let untyped = try_builtin("persistentCommit", &[a_point(), Value::Integer(0)])
            .unwrap()
            .unwrap();
        match (&typed, &untyped) {
            (Value::AlignedValue(a), Value::AlignedValue(b)) => assert_ne!(
                a, b,
                "persistentCommit must use the declared struct layout, not the untyped fallback"
            ),
            other => panic!("persistentCommit returned {other:?}"),
        }

        // transientCommit binds to the same canonical flattening.
        let canonical = encode_typed_with_defs(&a_point_struct(), &point_ty, &defs).unwrap();
        let expected = midnight_transient_crypto::hash::transient_commit(
            &ValueReprAlignedValue(canonical),
            midnight_transient_crypto::curve::Fr::from(0u64),
        );
        let got = try_builtin_typed(
            "transientCommit",
            &[a_point_struct(), Value::Integer(0)],
            &types,
            &defs,
        )
        .unwrap()
        .unwrap();
        match got {
            Value::AlignedValue(av) => assert_eq!(av, AlignedValue::from(expected)),
            other => panic!("transientCommit returned {other:?}"),
        }
    }

    /// The `Value::Struct` path, pinned against the atoms the canonical runtime
    /// emits for this exact input (`tests/conformance/expected/structs/`):
    /// value `["78563412", "01", "1122..1122"]` at alignment
    /// `[Bytes{4}, Bytes{1}, Bytes{32}]`.
    #[test]
    fn named_struct_encodes_to_the_canonical_atoms() {
        let encoded = encode_typed_with_defs(&a_point_struct(), &point_ty(), &point_defs())
            .expect("Point encodes");

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
        let defs = point_defs();
        assert_eq!(
            encode_typed_with_defs(&a_point_struct(), &point_ty(), &defs).unwrap(),
            encode_typed_with_defs(&a_point(), &point_ty(), &defs).unwrap(),
        );
    }

    /// The regression this whole issue is about: a struct used to flatten to the
    /// empty value, so hashing one produced the digest of *nothing*. That made
    /// it indistinguishable from hashing `Void`, and every distinct struct
    /// collided with every other.
    #[test]
    fn hashing_a_struct_is_not_hashing_nothing() {
        let defs = point_defs();
        let types = vec![Some(point_ty())];

        let hash = |v: Value, tys: &[Option<TypeRef>]| match try_builtin_typed(
            "persistentHash",
            &[v],
            tys,
            &defs,
        )
        .expect("persistentHash is a builtin")
        .expect("encodes")
        {
            Value::AlignedValue(av) => av,
            other => panic!("persistentHash returned {other:?}"),
        };

        let void_digest = hash(Value::Void, &[]);
        assert_ne!(hash(a_point_struct(), &types), void_digest);

        // The positional spelling is NOT expected to agree here. It does not
        // need its declared type to encode, so it keeps the width-preserving
        // type-free encoding rather than the struct's declared field widths.
        // Encoding it at the inferred type instead would move digests that are
        // correct today, for the reasons in `needs_declared_type`.
        assert_ne!(hash(a_point_struct(), &types), hash(a_point(), &types));

        // A different field value must move the digest.
        let other = Value::Struct(
            [
                ("x".to_string(), Value::Integer(0x1234_5679)),
                ("flag".to_string(), Value::Bool(true)),
                ("label".to_string(), label_value()),
            ]
            .into_iter()
            .collect(),
        );
        assert_ne!(hash(other, &types), hash(a_point_struct(), &types));
    }

    /// A struct that does not match its declaration is an error, never a
    /// partial or empty encoding.
    #[test]
    fn malformed_structs_are_rejected() {
        let defs = point_defs();

        let missing_field = Value::Struct(
            [
                ("x".to_string(), Value::Integer(1)),
                ("flag".to_string(), Value::Bool(true)),
                ("nope".to_string(), label_value()),
            ]
            .into_iter()
            .collect(),
        );
        assert!(encode_typed_with_defs(&missing_field, &point_ty(), &defs).is_err());

        let wrong_arity =
            Value::Struct([("x".to_string(), Value::Integer(1))].into_iter().collect());
        assert!(encode_typed_with_defs(&wrong_arity, &point_ty(), &defs).is_err());

        // No definition for the named struct.
        assert!(
            encode_typed_with_defs(
                &a_point_struct(),
                &point_ty(),
                &std::collections::HashMap::new()
            )
            .is_err()
        );
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

        for name in ["persistentHash", "persistentCommit", "transientCommit"] {
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
        let defs = std::collections::HashMap::new();
        let enc = |maxval: &str, n: u128| {
            encode_typed_with_defs(
                &Value::Integer(n),
                &TypeRef::Uint {
                    maxval: maxval.to_string(),
                },
                &defs,
            )
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

        // Bounds that do land on a primitive width are unchanged.
        for (maxval, n, want) in [
            ("255", 7u128, 1usize),
            ("65535", 7, 2),
            ("4294967295", 7, 4),
            ("18446744073709551615", 7, 8),
        ] {
            let av = enc(maxval, n);
            assert_eq!(
                av.alignment,
                Alignment(vec![midnight_base_crypto::fab::AlignmentSegment::Atom(
                    AlignmentAtom::Bytes {
                        length: want as u32
                    }
                )]),
                "Uint maxval {maxval} should stay {want} bytes"
            );
        }
    }

    /// A composite-typed value can arrive already flattened, e.g. sliced out of
    /// a struct receiver. That spelling encodes correctly and must not be
    /// rejected just because the declared type is a `Vector` or `Tuple`.
    #[test]
    fn composite_types_accept_an_already_flat_value() {
        let defs = std::collections::HashMap::new();
        let flat = AlignedValue::concat([AlignedValue::from([1u8; 32]), label_value_av()].iter());

        let vector_ty = TypeRef::Vector {
            length: 2,
            element: Box::new(TypeRef::Bytes { length: 32 }),
        };
        assert_eq!(
            encode_typed_with_defs(&Value::AlignedValue(flat.clone()), &vector_ty, &defs).unwrap(),
            flat
        );

        let tuple_ty = TypeRef::Tuple {
            types: vec![TypeRef::Bytes { length: 32 }, TypeRef::Bytes { length: 32 }],
        };
        assert_eq!(
            encode_typed_with_defs(&Value::AlignedValue(flat.clone()), &tuple_ty, &defs).unwrap(),
            flat
        );

        // And it still reaches the builtins rather than aborting the circuit.
        let types = vec![Some(vector_ty)];
        assert!(matches!(
            try_builtin_typed(
                "persistentHash",
                &[Value::AlignedValue(flat)],
                &types,
                &defs
            ),
            Some(Ok(_))
        ));
    }

    /// Only a struct is encoded through the inferred type. Integers keep the
    /// field-element encoding they have always had, because the portable IR
    /// types every literal `Field` and erases arithmetic widening, so trusting
    /// the inferred type would move digests that are correct today.
    #[test]
    fn only_structs_are_encoded_through_the_inferred_type() {
        let defs = point_defs();
        let uint_ty = vec![Some(TypeRef::Uint {
            maxval: "65535".to_string(),
        })];

        let typed = try_builtin_typed("persistentHash", &[Value::Integer(7)], &uint_ty, &defs);
        let untyped = try_builtin("persistentHash", &[Value::Integer(7)]);
        match (typed, untyped) {
            (Some(Ok(Value::AlignedValue(a))), Some(Ok(Value::AlignedValue(b)))) => {
                assert_eq!(a, b, "an integer's digest must not depend on the arg type")
            }
            other => panic!("unexpected: {other:?}"),
        }

        assert!(!needs_declared_type(&Value::Integer(7)));
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
            encode_typed_with_defs(&null, &TypeRef::Field, &point_defs()).is_err(),
            "a container state value has no aligned encoding at any type"
        );
    }
}
