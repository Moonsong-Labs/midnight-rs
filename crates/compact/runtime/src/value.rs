//! The runtime value domain used while interpreting a circuit IR body.

use std::collections::HashMap;

use midnight_typed_state::{AlignedValue, InMemoryDB, StateValue, variant_name};

use crate::error::InterpreterError;

/// Runtime value during IR interpretation.
#[derive(Debug, Clone)]
pub enum Value {
    Bool(bool),
    Integer(u128),
    AlignedValue(AlignedValue),
    StateValue(StateValue<InMemoryDB>),
    /// A struct/record with named fields.
    Struct(HashMap<String, Value>),
    /// A tuple/array with indexed elements.
    Tuple(Vec<Value>),
    Void,
}

impl Value {
    /// Extract as u32 for Op::Addi immediate.
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Value::Integer(n) => u32::try_from(*n).ok(),
            _ => None,
        }
    }

    /// Convert to an AlignedValue for use as circuit input.
    ///
    /// The type-free encoding: what a value flattens to when no declared
    /// `TypeRef` is in scope.
    ///
    /// `Value::Tuple` is flattened recursively into a concatenated
    /// `AlignedValue` so the prover sees one input value per leaf atom
    /// (matching the FAB encoding the circuit expects for `Vector<N, T>`
    /// arguments). A `Value::StateValue` holding a `Cell` unwraps to the
    /// `AlignedValue` the cell already contains.
    ///
    /// Two shapes have no type-free encoding and are an error rather than a
    /// guess. A `Value::Struct` encodes field-by-field at each field's
    /// *declared* width, which only the type carries: alignment participates in
    /// `AlignedValue` equality and `persistentHash` zero-pads each atom to its
    /// declared width, so inventing a width is a wrong digest. The container
    /// `StateValue` variants (`Null`, `Map`, `Array`, `BoundedMerkleTree`) are
    /// state-tree nodes with no aligned-value form at all. Encode those through
    /// [`crate::compact_types::encode_typed_with_defs`], which takes the type.
    ///
    /// This returns a `Result` precisely so neither can collapse to the empty
    /// value, which is what made a commitment silently bind to nothing.
    pub fn try_to_aligned_value(&self) -> Result<AlignedValue, InterpreterError> {
        match self {
            Value::AlignedValue(av) => Ok(av.clone()),
            Value::Integer(n) => Ok(integer_fallback_aligned(*n)),
            Value::Bool(b) => Ok(AlignedValue::from(*b)),
            Value::Void => Ok(AlignedValue::from(())),
            // Recurses, so a struct nested in a tuple is caught rather than
            // silently contributing nothing to the concatenation.
            Value::Tuple(elements) => {
                let parts = elements
                    .iter()
                    .map(Self::try_to_aligned_value)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(AlignedValue::concat(parts.iter()))
            }
            Value::StateValue(sv) => {
                crate::compact_types::cell_aligned_value(sv).ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "cannot encode a {} state value: only a Cell holds an aligned value",
                        variant_name(sv)
                    ))
                })
            }
            Value::Struct(_) => Err(InterpreterError::TypeError(
                "cannot encode a struct without its declared type: field widths come from the \
                 struct's declaration"
                    .to_string(),
            )),
        }
    }

    /// Convert to a StateValue for ledger storage.
    pub fn to_state_value(&self) -> StateValue<InMemoryDB> {
        match self {
            Value::AlignedValue(av) => StateValue::from(av.clone()),
            Value::Integer(n) => StateValue::from(integer_fallback_aligned(*n)),
            Value::Bool(b) => StateValue::from(AlignedValue::from(*b)),
            Value::Void => StateValue::from(AlignedValue::from(())),
            _ => StateValue::Null,
        }
    }
}

/// Width-preserving FAB encoding for an integer with no type information.
///
/// FAB atoms are zero-trimmed little-endian bytes (`ValueAtom::normalize` in
/// midnight-base-crypto, and `From<u64>` forwards through `From<u128>`), so
/// the atom bytes are identical for every integer width — only the declared
/// `AlignmentAtom::Bytes { length }` differs. That width is *not* cosmetic:
/// `AlignedValue`'s `Eq`/`Hash` include the alignment (so on-chain `Map` keys
/// of different widths are different keys) and `persistentHash` zero-pads
/// each atom to the declared width (`ValueAtom::binary_repr_unchecked` in
/// midnight-transient-crypto), so `Bytes{8}` and `Bytes{16}` encodings of the
/// same number hash differently.
///
/// Therefore: values that fit `u64` keep the historical 8-byte alignment so
/// every existing encoding (witness transcript outputs, circuit-argument
/// flattening, type-less ledger pushes) stays byte-for-byte identical.
/// Values above `u64::MAX` are encoded at the 16-byte width — the width the
/// type-aware encoder picks for every `Uint` bound that can hold such a
/// value — instead of being silently truncated as before.
pub fn integer_fallback_aligned(n: u128) -> AlignedValue {
    match u64::try_from(n) {
        Ok(small) => AlignedValue::from(small),
        Err(_) => AlignedValue::from(n),
    }
}
