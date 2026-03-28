//! Code generation library for Midnight Compact smart contract bindings.
//!
//! Parses a Compact compiler's `contract-info.json` and emits typed Rust code.
//! Used internally by the proc macro and the CLI tool.

pub mod expand;
pub mod ir;
pub mod schema;
pub mod types;

use std::path::Path;

pub use expand::GeneratedCrate;
pub use expand::helpers::to_snake_case;
pub use proc_macro2::TokenStream;

/// Generate a complete Rust crate from a `contract-info.json` file on disk.
/// Used by the CLI tool.
pub fn generate_from_file(
    input: &Path,
    contract_name: &str,
) -> Result<GeneratedCrate, Box<dyn std::error::Error>> {
    let info = schema::parse_contract_info(input)?;
    Ok(expand::generate_crate(&info, contract_name))
}

/// Generate bindings as a `TokenStream` from a contract-info.json string.
/// Used by the proc macro.
///
/// `crate_path` controls the import path for runtime types (e.g. `midnight_bindgen`
/// or `midnight_core::midnight_bindgen`). When `None`, defaults to `midnight_bindgen`.
pub fn generate_bindings_from_json(
    json: &str,
    contract_name: &str,
    crate_path: Option<&TokenStream>,
) -> Result<TokenStream, Box<dyn std::error::Error>> {
    let info: types::ContractInfo = serde_json::from_str(json)?;
    Ok(expand::generate_bindings(&info, contract_name, crate_path))
}
