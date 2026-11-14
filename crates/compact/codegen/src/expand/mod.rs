//! Code expansion modules -- each concern has its own file.
//!
//! Mirrors the alloy-rs `sol-macro-expander/src/expand/` pattern:
//! one module per item kind, orchestrated by `EmitCtxt`.

pub(crate) mod circuit_calls;
mod circuits;
mod constants;
mod data_types;
mod emit_ir;
pub(crate) mod helpers;
mod ledger;
pub(crate) mod types;

use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::quote;

use crate::types::ContractInfo;

/// Expansion context that centralises resolution state across all emitters.
pub(crate) struct EmitCtxt<'a> {
    pub info: &'a ContractInfo,
    pub contract_name: &'a str,
    pub crate_path: &'a TokenStream,
    /// The struct and enum types emitted so far, by name (deduplication).
    pub emitted_types: HashMap<String, crate::ir::Type>,
}

impl<'a> EmitCtxt<'a> {
    pub fn new(
        info: &'a ContractInfo,
        contract_name: &'a str,
        crate_path: &'a TokenStream,
    ) -> Self {
        Self {
            info,
            contract_name,
            crate_path,
            emitted_types: HashMap::new(),
        }
    }

    /// Run the full expansion pipeline, returning the combined `TokenStream`.
    pub fn expand(&mut self) -> TokenStream {
        let crate_path = self.crate_path;
        let constants = constants::emit_field_constants(&self.info.ledger);
        let data_types = data_types::emit_data_types(
            &self.info.ledger,
            &self.info.circuits,
            self.info.runnable_constructor(),
            &self.info.witnesses,
            &mut self.emitted_types,
        );
        let circuit_types = circuits::emit_circuit_types(&self.info.circuits, &self.info.witnesses);
        let ir_constants = circuit_calls::emit_circuit_ir_constants(self.info);
        let wrapper = ledger::emit_ledger_wrapper(
            &self.info.ledger,
            self.contract_name,
            &ir_constants,
            self.info,
        );
        let pure_circuits = circuits::emit_pure_circuits(&self.info.circuits, self.contract_name);
        let simulator = ledger::emit_simulator(self.info, self.contract_name);

        // Import midnight_contract via the facade so generated code can use
        // `midnight_contract::*` paths without forcing the calling crate to
        // depend on midnight-contract directly. The macro emits
        // `use compact_bindgen::midnight_contract;` (or whatever the active
        // crate_path is); compact-bindgen re-exports midnight_contract for
        // exactly this purpose.
        let contract_import = quote! { use #crate_path::midnight_contract; };

        // Explicit imports of exactly the runtime items the emitters
        // reference. No glob: `use #crate_path::*;` silently shadowed
        // user-defined items with the same names (e.g. a local `Value`,
        // `Bytes`, or `StateValue` next to a flat `contract!` invocation
        // would capture the generated code's references instead).
        // `allow(unused_imports)`: which items a given contract's bindings
        // use depends on its types (e.g. `EmbeddedGroupAffine` only for
        // JubjubPoint, `serde` only for witnesses).
        let runtime_imports = quote! {
            #[allow(unused_imports)]
            use #crate_path::{
                Aligned, AlignedValue, Alignment, Bytes, ContractMaintenanceAuthority,
                ContractState, EmbeddedGroupAffine, InMemoryDB, InvalidBuiltinDecode,
                ListAccessor, MapAccessor, MerkleTree, MerkleTreeAccessor, SetAccessor,
                StateError, StateValue, StorageArray, StorageHashMap, TransientFr, Uint,
                ValueSlice, Vector,
                cell_as, get_field_path, hex, serde, serde_json, tagged_deserialize,
            };
        };

        quote! {
            #runtime_imports
            #contract_import

            #constants
            #data_types
            #circuit_types
            #wrapper

            #pure_circuits
            #simulator
        }
    }
}

// --- Public API ---

/// Generate the Rust bindings as a `TokenStream` for use with the proc macro.
///
/// `crate_path` controls the import path for runtime types (e.g. `compact_bindgen`
/// or `midnight_core::compact_bindgen`). When `None`, defaults to `compact_bindgen`.
pub fn generate_bindings(
    info: &ContractInfo,
    contract_name: &str,
    crate_path: Option<&TokenStream>,
) -> TokenStream {
    let default_path = quote! { compact_bindgen };
    let crate_path = crate_path.unwrap_or(&default_path);
    let mut ctx = EmitCtxt::new(info, contract_name, crate_path);
    ctx.expand()
}

#[cfg(test)]
mod tests {
    /// Render generated bindings as source text. The emitters produce token
    /// streams; these tests assert on the rendered form.
    fn generated_source(info: &ContractInfo, contract_name: &str) -> String {
        let tokens = generate_bindings(info, contract_name, None);
        let file: syn::File =
            syn::parse2(tokens).expect("generated code must be a valid Rust file");
        prettyplease::unparse(&file)
    }

    use super::helpers::{make_ident, to_pascal_case};
    use super::types::{type_to_tokens, uint_tokens};
    use super::*;

    /// Load the artifact at `rel`, a path relative to this crate's manifest
    /// directory.
    fn load_fixture(rel: &str) -> ContractInfo {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        crate::artifact::load(&path).unwrap_or_else(|e| panic!("load {}: {e}", path.display()))
    }

    #[test]
    fn uint_type_mapping() {
        let bound = |digits: &str| digits.parse::<num_bigint::BigUint>().expect("decimal");
        assert_eq!(uint_tokens(&bound("255")).to_string(), "u8");
        assert_eq!(uint_tokens(&bound("65535")).to_string(), "u16");
        assert_eq!(
            uint_tokens(&bound("18446744073709551615")).to_string(),
            "u64"
        );
        assert_eq!(
            uint_tokens(&bound("340282366920938463463374607431768211455")).to_string(),
            "u128"
        );
        // A bound whose byte width a primitive has keeps the primitive.
        assert_eq!(uint_tokens(&bound("1000")).to_string(), "u16");
        // `BITS` is the bit length of the bound, not the byte width in bits.
        assert_eq!(uint_tokens(&bound("16777215")).to_string(), "Uint < 24 >");
        assert_eq!(uint_tokens(&bound("999999")).to_string(), "Uint < 20 >");
    }

    /// The interpreter holds a `Uint` in a `u128`. A wider bound must fail the
    /// build and name the type, not fall back to a type that encodes the value
    /// at another layout.
    #[test]
    fn a_uint_wider_than_u128_is_a_compile_error() {
        let bound: num_bigint::BigUint = num_bigint::BigUint::from(u128::MAX) + 1u32;
        let tokens = uint_tokens(&bound).to_string();
        assert!(tokens.starts_with("compile_error !"), "{tokens}");
        assert!(
            tokens.contains("Uint<0..340282366920938463463374607431768211457>"),
            "{tokens}"
        );
    }

    /// A tuple wider than the runtime decodes must fail the build with a named
    /// error, not leave a flat Rust tuple that has no decoder.
    #[test]
    fn a_tuple_wider_than_the_runtime_decodes_is_a_compile_error() {
        use crate::ir::Type;

        let tuple = |len: usize| type_to_tokens(&Type::Tuple(vec![Type::Boolean; len])).to_string();
        let widest = tuple(11);
        assert!(widest.starts_with('('), "{widest}");
        let wider = tuple(12);
        assert!(wider.starts_with("compile_error !"), "{wider}");
    }

    #[test]
    fn keyword_escaping() {
        assert_eq!(make_ident("type").to_string(), "r#type");
        assert_eq!(make_ident("match").to_string(), "r#match");
        assert_eq!(make_ident("fn").to_string(), "r#fn");
        assert_eq!(make_ident("async").to_string(), "r#async");
        assert_eq!(make_ident("try").to_string(), "r#try");
        assert_eq!(make_ident("gen").to_string(), "r#gen");
        // Non-keywords pass through unchanged.
        assert_eq!(make_ident("threshold").to_string(), "threshold");
        assert_eq!(make_ident("amount").to_string(), "amount");
    }

    /// Generated code must never panic on values that depend on
    /// analyzed-IR content, interpreter output, or provider responses.
    /// Token-level guard: no panicking constructs at all in generated code.
    fn assert_no_panic_paths(contract: &str, lib_rs: &str) {
        for needle in [
            "panic!",
            ".unwrap()",
            ".expect(",
            "unreachable!",
            "todo!",
            "unimplemented!",
            "assert!(",
            "assert_eq!(",
            "assert_ne!(",
        ] {
            assert!(
                !lib_rs.contains(needle),
                "generated code for `{contract}` contains `{needle}`"
            );
        }
    }

    /// Collect every `analyzed-ir.sexp` that `make fixtures` writes under the
    /// compiled fixture roots.
    fn contract_info_fixtures() -> Vec<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut fixtures = Vec::new();
        for dir in [
            root.join("tests/fixtures/compiled"),
            root.join("crates/midnight-contract/tests/fixtures"),
        ] {
            for entry in std::fs::read_dir(dir).unwrap() {
                let candidate = entry.unwrap().path().join("compiler/analyzed-ir.sexp");
                if candidate.is_file() {
                    fixtures.push(candidate);
                }
            }
        }
        fixtures.sort();
        fixtures
    }

    /// Derive a PascalCase contract name from a fixture path:
    /// `.../counter/compiler/analyzed-ir.sexp` -> `Counter`.
    fn fixture_contract_name(path: &std::path::Path) -> String {
        // `<contract>/compiler/analyzed-ir.sexp`
        let raw = path
            .parent()
            .and_then(std::path::Path::parent)
            .and_then(std::path::Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .unwrap();
        to_pascal_case(raw)
    }

    /// Expand every compiled fixture and assert the output is panic-free.
    #[test]
    fn generated_code_has_no_panic_paths() {
        let fixtures = contract_info_fixtures();
        // `make fixtures` compiles more than this; finding fewer means the
        // directory scan regressed (e.g. a moved fixture root) or the fixtures
        // were not compiled, not that contracts went away. Keep this in sync
        // when fixtures are added or removed.
        assert!(
            fixtures.len() >= 8,
            "fixture scan found only {} analyzed-ir files, expected at least 8",
            fixtures.len()
        );
        for path in fixtures {
            let name = fixture_contract_name(&path);
            let rel = path.display();
            let info = crate::artifact::load(&path).unwrap_or_else(|e| panic!("parse {rel}: {e}"));
            let generated = generated_source(&info, &name);
            assert_no_panic_paths(&name, &generated);
        }
    }

    /// An `Opaque` argument must reach the chain. The emitter used to encode
    /// every opaque type other than `JubjubPoint` / `Scalar<BLS12-381>` as
    /// `AlignedValue::from(())`, so a generated call builder bound the caller's
    /// argument and then sent unit in its place.
    #[test]
    fn opaque_arguments_are_encoded_not_dropped() {
        let info =
            load_fixture("../../midnight-contract/tests/fixtures/bboard/compiler/analyzed-ir.sexp");
        let generated = generated_source(&info, "Bboard");

        assert!(
            generated.contains("AlignedValue::from(new_message)"),
            "the `post` circuit should encode its `new_message` argument"
        );
        // Scoped to the argument: `AlignedValue::from(())` is legitimate
        // elsewhere (empty tuples).
        let flat: String = generated.split_whitespace().collect();
        assert!(
            !flat.contains(r#""new_message",midnight_contract::runtime::Value::AlignedValue(AlignedValue::from(()))"#),
            "the `new_message` argument is still encoded as unit"
        );
    }

    /// A contract-typed argument takes the generated `ContractAddress`, the
    /// struct that the compiler encodes a contract value as, and the call
    /// sends it. A unit encoding of the argument compiles too, so a build
    /// alone does not catch it.
    #[test]
    fn contract_arguments_are_typed_and_encoded() {
        let info =
            load_fixture("../../../tests/conformance/fixtures/peers/compiler/analyzed-ir.sexp");
        let flat = generated_source(&info, "Peers")
            .split_whitespace()
            .collect::<String>()
            .replace(",)", ")");

        assert!(
            flat.contains("fnswap_peer(&mutself,p:ContractAddress)"),
            "the `swap_peer` circuit should take its `p` argument as a `ContractAddress`"
        );
        assert!(
            flat.contains(
                r#"("p",midnight_contract::runtime::Value::AlignedValue(AlignedValue::from(p)))"#
            ),
            "the `swap_peer` circuit should encode its `p` argument"
        );
    }

    /// `ContractAddress` has the same shape as the standard library's coin key,
    /// so only the name keeps a coin key from converting into a contract address.
    #[test]
    fn only_the_coin_key_struct_converts_from_the_sdk_coin_key() {
        let info =
            load_fixture("../../../tests/fixtures/compiled/mint-probe/compiler/analyzed-ir.sexp");
        let generated = generated_source(&info, "MintProbe");

        assert!(generated.contains("pub struct ContractAddress"));
        assert_eq!(
            generated
                .matches("impl From<midnight_contract::CoinPublicKey> for")
                .count(),
            1
        );
        assert!(
            generated
                .contains("impl From<midnight_contract::CoinPublicKey> for ZswapCoinPublicKey")
        );
    }
}
