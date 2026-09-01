//! Code generation library for Midnight Compact smart contract bindings.
//!
//! Parses a Compact compiler's `analyzed-ir.sexp` artifact and emits typed
//! Rust code for the `compact_bindgen::contract!` proc macro.

pub mod arg_types;
pub mod artifact;
pub mod error;
pub mod expand;
pub mod types;
pub mod validate;

/// The analyzed-IR model: the single type and expression vocabulary
/// shared by the code generator, the interpreter, and generated bindings.
pub use compact_analyzed_ir as ir;
pub use error::CodegenError;
pub use expand::helpers::to_snake_case;
pub use proc_macro2::TokenStream;

/// Generate bindings as a `TokenStream` from an `analyzed-ir.sexp` string.
/// Used by the proc macro.
///
/// `crate_path` controls the import path for runtime types (e.g. `compact_bindgen`
/// or `midnight_core::compact_bindgen`). When `None`, defaults to `compact_bindgen`.
pub fn generate_bindings_from_artifact(
    text: &str,
    contract_name: &str,
    crate_path: Option<&TokenStream>,
) -> Result<TokenStream, Box<dyn std::error::Error>> {
    let info = artifact::load_str(text)?;
    Ok(expand::generate_bindings(&info, contract_name, crate_path)?)
}

/// Generate bindings for both ledger generations plus a wrapper over them.
///
/// The caller's crate must depend on `compact-bindgen-v8` and
/// `compact-bindgen-v9`, because the two binding sets import from those.
pub fn generate_dispatch_from_artifact(
    text: &str,
    contract_name: &str,
) -> Result<TokenStream, Box<dyn std::error::Error>> {
    use quote::quote;
    let info = artifact::load_str(text)?;
    let eight =
        expand::generate_bindings(&info, contract_name, Some(&quote! { compact_bindgen_v8 }))?;
    let nine =
        expand::generate_bindings(&info, contract_name, Some(&quote! { compact_bindgen_v9 }))?;
    let wrapper = expand::generate_dispatch_wrapper(&info, contract_name)?;
    Ok(quote! {
        pub use midnight_dispatch::Generation;

        // The wrapper sits outside the per-generation modules, so it needs
        // these names itself. They come from `compact-values`, which compiles
        // once, so either shim re-exports the same types.
        #[allow(unused_imports)]
        use compact_bindgen_v9::{AlignedValue, Bytes, Field, JubjubPoint, Vector};

        /// Bindings typed against ledger 8.
        pub mod ledger_8 {
            #eight
        }

        /// Bindings typed against ledger 9.
        pub mod ledger_9 {
            #nine
        }

        #wrapper
    })
}

pub use expand::generate_dispatch_wrapper;
