use midnight_base_crypto::fab::AlignedValue;
use midnight_onchain_state::state::StateValue;
use midnight_storage::db::InMemoryDB;

use crate::StateError;

/// Returns the name of the `StateValue` variant.
pub fn variant_name(sv: &StateValue<InMemoryDB>) -> &'static str {
    match sv {
        StateValue::Null => "Null",
        StateValue::Cell(_) => "Cell",
        StateValue::Map(_) => "Map",
        StateValue::Array(_) => "Array",
        StateValue::BoundedMerkleTree(_) => "BoundedMerkleTree",
        _ => "Unknown",
    }
}

/// Extracts the `AlignedValue` from a `StateValue::Cell`.
///
/// Returns an error if the value is not a `Cell`.
pub fn cell_value(sv: &StateValue<InMemoryDB>) -> Result<&AlignedValue, StateError> {
    match sv {
        StateValue::Cell(sp) => Ok(&**sp),
        other => Err(StateError::UnexpectedVariant {
            expected: "Cell",
            actual: variant_name(other),
        }),
    }
}

/// Retrieves a field by index from a `StateValue::Array`.
///
/// Returns an error if the value is not an `Array` or the index is out of bounds.
pub fn get_field(
    sv: &StateValue<InMemoryDB>,
    index: usize,
) -> Result<&StateValue<InMemoryDB>, StateError> {
    match sv {
        StateValue::Array(arr) => arr.get(index).ok_or(StateError::IndexOutOfBounds(index)),
        other => Err(StateError::UnexpectedVariant {
            expected: "Array",
            actual: variant_name(other),
        }),
    }
}

/// Navigates a path of indices through nested `StateValue::Array`s.
///
/// Each element in `path` is an index into the array at the current level.
pub fn get_field_path<'a>(
    sv: &'a StateValue<InMemoryDB>,
    path: &[usize],
) -> Result<&'a StateValue<InMemoryDB>, StateError> {
    let mut current = sv;
    for &index in path {
        current = get_field(current, index)?;
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_name_null() {
        let sv = StateValue::<InMemoryDB>::Null;
        assert_eq!(variant_name(&sv), "Null");
    }

    #[test]
    fn cell_value_on_null_returns_error() {
        let sv = StateValue::<InMemoryDB>::Null;
        let err = cell_value(&sv).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Cell") && msg.contains("Null"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn get_field_on_null_returns_error() {
        let sv = StateValue::<InMemoryDB>::Null;
        let err = get_field(&sv, 0).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Array") && msg.contains("Null"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn get_field_path_empty_path_returns_self() {
        let sv = StateValue::<InMemoryDB>::Null;
        let result = get_field_path(&sv, &[]).unwrap();
        assert_eq!(variant_name(result), "Null");
    }
}
