//! Type-aware FAB encoding: the on-chain aligned-value layout for each
//! Compact `TypeRef`, plus the struct-layout machinery used to slice
//! `Value::AlignedValue` receivers by field. The Rust counterpart of
//! Minokawa's `compact-types`.

use std::collections::HashMap;

use midnight_bindgen_runtime::AlignedValue;

use compact_codegen::ir::{StructDef, TypeRef};

use crate::conversions::{aligned_atom_to_u128, value_to_u128};
use crate::error::InterpreterError;
use crate::value::Value;

/// Precomputed layout of a struct: field name → (atom offset, atom count).
#[derive(Debug, Clone)]
pub struct StructLayout {
    /// Declaration-order list of (field name, offset, length) in atom slots.
    fields: Vec<(String, usize, usize)>,
}

impl StructLayout {
    pub fn field_slice(&self, name: &str) -> Option<(usize, usize)> {
        self.fields
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, o, l)| (*o, *l))
    }
}

/// Compute the number of FAB atoms a `TypeRef` occupies in an `AlignedValue`
/// encoding. Used to build struct layouts so `Expr::Field` can slice
/// `Value::AlignedValue` receivers by offset/length.
fn atom_count_for_type(ty: &TypeRef, layouts: &HashMap<String, StructLayout>) -> Option<usize> {
    match ty {
        TypeRef::Boolean | TypeRef::Uint { .. } | TypeRef::Field | TypeRef::Bytes { .. } => Some(1),
        TypeRef::Void => Some(0),
        TypeRef::Opaque { name } => match name.as_str() {
            "JubjubPoint" => Some(2),
            "Scalar<BLS12-381>" => Some(1),
            _ => Some(1),
        },
        TypeRef::Tuple { types } => {
            let mut total = 0;
            for t in types {
                total += atom_count_for_type(t, layouts)?;
            }
            Some(total)
        }
        TypeRef::Vector { length, element } => {
            let per = atom_count_for_type(element, layouts)?;
            Some(per * length)
        }
        TypeRef::Struct { name } => layouts
            .get(name)
            .map(|l| l.fields.iter().map(|(_, _, len)| *len).sum()),
        TypeRef::Maybe { inner } => atom_count_for_type(inner, layouts).map(|n| 1 + n),
        TypeRef::Enum { .. } => Some(1),
    }
}

/// Build struct layouts from shipped `StructDef` entries. Structs may
/// reference each other, so we iterate until fixed point (bounded by the
/// number of structs).
pub fn build_struct_layouts(defs: &[StructDef]) -> HashMap<String, StructLayout> {
    let mut layouts: HashMap<String, StructLayout> = HashMap::new();
    let max_passes = defs.len() + 1;
    for _ in 0..max_passes {
        let mut made_progress = false;
        for def in defs {
            if layouts.contains_key(&def.name) {
                continue;
            }
            let mut fields = Vec::with_capacity(def.fields.len());
            let mut offset = 0usize;
            let mut ok = true;
            for f in &def.fields {
                match atom_count_for_type(&f.ty, &layouts) {
                    Some(len) => {
                        fields.push((f.name.clone(), offset, len));
                        offset += len;
                    }
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                layouts.insert(def.name.clone(), StructLayout { fields });
                made_progress = true;
            }
        }
        if !made_progress {
            break;
        }
    }
    layouts
}

/// Parse a `Uint{maxval}` bound, check `n` against it, and return the parsed
/// bound so callers (e.g. [`encode_typed`]'s width ladder) reuse it instead of
/// re-parsing with a default that could drift from this one.
///
/// `maxval` is the decimal bound string shipped in the IR. Bounds wider than
/// `u128` fail to parse and are capped at `u128::MAX` — `Value::Integer`
/// cannot hold anything larger, so every representable value is in range.
pub fn check_uint_range(n: u128, maxval: &str) -> Result<u128, InterpreterError> {
    let max: u128 = maxval.parse().unwrap_or(u128::MAX);
    if n > max {
        return Err(InterpreterError::TypeError(format!(
            "integer {n} out of range for Uint with maxval {max}"
        )));
    }
    Ok(max)
}

/// Build a single-atom `AlignedValue` with `Bytes<length>` alignment from raw
/// bytes, trimming trailing zeros to satisfy the FAB normal-form invariant
/// (`is_in_normal_form`). The alignment metadata still records `length = N`
/// so equality against zero-padded constants works.
pub fn bytes_aligned_value(
    bytes: Vec<u8>,
    length: usize,
) -> Result<AlignedValue, InterpreterError> {
    use midnight_base_crypto::fab;
    let byte_len = bytes.len();
    let mut atom = bytes;
    while matches!(atom.last(), Some(0)) {
        atom.pop();
    }
    fab::AlignedValue::new(
        fab::Value(vec![fab::ValueAtom(atom)]),
        fab::Alignment::singleton(fab::AlignmentAtom::Bytes {
            length: length as u32,
        }),
    )
    .ok_or_else(|| {
        InterpreterError::TypeError(format!(
            "{byte_len} bytes do not fit a Bytes<{length}> alignment"
        ))
    })
}

/// Encode a runtime [`Value`] as an [`AlignedValue`] whose alignment matches
/// the declared [`TypeRef`]. This is the single type-aware FAB encoder:
/// `Expr::New` struct fields, ledger cell/key pushes ([`encode_ledger_key`]),
/// literal path keys ([`path_value_to_aligned`]) and `Idx` path variables all
/// route through here, so a new `TypeRef` variant only needs handling in one
/// place.
///
/// # Why the width matters
///
/// FAB atoms are zero-trimmed little-endian bytes, so the atom for a given
/// number is identical at every width; the declared width lives only in the
/// `AlignmentAtom::Bytes { length }`. That alignment participates in
/// `AlignedValue` equality/hashing (on-chain `Map` lookups compare the full
/// `AlignedValue`) and in `persistentHash`, which zero-pads each atom to the
/// declared width. The width ladder below (u8/u16/u32/u64/u128) must
/// therefore match the bindgen-emitted encoders (`uint_tokens` in
/// compact-codegen) byte-for-byte.
///
/// For `Value::Integer`, this picks the right number of bytes from the
/// target `Uint{maxval}` width — `Value::Integer(1000)` embedded as
/// `Uint<128>` becomes a 16-byte atom, not the 8-byte default
/// `to_aligned_value` would produce. Integers that exceed the declared bound
/// (e.g. 300 for `Uint{maxval: 255}`) are an error, never a silent wrap.
pub fn encode_typed(val: &Value, ty: &TypeRef) -> Result<AlignedValue, InterpreterError> {
    use midnight_base_crypto::fab;
    let unsupported =
        || InterpreterError::TypeError(format!("cannot encode value {val:?} as type {ty:?}"));
    match ty {
        TypeRef::Boolean => match val {
            Value::Bool(b) => Ok(AlignedValue::from(*b)),
            Value::Integer(n) => Ok(AlignedValue::from(*n != 0)),
            // A Boolean sliced out of a struct (e.g. `recipient.is_left`)
            // arrives as a single-byte `AlignedValue` (0x00/0x01); re-encode
            // it as a Boolean so it can flow into another struct field.
            Value::AlignedValue(av) => aligned_atom_to_u128(av)
                .map(|n| AlignedValue::from(n != 0))
                .ok_or_else(unsupported),
            _ => Err(unsupported()),
        },
        TypeRef::Uint { maxval } => {
            let n = value_to_u128(val).ok_or_else(unsupported)?;
            let max = check_uint_range(n, maxval)?;
            // Choose the smallest standard primitive width >= max so that
            // the alignment matches what the on-chain runtime expects.
            // (`From<u8/u16/u32/u64/u128>` set the alignment via `Aligned`.)
            // The `as` casts are lossless: `check_uint_range` guarantees
            // `n <= max`, and each branch requires `max` to fit the width.
            if max <= u8::MAX as u128 {
                Ok(AlignedValue::from(n as u8))
            } else if max <= u16::MAX as u128 {
                Ok(AlignedValue::from(n as u16))
            } else if max <= u32::MAX as u128 {
                Ok(AlignedValue::from(n as u32))
            } else if max <= u64::MAX as u128 {
                Ok(AlignedValue::from(n as u64))
            } else {
                // Bounds above u128 also land here (bindgen's `uint_tokens` falls back to
                // `Vec<u8>` instead, so the byte-for-byte claim above does not cover them),
                // but `Value::Integer` is u128 so no representable value can exceed 16 bytes.
                Ok(AlignedValue::from(n))
            }
        }
        TypeRef::Field => match val {
            Value::AlignedValue(av) => Ok(av.clone()),
            Value::Integer(n) => {
                use midnight_transient_crypto::curve::Fr;
                // Exact u128 → Fr conversion — see `value_to_fr`.
                Ok(AlignedValue::from(Fr::from(*n)))
            }
            _ => Err(unsupported()),
        },
        TypeRef::Bytes { length } => match val {
            Value::AlignedValue(av) => {
                // Re-tag with the requested Bytes<length> alignment so the
                // hash circuit sees the correct width even if the source
                // value carried a different alignment.
                let mut av = av.clone();
                av.alignment = fab::Alignment::singleton(fab::AlignmentAtom::Bytes {
                    length: *length as u32,
                });
                Ok(av)
            }
            Value::Void => bytes_aligned_value(Vec::new(), *length),
            _ => Err(unsupported()),
        },
        TypeRef::Opaque { .. } => match val {
            Value::AlignedValue(av) => Ok(av.clone()),
            // `default<Opaque<...>>` (e.g. via `none<Opaque<"string">>()`)
            // evaluates to Void. The Compact runtime encodes opaque values
            // as a single Compress-aligned atom and their default as the
            // empty value (compact-types.ts: CompactTypeOpaqueString /
            // CompactTypeOpaqueUint8Array), i.e. an empty atom.
            Value::Void => fab::AlignedValue::new(
                fab::Value(vec![fab::ValueAtom(Vec::new())]),
                fab::Alignment::singleton(fab::AlignmentAtom::Compress),
            )
            .ok_or_else(unsupported),
            _ => Err(unsupported()),
        },
        TypeRef::Tuple { types } => match val {
            Value::Tuple(elements) if elements.len() == types.len() => {
                let parts: Vec<AlignedValue> = elements
                    .iter()
                    .zip(types.iter())
                    .map(|(e, t)| encode_typed(e, t))
                    .collect::<Result<_, _>>()?;
                Ok(AlignedValue::concat(parts.iter()))
            }
            _ => Err(unsupported()),
        },
        TypeRef::Vector { length, element } => match val {
            Value::Tuple(elements) if elements.len() == *length => {
                let parts: Vec<AlignedValue> = elements
                    .iter()
                    .map(|e| encode_typed(e, element))
                    .collect::<Result<_, _>>()?;
                Ok(AlignedValue::concat(parts.iter()))
            }
            _ => Err(unsupported()),
        },
        // For Struct/Maybe receivers we'd need the layout registry to
        // recurse field-by-field; the current call sites (Expr::New) only
        // need the leaf type encodings above. Fall back to to_aligned_value.
        TypeRef::Struct { .. } | TypeRef::Maybe { .. } => Ok(val.to_aligned_value()),
        TypeRef::Void => match val {
            Value::Void => Ok(AlignedValue::from(())),
            _ => Err(unsupported()),
        },
        TypeRef::Enum { .. } => match val {
            // On-chain enums encode as their u8 declaration index.
            Value::Integer(n) => {
                let idx = u8::try_from(*n).map_err(|_| {
                    InterpreterError::TypeError(format!(
                        "integer {n} out of range for enum (max 255)"
                    ))
                })?;
                Ok(AlignedValue::from(idx))
            }
            Value::AlignedValue(av) => Ok(av.clone()),
            _ => Err(unsupported()),
        },
    }
}
