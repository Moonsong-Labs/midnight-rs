//! Generate the embedded circuit metadata for on-chain circuit calls.
//!
//! We generate:
//! - `__helpers()`, `__natives()`, `__witnesses()`: the declarations a call
//!   resolves against
//! - One `__helpers()`, `__structs()`, `__enums()` constructor each,
//!   shared across all circuits in the contract

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::ir::Type;
use crate::types::ContractInfo;

use super::types::{encode_to_aligned_value, type_to_tokens};

/// Emit the contract's declarations as typed constructor functions:
/// `__helpers()`, `__natives()` and `__witnesses()`. The compiler checks the
/// embedding, so the generated call path never parses at run time, and a
/// circuit's body is carried once rather than per call site.
pub(crate) fn emit_circuit_ir_constants(info: &ContractInfo) -> TokenStream {
    let model_imports = model_imports();
    // Every circuit the contract defines, so the async `Circuits` wrappers in
    // `ledger.rs` can hand them to `execute_with` and the interpreter can
    // resolve a `call` at run time instead of the generator inlining it.
    // Always emitted (empty when none) so callers can unconditionally call
    // `Self::__helpers()`.
    let helpers_ctor = super::emit_ir::circuits(&info.helpers);

    // Native and witness declarations: the interpreter routes a `call` by
    // declaration, and a witness-class native also appends to the private
    // transcript, so both must travel with the contract.
    let natives_ctor = super::emit_ir::natives(&info.natives);
    let witnesses_ctor = super::emit_ir::witnesses(&info.witnesses);

    quote! {
        #[doc(hidden)]
        pub fn __helpers() -> ::std::vec::Vec<midnight_contract::compact_codegen::ir::Circuit> {
            #model_imports
            #helpers_ctor
        }
        #[doc(hidden)]
        pub fn __natives() -> ::std::vec::Vec<midnight_contract::compact_codegen::ir::Native> {
            #model_imports
            #natives_ctor
        }
        #[doc(hidden)]
        pub fn __witnesses() -> ::std::vec::Vec<midnight_contract::compact_codegen::ir::Witness> {
            #model_imports
            #witnesses_ctor
        }
    }
}

/// The alias every embedded constructor is spliced against. A contract with
/// no circuits needs it nowhere, so the import carries its own allow.
pub(crate) fn model_imports() -> TokenStream {
    quote! {
        #[allow(unused_imports)]
        use midnight_contract::compact_codegen::ir as __ir;
    }
}

/// Returns true if this type is the unit type, the result type of a circuit
/// that returns nothing.
pub(crate) fn is_void_type(ty: &Type) -> bool {
    ty.is_unit()
}

/// Generate a token stream expression that converts `midnight_contract::runtime::Value`
/// (in variable `__val`) to the target Rust type. Used for circuit return
/// values and typed witness arguments.
///
/// `context` is a codegen-time label naming what is being converted (e.g.
/// ``circuit `increment` return value`` or ``witness `secret_key` argument
/// `idx` ``); it is baked into the generated `TypeError` messages so a
/// mismatch names its source instead of just the expected shape.
///
/// The generated expression evaluates to
/// `Result<T, midnight_contract::runtime::InterpreterError>` — the
/// interpreter's output is contract-data dependent, so a mismatch must flow
/// into the caller's error path instead of panicking. Callers `?` it: the
/// witness adapter already returns `InterpreterError`, and the async circuit
/// methods convert via `From<InterpreterError> for ContractError`.
pub(crate) fn value_to_type_conversion(ty: &Type, context: &str) -> TokenStream {
    match ty {
        Type::Boolean => {
            let mismatch_msg = format!("{context}: expected a Bool value, got {{:?}}");
            quote! {
                match __val {
                    midnight_contract::runtime::Value::Bool(__b) => {
                        ::core::result::Result::Ok(__b)
                    }
                    __other => ::core::result::Result::Err(
                        midnight_contract::runtime::InterpreterError::TypeError(
                            ::std::format!(#mismatch_msg, __other)
                        )
                    ),
                }
            }
        }
        Type::Unsigned(_) => {
            let rust_ty = type_to_tokens(ty);
            let overflow_msg = format!("{context}: value {{}} does not fit in {{}}");
            let mismatch_msg = format!("{context}: expected an Integer value, got {{:?}}");
            quote! {
                match __val {
                    midnight_contract::runtime::Value::Integer(__n) => {
                        <#rust_ty>::try_from(__n).map_err(|_| {
                            midnight_contract::runtime::InterpreterError::TypeError(
                                ::std::format!(
                                    #overflow_msg,
                                    __n,
                                    ::core::stringify!(#rust_ty)
                                )
                            )
                        })
                    }
                    __other => ::core::result::Result::Err(
                        midnight_contract::runtime::InterpreterError::TypeError(
                            ::std::format!(#mismatch_msg, __other)
                        )
                    ),
                }
            }
        }
        Type::Alias { ty: inner, .. } => value_to_type_conversion(inner, context),
        _ => {
            let rust_ty = type_to_tokens(ty);
            let convert_msg = format!("{context}: failed to convert value to {{}}: {{}}");
            let mismatch_msg = format!("{context}: expected an AlignedValue, got {{:?}}");
            quote! {
                match __val {
                    midnight_contract::runtime::Value::AlignedValue(__av) => {
                        <#rust_ty>::try_from(&*__av.value).map_err(|__e| {
                            midnight_contract::runtime::InterpreterError::TypeError(
                                ::std::format!(
                                    #convert_msg,
                                    ::core::stringify!(#rust_ty),
                                    __e
                                )
                            )
                        })
                    }
                    __other => ::core::result::Result::Err(
                        midnight_contract::runtime::InterpreterError::TypeError(
                            ::std::format!(#mismatch_msg, __other)
                        )
                    ),
                }
            }
        }
    }
}

/// Returns true if this type has a direct conversion into `AlignedValue`
/// (and therefore into `runtime::Value::AlignedValue`) via the
/// bindgen-emitted encoders. Keep in sync with `type_to_value_conversion`.
pub(crate) fn has_typed_conversion(ty: &Type) -> bool {
    match ty {
        Type::Boolean
        | Type::Unsigned(_)
        | Type::Field(_)
        | Type::Bytes(_)
        | Type::Struct { .. }
        | Type::Enum { .. }
        | Type::Vector { .. }
        | Type::Tuple(_) => true,
        Type::Alias { ty: inner, .. } => has_typed_conversion(inner),
        // Every opaque type has a typed Rust counterpart: `JubjubPoint` (which
        // the runtime also spells as a curve point) and `Scalar<BLS12-381>`
        // map to their own types, and the rest to `Vec<u8>`, which encodes as
        // the single `Compress` atom the Compact runtime uses for opaque
        // values. Without this the parameter falls back to the untyped
        // `runtime::Value` escape hatch and its value is dropped.
        Type::Opaque(_) | Type::Point(_) => true,
        Type::Contract { .. } => false,
        // Rejected at load by `artifact::check_type`; unreachable here.
        Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => false,
    }
}

/// Generate the expression to convert a typed Rust argument to
/// `runtime::Value`. Scalars stay as native variants; compound types
/// are encoded via `From<T> for AlignedValue` and wrapped in
/// `Value::AlignedValue(_)`.
pub(crate) fn type_to_value_conversion(arg_ident: &proc_macro2::Ident, ty: &Type) -> TokenStream {
    match ty {
        Type::Boolean => {
            quote! { midnight_contract::runtime::Value::Bool(#arg_ident) }
        }
        Type::Unsigned(_) => {
            quote! { midnight_contract::runtime::Value::Integer(#arg_ident as u128) }
        }
        // Vector arguments must be passed as `Value::Tuple` so the
        // interpreter's `index` op can walk into individual elements
        // (used by the unrolled `map`/`fold` lowering). Pre-flattening
        // a `Vector<N, T>` into a single `AlignedValue` would prevent
        // structural indexing — the interpreter would only see opaque
        // atoms with no element boundary.
        //
        // The flatten-to-`AlignedValue` step still happens at the prover
        // boundary via `Value::to_aligned_value`, which walks `Value::Tuple`
        // recursively. So this change preserves the on-chain encoding while
        // letting the off-chain interpreter index per-element.
        Type::Vector { ty: inner, .. } => {
            let elem_ident = format_ident!("__vec_elem");
            let elem_conv = type_to_value_conversion(&elem_ident, inner);
            quote! {
                midnight_contract::runtime::Value::Tuple(
                    ::std::iter::IntoIterator::into_iter(#arg_ident)
                        .map(|#elem_ident| #elem_conv)
                        .collect::<::std::vec::Vec<_>>()
                )
            }
        }
        Type::Alias { ty: inner, .. } => type_to_value_conversion(arg_ident, inner),
        _ => {
            let av = encode_to_aligned_value(&quote! { #arg_ident }, ty);
            quote! { midnight_contract::runtime::Value::AlignedValue(#av) }
        }
    }
}
