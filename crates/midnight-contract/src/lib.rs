pub mod call;
mod contract;
mod error;
pub mod interpreter;

pub use contract::Contract;
pub use error::ContractError;

// Re-export key call functions for convenience
pub use call::{
    call_circuit, call_circuit_with, deploy, deploy_with_provider, deserialize_state, fetch_state,
    format_address, parse_address, prove_circuit, prove_circuit_with,
};

/// Trait for types that can be deserialized from hex-encoded contract state.
///
/// Implement this for your generated ledger types. The `midnight_bindgen!`
/// macro generates these implementations automatically.
pub trait FromHex: Sized {
    fn from_hex(hex_state: &str) -> Result<Self, midnight_bindgen::StateError>;
}
