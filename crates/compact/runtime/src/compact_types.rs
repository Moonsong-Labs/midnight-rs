//! Type-aware FAB encoding: the on-chain aligned-value layout for each
//! Compact types, the default value of each type, plus the struct-layout
//! machinery used to slice `Value::AlignedValue` receivers by field. The Rust
//! counterpart of Minokawa's `compact-types`.

use std::ops::Range;

use midnight_typed_state::{Aligned, AlignedValue, Secp256k1Base, Secp256k1Point, Secp256k1Scalar};

use compact_codegen::ir::{Curve, Type, uint_byte_width};
use num_bigint::BigUint;

use crate::conversions::{aligned_atom_to_u128, value_to_u128};
use crate::error::InterpreterError;
use crate::secp256k1::{ForeignField, encode_point};
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

/// Compute the number of FAB atoms a type occupies in an `AlignedValue`
/// encoding. Used to build struct layouts so `Expr::Field` can slice
/// `Value::AlignedValue` receivers by offset/length.
///
/// Returns `None` for a type with no fixed width, or when the count does not
/// fit a `usize`.
pub fn atom_count_for_type(ty: &Type) -> Option<usize> {
    match ty {
        Type::Field(field) => Some(match ForeignField::of(field) {
            Some(ForeignField::Base) => Secp256k1Base::alignment().0.len(),
            Some(ForeignField::Scalar) => Secp256k1Scalar::alignment().0.len(),
            None => 1,
        }),
        Type::Boolean | Type::Unsigned(_) | Type::Bytes(_) => Some(1),
        // A Jubjub point is two atoms (x and y).
        Type::Point(Curve::Jubjub) => Some(2),
        Type::Point(Curve::Secp256k1) => Some(Secp256k1Point::alignment().0.len()),
        Type::Opaque(name) => match name.as_str() {
            "JubjubPoint" => Some(2),
            "Scalar<BLS12-381>" => Some(1),
            _ => Some(1),
        },
        Type::Tuple(types) => atoms_of(types),
        Type::Vector { len, ty } => {
            atom_count_for_type(ty)?.checked_mul(usize::try_from(*len).ok()?)
        }
        // Prefer the field list the type carries: it is exact per
        // instantiation, where a name is not. Two instantiations of one
        // generic struct share a name but not a layout.
        Type::Struct { fields, .. } => atoms_of(fields.iter().map(|(_, t)| t)),
        Type::Enum { .. } => Some(1),
        Type::Alias { ty, .. } => atom_count_for_type(ty),
        // A contract handle is a single address atom on the wire.
        Type::Contract { .. } => Some(1),
        // Rejected at load by `artifact::check_type`.
        Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => None,
    }
}

fn atoms_of<'a>(types: impl IntoIterator<Item = &'a Type>) -> Option<usize> {
    types.into_iter().try_fold(0usize, |total, t| {
        total.checked_add(atom_count_for_type(t)?)
    })
}

/// Compute a layout straight from a field list, for a type that carries its
/// own fields instead of being named in a table. Returns `None` when a field's
/// width cannot be determined, or when an offset does not fit a `usize`.
pub fn layout_from_fields(fields: &[(String, Type)]) -> Option<StructLayout> {
    let mut out = Vec::with_capacity(fields.len());
    let mut offset = 0usize;
    for (name, ty) in fields {
        let len = atom_count_for_type(ty)?;
        out.push((name.clone(), offset, len));
        offset = offset.checked_add(len)?;
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
            let start = atoms_of(types.get(..i)?)?;
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
    fab::AlignedValue::new(
        fab::Value(vec![fab::ValueAtom::from(bytes)]),
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
    use midnight_transient_crypto::fab::ValueReprAlignedValue;
    let hash = midnight_transient_crypto::merkle_tree::leaf_hash(&ValueReprAlignedValue(av));
    AlignedValue::from(hash.0)
}

/// Encode a runtime [`Value`] as an [`AlignedValue`] whose alignment matches
/// the declared [`Type`]. This is the single type-aware FAB encoder:
/// `Expr::New` struct fields, ledger cell/key pushes, literal path keys and
/// `Idx` path variables all route through here, so a new type variant only
/// needs handling in one place.
///
/// A struct accepts a `Value::Struct`, which names its fields, or a
/// `Value::Tuple`, which gives them in declaration order.
///
/// # Why the width matters
///
/// FAB atoms are zero-trimmed little-endian bytes, so the atom for a given
/// number is identical at every width; the declared width lives only in the
/// `AlignmentAtom::Bytes { length }`. That alignment participates in
/// `AlignedValue` equality/hashing (on-chain `Map` lookups compare the full
/// `AlignedValue`) and in `persistentHash`, which zero-pads each atom to the
/// declared width. So a `Uint` takes the width of [`uint_byte_width`], the
/// rule that the bindgen-emitted encoders (`uint_tokens` in compact-codegen)
/// also follow.
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

    // A Cell unwraps to its aligned value, which must still satisfy `ty`.
    if let Value::StateValue(_) = val {
        return encode_typed(&Value::AlignedValue(val.try_to_aligned_value()?), ty);
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
        Type::Field(field) => match (ForeignField::of(field), val) {
            (Some(foreign), _) => foreign.encode(val),
            (None, Value::AlignedValue(av)) => Ok(av.clone()),
            (None, Value::Integer(n)) => {
                use midnight_transient_crypto::curve::Fr;
                // Exact u128 → Fr conversion — see `value_to_fr`.
                Ok(AlignedValue::from(Fr::from(*n)))
            }
            (None, _) => Err(unsupported()),
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
        Type::Contract { .. } => encode_typed(val, &Type::contract_address()),
        Type::Point(Curve::Secp256k1) => encode_point(val),
        Type::Opaque(_) | Type::Point(Curve::Jubjub) => match val {
            Value::AlignedValue(av) => Ok(av.clone()),
            // `default<Opaque<...>>` (e.g. via `none<Opaque<"string">>()`)
            // evaluates to Void.
            Value::Void => Ok(empty_opaque()),
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
            let field_count = |n: usize| {
                if n == fields.len() {
                    Ok(())
                } else {
                    Err(InterpreterError::TypeError(format!(
                        "struct {name} expects {} fields, got {n}",
                        fields.len()
                    )))
                }
            };
            let values: Vec<&Value> = match val {
                // Already flat: `Expr::New` encodes struct literals eagerly, so a
                // struct-typed value usually arrives pre-encoded.
                Value::AlignedValue(av) => return Ok(av.clone()),
                // Named fields. The map has no inherent order, so declaration
                // order comes from `fields`; the value only supplies the
                // per-field contents. Field identity cannot be inferred from the
                // key set alone (a contract can ship `Maybe` and `Maybe_2` with
                // identical field names and different payload types), which is
                // why the type has to drive this.
                Value::Struct(supplied) => {
                    field_count(supplied.len())?;
                    fields
                        .iter()
                        .map(|(field_name, _)| {
                            supplied.get(field_name).ok_or_else(|| {
                                InterpreterError::TypeError(format!(
                                    "struct {name} is missing field '{field_name}'"
                                ))
                            })
                        })
                        .collect::<Result<_, _>>()?
                }
                Value::Tuple(elements) => {
                    field_count(elements.len())?;
                    elements.iter().collect()
                }
                _ => return Err(unsupported()),
            };
            let parts: Vec<AlignedValue> = fields
                .iter()
                .zip(values)
                .map(|((field_name, field_ty), v)| {
                    encode_typed(v, field_ty).map_err(|e| {
                        InterpreterError::TypeError(format!(
                            "cannot encode field `{field_name}` of `{name}`: {e}"
                        ))
                    })
                })
                .collect::<Result<_, _>>()?;
            Ok(AlignedValue::concat(parts.iter()))
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

/// The default of an opaque type. The Compact runtime encodes an opaque value as
/// a single Compress-aligned atom and its default as the empty value
/// (compact-types.ts: CompactTypeOpaqueString / CompactTypeOpaqueUint8Array),
/// which is one empty atom.
fn empty_opaque() -> AlignedValue {
    AlignedValue::from(Vec::<u8>::new())
}

/// The compiler's `default<T>`, in the interpreter's value domain.
///
/// The canonical runtime builds a default through the type's descriptor
/// (`CompactType*.toValue` of the zero value), so the FAB alignment is the
/// type's own: `default<Bytes<32>>` is an empty atom aligned `Bytes {32}`,
/// not the unit value. An alias takes the default of the type it names. A
/// contract type takes the default of `Type::contract_address()`, the type the
/// compiler encodes it as.
///
/// # Errors
///
/// - [`InterpreterError::Unsupported`] if `ty` holds an ADT, a type variable or
///   `Type::Unknown`, which have no value.
/// - [`InterpreterError::TypeError`] if a `Bytes` or `Vector` length in `ty`
///   does not fit in `usize`, or if a part of the default does not encode at
///   its type.
pub fn default_value(ty: &Type) -> Result<Value, InterpreterError> {
    match ty {
        Type::Boolean => Ok(Value::Bool(false)),
        Type::Unsigned(_) | Type::Enum { .. } => Ok(Value::Integer(0)),
        Type::Field(field) => Ok(Value::AlignedValue(match ForeignField::of(field) {
            Some(ForeignField::Base) => AlignedValue::from(Secp256k1Base::default()),
            Some(ForeignField::Scalar) => AlignedValue::from(Secp256k1Scalar::default()),
            None => AlignedValue::from(midnight_transient_crypto::curve::Fr::from(0u64)),
        })),
        Type::Bytes(length) => Ok(Value::AlignedValue(bytes_aligned_value(
            Vec::new(),
            ir_length(*length)?,
        )?)),
        // A curve point is two field atoms, its affine x and y, and its default
        // is the curve identity. Taking the opaque default instead gives a
        // single `Compress` atom, which reads back as the wrong alignment
        // wherever the type's own alignment is what is wanted: `max-sizeof`
        // sizes a `List` read's `concat` from it, and `(null <type>)` builds
        // that read's empty answer.
        Type::Point(Curve::Jubjub) => Ok(Value::AlignedValue(AlignedValue::from(
            midnight_transient_crypto::curve::EmbeddedGroupAffine::identity(),
        ))),
        Type::Point(Curve::Secp256k1) => Ok(Value::AlignedValue(AlignedValue::from(
            Secp256k1Point::default(),
        ))),
        Type::Opaque(_) => Ok(Value::AlignedValue(empty_opaque())),
        Type::Struct { fields, .. } => {
            let defaults = fields
                .iter()
                .map(|(_, field_ty)| default_value(field_ty))
                .collect::<Result<Vec<_>, _>>()?;
            encode_typed(&Value::Tuple(defaults), ty).map(Value::AlignedValue)
        }
        Type::Tuple(types) if types.is_empty() => Ok(Value::Void),
        Type::Tuple(types) => Ok(Value::Tuple(
            types
                .iter()
                .map(default_value)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Type::Vector { len, ty: element } => Ok(Value::Tuple(
            std::iter::repeat_with(|| default_value(element))
                .take(ir_length(*len)?)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Type::Alias { ty, .. } => default_value(ty),
        Type::Contract { .. } => default_value(&Type::contract_address()),
        Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => Err(InterpreterError::Unsupported(
            format!("default<{ty:?}>: the type has no value"),
        )),
    }
}

/// The compiler's `rt-null T`: `default<T>` encoded at the alignment of `T`.
///
/// This is the value a ledger `Cell` of type `T` holds before anything writes
/// it.
///
/// # Panics
///
/// If `ty` holds an ADT, a type variable or `Type::Unknown`, or on any other
/// error from [`default_value`] or from the encoding of its result at `ty`. The
/// artifact loader refuses an ADT or a type variable in a value position, and
/// `Type::Unknown` in a ledger field, so the type of a ledger cell in a loaded
/// artifact holds none of them.
pub fn default_aligned(ty: &Type) -> AlignedValue {
    default_value(ty)
        .and_then(|value| encode_typed(&value, ty))
        .unwrap_or_else(|e| panic!("default<{ty:?}> has no aligned value: {e}"))
}

/// Convert an IR-level element count or index (`u64`) to `usize`.
///
/// # Errors
///
/// [`InterpreterError::TypeError`] for a value the host cannot index, rather
/// than a value that wraps.
pub fn ir_length(length: u64) -> Result<usize, InterpreterError> {
    usize::try_from(length)
        .map_err(|_| InterpreterError::TypeError(format!("length {length} does not fit in usize")))
}
