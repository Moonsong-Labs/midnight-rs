use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};

/// Parsed macro input supporting these forms:
///
/// - `contract!("path.json")` — flat output, struct named `Ledger`
/// - `contract!(Gateway, "path.json")` — wrapped in `pub mod gateway { ... }`
/// - `contract!(#[allow(...)] Gateway, "path.json")` — attributes forwarded to the module
/// - `contract!(#[crate = midnight_core::midnight_bindgen] Gateway, "path.json")` — custom crate path
struct ContractInput {
    attrs: Vec<syn::Attribute>,
    name: Option<syn::Ident>,
    path: syn::LitStr,
    crate_path: Option<syn::Path>,
}

impl Parse for ContractInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(syn::Attribute::parse_outer)?;

        if input.peek(syn::Ident) && input.peek2(syn::Token![,]) {
            let name: syn::Ident = input.parse()?;
            let _comma: syn::Token![,] = input.parse()?;
            let path: syn::LitStr = input.parse()?;
            let crate_path = extract_crate_path(&attrs)?;
            Ok(ContractInput {
                attrs: strip_crate_attr(attrs),
                name: Some(name),
                path,
                crate_path,
            })
        } else {
            let path: syn::LitStr = input.parse()?;
            let crate_path = extract_crate_path(&attrs)?;
            let attrs = strip_crate_attr(attrs);
            if !attrs.is_empty() {
                return Err(syn::Error::new_spanned(
                    &path,
                    "attributes require a named contract: contract!(#[...] Name, \"path\")",
                ));
            }
            Ok(ContractInput {
                attrs,
                name: None,
                path,
                crate_path,
            })
        }
    }
}

/// Extract `#[crate = some::path]` from attributes, returning the path if found.
fn extract_crate_path(attrs: &[syn::Attribute]) -> syn::Result<Option<syn::Path>> {
    for attr in attrs {
        if attr.path().is_ident("crate") {
            let value: syn::Path = attr.parse_args()?;
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// Remove `#[crate(...)]` attributes so they aren't forwarded to the generated module.
fn strip_crate_attr(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !a.path().is_ident("crate"))
        .collect()
}

/// Generate typed Rust bindings from a Compact `contract-info.json` file.
///
/// The path is relative to the crate's `CARGO_MANIFEST_DIR`.
///
/// # Examples
///
/// ```ignore
/// // Flat: generates `Ledger` and all types directly in scope.
/// midnight_bindgen::contract!("compiled/gateway/compiler/contract-info.json");
///
/// // Module: generates `pub mod gateway { pub struct Gateway { ... } ... }`.
/// midnight_bindgen::contract!(Gateway, "compiled/gateway/compiler/contract-info.json");
///
/// // With attributes forwarded to the generated module.
/// midnight_bindgen::contract!(
///     #[allow(missing_docs)]
///     Gateway,
///     "compiled/gateway/compiler/contract-info.json"
/// );
///
/// // Custom crate path (e.g. when using midnight-bindgen through midnight-core).
/// midnight_bindgen::contract!(
///     #[crate(midnight_core::midnight_bindgen)]
///     Gateway,
///     "compiled/gateway/compiler/contract-info.json"
/// );
/// ```
#[proc_macro]
pub fn contract(input: TokenStream) -> TokenStream {
    let ContractInput {
        attrs,
        name,
        path,
        crate_path,
    } = syn::parse_macro_input!(input as ContractInput);
    let contract_name = name
        .as_ref()
        .map_or_else(|| "Ledger".into(), ToString::to_string);

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let full_path = std::path::Path::new(&manifest_dir).join(path.value());

    let json = match std::fs::read_to_string(&full_path) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("failed to read {}: {}", full_path.display(), e);
            return syn::Error::new(path.span(), msg).to_compile_error().into();
        }
    };

    let crate_path_tokens: Option<TokenStream2> = crate_path.map(|p| quote! { #p });
    let inner: TokenStream2 = match compact_codegen::generate_bindings_from_json(
        &json,
        &contract_name,
        crate_path_tokens.as_ref(),
    ) {
        Ok(tokens) => tokens,
        Err(e) => {
            let msg = format!(
                "failed to generate bindings from {}: {e}",
                full_path.display()
            );
            return syn::Error::new(path.span(), msg).to_compile_error().into();
        }
    };

    let full_path_str = full_path.to_string_lossy().to_string();
    let track_file = quote! {
        const _: &[u8] = include_bytes!(#full_path_str);
    };

    let output = if name.is_some() {
        let mod_name = syn::Ident::new(
            &compact_codegen::to_snake_case(&contract_name),
            proc_macro2::Span::call_site(),
        );
        quote! {
            #track_file
            #(#attrs)*
            #[allow(dead_code, clippy::borrow_deref_ref, clippy::explicit_auto_deref)]
            pub mod #mod_name {
                #inner
            }
        }
    } else {
        quote! {
            #track_file
            #inner
        }
    };

    output.into()
}
