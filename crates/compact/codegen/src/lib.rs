//! Code generation library for Midnight Compact smart contract bindings.
//!
//! Parses a Compact compiler's `normalized-ir.sexp` artifact and emits typed
//! Rust code for the `compact_bindgen::contract!` proc macro.

pub mod arg_types;
pub mod artifact;
pub mod error;
pub mod expand;
pub mod types;
pub mod validate;

/// The normalized-IR model: the single type and expression vocabulary
/// shared by the code generator, the interpreter, and generated bindings.
pub use compact_normalized_ir as nir;
pub use error::CodegenError;
pub use expand::helpers::to_snake_case;
pub use proc_macro2::TokenStream;

/// Generate bindings as a `TokenStream` from a `normalized-ir.sexp` string.
/// Used by the proc macro.
///
/// `crate_path` controls the import path for runtime types (e.g. `compact_bindgen`
/// or `midnight_core::compact_bindgen`). When `None`, defaults to `compact_bindgen`.
pub fn generate_bindings_from_normalized(
    text: &str,
    contract_name: &str,
    crate_path: Option<&TokenStream>,
) -> Result<TokenStream, Box<dyn std::error::Error>> {
    let info = artifact::load_str(text)?;
    Ok(expand::generate_bindings(&info, contract_name, crate_path)?)
}
