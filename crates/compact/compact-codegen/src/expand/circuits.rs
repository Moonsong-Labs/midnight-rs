use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::types::{Circuit, CircuitArgument, TypeNode, Witness};

use super::circuit_calls::{
    has_typed_conversion, is_void_type, type_to_value_conversion, value_to_type_conversion,
};
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

    if !witnesses.is_empty() {
        items.push(emit_witnesses(witnesses));
    }

    quote! { #(#items)* }
}

/// Emit the typed `Witnesses` trait and its `WitnessesAdapter`.
///
/// The trait has one typed method per witness (typed args + return) over an
/// associated `PrivateState`, so a witness author writes plain typed Rust. The
/// adapter implements the untyped runtime `WitnessProvider`: it (de)serializes
/// the private state (serde_json), dispatches by name, and converts args/return
/// between the interpreter's `Value` and the typed forms. All the stringly /
/// `Value` / byte plumbing lives here, in generated code.
fn emit_witnesses(witnesses: &[Witness]) -> TokenStream {
    // Trait method signatures.
    let trait_methods = witnesses.iter().map(|w| {
        let method = format_ident!("{}", w.name.replace(['$', '-'], "_"));
        let params = w.arguments.iter().map(|arg| {
            let pid = make_ident(&arg.name);
            let pty = if has_typed_conversion(&arg.type_node) {
                type_to_tokens(&arg.type_node)
            } else {
                quote! { midnight_contract::runtime::Value }
            };
            quote! { #pid: #pty }
        });
        let ret = result_type_to_tokens(&w.result_type);
        let doc = format!("Witness `{}`.", w.name);
        quote! {
            #[doc = #doc]
            fn #method(&self, ps: &mut Self::PrivateState #(, #params)*) -> #ret;
        }
    });

    // The witness names this contract declares; anything else is
    // `WitnessOutcome::Unknown` (the runtime then falls through to its
    // builtins/helpers), checked before touching the private state.
    // Precondition: `witnesses` is non-empty (gated by the caller's
    // `!witnesses.is_empty()` check); an empty list would expand
    // `#(#known_names)|*` into an unparsable empty pattern.
    let known_names = witnesses.iter().map(|w| &w.name);

    // Adapter dispatch arms (matched on the on-chain witness name).
    let arms = witnesses.iter().map(|w| {
        let name = &w.name;
        let method = format_ident!("{}", w.name.replace(['$', '-'], "_"));

        let mut bindings = Vec::new();
        let mut idents = Vec::new();
        for (i, arg) in w.arguments.iter().enumerate() {
            let ident = format_ident!("__arg{i}");
            let fetch = quote! {
                __args.get(#i).cloned().ok_or_else(|| {
                    midnight_contract::runtime::InterpreterError::Witness(
                        ::std::format!("{}: missing argument {}", #name, #i)
                    )
                })?
            };
            if has_typed_conversion(&arg.type_node) {
                // The conversion evaluates to Result<_, InterpreterError>;
                // `call_witness` returns the same error type, so `?` it.
                let conv = value_to_type_conversion(
                    &arg.type_node,
                    &format!("witness `{}` argument `{}`", w.name, arg.name),
                );
                bindings.push(quote! { let #ident = { let __val = #fetch; #conv }?; });
            } else {
                bindings.push(quote! { let #ident = #fetch; });
            }
            idents.push(ident);
        }

        let call = quote! { self.0.#method(&mut __ps #(, #idents)*) };
        let ret_expr = if is_void_type(&w.result_type) {
            quote! { { #call; midnight_contract::runtime::Value::Void } }
        } else {
            let ret_ty = result_type_to_tokens(&w.result_type);
            let conv = type_to_value_conversion(&format_ident!("__r"), &w.result_type);
            quote! { { let __r: #ret_ty = #call; #conv } }
        };

        quote! {
            #name => {
                #(#bindings)*
                #ret_expr
            }
        }
    });

    quote! {
        /// Typed witness implementations for this contract.
        ///
        /// Implement one method per `witness` declaration over your own
        /// `PrivateState` type; the SDK loads it before a call and persists it
        /// after. Attach an impl with [`Circuits::with_witnesses`].
        pub trait Witnesses: Send + Sync {
            /// The contract's off-chain private state. `Default` is used when the
            /// contract has none stored yet. Serialized with `serde_json`.
            type PrivateState: serde::Serialize
                + serde::de::DeserializeOwned
                + ::core::default::Default;

            #(#trait_methods)*
        }

        /// Adapts a typed [`Witnesses`] impl to the runtime's untyped
        /// `WitnessProvider`. Created by [`Circuits::with_witnesses`].
        #[doc(hidden)]
        pub struct WitnessesAdapter<'w, W: Witnesses>(pub &'w W);

        impl<'w, W: Witnesses> midnight_contract::runtime::WitnessProvider
            for WitnessesAdapter<'w, W>
        {
            fn call_witness(
                &self,
                __ctx: &mut midnight_contract::runtime::WitnessContext<'_>,
                __name: &str,
                __args: &[midnight_contract::runtime::Value],
            ) -> ::core::result::Result<
                midnight_contract::runtime::WitnessOutcome,
                midnight_contract::runtime::InterpreterError,
            > {
                // Unknown names are a non-error outcome (the runtime falls
                // through to builtins/helpers); decide before decoding the
                // private state so an undecodable blob can't mask it.
                match __name {
                    #(#known_names)|* => {}
                    _ => {
                        return ::core::result::Result::Ok(
                            midnight_contract::runtime::WitnessOutcome::Unknown,
                        );
                    }
                }
                let __bytes = __ctx.private_state();
                let mut __ps: <W as Witnesses>::PrivateState = if __bytes.is_empty() {
                    ::core::default::Default::default()
                } else {
                    serde_json::from_slice(__bytes).map_err(|__e| {
                        midnight_contract::runtime::InterpreterError::Witness(
                            ::std::format!("decode private state: {__e}")
                        )
                    })?
                };
                let __ret = match __name {
                    #(#arms)*
                    // Dead arm: the name was matched against the known set
                    // above. Kept non-panicking per the generated-code rule.
                    __other => {
                        return ::core::result::Result::Err(
                            midnight_contract::runtime::InterpreterError::Witness(
                                ::std::format!("witness dispatch desync: {__other} (bug in the generated WitnessesAdapter, please report)")
                            )
                        );
                    }
                };
                let __new = serde_json::to_vec(&__ps).map_err(|__e| {
                    midnight_contract::runtime::InterpreterError::Witness(
                        ::std::format!("encode private state: {__e}")
                    )
                })?;
                __ctx.set_private_state(__new);
                ::core::result::Result::Ok(
                    midnight_contract::runtime::WitnessOutcome::Value(__ret),
                )
            }
        }
    }
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
