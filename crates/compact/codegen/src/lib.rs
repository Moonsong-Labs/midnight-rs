//! Code generation library for Midnight Compact smart contract bindings.
//!
//! Parses a Compact compiler's `contract-info.json` and emits typed Rust code
//! for the `compact_bindgen::contract!` proc macro.

pub mod arg_types;
pub mod error;
pub mod expand;
pub mod ir;
pub mod schema;
pub mod types;
pub mod validate;

pub use error::CodegenError;
pub use expand::helpers::to_snake_case;
pub use proc_macro2::TokenStream;

/// Generate bindings as a `TokenStream` from a contract-info.json string.
/// Used by the proc macro.
///
/// `crate_path` controls the import path for runtime types (e.g. `compact_bindgen`
/// or `midnight_core::compact_bindgen`). When `None`, defaults to `compact_bindgen`.
pub fn generate_bindings_from_json(
    json: &str,
    contract_name: &str,
    crate_path: Option<&TokenStream>,
) -> Result<TokenStream, Box<dyn std::error::Error>> {
    let info: types::ContractInfo =
        serde_json::from_str(json).map_err(|e| format!("invalid contract-info.json: {e}"))?;
    Ok(expand::generate_bindings(&info, contract_name, crate_path)?)
}
