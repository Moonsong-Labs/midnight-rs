//! Emit Rust constructor expressions for embedded IR values.
//!
//! Generated bindings carry each circuit's IR, the helper/struct/enum
//! registries, and the per-circuit type metadata as typed constructor
//! functions, so the compiler checks the embedding and nothing parses at
//! run time. Every emitter expects two aliases in scope at the splice site:
//! `__ir` for `midnight_contract::compact_codegen::ir` (the execution nodes)
//! and `__nir` for `midnight_contract::compact_codegen::nir` (the types).

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{
    CircuitIrBody, EnumDef, Expr, Fun, HelperDef, LedgerOp, Param, PathEntry, StateEntry,
    StateInit, Stmt, StructDef, VmFn, VmOperand,
};
use crate::nir::{ContractCircuit, Curve, FieldType, Type};

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

pub(crate) fn circuit_ir_body(b: &CircuitIrBody) -> TokenStream {
    let body = stmt(&b.body);
    quote! { __ir::CircuitIrBody { body: #body } }
}

pub(crate) fn helper_defs(helpers: &[HelperDef]) -> TokenStream {
    vec_of(helpers.iter().map(|h| {
        let name = s(&h.name);
        let params = vec_of(h.params.iter().map(param));
        let body = stmt(&h.body);
        quote! { __ir::HelperDef { name: #name, params: #params, body: #body } }
    }))
}

pub(crate) fn struct_defs(structs: &[StructDef]) -> TokenStream {
    vec_of(structs.iter().map(|d| {
        let name = s(&d.name);
        let fields = vec_of(d.fields.iter().map(struct_field));
        quote! { __ir::StructDef { name: #name, fields: #fields } }
    }))
}

pub(crate) fn enum_defs(enums: &[EnumDef]) -> TokenStream {
    vec_of(enums.iter().map(|d| {
        let name = s(&d.name);
        let variants = vec_of(d.variants.iter().map(|v| s(v)));
        quote! { __ir::EnumDef { name: #name, variants: #variants } }
    }))
}

pub(crate) fn arg_types(args: &[(String, Type)]) -> TokenStream {
    vec_of(args.iter().map(|(name, ty)| {
        let name = s(name);
        let ty = type_ref(ty);
        quote! { (#name, #ty) }
    }))
}

/// A bound reaches 2^248-1, so no integer literal holds it. The digits go
/// through `FromStr`, whose target type the `Unsigned` field infers.
/// `unwrap_or_default` keeps generated code panic-free; the digits come from
/// `BigUint::to_string`, which `FromStr` accepts.
fn biguint(v: &num_bigint::BigUint) -> TokenStream {
    let digits = v.to_string();
    quote! { #digits.parse().unwrap_or_default() }
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

fn param(p: &Param) -> TokenStream {
    let name = s(&p.name);
    let ty = type_ref(&p.ty);
    quote! { __ir::Param { name: #name, ty: #ty } }
}

fn stmt(st: &Stmt) -> TokenStream {
    match st {
        Stmt::Seq { stmts } => {
            let stmts = vec_of(stmts.iter().map(stmt));
            quote! { __ir::Stmt::Seq { stmts: #stmts } }
        }
        Stmt::Let { name, value } => {
            let name = s(name);
            let value = expr(value);
            quote! { __ir::Stmt::Let { name: #name, value: #value } }
        }
        Stmt::ExprStmt { expr: e } => {
            let e = expr(e);
            quote! { __ir::Stmt::ExprStmt { expr: #e } }
        }
    }
}

fn binop(variant: TokenStream, left: &Expr, right: &Expr) -> TokenStream {
    let left = bx(expr(left));
    let right = bx(expr(right));
    quote! { __ir::Expr::#variant { left: #left, right: #right } }
}

fn expr(e: &Expr) -> TokenStream {
    match e {
        Expr::Var { name } => {
            let name = s(name);
            quote! { __ir::Expr::Var { name: #name } }
        }
        Expr::Lit { ty, value } => {
            let ty = type_ref(ty);
            let value = s(value);
            quote! { __ir::Expr::Lit { ty: #ty, value: #value } }
        }
        Expr::Add { left, right } => binop(quote! { Add }, left, right),
        Expr::Sub { left, right } => binop(quote! { Sub }, left, right),
        Expr::Mul { left, right } => binop(quote! { Mul }, left, right),
        Expr::Eq { left, right } => binop(quote! { Eq }, left, right),
        Expr::Neq { left, right } => binop(quote! { Neq }, left, right),
        Expr::Lt { left, right } => binop(quote! { Lt }, left, right),
        Expr::Le { left, right } => binop(quote! { Le }, left, right),
        Expr::Gt { left, right } => binop(quote! { Gt }, left, right),
        Expr::Ge { left, right } => binop(quote! { Ge }, left, right),
        Expr::Field { expr: inner, name } => {
            let inner = bx(expr(inner));
            let name = s(name);
            quote! { __ir::Expr::Field { expr: #inner, name: #name } }
        }
        Expr::Index { expr: inner, index } => {
            let inner = bx(expr(inner));
            quote! { __ir::Expr::Index { expr: #inner, index: #index } }
        }
        Expr::EnumMember { ty, member } => {
            let ty = type_ref(ty);
            let member = s(member);
            quote! { __ir::Expr::EnumMember { ty: #ty, member: #member } }
        }
        Expr::BytesIndex { expr: inner, index } => {
            let inner = bx(expr(inner));
            let index = bx(expr(index));
            quote! { __ir::Expr::BytesIndex { expr: #inner, index: #index } }
        }
        Expr::TupleSlice {
            expr: inner,
            index,
            length,
            ty,
        } => {
            let inner = bx(expr(inner));
            let ty = type_ref(ty);
            quote! { __ir::Expr::TupleSlice { expr: #inner, index: #index, length: #length, ty: #ty } }
        }
        Expr::VectorSlice {
            expr: inner,
            index,
            length,
            ty,
        } => {
            let inner = bx(expr(inner));
            let index = bx(expr(index));
            let ty = type_ref(ty);
            quote! { __ir::Expr::VectorSlice { expr: #inner, index: #index, length: #length, ty: #ty } }
        }
        Expr::BytesSlice {
            expr: inner,
            index,
            length,
        } => {
            let inner = bx(expr(inner));
            let index = bx(expr(index));
            quote! { __ir::Expr::BytesSlice { expr: #inner, index: #index, length: #length } }
        }
        Expr::Map { length, fun, args } => {
            let fun = fun_tokens(fun);
            let args = vec_of(args.iter().map(expr));
            quote! { __ir::Expr::Map { length: #length, fun: #fun, args: #args } }
        }
        Expr::Fold {
            length,
            fun,
            init,
            args,
        } => {
            let fun = fun_tokens(fun);
            let init = bx(expr(init));
            let args = vec_of(args.iter().map(expr));
            quote! { __ir::Expr::Fold { length: #length, fun: #fun, init: #init, args: #args } }
        }
        Expr::VectorIndex { expr: inner, index } => {
            let inner = bx(expr(inner));
            let index = bx(expr(index));
            quote! { __ir::Expr::VectorIndex { expr: #inner, index: #index } }
        }
        Expr::IfExpr { cond, then, else_ } => {
            let cond = bx(expr(cond));
            let then = bx(expr(then));
            let else_ = bx(expr(else_));
            quote! { __ir::Expr::IfExpr { cond: #cond, then: #then, else_: #else_ } }
        }
        Expr::Assert {
            expr: inner,
            message,
        } => {
            let inner = bx(expr(inner));
            let message = s(message);
            quote! { __ir::Expr::Assert { expr: #inner, message: #message } }
        }
        Expr::LedgerQuery { ops, result_type } => {
            let ops = vec_of(ops.iter().map(ledger_op));
            let result_type = type_ref(result_type);
            quote! { __ir::Expr::LedgerQuery { ops: #ops, result_type: #result_type } }
        }
        Expr::CallWitness {
            name,
            args,
            result_type,
        } => {
            let name = s(name);
            let args = vec_of(args.iter().map(expr));
            let result_type = type_ref(result_type);
            quote! { __ir::Expr::CallWitness { name: #name, args: #args, result_type: #result_type } }
        }
        Expr::CallPure {
            name,
            args,
            result_type,
        } => {
            let name = s(name);
            let args = vec_of(args.iter().map(expr));
            let result_type = type_ref(result_type);
            quote! { __ir::Expr::CallPure { name: #name, args: #args, result_type: #result_type } }
        }
        Expr::LetExpr { bindings, body } => {
            let bindings = vec_of(bindings.iter().map(stmt));
            let body = bx(expr(body));
            quote! { __ir::Expr::LetExpr { bindings: #bindings, body: #body } }
        }
        Expr::New { ty, elements } => {
            let ty = type_ref(ty);
            let elements = vec_of(elements.iter().map(expr));
            quote! { __ir::Expr::New { ty: #ty, elements: #elements } }
        }
        Expr::Cast {
            expr: inner,
            from,
            to,
        } => {
            let inner = bx(expr(inner));
            let from = type_ref(from);
            let to = type_ref(to);
            quote! { __ir::Expr::Cast { expr: #inner, from: #from, to: #to } }
        }
        Expr::Default { ty } => {
            let ty = type_ref(ty);
            quote! { __ir::Expr::Default { ty: #ty } }
        }
        Expr::Tuple { elements } => {
            let elements = vec_of(elements.iter().map(expr));
            quote! { __ir::Expr::Tuple { elements: #elements } }
        }
        Expr::Spread {
            length,
            expr: inner,
        } => {
            let inner = bx(expr(inner));
            quote! { __ir::Expr::Spread { length: #length, expr: #inner } }
        }
        Expr::FieldToBytes {
            length,
            expr: inner,
        } => {
            let inner = bx(expr(inner));
            quote! { __ir::Expr::FieldToBytes { length: #length, expr: #inner } }
        }
        Expr::BytesToVector {
            length,
            expr: inner,
        } => {
            let inner = bx(expr(inner));
            quote! { __ir::Expr::BytesToVector { length: #length, expr: #inner } }
        }
        Expr::VectorToBytes {
            length,
            expr: inner,
        } => {
            let inner = bx(expr(inner));
            quote! { __ir::Expr::VectorToBytes { length: #length, expr: #inner } }
        }
        Expr::ContractCall {
            circuit,
            contract,
            contract_type,
            args,
        } => {
            let circuit = s(circuit);
            let contract = bx(expr(contract));
            let contract_type = type_ref(contract_type);
            let args = vec_of(args.iter().map(expr));
            quote! { __ir::Expr::ContractCall {
                circuit: #circuit,
                contract: #contract,
                contract_type: #contract_type,
                args: #args,
            } }
        }
    }
}

fn fun_tokens(f: &Fun) -> TokenStream {
    match f {
        Fun::Named { call } => {
            let call = s(call);
            quote! { __ir::Fun::Named { call: #call } }
        }
        Fun::Inline { params, body } => {
            let params = vec_of(params.iter().map(param));
            let body = bx(expr(body));
            quote! { __ir::Fun::Inline { params: #params, body: #body } }
        }
    }
}

fn ledger_op(op: &LedgerOp) -> TokenStream {
    match op {
        LedgerOp::Dup { n } => quote! { __ir::LedgerOp::Dup { n: #n } },
        LedgerOp::Idx {
            cached,
            push_path,
            path,
        } => {
            let path = vec_of(path.iter().map(path_entry));
            quote! { __ir::LedgerOp::Idx { cached: #cached, push_path: #push_path, path: #path } }
        }
        LedgerOp::Addi { immediate } => {
            let immediate = vm_operand(immediate);
            quote! { __ir::LedgerOp::Addi { immediate: #immediate } }
        }
        LedgerOp::Ins { cached, n } => quote! { __ir::LedgerOp::Ins { cached: #cached, n: #n } },
        LedgerOp::Push { storage, value } => {
            let value = vm_operand(value);
            quote! { __ir::LedgerOp::Push { storage: #storage, value: #value } }
        }
        LedgerOp::Popeq { cached } => quote! { __ir::LedgerOp::Popeq { cached: #cached } },
        LedgerOp::Member => quote! { __ir::LedgerOp::Member },
        LedgerOp::Rem { cached } => quote! { __ir::LedgerOp::Rem { cached: #cached } },
        LedgerOp::Root => quote! { __ir::LedgerOp::Root },
        LedgerOp::Eq => quote! { __ir::LedgerOp::Eq },
        LedgerOp::Noop { n } => quote! { __ir::LedgerOp::Noop { n: #n } },
        LedgerOp::Ckpt => quote! { __ir::LedgerOp::Ckpt },
        LedgerOp::Swap { n } => quote! { __ir::LedgerOp::Swap { n: #n } },
        LedgerOp::Neg => quote! { __ir::LedgerOp::Neg },
        LedgerOp::Branch { skip } => quote! { __ir::LedgerOp::Branch { skip: #skip } },
        LedgerOp::Add => quote! { __ir::LedgerOp::Add },
    }
}

fn path_entry(p: &PathEntry) -> TokenStream {
    match p {
        PathEntry::Value { value, ty } => {
            let value = s(value);
            let ty = type_ref(ty);
            quote! { __ir::PathEntry::Value { value: #value, ty: #ty } }
        }
        PathEntry::Var { name } => {
            let name = s(name);
            quote! { __ir::PathEntry::Var { name: #name } }
        }
        PathEntry::Expr { expr: inner } => {
            let inner = bx(expr(inner));
            quote! { __ir::PathEntry::Expr { expr: #inner } }
        }
        PathEntry::Stack => quote! { __ir::PathEntry::Stack },
    }
}

fn vm_operand(o: &VmOperand) -> TokenStream {
    match o {
        VmOperand::Int(n) => quote! { __ir::VmOperand::Int(#n) },
        VmOperand::Bool(b) => quote! { __ir::VmOperand::Bool(#b) },
        VmOperand::Str(v) => {
            let v = s(v);
            quote! { __ir::VmOperand::Str(#v) }
        }
        VmOperand::Null => quote! { __ir::VmOperand::Null },
        VmOperand::Key(p) => {
            let p = path_entry(p);
            quote! { __ir::VmOperand::Key(#p) }
        }
        VmOperand::State(st) => {
            let st = state_init(st);
            quote! { __ir::VmOperand::State(#st) }
        }
        VmOperand::Vm(f) => {
            let f = vm_fn(f);
            quote! { __ir::VmOperand::Vm(#f) }
        }
        VmOperand::Expr(e) => {
            let e = bx(expr(e));
            quote! { __ir::VmOperand::Expr(#e) }
        }
    }
}

fn state_entry(e: &StateEntry) -> TokenStream {
    let key = vm_operand(&e.key);
    let value = vm_operand(&e.value);
    quote! { __ir::StateEntry { key: #key, value: #value } }
}

fn state_init(st: &StateInit) -> TokenStream {
    match st {
        StateInit::Array { values } => {
            let values = vec_of(values.iter().map(vm_operand));
            quote! { __ir::StateInit::Array { values: #values } }
        }
        StateInit::Map { entries } => {
            let entries = vec_of(entries.iter().map(state_entry));
            quote! { __ir::StateInit::Map { entries: #entries } }
        }
        StateInit::MerkleTree { depth, entries } => {
            let entries = vec_of(entries.iter().map(state_entry));
            quote! { __ir::StateInit::MerkleTree { depth: #depth, entries: #entries } }
        }
    }
}

fn vm_fn(f: &VmFn) -> TokenStream {
    match f {
        VmFn::Null { value } => {
            let value = bx(vm_operand(value));
            quote! { __ir::VmFn::Null { value: #value } }
        }
        VmFn::MaxSizeof { value } => {
            let value = bx(vm_operand(value));
            quote! { __ir::VmFn::MaxSizeof { value: #value } }
        }
        VmFn::LeafHash { value } => {
            let value = bx(vm_operand(value));
            quote! { __ir::VmFn::LeafHash { value: #value } }
        }
        VmFn::CoinCommit { coin, recipient } => {
            let coin = bx(vm_operand(coin));
            let recipient = bx(vm_operand(recipient));
            quote! { __ir::VmFn::CoinCommit { coin: #coin, recipient: #recipient } }
        }
        VmFn::AlignedConcat { values } => {
            let values = vec_of(values.iter().map(vm_operand));
            quote! { __ir::VmFn::AlignedConcat { values: #values } }
        }
    }
}
