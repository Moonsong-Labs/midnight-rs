//! Generate embedded IR constants and helper/struct/enum JSON for circuit calls.
//!
//! We generate:
//! - An `__IR_<NAME>` constant per impure circuit that has embedded IR
//! - One `__HELPERS_JSON`, `__STRUCTS_JSON`, `__ENUMS_JSON` constant each,
//!   shared across all circuits in the contract

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use std::collections::HashMap;

use crate::ir::EnumDef;
use crate::types::{ContractInfo, StructElement, TypeNode};

use super::types::{encode_to_aligned_value, type_to_tokens};

/// Walk every `TypeNode` reachable from `info` (ledger fields, circuit
/// args/results, witness args/results, struct fields) and collect a
/// deduplicated list of `EnumDef`s. Variant order is preserved (it
/// matches the on-chain `u8` index).
pub(crate) fn collect_enum_defs(info: &ContractInfo) -> Vec<EnumDef> {
    let mut acc: HashMap<String, Vec<String>> = HashMap::new();

    fn visit(node: &TypeNode, acc: &mut HashMap<String, Vec<String>>) {
        match node {
            TypeNode::Enum { name, elements } => {
                acc.entry(name.clone()).or_insert_with(|| elements.clone());
            }
            TypeNode::Vector { inner, .. } => visit(inner, acc),
            TypeNode::Tuple { types } => {
                for t in types {
                    visit(t, acc);
                }
            }
            TypeNode::Struct { elements, .. } => {
                for StructElement { type_node, .. } in elements {
                    visit(type_node, acc);
                }
            }
            TypeNode::Alias { inner, .. } => visit(inner, acc),
            _ => {}
        }
    }

    for f in &info.ledger {
        if let Some(t) = f.element_type.as_ref() {
            visit(t, &mut acc);
        }
        if let Some(t) = f.key.as_ref() {
            visit(t, &mut acc);
        }
        if let Some(t) = f.value.as_ref() {
            visit(t, &mut acc);
        }
    }
    for c in &info.circuits {
        for arg in &c.arguments {
            visit(&arg.type_node, &mut acc);
        }
        visit(&c.result_type, &mut acc);
    }
    for w in &info.witnesses {
        for arg in &w.arguments {
            visit(&arg.type_node, &mut acc);
        }
        visit(&w.result_type, &mut acc);
    }

    let mut out: Vec<EnumDef> = acc
        .into_iter()
        .map(|(name, variants)| EnumDef { name, variants })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Generate embedded IR constants and helper/struct/enum JSON constants.
///
/// Returns a token stream to be spliced into the Ledger `impl` block.
/// The `Circuits` struct (in `ledger.rs`) references these constants for
/// on-chain circuit calls.
pub(crate) fn emit_circuit_ir_constants(info: &ContractInfo) -> TokenStream {
    let mut ir_consts = Vec::new();

    for circuit in &info.circuits {
        // Only generate IR constants for impure circuits with IR
        if circuit.pure || circuit.ir.is_none() {
            continue;
        }

        // Infallible here: `validate::check_embedded_json` round-trips every
        // circuit's IR before expansion starts. The old `continue` on error
        // silently skipped the constant while `ledger.rs` still referenced it.
        let ir_json = serde_json::to_string(&circuit.ir)
            .expect("circuit IR serialization is checked during validation");

        let sanitized = circuit.name.replace(['$', '-'], "_");
        let ir_const = format_ident!("__IR_{}", sanitized.to_uppercase());

        ir_consts.push(quote! {
            #[doc(hidden)]
            pub const #ir_const: &str = #ir_json;
        });
    }

    // Embed the contract-level helper definitions as a single JSON constant
    // so the async `Circuits` wrappers in `ledger.rs` can hand them to
    // `execute_with`. The compiler emits user-defined helper circuits,
    // including ones that aren't declared `pure circuit`, into
    // `info.helpers` so the interpreter can resolve `call-pure` IR ops at
    // runtime without inlining them at compile time. Always emitted (empty
    // array if none) so callers can unconditionally reference
    // `Self::__HELPERS_JSON`.
    // Infallible: round-tripped by `validate::check_embedded_json` before
    // expansion (a silent `[]` fallback here would drop definitions the
    // interpreter needs at runtime).
    let helpers_json = serde_json::to_string(&info.helpers)
        .expect("helper serialization is checked during validation");

    // Nested struct/enum types used by circuit arguments are declared *inline*
    // in each circuit's `arguments` (with `elements`), not referenced from the
    // top-level `structs` array. Harvest them into the registry so the
    // interpreter can compute atom layouts when a circuit destructures a struct
    // argument (e.g. `recipient.is_left`) on the funded call path.
    let mut structs = info.structs.clone();
    let mut enum_defs = collect_enum_defs(info);
    for circuit in &info.circuits {
        crate::arg_types::collect_argument_defs(&circuit.arguments, &mut structs, &mut enum_defs);
    }
    for witness in &info.witnesses {
        crate::arg_types::collect_argument_defs(&witness.arguments, &mut structs, &mut enum_defs);
    }

    let structs_json =
        serde_json::to_string(&structs).expect("struct serialization is checked during validation");

    // Walk every TypeNode in `info` and collect each `Enum { name, elements }`
    // it references. The interpreter uses this to resolve enum variant
    // names to their declaration index when decoding `lit type=Enum value="<name>"`.
    let enums_json =
        serde_json::to_string(&enum_defs).expect("enum serialization is checked during validation");

    quote! {
        #[doc(hidden)]
        pub const __HELPERS_JSON: &str = #helpers_json;
        #[doc(hidden)]
        pub const __STRUCTS_JSON: &str = #structs_json;
        #[doc(hidden)]
        pub const __ENUMS_JSON: &str = #enums_json;
        #(#ir_consts)*
    }
}

/// Returns true if this TypeNode represents void (empty tuple).
pub(crate) fn is_void_type(ty: &TypeNode) -> bool {
    match ty {
        TypeNode::Tuple { types } if types.is_empty() => true,
        TypeNode::Alias { inner, .. } => is_void_type(inner),
        _ => false,
    }
}

/// Generate a token stream expression that converts `midnight_contract::interpreter::Value`
/// (in variable `__val`) to the target Rust type. Used for circuit return
/// values and typed witness arguments.
///
/// `context` is a codegen-time label naming what is being converted (e.g.
/// ``circuit `increment` return value`` or ``witness `secret_key` argument
/// `idx` ``); it is baked into the generated `TypeError` messages so a
/// mismatch names its source instead of just the expected shape.
///
/// The generated expression evaluates to
/// `Result<T, midnight_contract::interpreter::InterpreterError>` — the
/// interpreter's output is contract-data dependent, so a mismatch must flow
/// into the caller's error path instead of panicking. Callers `?` it: the
/// witness adapter already returns `InterpreterError`, and the async circuit
/// methods convert via `From<InterpreterError> for ContractError`.
pub(crate) fn value_to_type_conversion(ty: &TypeNode, context: &str) -> TokenStream {
    match ty {
        TypeNode::Boolean => {
            let mismatch_msg = format!("{context}: expected a Bool value, got {{:?}}");
            quote! {
                match __val {
                    midnight_contract::interpreter::Value::Bool(__b) => {
                        ::core::result::Result::Ok(__b)
                    }
                    __other => ::core::result::Result::Err(
                        midnight_contract::interpreter::InterpreterError::TypeError(
                            ::std::format!(#mismatch_msg, __other)
                        )
                    ),
                }
            }
        }
        TypeNode::Uint { .. } => {
            let rust_ty = type_to_tokens(ty);
            let overflow_msg = format!("{context}: value {{}} does not fit in {{}}");
            let mismatch_msg = format!("{context}: expected an Integer value, got {{:?}}");
            quote! {
                match __val {
                    midnight_contract::interpreter::Value::Integer(__n) => {
                        <#rust_ty>::try_from(__n).map_err(|_| {
                            midnight_contract::interpreter::InterpreterError::TypeError(
                                ::std::format!(
                                    #overflow_msg,
                                    __n,
                                    ::core::stringify!(#rust_ty)
                                )
                            )
                        })
                    }
                    __other => ::core::result::Result::Err(
                        midnight_contract::interpreter::InterpreterError::TypeError(
                            ::std::format!(#mismatch_msg, __other)
                        )
                    ),
                }
            }
        }
        TypeNode::Alias { inner, .. } => value_to_type_conversion(inner, context),
        _ => {
            let rust_ty = type_to_tokens(ty);
            let convert_msg = format!("{context}: failed to convert value to {{}}: {{}}");
            let mismatch_msg = format!("{context}: expected an AlignedValue, got {{:?}}");
            quote! {
                match __val {
                    midnight_contract::interpreter::Value::AlignedValue(__av) => {
                        <#rust_ty>::try_from(&*__av.value).map_err(|__e| {
                            midnight_contract::interpreter::InterpreterError::TypeError(
                                ::std::format!(
                                    #convert_msg,
                                    ::core::stringify!(#rust_ty),
                                    __e
                                )
                            )
                        })
                    }
                    __other => ::core::result::Result::Err(
                        midnight_contract::interpreter::InterpreterError::TypeError(
                            ::std::format!(#mismatch_msg, __other)
                        )
                    ),
                }
            }
        }
    }
}

/// Returns true if this type has a direct conversion into `AlignedValue`
/// (and therefore into `interpreter::Value::AlignedValue`) via the
/// bindgen-emitted encoders. Keep in sync with `type_to_value_conversion`.
pub(crate) fn has_typed_conversion(ty: &TypeNode) -> bool {
    match ty {
        TypeNode::Boolean
        | TypeNode::Uint { .. }
        | TypeNode::Field
        | TypeNode::Bytes { .. }
        | TypeNode::Struct { .. }
        | TypeNode::Enum { .. }
        | TypeNode::Vector { .. }
        | TypeNode::Tuple { .. } => true,
        TypeNode::Alias { inner, .. } => has_typed_conversion(inner),
        TypeNode::Opaque { ts_type } => matches!(
            ts_type.as_deref(),
            Some("JubjubPoint") | Some("Scalar<BLS12-381>")
        ),
        TypeNode::Contract { .. } | TypeNode::Unknown { .. } => false,
    }
}

/// Generate the expression to convert a typed Rust argument to
/// `interpreter::Value`. Scalars stay as native variants; compound types
/// are encoded via `From<T> for AlignedValue` and wrapped in
/// `Value::AlignedValue(_)`.
pub(crate) fn type_to_value_conversion(
    arg_ident: &proc_macro2::Ident,
    ty: &TypeNode,
) -> TokenStream {
    match ty {
        TypeNode::Boolean => {
            quote! { midnight_contract::interpreter::Value::Bool(#arg_ident) }
        }
        TypeNode::Uint { .. } => {
            quote! { midnight_contract::interpreter::Value::Integer(#arg_ident as u128) }
        }
        // Vector arguments must be passed as `Value::Tuple` so the
        // interpreter's `index` op can walk into individual elements
        // (used by the unrolled `map`/`fold` lowering). Pre-flattening
        // a `Vector<N, T>` into a single `AlignedValue` would prevent
        // structural indexing — the interpreter would only see opaque
        // atoms with no element boundary.
        //
        // The flatten-to-`AlignedValue` step still happens at the prover
        // boundary via `Value::to_aligned_value`, which walks `Value::Tuple`
        // recursively. So this change preserves the on-chain encoding while
        // letting the off-chain interpreter index per-element.
        TypeNode::Vector { inner, .. } => {
            let elem_ident = format_ident!("__vec_elem");
            let elem_conv = type_to_value_conversion(&elem_ident, inner);
            quote! {
                midnight_contract::interpreter::Value::Tuple(
                    ::std::iter::IntoIterator::into_iter(#arg_ident)
                        .map(|#elem_ident| #elem_conv)
                        .collect::<::std::vec::Vec<_>>()
                )
            }
        }
        TypeNode::Alias { inner, .. } => type_to_value_conversion(arg_ident, inner),
        _ => {
            let av = encode_to_aligned_value(&quote! { #arg_ident }, ty);
            quote! { midnight_contract::interpreter::Value::AlignedValue(#av) }
        }
    }
}
