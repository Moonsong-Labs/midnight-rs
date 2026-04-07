//! Generate circuit call methods on the Ledger struct.
//!
//! For each impure circuit that has embedded IR, we generate:
//! - A `call_<name>` method that executes the circuit against the current state
//! - Embedded IR JSON as a const string, deserialized on first use

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::types::{Circuit, ContractInfo, TypeNode};

use super::helpers::make_ident;
use super::types::{encode_to_aligned_value, type_to_tokens};

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

    let helpers_const = quote! {
        #[doc(hidden)]
        pub const __HELPERS_JSON: &str = #helpers_json;
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

            let result = midnight_contract::interpreter::execute_with(
                &ir,
                &self.state,
                #arg_bindings,
                &midnight_contract::interpreter::NoWitnesses,
                &helpers,
            )?;

            Ok(Self::new(result.state))
        }
    }
}
