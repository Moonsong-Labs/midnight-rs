//! Compact runtime builtin circuits: hashes, commitments, elliptic-curve
//! operations, and field/bytes casts invoked by name during interpretation.

use midnight_bindgen_runtime::AlignedValue;

use crate::conversions::{value_to_embedded_group, value_to_fr, value_to_hash_output};
use crate::error::InterpreterError;
use crate::value::Value;

/// Reject value shapes that [`Value::to_aligned_value`] encodes to an empty
/// value, which would make a commitment or hash silently bind to nothing.
/// Structs and on-chain state values must reach the commit/hash builtins
/// pre-encoded as [`Value::AlignedValue`] (via the type-aware encoder). The
/// interpreter never produces a `Value::Struct` at runtime today, so this only
/// guards future changes from a silent-wrong result. See issue #119.
fn ensure_encodable(value: &Value, builtin: &str) -> Result<(), InterpreterError> {
    if matches!(value, Value::Struct(_) | Value::StateValue(_)) {
        return Err(InterpreterError::TypeError(format!(
            "{builtin}: struct and state values must be pre-encoded as an AlignedValue"
        )));
    }
    Ok(())
}

/// Try to execute a Compact runtime builtin function.
/// Returns `Some(Ok(value))` if the function is a known builtin,
/// `Some(Err(..))` if it fails, or `None` if it's not a builtin.
pub fn try_builtin(name: &str, args: &[Value]) -> Option<Result<Value, InterpreterError>> {
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
            if let Err(e) = ensure_encodable(value, "persistentCommit") {
                return Some(Err(e));
            }
            // Flatten the value into a single AlignedValue (a `Value::Tuple` is
            // walked in declaration order; structs arrive already encoded as an
            // AlignedValue) and commit.
            let wrapped = ValueReprAlignedValue(value.to_aligned_value());
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
            if let Err(e) = ensure_encodable(value, "transientCommit") {
                return Some(Err(e));
            }
            let wrapped = ValueReprAlignedValue(value.to_aligned_value());
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
            for arg in args {
                match arg {
                    Value::AlignedValue(av) => {
                        let wrapped = ValueReprAlignedValue(av.clone());
                        wrapped.binary_repr(&mut hasher);
                    }
                    Value::Integer(n) => {
                        // Use Fr for field-compatible hashing. Exact u128
                        // conversion — see `value_to_fr`.
                        use midnight_transient_crypto::curve::Fr;
                        let av = AlignedValue::from(Fr::from(*n));
                        let wrapped = ValueReprAlignedValue(av);
                        wrapped.binary_repr(&mut hasher);
                    }
                    Value::Bool(b) => {
                        let av = AlignedValue::from(*b);
                        let wrapped = ValueReprAlignedValue(av);
                        wrapped.binary_repr(&mut hasher);
                    }
                    Value::Void => {
                        let av = AlignedValue::from(());
                        let wrapped = ValueReprAlignedValue(av);
                        wrapped.binary_repr(&mut hasher);
                    }
                    Value::Tuple(_) => {
                        // Flatten the tuple into a single `AlignedValue`
                        // (Value::to_aligned_value concatenates each leaf's
                        // atoms in declaration order) and binary_repr it. This
                        // matches what the on-chain persistent_hash circuit
                        // produces for the same typed input, because the same
                        // flattening rule is used by the bindgen-emitted
                        // `Into<AlignedValue>` impls.
                        let av = arg.to_aligned_value();
                        let wrapped = ValueReprAlignedValue(av);
                        wrapped.binary_repr(&mut hasher);
                    }
                    Value::Struct(_) | Value::StateValue(_) => {
                        // These encode to an empty AlignedValue, so hashing them
                        // would silently bind to nothing; reject instead. See
                        // the note on `ensure_encodable`.
                        return Some(Err(InterpreterError::TypeError(
                            "persistentHash: struct and state values must be pre-encoded as an AlignedValue"
                                .to_string(),
                        )));
                    }
                }
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
            let wrapped = ValueReprAlignedValue(value.to_aligned_value());
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
    use std::collections::HashMap;

    fn a_struct() -> Value {
        let mut fields = HashMap::new();
        fields.insert("x".to_string(), Value::Integer(1));
        Value::Struct(fields)
    }

    #[test]
    fn persistent_hash_rejects_struct() {
        // A struct encodes to an empty AlignedValue; hashing it must fail loudly
        // rather than bind to nothing. See `ensure_encodable` and issue #119.
        let r = try_builtin("persistentHash", &[a_struct()]);
        assert!(matches!(r, Some(Err(InterpreterError::TypeError(_)))));
    }

    #[test]
    fn persistent_commit_rejects_struct() {
        let opening = Value::Integer(0);
        let r = try_builtin("persistentCommit", &[a_struct(), opening]);
        assert!(matches!(r, Some(Err(InterpreterError::TypeError(_)))));
    }

    #[test]
    fn transient_commit_rejects_struct() {
        let opening = Value::Integer(0);
        let r = try_builtin("transientCommit", &[a_struct(), opening]);
        assert!(matches!(r, Some(Err(InterpreterError::TypeError(_)))));
    }
}
