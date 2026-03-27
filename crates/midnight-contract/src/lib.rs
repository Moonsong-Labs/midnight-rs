pub mod call;
mod contract;
mod error;
pub mod interpreter;

// Re-export for generated code
pub use compact_codegen;
pub use midnight_provider::Provider;

pub use contract::Contract;
pub use error::ContractError;

// High-level API
pub use call::{
    call_circuit, call_circuit_with, deploy, deploy_funded, deploy_local, deploy_with_provider,
    deserialize_state, fetch_state, format_address, parse_address, prove_circuit,
    prove_circuit_with, submit,
};

/// Trait for types that can be deserialized from hex-encoded contract state.
pub trait FromHex: Sized {
    fn from_hex(hex_state: &str) -> Result<Self, midnight_bindgen::StateError>;
}
