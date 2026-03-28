use proc_macro2::TokenStream;
use quote::quote;

use crate::types::TypeNode;

use super::helpers::Lit;

// --- Type mapping ---

pub(crate) fn type_to_tokens(ty: &TypeNode) -> TokenStream {
    match ty {
        TypeNode::Boolean => quote! { bool },
        TypeNode::Field => quote! { TransientFr },
        TypeNode::Uint { maxval } => uint_tokens(maxval),
        TypeNode::Bytes { length } => {
            let length = Lit(*length);
            quote! { Bytes<#length> }
        }
        TypeNode::Vector { length, inner } => {
            let inner_ty = type_to_tokens(inner);
            let length = Lit(*length);
            quote! { [#inner_ty; #length] }
        }
        TypeNode::Tuple { types } if types.is_empty() => quote! { () },
        TypeNode::Tuple { types } if types.len() == 1 => {
            let t = type_to_tokens(&types[0]);
            quote! { (#t,) }
        }
        TypeNode::Tuple { types } => {
            let inner: Vec<_> = types.iter().map(type_to_tokens).collect();
            quote! { (#(#inner),*) }
        }
        TypeNode::Struct { name, .. } | TypeNode::Enum { name, .. } => {
            let ident = super::helpers::make_ident(name);
            quote! { #ident }
        }
        TypeNode::Alias { inner, .. } => type_to_tokens(inner),
        TypeNode::Opaque { ts_type } => opaque_tokens(ts_type.as_deref()),
        TypeNode::Contract { .. } => quote! { Vec<u8> },
        TypeNode::Unknown => {
            eprintln!(
                "warning: contract JSON contains an unrecognized type node, falling back to Vec<u8>"
            );
            quote! { Vec<u8> }
        }
    }
}

pub(crate) fn uint_tokens(maxval: &serde_json::Value) -> TokenStream {
    let s = match maxval {
        serde_json::Value::Number(n) => {
            if let Some(v) = n.as_u64() {
                return match v {
                    0..=255 => quote! { u8 },
                    256..=65535 => quote! { u16 },
                    65_536..=4_294_967_295 => quote! { u32 },
                    _ => quote! { u64 },
                };
            }
            n.to_string()
        }
        serde_json::Value::String(s) => s.clone(),
        _ => return quote! { u128 },
    };
    if let Ok(v) = s.parse::<u128>() {
        if v <= u128::from(u64::MAX) {
            quote! { u64 }
        } else {
            quote! { u128 }
        }
    } else {
        quote! { Vec<u8> }
    }
}

// --- Opaque type mapping ---

pub(crate) fn opaque_tokens(ts_type: Option<&str>) -> TokenStream {
    match ts_type {
        Some("JubjubPoint") => quote! { EmbeddedGroupAffine },
        Some("Scalar<BLS12-381>") => quote! { TransientFr },
        _ => quote! { Vec<u8> },
    }
}

// --- Alignment helper ---

/// Generates a `TokenStream` for the `Alignment` expression of a given type.
/// Used by `Aligned` impls for structs.
pub(crate) fn alignment_expr(ty: &TypeNode) -> TokenStream {
    match ty {
        TypeNode::Struct { name, .. } | TypeNode::Enum { name, .. } => {
            let ident = super::helpers::make_ident(name);
            quote! { <#ident as Aligned>::alignment() }
        }
        TypeNode::Alias { inner, .. } => alignment_expr(inner),
        _ => {
            let rust_type = type_to_tokens(ty);
            quote! { <#rust_type as Aligned>::alignment() }
        }
    }
}
