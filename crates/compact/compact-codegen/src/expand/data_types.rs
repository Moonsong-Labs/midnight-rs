use std::collections::HashSet;

use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};

use crate::types::{Circuit, LedgerField, StructElement, TypeNode, Witness};

use super::helpers::{make_ident, to_pascal_case};
use super::types::{alignment_expr, type_to_tokens};

pub(crate) fn emit_data_types(
    fields: &[LedgerField],
    circuits: &[Circuit],
    witnesses: &[Witness],
    emitted: &mut HashSet<String>,
) -> TokenStream {
    let mut tokens = Vec::new();

    // Collect from ledger fields.
    for field in fields {
        for type_node in [
            &field.value_type,
            &field.cell_type,
            &field.element_type,
            &field.key_type,
        ]
        .into_iter()
        .flatten()
        {
            collect_types(type_node, emitted, &mut tokens);
        }
    }

    // Collect from circuit arguments and results.
    for circuit in circuits {
        for arg in &circuit.arguments {
            collect_types(&arg.type_node, emitted, &mut tokens);
        }
        collect_types(&circuit.result_type, emitted, &mut tokens);
    }

    // Collect from witness arguments and results.
    for witness in witnesses {
        for arg in &witness.arguments {
            collect_types(&arg.type_node, emitted, &mut tokens);
        }
        collect_types(&witness.result_type, emitted, &mut tokens);
    }

    quote! { #(#tokens)* }
}

fn collect_types(node: &TypeNode, emitted: &mut HashSet<String>, tokens: &mut Vec<TokenStream>) {
    match node {
        TypeNode::Struct { name, elements } => {
            if emitted.insert(name.clone()) {
                for elem in elements {
                    collect_types(&elem.type_node, emitted, tokens);
                }
                let ident = make_ident(name);
                tokens.push(emit_struct(&ident, elements));
                tokens.push(emit_struct_aligned(&ident, elements));
                tokens.push(emit_struct_try_from_value_slice(&ident, elements));
                // Maybe<T> structs get an into_option() method.
                if is_maybe_struct(name, elements) {
                    tokens.push(emit_maybe_into_option(&ident, elements));
                }
            }
        }
        TypeNode::Enum { name, elements } => {
            if emitted.insert(name.clone()) {
                let ident = make_ident(name);
                tokens.push(emit_enum(&ident, elements));
                tokens.push(emit_enum_aligned(&ident));
                tokens.push(emit_enum_try_from_value_slice(&ident, name, elements));
            }
        }
        TypeNode::Alias { inner, .. } | TypeNode::Vector { inner, .. } => {
            collect_types(inner, emitted, tokens);
        }
        TypeNode::Tuple { types } => {
            for t in types {
                collect_types(t, emitted, tokens);
            }
        }
        // Leaf types that map directly to built-in or runtime Rust types --
        // no user-defined type definitions need to be emitted for these.
        TypeNode::Boolean
        | TypeNode::Field
        | TypeNode::Uint { .. }
        | TypeNode::Bytes { .. }
        | TypeNode::Opaque { .. }
        | TypeNode::Contract { .. } => {}

        // Nothing to collect -- Unknown has no inner types.
        TypeNode::Unknown => {}
    }
}

/// Returns true if this struct matches the `Maybe<T>` pattern:
/// fields `[is_some: Boolean, value: T]`.
fn is_maybe_struct(name: &str, elements: &[StructElement]) -> bool {
    name == "Maybe"
        && elements.len() == 2
        && elements[0].name == "is_some"
        && matches!(elements[0].type_node, TypeNode::Boolean)
        && elements[1].name == "value"
}

fn emit_struct(name: &Ident, elements: &[StructElement]) -> TokenStream {
    let fields: Vec<_> = elements
        .iter()
        .map(|e| {
            let field_name = make_ident(&e.name);
            let field_type = type_to_tokens(&e.type_node);
            quote! { pub #field_name: #field_type }
        })
        .collect();

    quote! {
        #[derive(Debug, Clone, PartialEq)]
        pub struct #name {
            #(#fields),*
        }
    }
}

fn emit_struct_aligned(ident: &Ident, elements: &[StructElement]) -> TokenStream {
    let alignments: Vec<_> = elements
        .iter()
        .map(|e| {
            let expr = alignment_expr(&e.type_node);
            quote! { &#expr }
        })
        .collect();

    quote! {
        impl Aligned for #ident {
            fn alignment() -> Alignment {
                Alignment::concat([#(#alignments),*])
            }
        }
    }
}

fn emit_struct_try_from_value_slice(ident: &Ident, elements: &[StructElement]) -> TokenStream {
    let field_names: Vec<_> = elements.iter().map(|e| make_ident(&e.name)).collect();
    let field_types: Vec<_> = elements
        .iter()
        .map(|e| type_to_tokens(&e.type_node))
        .collect();

    quote! {
        impl<'a> TryFrom<&'a ValueSlice> for #ident {
            type Error = InvalidBuiltinDecode;

            fn try_from(vs: &'a ValueSlice) -> Result<Self, Self::Error> {
                let (#(#field_names),*): (#(#field_types),*) = vs.try_into()?;
                Ok(Self { #(#field_names),* })
            }
        }
    }
}

fn emit_maybe_into_option(ident: &Ident, elements: &[StructElement]) -> TokenStream {
    let value_type = type_to_tokens(&elements[1].type_node);
    quote! {
        impl #ident {
            /// Converts this `Maybe` into an `Option`, returning `Some(value)` when
            /// `is_some` is `true` and `None` otherwise.
            pub fn into_option(self) -> Option<#value_type> {
                if self.is_some {
                    Some(self.value)
                } else {
                    None
                }
            }
        }
    }
}

fn emit_enum(ident: &Ident, elements: &[String]) -> TokenStream {
    let variants: Vec<_> = elements
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let variant = format_ident!("{}", to_pascal_case(v));
            #[allow(clippy::cast_possible_truncation)]
            let idx = Literal::u8_unsuffixed(i as u8);
            quote! { #variant = #idx }
        })
        .collect();

    quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(u8)]
        pub enum #ident {
            #(#variants),*
        }
    }
}

fn emit_enum_aligned(ident: &Ident) -> TokenStream {
    quote! {
        impl Aligned for #ident {
            fn alignment() -> Alignment {
                <u8 as Aligned>::alignment()
            }
        }
    }
}

fn emit_enum_try_from_value_slice(
    ident: &Ident,
    name_str: &str,
    elements: &[String],
) -> TokenStream {
    let match_arms: Vec<_> = elements
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let variant = format_ident!("{}", to_pascal_case(v));
            #[allow(clippy::cast_possible_truncation)]
            let idx = Literal::u8_unsuffixed(i as u8);
            quote! { #idx => Ok(#ident::#variant) }
        })
        .collect();

    let err_msg = format!("invalid {name_str} variant");

    quote! {
        impl<'a> TryFrom<&'a ValueSlice> for #ident {
            type Error = InvalidBuiltinDecode;

            fn try_from(vs: &'a ValueSlice) -> Result<Self, Self::Error> {
                let v: u8 = vs.try_into()?;
                match v {
                    #(#match_arms,)*
                    _ => Err(InvalidBuiltinDecode(#err_msg)),
                }
            }
        }
    }
}
