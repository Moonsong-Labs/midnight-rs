use midnight_base_crypto::fab::InvalidBuiltinDecode;

/// Errors that can occur when reading contract ledger state.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// A ledger field index was out of bounds for the state array.
    #[error("field index {0} out of bounds")]
    IndexOutOfBounds(usize),

    /// The `StateValue` variant didn't match what was expected
    /// (e.g., expected `Cell` but got `Map`).
    #[error("expected StateValue::{expected}, got {actual}")]
    UnexpectedVariant {
        expected: &'static str,
        actual: &'static str,
    },

    /// A value could not be converted from its on-chain representation.
    #[error("value conversion failed: {0}")]
    Conversion(#[from] InvalidBuiltinDecode),

    /// Binary deserialization of `ContractState` failed.
    #[error("deserialization failed: {0}")]
    Deserialize(#[from] std::io::Error),

    /// Hex-encoded state string could not be decoded.
    #[error("hex decode failed: {0}")]
    HexDecode(String),
}
