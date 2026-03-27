pub mod call;
mod contract;
mod error;
pub mod interpreter;

// Re-export compact_codegen so generated circuit call methods can reference it
pub use compact_codegen;

pub use contract::Contract;
pub use error::ContractError;

// High-level API — the functions most users need
pub use call::{
    // Circuit execution
    call_circuit,
    call_circuit_with,
    // Deploy
    deploy,
    deploy_funded,
    deploy_local,
    deploy_with_provider,
    // State
    deserialize_state,
    fetch_state,
    // Addresses
    format_address,
    parse_address,
    prove_circuit,
    prove_circuit_with,
    // Submission
    submit,
};

/// Trait for types that can be deserialized from hex-encoded contract state.
///
/// Implement this for your generated ledger types. The `midnight_bindgen!`
/// macro generates these implementations automatically.
pub trait FromHex: Sized {
    fn from_hex(hex_state: &str) -> Result<Self, midnight_bindgen::StateError>;
}
