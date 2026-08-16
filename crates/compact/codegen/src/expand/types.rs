use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Curve, Type};

use super::helpers::Lit;

// --- Type mapping ---

pub(crate) fn type_to_tokens(ty: &Type) -> TokenStream {
    match ty {
        Type::Boolean => quote! { bool },
        Type::Field(_) => quote! { TransientFr },
        Type::Unsigned(maxval) => uint_tokens(maxval),
        Type::Bytes(length) => {
            let length = Lit(*length as usize);
            quote! { Bytes<#length> }
        }
        Type::Vector {
            len: length,
            ty: inner,
        } => {
            // `Vector<N, T>` maps to the `Vector` newtype rather than a bare
            // `[T; N]`, because a ledger struct field needs FAB trait impls the
            // orphan rule forbids on a raw array. See `midnight_typed_state::Vector`.
            let inner_ty = type_to_tokens(inner);
            let length = Lit(*length as usize);
            quote! { Vector<#length, #inner_ty> }
        }
        Type::Tuple(types) if types.is_empty() => quote! { () },
        Type::Tuple(types) if types.len() == 1 => {
            let t = type_to_tokens(&types[0]);
            quote! { (#t,) }
        }
        Type::Tuple(types) => {
            let inner: Vec<_> = types.iter().map(type_to_tokens).collect();
            quote! { (#(#inner),*) }
        }
        Type::Struct { name, .. } | Type::Enum { name, .. } => {
            let ident = super::helpers::make_ident(name);
            quote! { #ident }
        }
        Type::Alias { ty: inner, .. } => type_to_tokens(inner),
        // The runtime's EC values carry the wire spelling the generated
        // bindings already used for a Jubjub point.
        Type::Point(Curve::Jubjub) => opaque_tokens("JubjubPoint"),
        Type::Opaque(name) => opaque_tokens(name),
        Type::Contract { .. } => quote! { Vec<u8> },
        // Rejected at load by `artifact::check_type`; unreachable here.
        Type::Point(_) | Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => {
            quote! { Vec<u8> }
        }
    }
}

pub(crate) fn uint_tokens(maxval: &num_bigint::BigUint) -> TokenStream {
    match u128::try_from(maxval) {
        Ok(v) => match v {
            0..=255 => quote! { u8 },
            256..=65535 => quote! { u16 },
            65_536..=4_294_967_295 => quote! { u32 },
            v if v <= u128::from(u64::MAX) => quote! { u64 },
            _ => quote! { u128 },
        },
        Err(_) => quote! { Vec<u8> },
    }
}

// --- Opaque type mapping ---

pub(crate) fn opaque_tokens(ts_type: &str) -> TokenStream {
    match ts_type {
        "JubjubPoint" => quote! { EmbeddedGroupAffine },
        "Scalar<BLS12-381>" => quote! { TransientFr },
        _ => quote! { Vec<u8> },
    }
}

// --- Encode helper ---

/// Generate a `TokenStream` that evaluates to an `AlignedValue` built from
/// an expression of the given [`Type`]. Used by the per-struct
/// `impl From<T> for AlignedValue` codegen and by the per-circuit method
/// codegen when threading typed arguments into `Contract::call_with`.
///
/// Field/element order MUST match `alignment_expr`, because
/// `Aligned::alignment()` for a compound type is `Alignment::concat` of the
/// per-field alignments in the same order. The encoded value must fit the
/// declared alignment for the prover to accept it.
pub(crate) fn encode_to_aligned_value(expr: &TokenStream, ty: &Type) -> TokenStream {
    match ty {
        Type::Boolean
        | Type::Unsigned(_)
        | Type::Field(_)
        | Type::Bytes(_)
        | Type::Struct { .. }
        | Type::Enum { .. } => {
            quote! { AlignedValue::from(#expr) }
        }
        // Every opaque type encodes as a single `Compress`-aligned atom holding
        // its bytes, matching the Compact runtime's `CompactTypeOpaqueString` /
        // `CompactTypeOpaqueUint8Array`. `JubjubPoint` and `Scalar<BLS12-381>`
        // map to their own Rust types; everything else maps to `Vec<u8>`, whose
        // `Aligned` impl is that same single `Compress` atom. So one conversion
        // covers all of them.
        Type::Opaque(_) | Type::Point(_) => quote! { AlignedValue::from(#expr) },
        Type::Alias { ty: inner, .. } => encode_to_aligned_value(expr, inner),
        Type::Vector { ty: inner, .. } => {
            // Iterate the array/slice and concat per-element AlignedValues.
            let elem_enc = encode_to_aligned_value(&quote! { __elem }, inner);
            quote! {
                {
                    let __elems: ::std::vec::Vec<AlignedValue> = (#expr)
                        .into_iter()
                        .map(|__elem| #elem_enc)
                        .collect();
                    AlignedValue::concat(__elems.iter())
                }
            }
        }
        Type::Tuple(types) => {
            if types.is_empty() {
                return quote! { AlignedValue::from(()) };
            }
            let idents: Vec<_> = (0..types.len())
                .map(|i| {
                    proc_macro2::Ident::new(&format!("__t{i}"), proc_macro2::Span::call_site())
                })
                .collect();
            let parts: Vec<_> = idents
                .iter()
                .zip(types.iter())
                .map(|(id, t)| encode_to_aligned_value(&quote! { #id }, t))
                .collect();
            quote! {
                {
                    let (#(#idents),*) = #expr;
                    let __parts: ::std::vec::Vec<AlignedValue> = vec![#(#parts),*];
                    AlignedValue::concat(__parts.iter())
                }
            }
        }
        // Contract addresses: fall back to unit so the caller still
        // compiles; these aren't currently reachable as typed args.
        Type::Contract { .. } | Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => {
            quote! { AlignedValue::from(()) }
        }
    }
}

// --- Alignment helper ---

/// Generates a `TokenStream` for the `Alignment` expression of a given type.
/// Used by `Aligned` impls for structs.
pub(crate) fn alignment_expr(ty: &Type) -> TokenStream {
    match ty {
        Type::Struct { name, .. } | Type::Enum { name, .. } => {
            let ident = super::helpers::make_ident(name);
            quote! { <#ident as Aligned>::alignment() }
        }
        Type::Alias { ty: inner, .. } => alignment_expr(inner),
        _ => {
            let rust_type = type_to_tokens(ty);
            quote! { <#rust_type as Aligned>::alignment() }
        }
    }
}
