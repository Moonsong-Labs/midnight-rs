use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::types::{Circuit, CircuitArgument, TypeNode, Witness};

use super::helpers::{make_ident, to_pascal_case};
use super::types::type_to_tokens;

pub(crate) fn emit_circuit_types(circuits: &[Circuit], witnesses: &[Witness]) -> TokenStream {
    let mut items = Vec::new();

    for circuit in circuits {
        items.push(emit_call_struct(
            &circuit.name,
            &circuit.arguments,
            circuit.pure,
            circuit.proof,
        ));
        items.push(emit_return_type(&circuit.name, &circuit.result_type));
    }

    for witness in witnesses {
        items.push(emit_call_struct(
            &witness.name,
            &witness.arguments,
            false,
            false,
        ));
        items.push(emit_return_type(&witness.name, &witness.result_type));
    }

    if !circuits.is_empty() || !witnesses.is_empty() {
        items.push(emit_calls_enum(circuits, witnesses));
    }

    quote! { #(#items)* }
}

fn emit_call_struct(
    name: &str,
    arguments: &[CircuitArgument],
    pure: bool,
    proof: bool,
) -> TokenStream {
    let type_name = format_ident!("{}Call", to_pascal_case(name));

    let fields: Vec<_> = arguments
        .iter()
        .map(|arg| {
            let field_name = make_ident(&arg.name);
            let field_type = type_to_tokens(&arg.type_node);
            quote! { pub #field_name: #field_type }
        })
        .collect();

    let doc = format!("Arguments for the `{name}` circuit.");

    quote! {
        #[doc = #doc]
        #[derive(Debug, Clone)]
        pub struct #type_name {
            #(#fields),*
        }

        impl #type_name {
            pub const NAME: &str = #name;
            pub const PURE: bool = #pure;
            pub const PROOF: bool = #proof;
        }
    }
}

fn emit_return_type(name: &str, result_type: &TypeNode) -> TokenStream {
    let type_name = format_ident!("{}Return", to_pascal_case(name));
    let rust_type = result_type_to_tokens(result_type);
    let doc = format!("Return type of the `{name}` circuit.");

    quote! {
        #[doc = #doc]
        pub type #type_name = #rust_type;
    }
}

fn emit_calls_enum(circuits: &[Circuit], witnesses: &[Witness]) -> TokenStream {
    let variants: Vec<_> = circuits
        .iter()
        .map(|c| &c.name)
        .chain(witnesses.iter().map(|w| &w.name))
        .map(|name| {
            let variant = format_ident!("{}", to_pascal_case(name));
            let call_type = format_ident!("{}Call", to_pascal_case(name));
            quote! { #variant(#call_type) }
        })
        .collect();

    quote! {
        /// All circuit/witness calls for this contract.
        #[derive(Debug, Clone)]
        pub enum Calls {
            #(#variants),*
        }
    }
}

fn result_type_to_tokens(ty: &TypeNode) -> TokenStream {
    match ty {
        TypeNode::Tuple { types } if types.is_empty() => quote! { () },
        other => type_to_tokens(other),
    }
}
