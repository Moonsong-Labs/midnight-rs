use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::ToTokens;

/// Create an unsuffixed integer literal (produces `42` instead of `42usize`).
pub(crate) struct Lit(pub usize);

impl ToTokens for Lit {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        Literal::usize_unsuffixed(self.0).to_tokens(tokens);
    }
}

/// Create an identifier, using raw syntax for reserved keywords.
/// Replaces characters invalid in Rust identifiers (`$`, etc.) with `_`.
pub(crate) fn make_ident(name: &str) -> Ident {
    // Edition 2024 reserved keywords that syn may not reject.
    const EDITION_2024_RESERVED: &[&str] = &["gen"];

    // Replace characters that are invalid in Rust identifiers.
    // Compact uses `$` in witness/circuit names (e.g., `vote$commit`).
    let sanitized = sanitize_ident(name);

    if EDITION_2024_RESERVED.contains(&sanitized.as_str()) {
        return Ident::new_raw(&sanitized, Span::call_site());
    }
    syn::parse_str::<Ident>(&sanitized)
        .unwrap_or_else(|_| Ident::new_raw(&sanitized, Span::call_site()))
}

/// Replace characters that are not valid in Rust identifiers with `_`.
fn sanitize_ident(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn to_pascal_case(s: &str) -> String {
    let mut result = String::new();
    let mut cap = true;
    for c in s.chars() {
        if c == '_' || c == '-' || c == '$' {
            cap = true;
        } else if cap {
            result.push(c.to_uppercase().next().unwrap_or(c));
            cap = false;
        } else {
            result.push(c);
        }
    }
    result
}

pub fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c == '$' || c == '-' {
            result.push('_');
        } else if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_lowercase().next().unwrap_or(c));
        } else {
            result.push(c);
        }
    }
    result
}
