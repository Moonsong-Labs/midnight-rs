use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::types::{FieldIndex, LedgerField};

use super::helpers::Lit;

pub(crate) fn emit_field_constants(fields: &[LedgerField]) -> TokenStream {
    let items: Vec<_> = fields
        .iter()
        .map(|field| {
            let name = format_ident!("FIELD_{}", field.name.to_uppercase());
            match &field.index {
                FieldIndex::Single(idx) => {
                    let idx = Lit(*idx);
                    quote! { pub const #name: usize = #idx; }
                }
                FieldIndex::Path(path) => {
                    let indices: Vec<_> = path.iter().map(|&i| Lit(i)).collect();
                    quote! { pub const #name: &[usize] = &[#(#indices),*]; }
                }
            }
        })
        .collect();

    quote! { #(#items)* }
}
