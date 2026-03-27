pub mod call;
mod contract;
mod error;
pub mod interpreter;

// Re-export for generated code
pub use compact_codegen;
pub use midnight_provider::Provider;

// Primary API
pub use contract::{Contract, ContractBuilder};
pub use error::ContractError;

// Lower-level building blocks
pub use call::{
    DeployResult, call_funded, call_funded_with, deploy_funded, deploy_local, deserialize_state,
    fetch_state, format_address, parse_address, submit, wait_for_deployment, with_zk_keys,
};

/// Trait for types that can be deserialized from hex-encoded contract state.
pub trait FromHex: Sized {
    fn from_hex(hex_state: &str) -> Result<Self, midnight_bindgen::StateError>;
}
