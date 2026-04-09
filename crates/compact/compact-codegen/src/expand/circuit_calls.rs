//! Generate circuit call methods on the Ledger struct.
//!
//! For each impure circuit that has embedded IR, we generate:
//! - A `call_<name>` method that executes the circuit against the current state
//! - Embedded IR JSON as a const string, deserialized on first use

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use std::collections::HashMap;

use crate::ir::EnumDef;
use crate::types::{Circuit, ContractInfo, StructElement, TypeNode};

use super::helpers::make_ident;
use super::types::{encode_to_aligned_value, type_to_tokens};

/// Walk every `TypeNode` reachable from `info` (ledger fields, circuit
/// args/results, witness args/results, struct fields) and collect a
/// deduplicated list of `EnumDef`s. Variant order is preserved (it
/// matches the on-chain `u8` index).
fn collect_enum_defs(info: &ContractInfo) -> Vec<EnumDef> {
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

/// Generate circuit call methods and the embedded IR/helpers constants.
///
/// Returns a token stream to be spliced into the Ledger `impl` block.
pub(crate) fn emit_circuit_call_methods(info: &ContractInfo) -> TokenStream {
    let mut methods = Vec::new();

    for circuit in &info.circuits {
        // Only generate call methods for impure circuits with IR
        if circuit.pure || circuit.ir.is_none() {
            continue;
        }

        let ir_json = match serde_json::to_string(&circuit.ir) {
            Ok(json) => json,
            Err(_) => continue,
        };

        methods.push(emit_call_method(circuit, &ir_json));
    }

    // Embed the contract-level helper definitions as a single JSON constant
    // so the generated `call_<circuit>` methods (and the async `Circuits`
    // wrappers in `ledger.rs`) can hand them to `execute_with`. The compiler
    // emits user-defined helper circuits — including ones that aren't
    // declared `pure circuit` — into `info.helpers` so the interpreter can
    // resolve `call-pure` IR ops at runtime without inlining them at
    // compile time. Always emitted (empty array if none) so callers can
    // unconditionally reference `Self::__HELPERS_JSON`.
    let helpers_json = serde_json::to_string(&info.helpers).unwrap_or_else(|_| "[]".to_string());
    let structs_json = serde_json::to_string(&info.structs).unwrap_or_else(|_| "[]".to_string());

    // Walk every TypeNode in `info` and collect each `Enum { name, elements }`
    // it references. The interpreter uses this to resolve enum variant
    // names to their declaration index when decoding `lit type=Enum value="<name>"`.
    let enum_defs = collect_enum_defs(info);
    let enums_json = serde_json::to_string(&enum_defs).unwrap_or_else(|_| "[]".to_string());

    let helpers_const = quote! {
        #[doc(hidden)]
        pub const __HELPERS_JSON: &str = #helpers_json;
        #[doc(hidden)]
        pub const __STRUCTS_JSON: &str = #structs_json;
        #[doc(hidden)]
        pub const __ENUMS_JSON: &str = #enums_json;
    };

    if methods.is_empty() {
        return quote! { #helpers_const };
    }

    quote! {
        #helpers_const
        #(#methods)*
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
        TypeNode::Contract { .. } | TypeNode::Unknown => false,
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

fn emit_call_method(circuit: &Circuit, ir_json: &str) -> TokenStream {
    let sanitized = circuit.name.replace(['$', '-'], "_");
    let method_name = format_ident!("call_{}", sanitized);
    let circuit_name_str = &circuit.name;
    let ir_const = format_ident!("__IR_{}", sanitized.to_uppercase());

    let doc = format!(
        "Execute the `{}` circuit against the current contract state.\n\n\
         Returns the updated ledger wrapping the new state on success.",
        circuit.name
    );

    // Generate typed argument parameters and conversion to Value
    let (params, arg_bindings) = if circuit.arguments.is_empty() {
        (quote! {}, quote! { &[] })
    } else {
        let param_list: Vec<_> = circuit
            .arguments
            .iter()
            .map(|arg| {
                let name = make_ident(&arg.name);
                if has_typed_conversion(&arg.type_node) {
                    let ty = type_to_tokens(&arg.type_node);
                    quote! { #name: #ty }
                } else {
                    quote! { #name: midnight_contract::interpreter::Value }
                }
            })
            .collect();

        let binding_list: Vec<_> = circuit
            .arguments
            .iter()
            .map(|arg| {
                let name_str = &arg.name;
                let name_ident = make_ident(&arg.name);
                let conversion = type_to_value_conversion(&name_ident, &arg.type_node);
                quote! { (#name_str, #conversion) }
            })
            .collect();

        (
            quote! { , #(#param_list),* },
            quote! { &[#(#binding_list),*] },
        )
    };

    quote! {
        #[doc(hidden)]
        pub const #ir_const: &str = #ir_json;

        #[doc = #doc]
        pub fn #method_name(
            &self
            #params
        ) -> Result<Self, midnight_contract::interpreter::InterpreterError> {
            let ir: midnight_contract::compact_codegen::ir::CircuitIrBody =
                serde_json::from_str(Self::#ir_const).expect(
                    concat!("embedded IR for `", #circuit_name_str, "` must be valid JSON")
                );

            let helpers: Vec<midnight_contract::compact_codegen::ir::HelperDef> =
                serde_json::from_str(Self::__HELPERS_JSON).expect(
                    "embedded helper definitions must be valid JSON"
                );

            let structs: Vec<midnight_contract::compact_codegen::ir::StructDef> =
                serde_json::from_str(Self::__STRUCTS_JSON).expect(
                    "embedded struct definitions must be valid JSON"
                );

            let enums: Vec<midnight_contract::compact_codegen::ir::EnumDef> =
                serde_json::from_str(Self::__ENUMS_JSON).expect(
                    "embedded enum definitions must be valid JSON"
                );

            let result = midnight_contract::interpreter::execute_with_enums(
                &ir,
                &self.state,
                #arg_bindings,
                &midnight_contract::interpreter::NoWitnesses,
                &helpers,
                &structs,
                &enums,
            )?;

            Ok(Self::new(result.state))
        }
    }
}
