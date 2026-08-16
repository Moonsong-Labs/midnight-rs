use std::collections::HashSet;

use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};

use crate::nir::{Type, Witness};
use crate::types::{Circuit, LedgerField};

use super::helpers::{make_ident, to_pascal_case};
use super::types::{alignment_expr, encode_to_aligned_value, type_to_tokens};

pub(crate) fn emit_data_types(
    fields: &[LedgerField],
    circuits: &[Circuit],
    witnesses: &[Witness],
    emitted: &mut HashSet<String>,
) -> TokenStream {
    let mut tokens = Vec::new();

    // Collect from ledger fields.
    for field in fields {
        for type_node in [&field.element_type, &field.key, &field.value]
            .into_iter()
            .flatten()
        {
            collect_types(type_node, emitted, &mut tokens);
        }
    }

    // Collect from circuit arguments and results.
    for circuit in circuits {
        for arg in circuit.arguments() {
            collect_types(&arg.ty, emitted, &mut tokens);
        }
        collect_types(circuit.result_type(), emitted, &mut tokens);
    }

    // Collect from witness arguments and results.
    for witness in witnesses {
        for arg in &witness.arguments {
            collect_types(&arg.ty, emitted, &mut tokens);
        }
        collect_types(&witness.result_type, emitted, &mut tokens);
    }

    quote! { #(#tokens)* }
}

fn collect_types(node: &Type, emitted: &mut HashSet<String>, tokens: &mut Vec<TokenStream>) {
    match node {
        Type::Struct { name, fields } => {
            if emitted.insert(name.clone()) {
                for (_, field_ty) in fields {
                    collect_types(field_ty, emitted, tokens);
                }
                let ident = make_ident(name);
                tokens.push(emit_struct(&ident, fields));
                tokens.push(emit_struct_aligned(&ident, fields));
                tokens.push(emit_struct_try_from_value_slice(&ident, fields));
                tokens.push(emit_struct_into_aligned_value(&ident, fields));
                // Maybe<T> structs get an into_option() method.
                if is_maybe_struct(name, fields) {
                    tokens.push(emit_maybe_into_option(&ident, fields));
                }
            }
        }
        Type::Enum { name, variants } => {
            if emitted.insert(name.clone()) {
                let ident = make_ident(name);
                tokens.push(emit_enum(&ident, variants));
                tokens.push(emit_enum_aligned(&ident));
                tokens.push(emit_enum_try_from_value_slice(&ident, name, variants));
                tokens.push(emit_enum_into_aligned_value(&ident));
            }
        }
        Type::Alias { ty: inner, .. } | Type::Vector { ty: inner, .. } => {
            collect_types(inner, emitted, tokens);
        }
        Type::Tuple(types) => {
            for t in types {
                collect_types(t, emitted, tokens);
            }
        }
        // Leaf types that map directly to built-in or runtime Rust types --
        // no user-defined type definitions need to be emitted for these.
        Type::Boolean
        | Type::Field(_)
        | Type::Unsigned(_)
        | Type::Point(_)
        | Type::Bytes(_)
        | Type::Opaque(_)
        | Type::Contract { .. } => {}
        // Rejected at load by `artifact::check_type`; unreachable here.
        Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => {}
    }
}

/// Returns true if this struct matches the `Maybe<T>` pattern:
/// fields `[is_some: Boolean, value: T]`.
fn is_maybe_struct(name: &str, fields: &[(String, Type)]) -> bool {
    name == "Maybe"
        && fields.len() == 2
        && fields[0].0 == "is_some"
        && matches!(fields[0].1, Type::Boolean)
        && fields[1].0 == "value"
}

fn emit_struct(name: &Ident, elements: &[(String, Type)]) -> TokenStream {
    let fields: Vec<_> = elements
        .iter()
        .map(|(elem_name, elem_ty)| {
            let field_name = make_ident(elem_name);
            let field_type = type_to_tokens(elem_ty);
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

fn emit_struct_aligned(ident: &Ident, elements: &[(String, Type)]) -> TokenStream {
    let alignments: Vec<_> = elements
        .iter()
        .map(|(_, elem_ty)| {
            let expr = alignment_expr(elem_ty);
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

fn emit_struct_try_from_value_slice(ident: &Ident, elements: &[(String, Type)]) -> TokenStream {
    let field_names: Vec<_> = elements.iter().map(|(n, _)| make_ident(n)).collect();
    let field_types: Vec<_> = elements.iter().map(|(_, t)| type_to_tokens(t)).collect();

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

fn emit_maybe_into_option(ident: &Ident, elements: &[(String, Type)]) -> TokenStream {
    let value_type = type_to_tokens(&elements[1].1);
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

/// Emit `impl From<Struct> for AlignedValue` by concatenating per-field
/// `AlignedValue`s in declaration order. Field order MUST match
/// `emit_struct_aligned` above, because the resulting alignment is the
/// concat of each field's alignment in declaration order.
fn emit_struct_into_aligned_value(ident: &Ident, elements: &[(String, Type)]) -> TokenStream {
    let per_field: Vec<_> = elements
        .iter()
        .map(|(elem_name, elem_ty)| {
            let field_name = make_ident(elem_name);
            let expr = quote! { __val.#field_name };
            encode_to_aligned_value(&expr, elem_ty)
        })
        .collect();

    quote! {
        impl From<#ident> for AlignedValue {
            fn from(__val: #ident) -> AlignedValue {
                let __parts: ::std::vec::Vec<AlignedValue> = vec![#(#per_field),*];
                AlignedValue::concat(__parts.iter())
            }
        }
    }
}

/// Emit `impl From<Enum> for AlignedValue` via the `#[repr(u8)]` discriminant.
fn emit_enum_into_aligned_value(ident: &Ident) -> TokenStream {
    quote! {
        impl From<#ident> for AlignedValue {
            fn from(__val: #ident) -> AlignedValue {
                AlignedValue::from(__val as u8)
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
