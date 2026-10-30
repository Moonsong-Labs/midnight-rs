use std::collections::HashMap;

use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};

use crate::ir::{Type, Witness};
use crate::types::{Circuit, LedgerField, StorageKind};

use super::helpers::{make_ident, to_pascal_case};
use super::types::{
    MAX_TUPLE_LEN, alignment_expr, encode_to_aligned_value, library_struct, type_to_tokens,
};

pub(crate) fn emit_data_types(
    fields: &[LedgerField],
    circuits: &[Circuit],
    witnesses: &[Witness],
    emitted: &mut HashMap<String, Type>,
) -> TokenStream {
    let mut tokens = Vec::new();
    let mut tree_elements = Vec::new();

    // Collect from ledger fields.
    for field in fields {
        if matches!(
            field.storage,
            StorageKind::MerkleTree | StorageKind::HistoricMerkleTree
        ) {
            tree_elements.extend(field.element_type.iter());
            continue;
        }
        for type_node in [&field.element_type, &field.key, &field.value]
            .into_iter()
            .flatten()
        {
            collect_types(type_node, emitted, &mut tokens, true);
        }
    }

    // Collect from circuit arguments and results.
    for circuit in circuits {
        for arg in circuit.arguments() {
            collect_types(&arg.ty, emitted, &mut tokens, true);
        }
        collect_types(circuit.result_type(), emitted, &mut tokens, true);
    }

    // Collect from witness arguments and results.
    for witness in witnesses {
        for arg in &witness.arguments {
            collect_types(&arg.ty, emitted, &mut tokens, true);
        }
        collect_types(&witness.result_type, emitted, &mut tokens, true);
    }

    // Generated code never names a tree's element type, so it comes last and
    // only adds a struct that no other position declared.
    for type_node in tree_elements {
        collect_types(type_node, emitted, &mut tokens, false);
    }

    quote! { #(#tokens)* }
}

/// Collect the struct and enum definitions that `node` needs.
///
/// A struct name serves one Rust struct. With `refuse_collisions`, a second
/// instantiation that renders other Rust fields is a compile error.
fn collect_types(
    node: &Type,
    emitted: &mut HashMap<String, Type>,
    tokens: &mut Vec<TokenStream>,
    refuse_collisions: bool,
) {
    match node {
        Type::Struct { name, fields } => {
            if let Some((_, args)) = library_struct(name, fields) {
                for arg in args {
                    collect_types(arg, emitted, tokens, refuse_collisions);
                }
                return;
            }
            match emitted.get(name) {
                Some(Type::Struct {
                    fields: first_fields,
                    ..
                }) => {
                    if rust_fields(first_fields) == rust_fields(fields) {
                        // The same Rust fields can still hold structs that differ.
                        for (_, field_ty) in fields {
                            collect_types(field_ty, emitted, tokens, refuse_collisions);
                        }
                    } else if refuse_collisions {
                        let msg = format!(
                            "the Compact struct `{name}` has two instantiations with different \
                             fields, and the bindings give one Rust struct per name"
                        );
                        tokens.push(quote! { compile_error!(#msg); });
                    }
                }
                Some(_) => {}
                None => {
                    emitted.insert(name.clone(), node.clone());
                    for (_, field_ty) in fields {
                        collect_types(field_ty, emitted, tokens, refuse_collisions);
                    }
                    let ident = make_ident(name);
                    tokens.push(emit_struct(&ident, fields));
                    tokens.push(emit_struct_aligned(&ident, fields));
                    tokens.push(emit_struct_try_from_value_slice(&ident, fields));
                    tokens.push(emit_struct_into_aligned_value(&ident, fields));
                    if let Some(sdk) = sdk_counterpart(name, fields) {
                        tokens.push(emit_from_sdk(&ident, &sdk));
                    }
                }
            }
        }
        Type::Enum { name, variants } => {
            if emitted.insert(name.clone(), node.clone()).is_none() {
                let ident = make_ident(name);
                tokens.push(emit_enum(&ident, variants));
                tokens.push(emit_enum_aligned(&ident));
                tokens.push(emit_enum_try_from_value_slice(&ident, name, variants));
                tokens.push(emit_enum_into_aligned_value(&ident));
            }
        }
        Type::Alias { ty: inner, .. } | Type::Vector { ty: inner, .. } => {
            collect_types(inner, emitted, tokens, refuse_collisions);
        }
        Type::Tuple(types) => {
            for t in types {
                collect_types(t, emitted, tokens, refuse_collisions);
            }
        }
        Type::Contract { .. } => collect_types(
            &Type::contract_address(),
            emitted,
            tokens,
            refuse_collisions,
        ),
        // Leaf types that map directly to built-in or runtime Rust types --
        // no user-defined type definitions need to be emitted for these.
        Type::Boolean
        | Type::Field(_)
        | Type::Unsigned(_)
        | Type::Point(_)
        | Type::Bytes(_)
        | Type::Opaque(_) => {}
        // Rejected at load by `artifact::check_type`; unreachable here.
        Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => {}
    }
}

/// The fields of the Rust struct that a struct type generates: each name with
/// its Rust type.
fn rust_fields(fields: &[(String, Type)]) -> Vec<(&str, String)> {
    fields
        .iter()
        .map(|(name, ty)| (name.as_str(), type_to_tokens(ty).to_string()))
        .collect()
}

/// The SDK type that converts into this struct, when the struct is the
/// standard library's `ZswapCoinPublicKey` or `UserAddress`.
///
/// `ContractAddress` has the same single field `bytes: Bytes<32>`, so only the
/// name tells the three apart.
fn sdk_counterpart(name: &str, fields: &[(String, Type)]) -> Option<TokenStream> {
    if !matches!(fields, [(field, Type::Bytes(32))] if field == "bytes") {
        return None;
    }
    match name {
        "ZswapCoinPublicKey" => Some(quote! { midnight_contract::CoinPublicKey }),
        "UserAddress" => Some(quote! { midnight_contract::UserAddress }),
        _ => None,
    }
}

fn emit_from_sdk(ident: &Ident, sdk: &TokenStream) -> TokenStream {
    quote! {
        impl From<#sdk> for #ident {
            fn from(value: #sdk) -> Self {
                Self { bytes: Bytes(value.0.0) }
            }
        }
    }
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
    let fields: Vec<_> = field_names
        .iter()
        .zip(elements)
        .map(|(name, (_, ty))| (quote! { #name }, type_to_tokens(ty)))
        .collect();
    let (pattern, tuple_type) = nested_tuple(&fields);

    quote! {
        impl<'a> TryFrom<&'a ValueSlice> for #ident {
            type Error = InvalidBuiltinDecode;

            fn try_from(vs: &'a ValueSlice) -> Result<Self, Self::Error> {
                let #pattern: #tuple_type = vs.try_into()?;
                Ok(Self { #(#field_names),* })
            }
        }
    }
}

/// The tuple pattern and the tuple type that decode `items`, a list of
/// patterns with their types. More than [`MAX_TUPLE_LEN`] items nest in
/// chunks, so that each tuple has a `TryFrom<&ValueSlice>` impl.
fn nested_tuple(items: &[(TokenStream, TokenStream)]) -> (TokenStream, TokenStream) {
    if items.len() <= MAX_TUPLE_LEN {
        let patterns = items.iter().map(|(pattern, _)| pattern);
        let types = items.iter().map(|(_, ty)| ty);
        return (quote! { (#(#patterns,)*) }, quote! { (#(#types,)*) });
    }
    let chunks: Vec<_> = items.chunks(MAX_TUPLE_LEN).map(nested_tuple).collect();
    nested_tuple(&chunks)
}

/// The variant idents of an enum and their `u8` discriminants. The
/// discriminant is the declaration index: the enum definition, its decoder
/// and the `as u8` of [`emit_enum_into_aligned_value`] all depend on it.
fn enum_variants(elements: &[String]) -> (Vec<Ident>, Vec<Literal>) {
    elements
        .iter()
        .enumerate()
        .map(|(i, v)| {
            #[allow(clippy::cast_possible_truncation)]
            let idx = Literal::u8_unsuffixed(i as u8);
            (format_ident!("{}", to_pascal_case(v)), idx)
        })
        .unzip()
}

fn emit_enum(ident: &Ident, elements: &[String]) -> TokenStream {
    let (variants, idxs) = enum_variants(elements);
    quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(u8)]
        pub enum #ident {
            #(#variants = #idxs),*
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
    let (variants, idxs) = enum_variants(elements);
    let err_msg = format!("invalid {name_str} variant");

    quote! {
        impl<'a> TryFrom<&'a ValueSlice> for #ident {
            type Error = InvalidBuiltinDecode;

            fn try_from(vs: &'a ValueSlice) -> Result<Self, Self::Error> {
                let v: u8 = vs.try_into()?;
                match v {
                    #(#idxs => Ok(#ident::#variants),)*
                    _ => Err(InvalidBuiltinDecode(#err_msg)),
                }
            }
        }
    }
}
