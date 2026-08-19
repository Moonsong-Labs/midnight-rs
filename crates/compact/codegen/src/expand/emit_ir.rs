//! Emit Rust constructor expressions for embedded IR values.
//!
//! Generated bindings carry each circuit's definition, the witnesses and the
//! natives as typed constructor expressions, so the Rust compiler checks the
//! embedding and nothing parses at run time. Every emitter expects the alias
//! `__ir` for `midnight_contract::compact_codegen::ir` in scope at the splice
//! site.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{
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

pub(crate) fn circuit(c: &crate::ir::Circuit) -> TokenStream {
    let name = ident(&c.name);
    let exported = c.exported;
    let pure = c.pure;
    let proof = c.proof;
    let arguments = vec_of(c.arguments.iter().map(argument));
    let result_type = type_ref(&c.result_type);
    let body = expr(&c.body);
    quote! {
        __ir::Circuit {
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

pub(crate) fn circuits(defs: &[crate::ir::Circuit]) -> TokenStream {
    vec_of(defs.iter().map(circuit))
}

pub(crate) fn natives(defs: &[crate::ir::Native]) -> TokenStream {
    vec_of(defs.iter().map(|n| {
        let name = ident(&n.name);
        let entry = s(&n.entry);
        let class = s(&n.class);
        let type_arguments = vec_of(n.type_arguments.iter().map(type_ref));
        let arguments = vec_of(n.arguments.iter().map(argument));
        let result_type = type_ref(&n.result_type);
        quote! {
            __ir::Native {
                name: #name,
                entry: #entry,
                class: #class,
                type_arguments: #type_arguments,
                arguments: #arguments,
                result_type: #result_type,
            }
        }
    }))
}

pub(crate) fn witnesses(defs: &[crate::ir::Witness]) -> TokenStream {
    vec_of(defs.iter().map(|w| {
        let name = ident(&w.name);
        let arguments = vec_of(w.arguments.iter().map(argument));
        let result_type = type_ref(&w.result_type);
        quote! {
            __ir::Witness {
                name: #name,
                arguments: #arguments,
                result_type: #result_type,
            }
        }
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
    quote! { __ir::Ident(#text) }
}

fn argument(a: &Argument) -> TokenStream {
    let name = ident(&a.name);
    let ty = type_ref(&a.ty);
    quote! { __ir::Argument { name: #name, ty: #ty } }
}

pub(crate) fn type_ref(t: &Type) -> TokenStream {
    match t {
        Type::Boolean => quote! { __ir::Type::Boolean },
        Type::Field(ft) => {
            let ft = field_type(ft);
            quote! { __ir::Type::Field(#ft) }
        }
        Type::Unsigned(maxval) => {
            let maxval = biguint(maxval);
            quote! { __ir::Type::Unsigned(#maxval) }
        }
        Type::Point(c) => {
            let c = curve(c);
            quote! { __ir::Type::Point(#c) }
        }
        Type::Bytes(length) => quote! { __ir::Type::Bytes(#length) },
        Type::Opaque(name) => {
            let name = s(name);
            quote! { __ir::Type::Opaque(#name) }
        }
        Type::Struct { name, fields } => {
            let name = s(name);
            let fields = vec_of(fields.iter().map(struct_field));
            quote! { __ir::Type::Struct { name: #name, fields: #fields } }
        }
        Type::Enum { name, variants } => {
            let name = s(name);
            let variants = vec_of(variants.iter().map(|v| s(v)));
            quote! { __ir::Type::Enum { name: #name, variants: #variants } }
        }
        Type::Tuple(types) => {
            let types = vec_of(types.iter().map(type_ref));
            quote! { __ir::Type::Tuple(#types) }
        }
        Type::Vector { len, ty } => {
            let ty = bx(type_ref(ty));
            quote! { __ir::Type::Vector { len: #len, ty: #ty } }
        }
        Type::Alias { nominal, name, ty } => {
            let name = s(name);
            let ty = bx(type_ref(ty));
            quote! { __ir::Type::Alias { nominal: #nominal, name: #name, ty: #ty } }
        }
        Type::Contract { name, circuits } => {
            let name = s(name);
            let circuits = vec_of(circuits.iter().map(contract_circuit));
            quote! { __ir::Type::Contract { name: #name, circuits: #circuits } }
        }
        Type::Unknown => quote! { __ir::Type::Unknown },
        // Rejected at load by `artifact::check_type`; unreachable here.
        Type::Adt { .. } | Type::TypeVar(_) => quote! { __ir::Type::Unknown },
    }
}

fn curve(c: &Curve) -> TokenStream {
    match c {
        Curve::Jubjub => quote! { __ir::Curve::Jubjub },
        Curve::Secp256k1 => quote! { __ir::Curve::Secp256k1 },
    }
}

fn field_type(ft: &FieldType) -> TokenStream {
    match ft {
        FieldType::Native => quote! { __ir::FieldType::Native },
        FieldType::Base(c) => {
            let c = curve(c);
            quote! { __ir::FieldType::Base(#c) }
        }
        FieldType::Scalar(c) => {
            let c = curve(c);
            quote! { __ir::FieldType::Scalar(#c) }
        }
    }
}

fn contract_circuit(c: &ContractCircuit) -> TokenStream {
    let name = s(&c.name);
    let pure = c.pure;
    let argument_types = vec_of(c.argument_types.iter().map(type_ref));
    let result_type = type_ref(&c.result_type);
    quote! {
        __ir::ContractCircuit {
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
            quote! { __ir::Literal::Int(#i) }
        }
        Literal::Bool(b) => quote! { __ir::Literal::Bool(#b) },
        Literal::Bytes(b) => {
            let b = bytes(b);
            quote! { __ir::Literal::Bytes(#b) }
        }
    }
}

fn tuple_arg(a: &TupleArg) -> TokenStream {
    match a {
        TupleArg::Single(e) => {
            let e = expr(e);
            quote! { __ir::TupleArg::Single(#e) }
        }
        TupleArg::Spread { len, expr: e } => {
            let e = expr(e);
            quote! { __ir::TupleArg::Spread { len: #len, expr: #e } }
        }
    }
}

fn map_arg(a: &MapArg) -> TokenStream {
    let e = expr(&a.expr);
    let ty = type_ref(&a.ty);
    let element_ty = type_ref(&a.element_ty);
    quote! { __ir::MapArg { expr: #e, ty: #ty, element_ty: #element_ty } }
}

fn fun(f: &Fun) -> TokenStream {
    match f {
        Fun::Ref(id) => {
            let id = ident(id);
            quote! { __ir::Fun::Ref(#id) }
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
                __ir::Fun::Circuit {
                    arguments: #arguments,
                    result_type: #result_type,
                    body: #body,
                }
            }
        }
    }
}

fn class(c: &crate::ir::OpClass) -> TokenStream {
    match c {
        crate::ir::OpClass::Plain(name) => {
            let name = s(name);
            quote! { __ir::OpClass::Plain(#name) }
        }
        crate::ir::OpClass::CoinCheck {
            name,
            coin,
            recipient,
        } => {
            let name = s(name);
            quote! { __ir::OpClass::CoinCheck { name: #name, coin: #coin, recipient: #recipient } }
        }
    }
}

fn path_element(p: &PathElement) -> TokenStream {
    match p {
        PathElement::Index(i) => quote! { __ir::PathElement::Index(#i) },
        PathElement::Computed { ty, expr: e } => {
            let ty = bx(type_ref(ty));
            let e = bx(expr(e));
            quote! { __ir::PathElement::Computed { ty: #ty, expr: #e } }
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
    quote! { __ir::Instruction { op: #op, args: #args } }
}

fn operand(o: &Operand) -> TokenStream {
    match o {
        Operand::Int(i) => {
            let i = bigint(i);
            quote! { __ir::Operand::Int(#i) }
        }
        Operand::Bool(b) => quote! { __ir::Operand::Bool(#b) },
        Operand::Str(v) => {
            let v = s(v);
            quote! { __ir::Operand::Str(#v) }
        }
        Operand::Align { value, bytes: n } => {
            let value = biguint(value);
            quote! { __ir::Operand::Align { value: #value, bytes: #n } }
        }
        Operand::Stack => quote! { __ir::Operand::Stack },
        Operand::Void => quote! { __ir::Operand::Void },
        Operand::ValueToInt(x) => {
            let x = bx(operand(x));
            quote! { __ir::Operand::ValueToInt(#x) }
        }
        Operand::Null(t) => {
            let t = type_ref(t);
            quote! { __ir::Operand::Null(#t) }
        }
        Operand::MaxSizeof(t) => {
            let t = type_ref(t);
            quote! { __ir::Operand::MaxSizeof(#t) }
        }
        Operand::Add(a, b) => {
            let a = bx(operand(a));
            let b = bx(operand(b));
            quote! { __ir::Operand::Add(#a, #b) }
        }
        Operand::LeafHash(x) => {
            let x = bx(operand(x));
            quote! { __ir::Operand::LeafHash(#x) }
        }
        Operand::CoinCommit(coin, recipient) => {
            let coin = bx(operand(coin));
            let recipient = bx(operand(recipient));
            quote! { __ir::Operand::CoinCommit(#coin, #recipient) }
        }
        Operand::AlignedConcat(xs) => {
            let xs = vec_of(xs.iter().map(operand));
            quote! { __ir::Operand::AlignedConcat(#xs) }
        }
        Operand::StateValue(sv) => {
            let sv = state_value(sv);
            quote! { __ir::Operand::StateValue(#sv) }
        }
        Operand::Expr(e) => {
            let e = bx(expr(e));
            quote! { __ir::Operand::Expr(#e) }
        }
        Operand::List(xs) => {
            let xs = vec_of(xs.iter().map(operand));
            quote! { __ir::Operand::List(#xs) }
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
        StateValue::Null => quote! { __ir::StateValue::Null },
        StateValue::Cell(x) => {
            let x = bx(operand(x));
            quote! { __ir::StateValue::Cell(#x) }
        }
        StateValue::Adt(x, t) => {
            let x = bx(operand(x));
            let t = type_ref(t);
            quote! { __ir::StateValue::Adt(#x, #t) }
        }
        StateValue::Array(xs) => {
            let xs = vec_of(xs.iter().map(operand));
            quote! { __ir::StateValue::Array(#xs) }
        }
        StateValue::Map(entries) => {
            let entries = operand_pairs(entries);
            quote! { __ir::StateValue::Map(#entries) }
        }
        StateValue::MerkleTree { depth, entries } => {
            let entries = operand_pairs(entries);
            quote! { __ir::StateValue::MerkleTree { depth: #depth, entries: #entries } }
        }
    }
}

/// The arithmetic and equality operators, which all carry an operand type.
fn typed_binop(variant: TokenStream, ty: &Type, left: &Expr, right: &Expr) -> TokenStream {
    let ty = type_ref(ty);
    let left = bx(expr(left));
    let right = bx(expr(right));
    quote! { __ir::Expr::#variant { ty: #ty, left: #left, right: #right } }
}

/// The ordering comparisons, which carry the operand width instead of a type.
fn cmp(variant: TokenStream, bits: u64, left: &Expr, right: &Expr) -> TokenStream {
    let left = bx(expr(left));
    let right = bx(expr(right));
    quote! { __ir::Expr::#variant { bits: #bits, left: #left, right: #right } }
}

fn expr(e: &Expr) -> TokenStream {
    match e {
        Expr::Quote(lit) => {
            let lit = literal(lit);
            quote! { __ir::Expr::Quote(#lit) }
        }
        Expr::VarRef(id) => {
            let id = ident(id);
            quote! { __ir::Expr::VarRef(#id) }
        }
        Expr::Default(ty) => {
            let ty = type_ref(ty);
            quote! { __ir::Expr::Default(#ty) }
        }
        Expr::If { cond, then, els } => {
            let cond = bx(expr(cond));
            let then = bx(expr(then));
            let els = bx(expr(els));
            quote! { __ir::Expr::If { cond: #cond, then: #then, els: #els } }
        }
        Expr::EltRef {
            expr: inner,
            elt,
            index,
        } => {
            let inner = bx(expr(inner));
            let elt = s(elt);
            quote! { __ir::Expr::EltRef { expr: #inner, elt: #elt, index: #index } }
        }
        Expr::EnumRef { ty, elt } => {
            let ty = type_ref(ty);
            let elt = s(elt);
            quote! { __ir::Expr::EnumRef { ty: #ty, elt: #elt } }
        }
        Expr::Tuple(args) => {
            let args = vec_of(args.iter().map(tuple_arg));
            quote! { __ir::Expr::Tuple(#args) }
        }
        Expr::VectorLit(args) => {
            let args = vec_of(args.iter().map(tuple_arg));
            quote! { __ir::Expr::VectorLit(#args) }
        }
        Expr::TupleRef { expr: inner, index } => {
            let inner = bx(expr(inner));
            quote! { __ir::Expr::TupleRef { expr: #inner, index: #index } }
        }
        Expr::TupleSlice {
            ty,
            expr: inner,
            index,
            len,
        } => {
            let ty = type_ref(ty);
            let inner = bx(expr(inner));
            quote! { __ir::Expr::TupleSlice { ty: #ty, expr: #inner, index: #index, len: #len } }
        }
        Expr::VectorRef {
            ty,
            expr: inner,
            index,
        } => {
            let ty = type_ref(ty);
            let inner = bx(expr(inner));
            let index = bx(expr(index));
            quote! { __ir::Expr::VectorRef { ty: #ty, expr: #inner, index: #index } }
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
            quote! { __ir::Expr::VectorSlice { ty: #ty, expr: #inner, index: #index, len: #len } }
        }
        Expr::BytesRef {
            ty,
            expr: inner,
            index,
        } => {
            let ty = type_ref(ty);
            let inner = bx(expr(inner));
            let index = bx(expr(index));
            quote! { __ir::Expr::BytesRef { ty: #ty, expr: #inner, index: #index } }
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
            quote! { __ir::Expr::BytesSlice { ty: #ty, expr: #inner, index: #index, len: #len } }
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
            quote! { __ir::Expr::Map { len: #len, fun: #f, args: #args } }
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
                __ir::Expr::Fold {
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
            quote! { __ir::Expr::Call { name: #name, args: #args } }
        }
        Expr::New { ty, elements } => {
            let ty = type_ref(ty);
            let elements = vec_of(elements.iter().map(expr));
            quote! { __ir::Expr::New { ty: #ty, elements: #elements } }
        }
        Expr::Seq(items) => {
            let items = vec_of(items.iter().map(expr));
            quote! { __ir::Expr::Seq(#items) }
        }
        Expr::LetStar { bindings, body } => {
            let bindings = vec_of(bindings.iter().map(|(a, value)| {
                let a = argument(a);
                let value = expr(value);
                quote! { (#a, #value) }
            }));
            let body = bx(expr(body));
            quote! { __ir::Expr::LetStar { bindings: #bindings, body: #body } }
        }
        Expr::Assert {
            expr: inner,
            message,
        } => {
            let inner = bx(expr(inner));
            let message = s(message);
            quote! { __ir::Expr::Assert { expr: #inner, message: #message } }
        }
        Expr::FieldToBytes {
            len,
            field_type: ft,
            expr: inner,
        } => {
            let ft = field_type(ft);
            let inner = bx(expr(inner));
            quote! { __ir::Expr::FieldToBytes { len: #len, field_type: #ft, expr: #inner } }
        }
        Expr::CastFromBytes {
            ty,
            len,
            expr: inner,
        } => {
            let ty = type_ref(ty);
            let inner = bx(expr(inner));
            quote! { __ir::Expr::CastFromBytes { ty: #ty, len: #len, expr: #inner } }
        }
        Expr::VectorToBytes { len, expr: inner } => {
            let inner = bx(expr(inner));
            quote! { __ir::Expr::VectorToBytes { len: #len, expr: #inner } }
        }
        Expr::BytesToVector { len, expr: inner } => {
            let inner = bx(expr(inner));
            quote! { __ir::Expr::BytesToVector { len: #len, expr: #inner } }
        }
        Expr::CastFromEnum {
            ty,
            from,
            expr: inner,
        } => {
            let ty = type_ref(ty);
            let from = type_ref(from);
            let inner = bx(expr(inner));
            quote! { __ir::Expr::CastFromEnum { ty: #ty, from: #from, expr: #inner } }
        }
        Expr::CastToEnum {
            ty,
            from,
            expr: inner,
        } => {
            let ty = type_ref(ty);
            let from = type_ref(from);
            let inner = bx(expr(inner));
            quote! { __ir::Expr::CastToEnum { ty: #ty, from: #from, expr: #inner } }
        }
        Expr::CastToField {
            field_type: ft,
            from,
            expr: inner,
        } => {
            let ft = field_type(ft);
            let from = type_ref(from);
            let inner = bx(expr(inner));
            quote! { __ir::Expr::CastToField { field_type: #ft, from: #from, expr: #inner } }
        }
        Expr::CastFromField {
            maxval,
            field_type: ft,
            expr: inner,
        } => {
            let maxval = biguint(maxval);
            let ft = field_type(ft);
            let inner = bx(expr(inner));
            quote! { __ir::Expr::CastFromField { maxval: #maxval, field_type: #ft, expr: #inner } }
        }
        Expr::SafeCast {
            ty,
            from,
            expr: inner,
        } => {
            let ty = type_ref(ty);
            let from = type_ref(from);
            let inner = bx(expr(inner));
            quote! { __ir::Expr::SafeCast { ty: #ty, from: #from, expr: #inner } }
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
                __ir::Expr::DowncastUnsigned {
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
                __ir::Expr::ContractCall {
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
                __ir::Expr::Emit {
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
            op_class,
            path,
            op,
            result_type,
            instructions,
            args,
        } => {
            let field = ident(field);
            let op_class = class(op_class);
            let path = vec_of(path.iter().map(path_element));
            let op = s(op);
            let result_type = type_ref(result_type);
            let instructions = vec_of(instructions.iter().map(instruction));
            let args = vec_of(args.iter().map(expr));
            quote! {
                __ir::Expr::PublicLedger {
                    field: #field,
                    op_class: #op_class,
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
            quote! { __ir::Expr::Return(#inner) }
        }
    }
}
