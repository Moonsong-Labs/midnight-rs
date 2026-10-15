use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};

use crate::types::{FieldIndex, LedgerField};

use super::helpers::Lit;

/// The name of the constant that holds the state index of the ledger field
/// `field_name`.
pub(super) fn field_const_ident(field_name: &str) -> Ident {
    format_ident!("FIELD_{}", field_name.to_uppercase())
}

pub(crate) fn emit_field_constants(fields: &[LedgerField]) -> TokenStream {
    let items: Vec<_> = fields
        .iter()
        .map(|field| {
            let name = field_const_ident(&field.name);
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
