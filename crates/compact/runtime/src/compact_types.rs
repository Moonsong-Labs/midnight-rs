//! Type-aware FAB encoding: the on-chain aligned-value layout for each
//! Compact types, plus the struct-layout machinery used to slice
//! `Value::AlignedValue` receivers by field. The Rust counterpart of
//! Minokawa's `compact-types`.

use std::ops::Range;

use midnight_typed_state::{AlignedValue, InMemoryDB, StateValue, variant_name};

use compact_codegen::ir::Type;
use num_bigint::BigUint;

use crate::conversions::{aligned_atom_to_u128, value_to_u128};
use crate::error::InterpreterError;
use crate::value::Value;

/// The `AlignedValue` a `StateValue::Cell` wraps, or `None` for any other
/// variant. `Null`, `Map`, `Array` and `BoundedMerkleTree` are state-tree
/// containers with no aligned-value form.
pub fn cell_aligned_value(sv: &StateValue<InMemoryDB>) -> Option<AlignedValue> {
    match sv {
        StateValue::Cell(sp) => Some((**sp).clone()),
        _ => None,
    }
}

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

/// Compute the number of FAB atoms a type occupies in an `AlignedValue`
/// encoding. Used to build struct layouts so `Expr::Field` can slice
/// `Value::AlignedValue` receivers by offset/length.
pub fn atom_count_for_type(ty: &Type) -> Option<usize> {
    match ty {
        Type::Boolean | Type::Unsigned(_) | Type::Field(_) | Type::Bytes(_) => Some(1),
        // A curve point is two atoms (x and y).
        Type::Point(_) => Some(2),
        Type::Opaque(name) => match name.as_str() {
            "JubjubPoint" => Some(2),
            "Scalar<BLS12-381>" => Some(1),
            _ => Some(1),
        },
        Type::Tuple(types) => {
            let mut total = 0;
            for t in types {
                total += atom_count_for_type(t)?;
            }
            Some(total)
        }
        Type::Vector { len, ty } => {
            let per = atom_count_for_type(ty)?;
            Some(per * (*len as usize))
        }
        // Prefer the field list the type carries: it is exact per
        // instantiation, where a name is not. Two instantiations of one
        // generic struct share a name but not a layout.
        Type::Struct { fields, .. } => {
            let mut total = 0;
            for (_, t) in fields {
                total += atom_count_for_type(t)?;
            }
            Some(total)
        }
        Type::Enum { .. } => Some(1),
        Type::Alias { ty, .. } => atom_count_for_type(ty),
        // A contract handle is a single address atom on the wire.
        Type::Contract { .. } => Some(1),
        // Rejected at load by `artifact::check_type`.
        Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => None,
    }
}

/// Compute a layout straight from a field list, for a type that carries its
/// own fields instead of being named in a table. Returns `None` when a field's
/// width cannot be determined.
pub fn layout_from_fields(fields: &[(String, Type)]) -> Option<StructLayout> {
    let mut out = Vec::with_capacity(fields.len());
    let mut offset = 0usize;
    for (name, ty) in fields {
        let len = atom_count_for_type(ty)?;
        out.push((name.clone(), offset, len));
        offset += len;
    }
    Some(StructLayout { fields: out })
}

/// How many elements a tuple- or vector-typed value carries, or `None` for a
/// type that carries no element-wise layout.
pub fn element_count(ty: &Type) -> Option<usize> {
    match ty.resolved() {
        Type::Vector { len, .. } => usize::try_from(*len).ok(),
        Type::Tuple(types) => Some(types.len()),
        _ => None,
    }
}

/// The declared type of element `i` of a tuple- or vector-typed value.
///
/// Pass `None` for an index that is not a compile-time constant. Only a tuple
/// needs it, because a vector's elements share one type.
pub fn element_type_at(ty: &Type, i: Option<usize>) -> Option<&Type> {
    match ty.resolved() {
        Type::Vector { ty, .. } => Some(ty),
        Type::Tuple(types) => types.get(i?),
        _ => None,
    }
}

/// The atom range element `i` occupies in a value flattened by `ty`.
///
/// `None` when the type carries no element-wise layout, when `i` is past the
/// length the type declares, or when an element width is unknown. A
/// heterogeneous tuple's elements start at the sum of the widths before them,
/// so an offset is not `i` times one stride.
pub fn element_atom_range(ty: &Type, i: usize) -> Option<Range<usize>> {
    match ty.resolved() {
        Type::Vector { len, ty } => {
            if i >= usize::try_from(*len).ok()? {
                return None;
            }
            let stride = atom_count_for_type(ty)?;
            let start = i.checked_mul(stride)?;
            Some(start..start.checked_add(stride)?)
        }
        Type::Tuple(types) => {
            let mut start = 0usize;
            for t in types.get(..i)? {
                start = start.checked_add(atom_count_for_type(t)?)?;
            }
            let width = atom_count_for_type(types.get(i)?)?;
            Some(start..start.checked_add(width)?)
        }
        _ => None,
    }
}

/// Take the atoms in `range` from `av`, value and alignment together.
///
/// `None` when the alignment does not give exactly one atom per value atom.
/// Every Compact type encodes as one `AlignmentSegment::Atom` per value atom,
/// so the two sequences line up and one range indexes both. The FAB wire
/// format also carries `AlignmentSegment::Option`, which covers a whole
/// disjoint union in one entry; a value holding one does not line up, and
/// slicing it positionally would attach a neighbour's alignment.
pub fn slice_atoms(av: &AlignedValue, range: Range<usize>) -> Option<AlignedValue> {
    use midnight_base_crypto::fab;
    let lines_up = av.alignment.0.len() == av.value.0.len()
        && av
            .alignment
            .0
            .iter()
            .all(|segment| matches!(segment, fab::AlignmentSegment::Atom(_)));
    if !lines_up || range.end > av.value.0.len() {
        return None;
    }
    Some(AlignedValue {
        value: fab::Value(av.value.0[range.clone()].to_vec()),
        alignment: fab::Alignment(av.alignment.0[range].to_vec()),
    })
}

/// Check `n` against a `Uint` bound and return the bound as a `u128`.
///
/// A `Value::Integer` is a `u128`, so a bound wider than `u128` cannot be
/// exceeded by any representable value; it saturates for the comparison.
pub fn check_uint_range(n: u128, maxval: &BigUint) -> Result<u128, InterpreterError> {
    let max = u128::try_from(maxval).unwrap_or(u128::MAX);
    if n > max {
        return Err(InterpreterError::TypeError(format!(
            "integer {n} out of range for Uint with maxval {maxval}"
        )));
    }
    Ok(max)
}

/// Declared byte width of `Uint<0..maxval>`, as the compiler computes it.
///
/// `compactc` emits `CompactTypeUnsignedInteger(maxval, byte-length(maxval))`
/// with `byte-length(n) = ceil(integer-length(n) / 8)`, and the canonical
/// runtime uses that length verbatim as the `Bytes{length}` alignment. So the
/// width is the minimal number of bytes holding the bound, not the next
/// primitive size up: `Uint<24>` is 3 bytes and `Uint<48>` is 6, widths no
/// `u8/u16/u32/u64/u128` ladder can express. Since `persistentHash` zero-pads
/// each atom to its declared width, rounding up here is a wrong digest.
///
/// A zero bound is one byte, not zero. `byte-length(0)` is 0 in the compiler,
/// which gave `Uint<0..1>` (and single-variant enums, which lower to it) an
/// `(abytes 0)` alignment that the ledger rejects as a malformed transcript.
/// Fixed upstream in LFDT-Minokawa/compact#626 by giving them `(abytes 1)`.
pub fn uint_byte_width(maxval: &BigUint) -> usize {
    match maxval.bits() {
        0 => 1,
        bits => (bits as usize).div_ceil(8),
    }
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

/// The Merkle leaf hash of an aligned value, as a `Bytes<32>` aligned value.
///
/// Mirrors the canonical runtime's `leafHash`: the on-chain Merkle tree stores
/// this digest, never the value itself, so a ledger tree write and the
/// `leafHash` builtin must produce the same bytes.
pub fn merkle_leaf_hash(av: AlignedValue) -> AlignedValue {
    use midnight_types::ValueReprAlignedValue;
    let hash = midnight_types::transient_crypto::merkle_tree::leaf_hash(&ValueReprAlignedValue(av));
    AlignedValue::from(hash.0)
}

/// Encode a runtime [`Value`] as an [`AlignedValue`] whose alignment matches
/// the declared [`Type`]. This is the single type-aware FAB encoder:
/// `Expr::New` struct fields, ledger cell/key pushes, literal path keys and
/// `Idx` path variables all route through here, so a new type variant only
/// needs handling in one place.
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
pub fn encode_typed(val: &Value, ty: &Type) -> Result<AlignedValue, InterpreterError> {
    use midnight_base_crypto::fab;
    let unsupported =
        || InterpreterError::TypeError(format!("cannot encode value {val:?} as type {ty:?}"));

    // A `StateValue::Cell` wraps exactly one `AlignedValue`, so unwrapping it is
    // the encoding; re-dispatch on the flat value so it still has to satisfy the
    // declared type. The other variants are containers in the state tree with no
    // aligned-value representation at all (upstream defines only
    // `From<AlignedValue> for StateValue`, never the reverse, and the canonical
    // runtime exposes just `asCell()`), so there is nothing to encode and this
    // reports that rather than substituting an empty value. Mirrors
    // `midnight_typed_state::nav::cell_value`.
    if let Value::StateValue(sv) = val {
        return match cell_aligned_value(sv) {
            Some(av) => encode_typed(&Value::AlignedValue(av), ty),
            None => Err(InterpreterError::TypeError(format!(
                "cannot encode a {} state value: only a Cell holds an aligned value",
                variant_name(sv)
            ))),
        };
    }

    match ty {
        Type::Boolean => match val {
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
        Type::Unsigned(maxval) => {
            let n = value_to_u128(val).ok_or_else(unsupported)?;
            check_uint_range(n, maxval)?;
            bytes_aligned_value(n.to_le_bytes().to_vec(), uint_byte_width(maxval))
        }
        Type::Field(_) => match val {
            Value::AlignedValue(av) => Ok(av.clone()),
            Value::Integer(n) => {
                use midnight_types::Fr;
                // Exact u128 → Fr conversion — see `value_to_fr`.
                Ok(AlignedValue::from(Fr::from(*n)))
            }
            _ => Err(unsupported()),
        },
        Type::Bytes(length) => match val {
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
            Value::Void => bytes_aligned_value(Vec::new(), *length as usize),
            _ => Err(unsupported()),
        },
        Type::Alias { ty: inner, .. } => encode_typed(val, inner),
        Type::Opaque(_) | Type::Point(_) | Type::Contract { .. } => match val {
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
        // Composite types accept either the structural spelling, which is
        // encoded element-by-element, or an already-flat `AlignedValue`. The
        // flat spelling is not exotic: slicing a field out of a struct receiver
        // yields one, as does any witness or ledger read whose declared type is
        // composite. Rejecting it would break values that encode correctly.
        Type::Tuple(types) => match val {
            Value::AlignedValue(av) => Ok(av.clone()),
            // Unit: the empty tuple is the language's void.
            Value::Void if types.is_empty() => Ok(AlignedValue::from(())),
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
        Type::Vector {
            len: length,
            ty: element,
        } => match val {
            Value::AlignedValue(av) => Ok(av.clone()),
            Value::Tuple(elements) if elements.len() as u64 == *length => {
                let parts: Vec<AlignedValue> = elements
                    .iter()
                    .map(|e| encode_typed(e, element))
                    .collect::<Result<_, _>>()?;
                Ok(AlignedValue::concat(parts.iter()))
            }
            _ => Err(unsupported()),
        },
        // A struct's FAB encoding is the concat of its fields' encodings in
        // declaration order, each at its own declared width. This is the rule
        // the canonical runtime's generated per-struct descriptor applies
        // (`toValue`/`alignment` concat the field descriptors in order) and the
        // one bindgen emits in `emit_struct_into_aligned_value`. There is no
        // tag and no length prefix, so a nested struct flattens transitively
        // into the parent's atom list.
        //
        // The declared width is load-bearing, which is why this cannot be done
        // in the type-free `Value::to_aligned_value`: a `Uint<32>` field must
        // encode as a 4-byte atom, and `integer_fallback_aligned` would emit 8.
        // Alignment participates in `AlignedValue` equality and `persistentHash`
        // zero-pads each atom to its declared width, so a wrong width is a wrong
        // digest.
        Type::Struct { name, fields } => {
            // Already flat: `Expr::New` encodes struct literals eagerly, so a
            // struct-typed value usually arrives pre-encoded.
            if let Value::AlignedValue(av) = val {
                return Ok(av.clone());
            }
            match val {
                // Named fields. The map has no inherent order, so declaration
                // order comes from `fields`; the value only supplies the
                // per-field contents. Field identity cannot be inferred from the
                // key set alone (a contract can ship `Maybe` and `Maybe_2` with
                // identical field names and different payload types), which is
                // why the type has to drive this.
                Value::Struct(supplied) => {
                    if supplied.len() != fields.len() {
                        return Err(InterpreterError::TypeError(format!(
                            "struct {name} expects {} fields, got {}",
                            fields.len(),
                            supplied.len()
                        )));
                    }
                    let parts: Vec<AlignedValue> = fields
                        .iter()
                        .map(|(field_name, field_ty)| {
                            let v = supplied.get(field_name).ok_or_else(|| {
                                InterpreterError::TypeError(format!(
                                    "struct {name} is missing field '{field_name}'"
                                ))
                            })?;
                            encode_typed(v, field_ty)
                        })
                        .collect::<Result<_, _>>()?;
                    Ok(AlignedValue::concat(parts.iter()))
                }
                // Positional spelling of the same struct.
                Value::Tuple(elements) if elements.len() == fields.len() => {
                    let parts: Vec<AlignedValue> = fields
                        .iter()
                        .zip(elements)
                        .map(|((_, field_ty), e)| encode_typed(e, field_ty))
                        .collect::<Result<_, _>>()?;
                    Ok(AlignedValue::concat(parts.iter()))
                }
                _ => Err(unsupported()),
            }
        }
        // An enum encodes as its declaration index in a single byte. The
        // compiler derives the width from the highest index, so this holds for
        // every enum up to 256 variants. A single-variant enum lowers to
        // `Uint<0..1>`, whose width was 0 until LFDT-Minokawa/compact#626 gave
        // it one byte; one byte is the post-fix width, so no special case.
        // Rejected at load by `artifact::check_type`, so reaching one here
        // means a consumer built a type the artifact could not express.
        Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => Err(InterpreterError::TypeError(
            format!("cannot encode a value as the non-executable type {ty:?}"),
        )),
        Type::Enum { .. } => match val {
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
