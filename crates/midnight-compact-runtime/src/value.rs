//! The runtime value domain used while interpreting a circuit IR body.

use std::collections::HashMap;

use midnight_bindgen_runtime::{AlignedValue, InMemoryDB, StateValue};

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
    /// `Value::Tuple` is flattened recursively into a concatenated
    /// `AlignedValue` so the prover sees one input value per leaf atom
    /// (matching the FAB encoding the circuit expects for `Vector<N, T>`
    /// arguments). `Value::Struct` cannot be flattened deterministically
    /// here because the underlying `HashMap` has no canonical iteration
    /// order; callers that need to pass a struct as a circuit argument
    /// should pre-encode it as a single `Value::AlignedValue` so this
    /// path stays unambiguous.
    pub fn to_aligned_value(&self) -> AlignedValue {
        match self {
            Value::AlignedValue(av) => av.clone(),
            Value::Integer(n) => integer_fallback_aligned(*n),
            Value::Bool(b) => AlignedValue::from(*b),
            Value::Void => AlignedValue::from(()),
            Value::Tuple(elements) => {
                let parts: Vec<AlignedValue> =
                    elements.iter().map(Self::to_aligned_value).collect();
                AlignedValue::concat(parts.iter())
            }
            Value::StateValue(_) | Value::Struct(_) => AlignedValue::from(()),
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
