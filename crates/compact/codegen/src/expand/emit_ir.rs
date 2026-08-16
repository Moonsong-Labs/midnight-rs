//! Emit Rust constructor expressions for embedded IR values.
//!
//! Generated bindings carry each circuit's definition, the helper/struct/enum
//! registries, and the per-circuit type metadata as typed constructor
//! functions, so the compiler checks the embedding and nothing parses at
//! run time. Every emitter expects two aliases in scope at the splice site:
//! `__ir` for `midnight_contract::compact_codegen::ir` (the struct/enum
//! registries) and `__nir` for `midnight_contract::compact_codegen::nir`
//! (the IR model).

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::StructDef;
use crate::nir::{
    Argument, ContractCircuit, Curve, Expr, FieldType, Fun, Ident, Instruction, Literal, MapArg,
    Operand, PathElement, StateValue, TupleArg, Type,
};

fn s(v: &str) -> TokenStream {
    quote! { ::std::string::String::from(#v) }
}

fn bx(inner: TokenStream) -> TokenStream {
    quote! { ::std::boxed::Box::new(#inner) }
}

fn vec_of(items: impl Iterator<Item = TokenStream>) -> TokenStream {
    let items: Vec<_> = items.collect();
    quote! { ::std::vec![#(#items),*] }
}

pub(crate) fn circuit(c: &crate::nir::Circuit) -> TokenStream {
    let name = ident(&c.name);
    let exported = c.exported;
    let pure = c.pure;
    let proof = c.proof;
    let arguments = vec_of(c.arguments.iter().map(argument));
    let result_type = type_ref(&c.result_type);
    let body = expr(&c.body);
    quote! {
        __nir::Circuit {
            name: #name,
            exported: #exported,
            pure: #pure,
            proof: #proof,
            arguments: #arguments,
            result_type: #result_type,
            body: #body,
        }
    }
}

pub(crate) fn circuits(defs: &[crate::nir::Circuit]) -> TokenStream {
    vec_of(defs.iter().map(circuit))
}

pub(crate) fn natives(defs: &[crate::nir::Native]) -> TokenStream {
    vec_of(defs.iter().map(|n| {
        let name = ident(&n.name);
        let entry = s(&n.entry);
        let class = s(&n.class);
        let arguments = vec_of(n.arguments.iter().map(argument));
        let result_type = type_ref(&n.result_type);
        quote! {
            __nir::Native {
                name: #name,
                entry: #entry,
                class: #class,
                arguments: #arguments,
                result_type: #result_type,
            }
        }
    }))
}

pub(crate) fn witnesses(defs: &[crate::nir::Witness]) -> TokenStream {
    vec_of(defs.iter().map(|w| {
        let name = ident(&w.name);
        let arguments = vec_of(w.arguments.iter().map(argument));
        let result_type = type_ref(&w.result_type);
        quote! {
            __nir::Witness {
                name: #name,
                arguments: #arguments,
                result_type: #result_type,
            }
        }
    }))
}

pub(crate) fn struct_defs(structs: &[StructDef]) -> TokenStream {
    vec_of(structs.iter().map(|d| {
        let name = s(&d.name);
        let fields = vec_of(d.fields.iter().map(struct_field));
        quote! { __ir::StructDef { name: #name, fields: #fields } }
    }))
}

/// A bound reaches 2^248-1, so no integer literal holds it. The digits go
/// through `FromStr`, whose target type the field infers.
/// `unwrap_or_default` keeps generated code panic-free; the digits come from
/// `BigUint::to_string`, which `FromStr` accepts.
fn biguint(v: &num_bigint::BigUint) -> TokenStream {
    let digits = v.to_string();
    quote! { #digits.parse().unwrap_or_default() }
}

/// The signed counterpart of [`biguint`], for literals and VM operands.
fn bigint(v: &num_bigint::BigInt) -> TokenStream {
    let digits = v.to_string();
    quote! { #digits.parse().unwrap_or_default() }
}

fn bytes(v: &[u8]) -> TokenStream {
    vec_of(v.iter().map(|b| quote! { #b }))
}

fn ident(id: &Ident) -> TokenStream {
    let text = s(&id.0);
    quote! { __nir::Ident(#text) }
}

fn argument(a: &Argument) -> TokenStream {
    let name = ident(&a.name);
    let ty = type_ref(&a.ty);
    quote! { __nir::Argument { name: #name, ty: #ty } }
}

pub(crate) fn type_ref(t: &Type) -> TokenStream {
    match t {
        Type::Boolean => quote! { __nir::Type::Boolean },
        Type::Field(ft) => {
            let ft = field_type(ft);
            quote! { __nir::Type::Field(#ft) }
        }
        Type::Unsigned(maxval) => {
            let maxval = biguint(maxval);
            quote! { __nir::Type::Unsigned(#maxval) }
        }
        Type::Point(c) => {
            let c = curve(c);
            quote! { __nir::Type::Point(#c) }
        }
        Type::Bytes(length) => quote! { __nir::Type::Bytes(#length) },
        Type::Opaque(name) => {
            let name = s(name);
            quote! { __nir::Type::Opaque(#name) }
        }
        Type::Struct { name, fields } => {
            let name = s(name);
            let fields = vec_of(fields.iter().map(struct_field));
            quote! { __nir::Type::Struct { name: #name, fields: #fields } }
        }
        Type::Enum { name, variants } => {
            let name = s(name);
            let variants = vec_of(variants.iter().map(|v| s(v)));
            quote! { __nir::Type::Enum { name: #name, variants: #variants } }
        }
        Type::Tuple(types) => {
            let types = vec_of(types.iter().map(type_ref));
            quote! { __nir::Type::Tuple(#types) }
        }
        Type::Vector { len, ty } => {
            let ty = bx(type_ref(ty));
            quote! { __nir::Type::Vector { len: #len, ty: #ty } }
        }
        Type::Alias { nominal, name, ty } => {
            let name = s(name);
            let ty = bx(type_ref(ty));
            quote! { __nir::Type::Alias { nominal: #nominal, name: #name, ty: #ty } }
        }
        Type::Contract { name, circuits } => {
            let name = s(name);
            let circuits = vec_of(circuits.iter().map(contract_circuit));
            quote! { __nir::Type::Contract { name: #name, circuits: #circuits } }
        }
        Type::Unknown => quote! { __nir::Type::Unknown },
        // Rejected at load by `normalized::check_type`; unreachable here.
        Type::Adt { .. } | Type::TypeVar(_) => quote! { __nir::Type::Unknown },
    }
}

fn curve(c: &Curve) -> TokenStream {
    match c {
        Curve::Jubjub => quote! { __nir::Curve::Jubjub },
        Curve::Secp256k1 => quote! { __nir::Curve::Secp256k1 },
    }
}

fn field_type(ft: &FieldType) -> TokenStream {
    match ft {
        FieldType::Native => quote! { __nir::FieldType::Native },
        FieldType::Base(c) => {
            let c = curve(c);
            quote! { __nir::FieldType::Base(#c) }
        }
        FieldType::Scalar(c) => {
            let c = curve(c);
            quote! { __nir::FieldType::Scalar(#c) }
        }
    }
}

fn contract_circuit(c: &ContractCircuit) -> TokenStream {
    let name = s(&c.name);
    let pure = c.pure;
    let argument_types = vec_of(c.argument_types.iter().map(type_ref));
    let result_type = type_ref(&c.result_type);
    quote! {
        __nir::ContractCircuit {
            name: #name,
            pure: #pure,
            argument_types: #argument_types,
            result_type: #result_type,
        }
    }
}

fn struct_field(f: &(String, Type)) -> TokenStream {
    let name = s(&f.0);
    let ty = type_ref(&f.1);
    quote! { (#name, #ty) }
}

fn literal(l: &Literal) -> TokenStream {
    match l {
        Literal::Int(i) => {
            let i = bigint(i);
            quote! { __nir::Literal::Int(#i) }
        }
        Literal::Bool(b) => quote! { __nir::Literal::Bool(#b) },
        Literal::Bytes(b) => {
            let b = bytes(b);
            quote! { __nir::Literal::Bytes(#b) }
        }
    }
}

fn tuple_arg(a: &TupleArg) -> TokenStream {
    match a {
        TupleArg::Single(e) => {
            let e = expr(e);
            quote! { __nir::TupleArg::Single(#e) }
        }
        TupleArg::Spread { len, expr: e } => {
            let e = expr(e);
            quote! { __nir::TupleArg::Spread { len: #len, expr: #e } }
        }
    }
}

fn map_arg(a: &MapArg) -> TokenStream {
    let e = expr(&a.expr);
    let ty = type_ref(&a.ty);
    let element_ty = type_ref(&a.element_ty);
    quote! { __nir::MapArg { expr: #e, ty: #ty, element_ty: #element_ty } }
}

fn fun(f: &Fun) -> TokenStream {
    match f {
        Fun::Ref(id) => {
            let id = ident(id);
            quote! { __nir::Fun::Ref(#id) }
        }
        Fun::Circuit {
            arguments,
            result_type,
            body,
        } => {
            let arguments = vec_of(arguments.iter().map(argument));
            let result_type = type_ref(result_type);
            let body = bx(expr(body));
            quote! {
                __nir::Fun::Circuit {
                    arguments: #arguments,
                    result_type: #result_type,
                    body: #body,
                }
            }
        }
    }
}

fn path_element(p: &PathElement) -> TokenStream {
    match p {
        PathElement::Index(i) => quote! { __nir::PathElement::Index(#i) },
        PathElement::Computed { ty, expr: e } => {
            let ty = bx(type_ref(ty));
            let e = bx(expr(e));
            quote! { __nir::PathElement::Computed { ty: #ty, expr: #e } }
        }
    }
}

fn instruction(i: &Instruction) -> TokenStream {
    let op = s(&i.op);
    let args = vec_of(i.args.iter().map(|(name, value)| {
        let name = s(name);
        let value = operand(value);
        quote! { (#name, #value) }
    }));
    quote! { __nir::Instruction { op: #op, args: #args } }
}

fn operand(o: &Operand) -> TokenStream {
    match o {
        Operand::Int(i) => {
            let i = bigint(i);
            quote! { __nir::Operand::Int(#i) }
        }
        Operand::Bool(b) => quote! { __nir::Operand::Bool(#b) },
        Operand::Str(v) => {
            let v = s(v);
            quote! { __nir::Operand::Str(#v) }
        }
        Operand::Align { value, bytes: n } => {
            let value = biguint(value);
            quote! { __nir::Operand::Align { value: #value, bytes: #n } }
        }
        Operand::Stack => quote! { __nir::Operand::Stack },
        Operand::Void => quote! { __nir::Operand::Void },
        Operand::ValueToInt(x) => {
            let x = bx(operand(x));
            quote! { __nir::Operand::ValueToInt(#x) }
        }
        Operand::Null(x) => {
            let x = bx(operand(x));
            quote! { __nir::Operand::Null(#x) }
        }
        Operand::MaxSizeof(x) => {
            let x = bx(operand(x));
            quote! { __nir::Operand::MaxSizeof(#x) }
        }
        Operand::LeafHash(x) => {
            let x = bx(operand(x));
            quote! { __nir::Operand::LeafHash(#x) }
        }
        Operand::CoinCommit(coin, recipient) => {
            let coin = bx(operand(coin));
            let recipient = bx(operand(recipient));
            quote! { __nir::Operand::CoinCommit(#coin, #recipient) }
        }
        Operand::AlignedConcat(xs) => {
            let xs = vec_of(xs.iter().map(operand));
            quote! { __nir::Operand::AlignedConcat(#xs) }
        }
        Operand::StateValue(sv) => {
            let sv = state_value(sv);
            quote! { __nir::Operand::StateValue(#sv) }
        }
        Operand::Expr(e) => {
            let e = bx(expr(e));
            quote! { __nir::Operand::Expr(#e) }
        }
        Operand::List(xs) => {
            let xs = vec_of(xs.iter().map(operand));
            quote! { __nir::Operand::List(#xs) }
        }
    }
}

fn operand_pairs(entries: &[(Operand, Operand)]) -> TokenStream {
    vec_of(entries.iter().map(|(k, v)| {
        let k = operand(k);
        let v = operand(v);
        quote! { (#k, #v) }
    }))
}

fn state_value(sv: &StateValue) -> TokenStream {
    match sv {
        StateValue::Null => quote! { __nir::StateValue::Null },
        StateValue::Cell(x) => {
            let x = bx(operand(x));
            quote! { __nir::StateValue::Cell(#x) }
        }
        StateValue::Adt(x) => {
            let x = bx(operand(x));
            quote! { __nir::StateValue::Adt(#x) }
        }
        StateValue::Array(xs) => {
            let xs = vec_of(xs.iter().map(operand));
            quote! { __nir::StateValue::Array(#xs) }
        }
        StateValue::Map(entries) => {
            let entries = operand_pairs(entries);
            quote! { __nir::StateValue::Map(#entries) }
        }
        StateValue::MerkleTree { depth, entries } => {
            let entries = operand_pairs(entries);
            quote! { __nir::StateValue::MerkleTree { depth: #depth, entries: #entries } }
        }
    }
}

/// The arithmetic and equality operators, which all carry an operand type.
fn typed_binop(variant: TokenStream, ty: &Type, left: &Expr, right: &Expr) -> TokenStream {
    let ty = type_ref(ty);
    let left = bx(expr(left));
    let right = bx(expr(right));
    quote! { __nir::Expr::#variant { ty: #ty, left: #left, right: #right } }
}

/// The ordering comparisons, which carry the operand width instead of a type.
fn cmp(variant: TokenStream, bits: u64, left: &Expr, right: &Expr) -> TokenStream {
    let left = bx(expr(left));
    let right = bx(expr(right));
    quote! { __nir::Expr::#variant { bits: #bits, left: #left, right: #right } }
}

fn expr(e: &Expr) -> TokenStream {
    match e {
        Expr::Quote(lit) => {
            let lit = literal(lit);
            quote! { __nir::Expr::Quote(#lit) }
        }
        Expr::VarRef(id) => {
            let id = ident(id);
            quote! { __nir::Expr::VarRef(#id) }
        }
        Expr::Default(ty) => {
            let ty = type_ref(ty);
            quote! { __nir::Expr::Default(#ty) }
        }
        Expr::If { cond, then, els } => {
            let cond = bx(expr(cond));
            let then = bx(expr(then));
            let els = bx(expr(els));
            quote! { __nir::Expr::If { cond: #cond, then: #then, els: #els } }
        }
        Expr::EltRef {
            expr: inner,
            elt,
            index,
        } => {
            let inner = bx(expr(inner));
            let elt = s(elt);
            quote! { __nir::Expr::EltRef { expr: #inner, elt: #elt, index: #index } }
        }
        Expr::EnumRef { ty, elt } => {
            let ty = type_ref(ty);
            let elt = s(elt);
            quote! { __nir::Expr::EnumRef { ty: #ty, elt: #elt } }
        }
        Expr::Tuple(args) => {
            let args = vec_of(args.iter().map(tuple_arg));
            quote! { __nir::Expr::Tuple(#args) }
        }
        Expr::VectorLit(args) => {
            let args = vec_of(args.iter().map(tuple_arg));
            quote! { __nir::Expr::VectorLit(#args) }
        }
        Expr::TupleRef { expr: inner, index } => {
            let inner = bx(expr(inner));
            quote! { __nir::Expr::TupleRef { expr: #inner, index: #index } }
        }
        Expr::TupleSlice {
            ty,
            expr: inner,
            index,
            len,
        } => {
            let ty = type_ref(ty);
            let inner = bx(expr(inner));
            quote! { __nir::Expr::TupleSlice { ty: #ty, expr: #inner, index: #index, len: #len } }
        }
        Expr::VectorRef {
            ty,
            expr: inner,
            index,
        } => {
            let ty = type_ref(ty);
            let inner = bx(expr(inner));
            let index = bx(expr(index));
            quote! { __nir::Expr::VectorRef { ty: #ty, expr: #inner, index: #index } }
        }
        Expr::VectorSlice {
            ty,
            expr: inner,
            index,
            len,
        } => {
            let ty = type_ref(ty);
            let inner = bx(expr(inner));
            let index = bx(expr(index));
            quote! { __nir::Expr::VectorSlice { ty: #ty, expr: #inner, index: #index, len: #len } }
        }
        Expr::BytesRef {
            ty,
            expr: inner,
            index,
        } => {
            let ty = type_ref(ty);
            let inner = bx(expr(inner));
            let index = bx(expr(index));
            quote! { __nir::Expr::BytesRef { ty: #ty, expr: #inner, index: #index } }
        }
        Expr::BytesSlice {
            ty,
            expr: inner,
            index,
            len,
        } => {
            let ty = type_ref(ty);
            let inner = bx(expr(inner));
            let index = bx(expr(index));
            quote! { __nir::Expr::BytesSlice { ty: #ty, expr: #inner, index: #index, len: #len } }
        }
        Expr::Add { ty, left, right } => typed_binop(quote! { Add }, ty, left, right),
        Expr::Sub { ty, left, right } => typed_binop(quote! { Sub }, ty, left, right),
        Expr::Mul { ty, left, right } => typed_binop(quote! { Mul }, ty, left, right),
        Expr::Eq { ty, left, right } => typed_binop(quote! { Eq }, ty, left, right),
        Expr::Neq { ty, left, right } => typed_binop(quote! { Neq }, ty, left, right),
        Expr::Lt { bits, left, right } => cmp(quote! { Lt }, *bits, left, right),
        Expr::Le { bits, left, right } => cmp(quote! { Le }, *bits, left, right),
        Expr::Gt { bits, left, right } => cmp(quote! { Gt }, *bits, left, right),
        Expr::Ge { bits, left, right } => cmp(quote! { Ge }, *bits, left, right),
        Expr::Map { len, fun: f, args } => {
            let f = fun(f);
            let args = vec_of(args.iter().map(map_arg));
            quote! { __nir::Expr::Map { len: #len, fun: #f, args: #args } }
        }
        Expr::Fold {
            len,
            fun: f,
            init,
            init_ty,
            args,
        } => {
            let f = fun(f);
            let init = bx(expr(init));
            let init_ty = type_ref(init_ty);
            let args = vec_of(args.iter().map(map_arg));
            quote! {
                __nir::Expr::Fold {
                    len: #len,
                    fun: #f,
                    init: #init,
                    init_ty: #init_ty,
                    args: #args,
                }
            }
        }
        Expr::Call { name, args } => {
            let name = ident(name);
            let args = vec_of(args.iter().map(expr));
            quote! { __nir::Expr::Call { name: #name, args: #args } }
        }
        Expr::New { ty, elements } => {
            let ty = type_ref(ty);
            let elements = vec_of(elements.iter().map(expr));
            quote! { __nir::Expr::New { ty: #ty, elements: #elements } }
        }
        Expr::Seq(items) => {
            let items = vec_of(items.iter().map(expr));
            quote! { __nir::Expr::Seq(#items) }
        }
        Expr::LetStar { bindings, body } => {
            let bindings = vec_of(bindings.iter().map(|(a, value)| {
                let a = argument(a);
                let value = expr(value);
                quote! { (#a, #value) }
            }));
            let body = bx(expr(body));
            quote! { __nir::Expr::LetStar { bindings: #bindings, body: #body } }
        }
        Expr::Assert {
            expr: inner,
            message,
        } => {
            let inner = bx(expr(inner));
            let message = s(message);
            quote! { __nir::Expr::Assert { expr: #inner, message: #message } }
        }
        Expr::FieldToBytes {
            len,
            field_type: ft,
            expr: inner,
        } => {
            let ft = field_type(ft);
            let inner = bx(expr(inner));
            quote! { __nir::Expr::FieldToBytes { len: #len, field_type: #ft, expr: #inner } }
        }
        Expr::CastFromBytes {
            ty,
            len,
            expr: inner,
        } => {
            let ty = type_ref(ty);
            let inner = bx(expr(inner));
            quote! { __nir::Expr::CastFromBytes { ty: #ty, len: #len, expr: #inner } }
        }
        Expr::VectorToBytes { len, expr: inner } => {
            let inner = bx(expr(inner));
            quote! { __nir::Expr::VectorToBytes { len: #len, expr: #inner } }
        }
        Expr::BytesToVector { len, expr: inner } => {
            let inner = bx(expr(inner));
            quote! { __nir::Expr::BytesToVector { len: #len, expr: #inner } }
        }
        Expr::CastFromEnum {
            ty,
            from,
            expr: inner,
        } => {
            let ty = type_ref(ty);
            let from = type_ref(from);
            let inner = bx(expr(inner));
            quote! { __nir::Expr::CastFromEnum { ty: #ty, from: #from, expr: #inner } }
        }
        Expr::CastToEnum {
            ty,
            from,
            expr: inner,
        } => {
            let ty = type_ref(ty);
            let from = type_ref(from);
            let inner = bx(expr(inner));
            quote! { __nir::Expr::CastToEnum { ty: #ty, from: #from, expr: #inner } }
        }
        Expr::CastToField {
            field_type: ft,
            from,
            expr: inner,
        } => {
            let ft = field_type(ft);
            let from = type_ref(from);
            let inner = bx(expr(inner));
            quote! { __nir::Expr::CastToField { field_type: #ft, from: #from, expr: #inner } }
        }
        Expr::CastFromField {
            maxval,
            field_type: ft,
            expr: inner,
        } => {
            let maxval = biguint(maxval);
            let ft = field_type(ft);
            let inner = bx(expr(inner));
            quote! { __nir::Expr::CastFromField { maxval: #maxval, field_type: #ft, expr: #inner } }
        }
        Expr::SafeCast {
            ty,
            from,
            expr: inner,
        } => {
            let ty = type_ref(ty);
            let from = type_ref(from);
            let inner = bx(expr(inner));
            quote! { __nir::Expr::SafeCast { ty: #ty, from: #from, expr: #inner } }
        }
        Expr::DowncastUnsigned {
            from_maxval,
            to_maxval,
            expr: inner,
        } => {
            let from_maxval = biguint(from_maxval);
            let to_maxval = biguint(to_maxval);
            let inner = bx(expr(inner));
            quote! {
                __nir::Expr::DowncastUnsigned {
                    from_maxval: #from_maxval,
                    to_maxval: #to_maxval,
                    expr: #inner,
                }
            }
        }
        Expr::ContractCall {
            circuit: name,
            receiver,
            contract_type,
            args,
        } => {
            let name = s(name);
            let receiver = bx(expr(receiver));
            let contract_type = type_ref(contract_type);
            let args = vec_of(args.iter().map(expr));
            quote! {
                __nir::Expr::ContractCall {
                    circuit: #name,
                    receiver: #receiver,
                    contract_type: #contract_type,
                    args: #args,
                }
            }
        }
        Expr::Emit {
            event_version,
            event_tag,
            len,
            payload,
            instructions,
        } => {
            let payload = bx(expr(payload));
            let instructions = vec_of(instructions.iter().map(instruction));
            quote! {
                __nir::Expr::Emit {
                    event_version: #event_version,
                    event_tag: #event_tag,
                    len: #len,
                    payload: #payload,
                    instructions: #instructions,
                }
            }
        }
        Expr::PublicLedger {
            field,
            path,
            op,
            result_type,
            instructions,
            args,
        } => {
            let field = ident(field);
            let path = vec_of(path.iter().map(path_element));
            let op = s(op);
            let result_type = type_ref(result_type);
            let instructions = vec_of(instructions.iter().map(instruction));
            let args = vec_of(args.iter().map(expr));
            quote! {
                __nir::Expr::PublicLedger {
                    field: #field,
                    path: #path,
                    op: #op,
                    result_type: #result_type,
                    instructions: #instructions,
                    args: #args,
                }
            }
        }
        Expr::Return(inner) => {
            let inner = bx(expr(inner));
            quote! { __nir::Expr::Return(#inner) }
        }
    }
}
