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

use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::quote;

use crate::error::CodegenError;
use crate::types::ContractInfo;

/// Expansion context that centralises resolution state across all emitters.
pub(crate) struct EmitCtxt<'a> {
    pub info: &'a ContractInfo,
    pub contract_name: &'a str,
    pub crate_path: &'a TokenStream,
    /// Tracks which struct/enum names have already been emitted (deduplication).
    pub emitted_types: HashSet<String>,
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
            emitted_types: HashSet::new(),
        }
    }

    /// Run the full expansion pipeline, returning the combined `TokenStream`.
    ///
    /// Validates the contract info first (version gate, unknown type nodes,
    /// embedded JSON round-trip); any violation aborts with a [`CodegenError`]
    /// instead of generating broken or panicking code.
    pub fn expand(&mut self) -> Result<TokenStream, CodegenError> {
        crate::validate::validate(self.info)?;

        let crate_path = self.crate_path;
        let constants = constants::emit_field_constants(&self.info.ledger);
        let data_types = data_types::emit_data_types(
            &self.info.ledger,
            &self.info.circuits,
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
        let lazy_wrapper = ledger::emit_lazy_ledger_wrapper(&self.info.ledger, self.contract_name);

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
                ListAccessor, MapAccessor, MerkleTreeAccessor, SetAccessor, StateError,
                StateValue, StorageArray, StorageHashMap, TransientFr, ValueSlice, Vector,
                cell_value, get_field, get_field_path, hex, lazy, serde, serde_json,
                tagged_deserialize, variant_name,
            };
        };

        Ok(quote! {
            #runtime_imports
            #contract_import

            #constants
            #data_types
            #circuit_types
            #wrapper

            mod __lazy_query {
                use super::*;
                #lazy_wrapper
            }
            pub use __lazy_query::*;
        })
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
) -> Result<TokenStream, CodegenError> {
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
        let tokens = generate_bindings(info, contract_name, None).expect("codegen should succeed");
        let file: syn::File =
            syn::parse2(tokens).expect("generated code must be a valid Rust file");
        prettyplease::unparse(&file)
    }

    use super::helpers::{make_ident, to_pascal_case};
    use super::types::{type_to_tokens, uint_tokens};
    use super::*;

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
    }

    #[test]
    fn pascal_case() {
        assert_eq!(to_pascal_case("pending"), "Pending");
        assert_eq!(to_pascal_case("some_value"), "SomeValue");
    }

    #[test]
    fn tuple_type_mapping() {
        use crate::ir::Type;

        // Empty tuple -> unit type.
        let empty = type_to_tokens(&Type::unit()).to_string();
        assert_eq!(empty, "()");

        // Single-element tuple needs trailing comma.
        let single = type_to_tokens(&Type::Tuple(vec![Type::Boolean])).to_string();
        assert!(single.contains("bool") && single.contains(','));

        // Multi-element tuple.
        let multi = type_to_tokens(&Type::Tuple(vec![
            Type::Boolean,
            Type::Unsigned(255u32.into()),
        ]))
        .to_string();
        assert!(multi.contains("bool") && multi.contains("u8"));
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
    /// contract-info.json content, interpreter output, or provider responses.
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

    /// Collect every committed `analyzed-ir.sexp` fixture under the
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

    /// Expand every committed fixture and assert the output is panic-free.
    #[test]
    fn generated_code_has_no_panic_paths() {
        let fixtures = contract_info_fixtures();
        // 10 fixtures are committed today; finding fewer means the directory
        // scan regressed (e.g. a moved fixture root), not that contracts went
        // away. Keep this in sync when fixtures are added or removed.
        assert!(
            fixtures.len() >= 10,
            "fixture scan found only {} analyzed-ir files, expected at least 10",
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
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../midnight-contract/tests/fixtures/bboard/compiler/analyzed-ir.sexp");
        let info = crate::artifact::load(&path).unwrap();
        let generated = generated_source(&info, "Bboard");

        assert!(
            generated.contains("AlignedValue::from(new_message)"),
            "the `post` circuit should encode its `new_message` argument"
        );
        // Scoped to the argument: `AlignedValue::from(())` is legitimate
        // elsewhere (empty tuples, unset-cell defaults).
        let flat: String = generated.split_whitespace().collect();
        assert!(
            !flat.contains(r#""new_message",midnight_contract::runtime::Value::AlignedValue(AlignedValue::from(()))"#),
            "the `new_message` argument is still encoded as unit"
        );
    }

    #[test]
    fn generate_gateway_crate() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/compiled/gateway/compiler/analyzed-ir.sexp");
        let info = crate::artifact::load(&path).unwrap();
        let generated = generated_source(&info, "Gateway");

        // Ledger types and accessors
        assert!(generated.contains("FIELD_THRESHOLD"));
        assert!(generated.contains("pub struct EgressJob"));
        assert!(generated.contains("pub enum JobStatus"));
        assert!(generated.contains("pub struct Gateway"));
        assert!(generated.contains("fn new("));
        assert!(generated.contains("fn threshold("));
        assert!(generated.contains("fn egress_jobs("));

        // New-style impls: Aligned + TryFrom<&ValueSlice>
        assert!(generated.contains("impl Aligned for EgressJob"));
        assert!(generated.contains("impl Aligned for JobStatus"));
        assert!(
            generated.contains("TryFrom<&'a ValueSlice> for EgressJob")
                || generated.contains("TryFrom<&'a ValueSlice>")
        );

        // Old-style impls must NOT be present
        assert!(!generated.contains("TryFromStateValue"));
        assert!(!generated.contains("TryFromAlignedValue"));
        assert!(!generated.contains("try_from_atoms"));
        assert!(generated.contains("from_hex"));

        // Circuit call types
        assert!(generated.contains("pub struct ClaimDepositCall"));
        assert!(generated.contains("pub struct WithdrawCall"));
        assert!(generated.contains("ClaimDepositReturn"));
        assert!(generated.contains("WithdrawReturn"));
        assert!(generated.contains("pub enum Calls"));
        assert!(generated.contains("ClaimDeposit"));
        assert!(generated.contains("Withdraw"));

        // Struct types referenced only in circuits (not in ledger)
        assert!(generated.contains("pub struct Maybe"));
        assert!(generated.contains("pub struct ValidatorSignature"));
        assert!(generated.contains("pub struct ShieldedCoinInfo"));

        // Maybe gets into_option()
        assert!(generated.contains("into_option"));

        // cell_value / get_field used in accessor bodies
        assert!(generated.contains("cell_value"));
        assert!(generated.contains("get_field"));

        // Lazy query wrapper — map fields get key-lookup accessors
        assert!(
            generated.contains("pub struct GatewayQuery"),
            "missing lazy GatewayQuery struct"
        );
        assert!(
            generated.contains("async fn threshold("),
            "missing async threshold accessor in GatewayQuery"
        );
        // Map accessor takes a key argument
        assert!(
            generated.contains("async fn egress_jobs("),
            "missing async egress_jobs accessor in GatewayQuery"
        );
        assert!(
            generated.contains("value_to_query_key"),
            "missing value_to_query_key call for map key lookup"
        );
    }

    #[test]
    fn generate_counter_crate() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/compiled/counter/compiler/analyzed-ir.sexp");
        let info = crate::artifact::load(&path).unwrap();
        let generated = generated_source(&info, "Counter");

        // Verify it generated valid Rust (syn::parse2 inside tokens_to_string would panic otherwise)
        assert!(!generated.is_empty());

        // Counter has one exported ledger field: round (Counter storage)
        assert!(generated.contains("FIELD_ROUND"));
        assert!(generated.contains("fn round("));

        // Circuit types
        assert!(generated.contains("pub struct IncrementCall"));
        assert!(generated.contains("IncrementReturn"));

        // Calls enum with the single circuit
        assert!(generated.contains("pub enum Calls"));
        assert!(generated.contains("Increment"));

        // Ledger wrapper struct
        assert!(generated.contains("pub struct Counter"));
        assert!(generated.contains("fn new("));
        assert!(generated.contains("from_hex"));

        // Lazy query wrapper (behind cfg(feature = "provider"))
        assert!(
            generated.contains("pub struct CounterQuery"),
            "missing lazy CounterQuery struct"
        );
        assert!(
            generated.contains("async fn round("),
            "missing async round accessor in CounterQuery"
        );
        assert!(
            generated.contains("query_contract_state"),
            "missing query_contract_state call in lazy accessor"
        );
        assert!(
            generated.contains("decode_state_value"),
            "missing decode_state_value call in lazy accessor"
        );
    }

    #[test]
    fn generate_election_crate() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/compiled/election/compiler/analyzed-ir.sexp");
        let info = crate::artifact::load(&path).expect("election analyzed-ir.sexp should parse");
        let generated = generated_source(&info, "Election");

        // Verify it generated valid Rust
        assert!(!generated.is_empty());

        // Multiple circuits
        assert!(generated.contains("pub enum Calls"));
        assert!(generated.contains("AdvanceCall"));
        assert!(generated.contains("SetTopicCall"));
        assert!(generated.contains("AddVoterCall"));

        // Enum types from witnesses
        assert!(generated.contains("pub enum PermissibleVotes"));
        assert!(generated.contains("pub enum PrivateState"));

        // Maybe struct with into_option
        assert!(generated.contains("pub struct Maybe"));
        assert!(generated.contains("into_option"));

        // Ledger wrapper with fields
        assert!(generated.contains("pub struct Election"));
        assert!(generated.contains("FIELD_AUTHORITY"));
        assert!(generated.contains("FIELD_STATE"));
        assert!(generated.contains("FIELD_TALLY_YES"));
        assert!(generated.contains("FIELD_TALLY_NO"));
        assert!(generated.contains("FIELD_COMMITTED"));
        assert!(generated.contains("FIELD_REVEALED"));

        // Storage kind accessors
        assert!(generated.contains("fn authority("));
        assert!(generated.contains("fn tally_yes("));
        assert!(generated.contains("SetAccessor"));

        // Merkle tree accessors
        assert!(generated.contains("MerkleTreeAccessor"));
        assert!(generated.contains("fn committed_votes("));
        assert!(generated.contains("fn eligible_voters("));

        // Lazy query wrapper
        assert!(
            generated.contains("pub struct ElectionQuery"),
            "missing lazy ElectionQuery struct"
        );
        // Cell/counter fields should have lazy accessors
        assert!(
            generated.contains("async fn authority("),
            "missing async authority accessor in ElectionQuery"
        );
        assert!(
            generated.contains("async fn tally_yes("),
            "missing async tally_yes accessor in ElectionQuery"
        );
        // Set fields should have lazy membership accessors
        assert!(
            generated.contains("async fn committed("),
            "missing async committed set accessor in ElectionQuery"
        );
    }

    #[test]
    fn generate_tiny_crate() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/compiled/tiny/compiler/analyzed-ir.sexp");
        let info = crate::artifact::load(&path).expect("tiny analyzed-ir.sexp should parse");
        let generated = generated_source(&info, "Tiny");

        // Verify it generated valid Rust
        assert!(!generated.is_empty());

        // Circuit types
        assert!(generated.contains("pub struct SetCall"));
        assert!(generated.contains("pub struct GetCall"));
        assert!(generated.contains("pub struct ClearCall"));
        assert!(generated.contains("pub struct PublicKeyCall"));
        assert!(generated.contains("pub enum Calls"));

        // Witness call type (name contains $ which to_pascal_case doesn't split on)
        assert!(
            generated.contains("Private$secretKeyCall")
                || generated.contains("Private$secret_keyCall")
                || generated.contains("PrivateSecretKeyCall")
                || generated.contains("Private")
        );

        // Ledger wrapper
        assert!(generated.contains("pub struct Tiny"));
    }

    #[test]
    fn generate_zerocash_crate() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/compiled/zerocash/compiler/analyzed-ir.sexp");
        let info = crate::artifact::load(&path).expect("zerocash analyzed-ir.sexp should parse");
        let generated = generated_source(&info, "Zerocash");

        // Verify it generated valid Rust
        assert!(!generated.is_empty());

        // Circuit types
        assert!(generated.contains("pub struct SpendCall"));
        assert!(generated.contains("SpendReturn"));
        assert!(generated.contains("pub enum Calls"));

        // Custom struct types from witnesses
        assert!(generated.contains("pub struct Nonce"));
        assert!(generated.contains("pub struct MerkleTreePath"));
        assert!(generated.contains("pub struct MerkleTreePathEntry"));
        assert!(generated.contains("pub struct MerkleTreeDigest"));

        // Ledger wrapper
        assert!(generated.contains("pub struct Zerocash"));

        // Historic merkle tree accessor (commitments field)
        assert!(generated.contains("MerkleTreeAccessor"));
        assert!(generated.contains("fn commitments("));
    }

    #[test]
    fn generate_many_fields_crate() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/compiled/many-fields/compiler/analyzed-ir.sexp");
        let info = crate::artifact::load(&path).unwrap();
        let generated = generated_source(&info, "ManyFields");

        // Verify it generated valid Rust
        assert!(!generated.is_empty());

        // All 16 fields should have accessors
        for i in 1..=16 {
            let field_name = format!("fn f{i:02}(");
            assert!(
                generated.contains(&field_name),
                "missing accessor for f{i:02}"
            );
        }

        // B-tree path indices: should use get_field_path, not just get_field
        assert!(
            generated.contains("get_field_path"),
            "expected get_field_path for B-tree indices but got:\n{}",
            generated
        );

        // Wrapper struct
        assert!(generated.contains("pub struct ManyFields"));

        // Lazy query wrapper with B-tree path fields
        assert!(
            generated.contains("pub struct ManyFieldsQuery"),
            "missing lazy ManyFieldsQuery struct"
        );
        // All 16 fields are cells, so all should have lazy accessors
        for i in 1..=16 {
            let field_name = format!("async fn f{i:02}(");
            assert!(
                generated.contains(&field_name),
                "missing async lazy accessor for f{i:02}"
            );
        }
    }

    /// The fixture's cells sit at `(0 0)` and `(1 0)..(1 14)`, so its deploy
    /// state is an array of two arrays. `build()` used to flatten every cell
    /// into one array instead — a layout no path accessor reads, and one the
    /// runtime rejects outright once a ledger passes sixteen cells.
    #[test]
    fn many_fields_initial_state_nests_like_the_artifact() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/compiled/many-fields/compiler/analyzed-ir.sexp");
        let info = crate::artifact::load(&path).unwrap();
        let generated = generated_source(&info, "ManyFields");
        let flat: String = generated.split_whitespace().collect();

        // The state expression `build()` hands to `ContractState::new`.
        let state = flat
            .split("fnbuild(self)->ContractState<InMemoryDB>{ContractState::new(")
            .nth(1)
            .and_then(|tail| tail.split(",StorageHashMap::new()").next())
            .unwrap_or_else(|| panic!("no build() on ManyFieldsInitialState:\n{generated}"));

        assert!(
            state.starts_with(
                "StateValue::Array(vec![StateValue::Array(vec![StateValue::from(AlignedValue::from(self.f01))].into()),StateValue::Array(vec![StateValue::from(AlignedValue::from(self.f02)),"
            ),
            "expected f01 alone in the first group and f02 opening the second, got: {state}"
        );
        assert!(
            state.contains("AlignedValue::from(self.f16))].into())].into()"),
            "expected f16 to close the second group and the root array, got: {state}"
        );
    }

    #[test]
    fn generate_counter_with_ir() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/conformance/fixtures/counter/compiler/analyzed-ir.sexp");
        let info = crate::artifact::load(&path).unwrap();
        assert!(
            info.circuits.iter().any(|c| !c.pure()),
            "the counter fixture should carry an on-chain circuit"
        );
        let generated = generated_source(&info, "Counter");

        // The declarations are embedded as typed constructors the compiler
        // checks, and the call path resolves its circuit out of them rather
        // than carrying a second copy.
        assert!(
            generated.contains("fn __helpers()"),
            "missing __helpers() constructor"
        );
        assert!(
            generated.contains("Program::new") && generated.contains(".circuit(\""),
            "the call path should resolve its circuit from the program"
        );
        assert!(
            !generated.contains("fn __ir_"),
            "a circuit body should not be embedded twice"
        );

        assert!(
            generated.contains("midnight_contract"),
            "missing midnight_contract reference"
        );
        assert!(
            generated.contains("fn contract_state("),
            "missing contract_state() accessor"
        );
        assert!(
            generated.contains("fn into_contract_state("),
            "missing into_contract_state() accessor"
        );
    }

    #[test]
    fn generate_gateway_initial_state() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/compiled/gateway/compiler/analyzed-ir.sexp");
        let info = crate::artifact::load(&path).unwrap();
        let generated = generated_source(&info, "Gateway");

        // InitialState struct
        assert!(
            generated.contains("pub struct GatewayInitialState"),
            "missing GatewayInitialState struct"
        );

        // Fields with correct types
        assert!(
            generated.contains("pub threshold:"),
            "missing threshold field in InitialState"
        );
        assert!(
            generated.contains("pub signing_fee:"),
            "missing signing_fee field in InitialState"
        );
        assert!(
            generated.contains("pub next_job_id: u64"),
            "missing next_job_id counter field in InitialState"
        );

        // Default impl
        assert!(
            generated.contains("impl Default for GatewayInitialState"),
            "missing Default impl for GatewayInitialState"
        );

        // build() method
        assert!(
            generated.contains("fn build(self) -> ContractState"),
            "missing build() on InitialState"
        );

        // into_ledger() method
        assert!(
            generated.contains("fn into_ledger(self) -> Gateway"),
            "missing into_ledger() on InitialState"
        );

        // from_provider on Ledger
        assert!(
            generated.contains("async fn from_provider"),
            "missing from_provider on Gateway"
        );
    }

    #[test]
    fn generate_empty_contract() {
        let info = ContractInfo {
            compiler_version: "0.33.122".to_string(),
            language_version: "0.25.107".to_string(),
            runtime_version: "0.16.101".to_string(),
            circuits: vec![crate::types::Circuit {
                name: "noop".to_string(),
                def: crate::ir::Circuit {
                    name: crate::ir::Ident("noop".to_string()),
                    exported: true,
                    pure: true,
                    proof: false,
                    arguments: Vec::new(),
                    result_type: crate::ir::Type::unit(),
                    body: crate::ir::Expr::Seq(Vec::new()),
                },
            }],
            witnesses: Vec::new(),
            contracts: Vec::new(),
            ledger: Vec::new(),
            helpers: Vec::new(),
            natives: Vec::new(),
        };
        let generated = generated_source(&info, "Empty");

        assert!(!generated.is_empty());
        assert!(generated.contains("pub struct Empty"));
        assert!(generated.contains("fn new("));
        assert!(generated.contains("NoopCall"));
        // No FIELD_ constants for empty ledger
        assert!(!generated.contains("FIELD_"));

        // Lazy query wrapper still generated (but with no accessors)
        assert!(
            generated.contains("pub struct EmptyQuery"),
            "missing lazy EmptyQuery struct"
        );
    }
}
