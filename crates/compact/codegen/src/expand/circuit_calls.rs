//! Generate the embedded circuit metadata for on-chain circuit calls.
//!
//! We generate:
//! - An `__ir_<name>()` constructor per impure circuit
//! - One `__helpers()`, `__structs()`, `__enums()` constructor each,
//!   shared across all circuits in the contract

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::nir::Type;
use crate::types::ContractInfo;

use super::types::{encode_to_aligned_value, type_to_tokens};

/// Emit the embedded circuit metadata as typed constructor functions:
/// one `__ir_<name>()` per impure circuit, plus `__helpers()`,
/// `__structs()`, and `__enums()`. The compiler checks the embedding, so
/// the generated call path never parses at run time.
pub(crate) fn emit_circuit_ir_constants(info: &ContractInfo) -> TokenStream {
    let model_imports = model_imports();
    let mut ir_fns = Vec::new();

    for circuit in &info.circuits {
        // A pure circuit is evaluated off-chain and has no call path.
        if circuit.pure() {
            continue;
        }
        let ctor = super::emit_ir::circuit(&circuit.def);
        let sanitized = circuit.name.replace(['$', '-'], "_");
        let ir_fn = format_ident!("__ir_{}", sanitized);
        ir_fns.push(quote! {
            #[doc(hidden)]
            pub fn #ir_fn() -> midnight_contract::compact_codegen::nir::Circuit {
                #model_imports
                #ctor
            }
        });
    }

    // Every circuit the contract defines, so the async `Circuits` wrappers in
    // `ledger.rs` can hand them to `execute_with` and the interpreter can
    // resolve a `call` at run time instead of the generator inlining it.
    // Always emitted (empty when none) so callers can unconditionally call
    // `Self::__helpers()`.
    let helpers_ctor = super::emit_ir::circuits(&info.helpers);

    // Nested struct/enum types used by circuit arguments are declared *inline*
    // in each circuit's `arguments`. Harvest them into the
    // registry so the interpreter can compute atom layouts when a circuit
    // destructures a struct argument (e.g. `recipient.is_left`) on the funded
    // call path.
    let mut structs = Vec::new();
    for circuit in &info.circuits {
        crate::arg_types::collect_argument_defs(circuit.arguments(), &mut structs);
    }
    for witness in &info.witnesses {
        crate::arg_types::collect_argument_defs(&witness.arguments, &mut structs);
    }
    let structs_ctor = super::emit_ir::struct_defs(&structs);

    // Native and witness declarations: the interpreter routes a `call` by
    // declaration, and a witness-class native also appends to the private
    // transcript, so both must travel with the contract.
    let natives_ctor = super::emit_ir::natives(&info.natives);
    let witnesses_ctor = super::emit_ir::witnesses(&info.witnesses);

    quote! {
        #[doc(hidden)]
        pub fn __helpers() -> ::std::vec::Vec<midnight_contract::compact_codegen::nir::Circuit> {
            #model_imports
            #helpers_ctor
        }
        #[doc(hidden)]
        pub fn __natives() -> ::std::vec::Vec<midnight_contract::compact_codegen::nir::Native> {
            #model_imports
            #natives_ctor
        }
        #[doc(hidden)]
        pub fn __witnesses() -> ::std::vec::Vec<midnight_contract::compact_codegen::nir::Witness> {
            #model_imports
            #witnesses_ctor
        }
        #[doc(hidden)]
        pub fn __structs() -> ::std::vec::Vec<midnight_contract::compact_codegen::ir::StructDef> {
            #model_imports
            #structs_ctor
        }
        #(#ir_fns)*
    }
}

/// The aliases every embedded constructor is spliced against: `__ir` for the
/// struct/enum registries, `__nir` for the IR model. A registry with no
/// entries needs neither, so the import carries its own allow.
pub(crate) fn model_imports() -> TokenStream {
    quote! {
        #[allow(unused_imports)]
        use midnight_contract::compact_codegen::{ir as __ir, nir as __nir};
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
        // Rejected at load by `normalized::check_type`; unreachable here.
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
