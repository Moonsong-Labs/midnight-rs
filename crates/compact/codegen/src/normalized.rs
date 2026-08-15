//! Load a `normalized-ir.sexp` artifact into [`ContractInfo`].
//!
//! The normalized artifact is the compiler's analyzed IR in its own
//! vocabulary. This module builds the crate's typed model (`ContractInfo`,
//! [`ir::Expr`](Expr), [`ir::TypeRef`](TypeRef)) from it directly, so the
//! interpreter and the code generator run on one representation with no
//! intermediate encoding. The conversion mirrors the retired JSON emitter's
//! mapping, and the conformance suite holds the result to the same goldens.
//!
//! Naming: the artifact prints identifiers as `%sym.uniq`. A binder whose
//! symbol is unique within its body keeps the bare symbol, so signatures and
//! generated bindings read as the source does; binders that share a symbol
//! (compiler temporaries, shadowed locals) are qualified as `sym.uniq` to
//! keep binding and reference sites in agreement.

use std::collections::HashMap;
use std::path::Path;

use compact_normalized_ir as nir;
use serde_json::{Number, Value, json};

use crate::ir::{
    CircuitIrBody, Expr, Fun, HelperDef, LedgerOp, Param, PathEntry, Stmt, StructField, TypeRef,
};
use crate::types::{Circuit, CircuitArgument, ContractInfo, FieldIndex, LedgerField, StorageKind};

/// A conversion failure: a construct the internal IR cannot represent yet
/// (events, foreign fields, curve points) or a malformed artifact.
#[derive(Debug)]
pub struct NormalizedError(String);

impl std::fmt::Display for NormalizedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "normalized-ir: {}", self.0)
    }
}

impl std::error::Error for NormalizedError {}

fn unsupported<T>(what: &str) -> Result<T, NormalizedError> {
    Err(NormalizedError(format!(
        "unsupported by the interpreter IR: {what}"
    )))
}

pub fn parse_normalized(path: &Path) -> Result<ContractInfo, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    contract_info_from_str(&content)
}

pub fn contract_info_from_str(text: &str) -> Result<ContractInfo, Box<dyn std::error::Error>> {
    let ir = nir::parse_str(text).map_err(|e| {
        // The reader's Display already carries the format name.
        NormalizedError(
            e.to_string()
                .trim_start_matches("normalized-ir: ")
                .to_string(),
        )
    })?;
    Ok(contract_info(&ir)?)
}

/// The whole artifact as the crate's typed model.
pub fn contract_info(ir: &nir::NormalizedIr) -> Result<ContractInfo, NormalizedError> {
    let cx = Context::new(ir);

    let mut circuits = Vec::new();
    for (export_name, id) in &ir.exports {
        let Some(c) = cx.circuits.get(id.0.as_str()) else {
            continue; // an exported ledger field
        };
        let renames = cx.body_renames(c);
        circuits.push(Circuit {
            name: export_name.clone(),
            pure: c.pure,
            proof: c.proof,
            arguments: cx.circuit_arguments(&c.arguments, &renames)?,
            result_type: cx.ty(&c.result_type)?,
            ir: Some(CircuitIrBody { body: cx.body(c)? }),
        });
    }

    let mut helpers = Vec::new();
    for c in ir.circuits() {
        let renames = cx.body_renames(c);
        helpers.push(HelperDef {
            name: cx.circuit_names[c.name.0.as_str()].clone(),
            params: cx.params(&c.arguments, &renames)?,
            body: cx.body(c)?,
        });
    }

    let mut witnesses = Vec::new();
    for w in ir.elements.iter().filter_map(|e| match e {
        nir::ProgramElement::Witness(w) => Some(w),
        _ => None,
    }) {
        let renames = HashMap::new(); // witness signatures bind no locals
        witnesses.push(crate::types::Witness {
            name: w.name.name().to_string(),
            arguments: cx.circuit_arguments(&w.arguments, &renames)?,
            result_type: cx.ty(&w.result_type)?,
        });
    }

    let ledger = match ir.ledger() {
        Some(decl) => decl
            .fields
            .iter()
            .map(|f| cx.ledger_field(f))
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };

    let contracts: Vec<String> = ir
        .contract_types
        .iter()
        .filter_map(|t| match t {
            nir::Type::Contract { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();

    Ok(ContractInfo {
        compiler_version: ir.compiler_version.clone(),
        language_version: ir.language_version.clone(),
        runtime_version: ir.runtime_version.clone(),
        circuits,
        witnesses,
        contracts,
        ledger,
        helpers,
    })
}

/// What a `call` name resolves to.
enum Callee<'a> {
    Circuit(&'a nir::Circuit),
    NativeCircuit(&'a nir::Native),
    NativeWitness(&'a nir::Native),
    Witness(&'a nir::Witness),
}

struct Context<'a> {
    /// Full id text -> the circuit definition.
    circuits: HashMap<&'a str, &'a nir::Circuit>,
    /// Full id text -> the emitted circuit/helper name. Declaration order,
    /// with instantiations of one generic numbered apart.
    circuit_names: HashMap<&'a str, String>,
    /// Bare symbol -> native / witness declaration.
    natives: HashMap<String, &'a nir::Native>,
    witnesses: HashMap<String, &'a nir::Witness>,
}

impl<'a> Context<'a> {
    fn new(ir: &'a nir::NormalizedIr) -> Self {
        let mut circuits = HashMap::new();
        let mut circuit_names = HashMap::new();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let mut natives = HashMap::new();
        let mut witnesses = HashMap::new();
        for e in &ir.elements {
            match e {
                nir::ProgramElement::Circuit(c) => {
                    circuits.insert(c.name.0.as_str(), c);
                    let base = c.name.name();
                    let n = counts.entry(base).or_insert(0);
                    let emitted = if *n == 0 {
                        base.to_string()
                    } else {
                        format!("{base}_{n}")
                    };
                    *n += 1;
                    circuit_names.insert(c.name.0.as_str(), emitted);
                }
                nir::ProgramElement::Native(nat) => {
                    natives.insert(nat.name.name().to_string(), nat);
                }
                nir::ProgramElement::Witness(w) => {
                    witnesses.insert(w.name.name().to_string(), w);
                }
                _ => {}
            }
        }
        Context {
            circuits,
            circuit_names,
            natives,
            witnesses,
        }
    }

    fn callee(&self, id: &nir::Ident) -> Result<Callee<'a>, NormalizedError> {
        if let Some(c) = self.circuits.get(id.0.as_str()) {
            return Ok(Callee::Circuit(c));
        }
        if let Some(n) = self.natives.get(id.name()) {
            return Ok(if n.class == "witness" {
                Callee::NativeWitness(n)
            } else {
                Callee::NativeCircuit(n)
            });
        }
        if let Some(w) = self.witnesses.get(id.name()) {
            return Ok(Callee::Witness(w));
        }
        unsupported(&format!("unknown callee {}", id.0))
    }

    // -- naming ---------------------------------------------------------

    /// Binder renames for one circuit body: bare symbol when unique,
    /// `sym.uniq` when several binders share it.
    fn body_renames(&self, c: &nir::Circuit) -> HashMap<String, String> {
        let mut binders: Vec<&nir::Ident> = c.arguments.iter().map(|a| &a.name).collect();
        collect_binders(&c.body, &mut binders);
        let mut by_base: HashMap<&str, usize> = HashMap::new();
        for b in &binders {
            *by_base.entry(b.name()).or_insert(0) += 1;
        }
        binders
            .into_iter()
            .map(|b| {
                let base = b.name();
                let emitted = if by_base[base] == 1 {
                    base.to_string()
                } else {
                    format!("{}.{}", base, uniq_suffix(&b.0))
                };
                (b.0.clone(), emitted)
            })
            .collect()
    }

    fn var(&self, id: &nir::Ident, renames: &HashMap<String, String>) -> String {
        renames
            .get(&id.0)
            .cloned()
            .unwrap_or_else(|| id.name().to_string())
    }

    fn circuit_arguments(
        &self,
        args: &[nir::Argument],
        renames: &HashMap<String, String>,
    ) -> Result<Vec<CircuitArgument>, NormalizedError> {
        args.iter()
            .map(|a| {
                Ok(CircuitArgument {
                    name: self.var(&a.name, renames),
                    ty: self.ty(&a.ty)?,
                })
            })
            .collect()
    }

    fn params(
        &self,
        args: &[nir::Argument],
        renames: &HashMap<String, String>,
    ) -> Result<Vec<Param>, NormalizedError> {
        args.iter()
            .map(|a| {
                Ok(Param {
                    name: self.var(&a.name, renames),
                    ty: self.ty(&a.ty)?,
                })
            })
            .collect()
    }

    // -- types ----------------------------------------------------------

    fn ty(&self, t: &nir::Type) -> Result<TypeRef, NormalizedError> {
        use nir::Type::*;
        Ok(match t {
            Boolean => TypeRef::Boolean,
            Field(nir::FieldType::Native) => TypeRef::Field,
            // The interpreter computes Jubjub scalars in the native field,
            // as artifacts did before the compiler distinguished them.
            Field(nir::FieldType::Scalar(nir::Curve::Jubjub)) => TypeRef::Field,
            Field(_) => return unsupported("secp256k1 field type"),
            // The runtime's EC values ride the wire spelling artifacts used
            // before the compiler made the point type native.
            Point(nir::Curve::Jubjub) => TypeRef::Opaque {
                name: "JubjubPoint".to_string(),
            },
            Point(nir::Curve::Secp256k1) => return unsupported("a secp256k1 point type"),
            Unsigned(maxval) => TypeRef::Uint {
                maxval: maxval.to_string(),
            },
            Bytes(len) => TypeRef::Bytes {
                length: as_usize(*len, "a Bytes length")?,
            },
            Opaque(ts) => TypeRef::Opaque { name: ts.clone() },
            Vector { len, ty } => TypeRef::Vector {
                length: as_usize(*len, "a Vector length")?,
                element: Box::new(self.ty(ty)?),
            },
            Tuple(types) => TypeRef::Tuple {
                types: types
                    .iter()
                    .map(|t| self.ty(t))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Struct { name, fields } => TypeRef::Struct {
                name: name.clone(),
                elements: fields
                    .iter()
                    .map(|(n, t)| {
                        Ok(StructField {
                            name: n.clone(),
                            ty: self.ty(t)?,
                        })
                    })
                    .collect::<Result<Vec<_>, NormalizedError>>()?,
            },
            Enum { name, variants } => TypeRef::Enum {
                name: name.clone(),
                variants: variants.clone(),
            },
            Alias { nominal, name, ty } => {
                if *nominal {
                    TypeRef::Alias {
                        name: name.clone(),
                        inner: Box::new(self.ty(ty)?),
                    }
                } else {
                    self.ty(ty)?
                }
            }
            Contract { name, .. } => TypeRef::Contract {
                name: Some(name.clone()),
            },
            // The element type of an empty vector; no value of it exists,
            // so it maps to the unit type.
            Unknown => TypeRef::Tuple { types: Vec::new() },
            Adt { name, .. } => {
                return unsupported(&format!("ADT type {name} in a value position"));
            }
            TypeVar(v) => return unsupported(&format!("type variable {v}")),
        })
    }

    fn ledger_field(&self, f: &nir::LedgerBinding) -> Result<LedgerField, NormalizedError> {
        let index = if f.path.len() == 1 {
            FieldIndex::Single(as_usize(f.path[0], "a ledger index")?)
        } else {
            FieldIndex::Path(
                f.path
                    .iter()
                    .map(|i| as_usize(*i, "a ledger index"))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        };
        let nir::Type::Adt { name, args } = strip_alias(&f.ty) else {
            return unsupported("a ledger field whose type is not a storage kind");
        };
        let kind = name.strip_prefix("__compact_").unwrap_or(name);
        let arg_ty = |i: usize| -> Result<TypeRef, NormalizedError> {
            match &args[i] {
                nir::AdtArg::Type(t) => self.ty(t),
                nir::AdtArg::Nat(n) => unsupported(&format!("a numeric type argument {n}")),
            }
        };
        let arg_nat = |i: usize| -> Result<u64, NormalizedError> {
            match &args[i] {
                nir::AdtArg::Nat(n) => Ok(*n),
                nir::AdtArg::Type(_) => unsupported("a type where a depth was expected"),
            }
        };
        let mut field = LedgerField {
            name: f.name.name().to_string(),
            index,
            storage: match kind {
                "Cell" => StorageKind::Cell,
                "Counter" => StorageKind::Counter,
                "Map" => StorageKind::Map,
                "Set" => StorageKind::Set,
                "List" => StorageKind::List,
                "MerkleTree" => StorageKind::MerkleTree,
                "HistoricMerkleTree" => StorageKind::HistoricMerkleTree,
                other => return unsupported(&format!("ledger storage kind {other}")),
            },
            exported: f.exported,
            element_type: None,
            key: None,
            value: None,
            depth: None,
        };
        match field.storage {
            StorageKind::Cell | StorageKind::Set | StorageKind::List => {
                field.element_type = Some(arg_ty(0)?);
            }
            StorageKind::Counter => {}
            StorageKind::Map => {
                field.key = Some(arg_ty(0)?);
                field.value = Some(arg_ty(1)?);
            }
            StorageKind::MerkleTree | StorageKind::HistoricMerkleTree => {
                field.depth = Some(arg_nat(0)?);
                field.element_type = Some(arg_ty(1)?);
            }
        }
        Ok(field)
    }

    // -- bodies ---------------------------------------------------------

    /// A circuit body: a `seq` of expression statements whose last value is
    /// the return value.
    fn body(&self, c: &nir::Circuit) -> Result<Stmt, NormalizedError> {
        let renames = self.body_renames(c);
        let stmts: Vec<&nir::Expr> = match &c.body {
            nir::Expr::Seq(items) => items.iter().collect(),
            other => vec![other],
        };
        Ok(Stmt::Seq {
            stmts: stmts
                .iter()
                .map(|e| {
                    Ok(Stmt::ExprStmt {
                        expr: self.expr(e, &renames)?,
                    })
                })
                .collect::<Result<Vec<_>, NormalizedError>>()?,
        })
    }

    fn expr(
        &self,
        e: &nir::Expr,
        renames: &HashMap<String, String>,
    ) -> Result<Expr, NormalizedError> {
        use nir::Expr::*;
        let sub = |e: &nir::Expr| -> Result<Box<Expr>, NormalizedError> {
            Ok(Box::new(self.expr(e, renames)?))
        };
        let subs = |es: &[nir::Expr]| -> Result<Vec<Expr>, NormalizedError> {
            es.iter().map(|e| self.expr(e, renames)).collect()
        };
        Ok(match e {
            Quote(lit) => match lit {
                nir::Literal::Bool(b) => Expr::Lit {
                    ty: TypeRef::Boolean,
                    value: if *b { "true" } else { "false" }.to_string(),
                },
                nir::Literal::Int(i) => Expr::Lit {
                    ty: TypeRef::Field,
                    value: i.to_string(),
                },
                nir::Literal::Bytes(b) => Expr::Lit {
                    ty: TypeRef::Bytes { length: b.len() },
                    value: b.iter().map(|x| format!("{x:02X}")).collect::<String>(),
                },
            },
            VarRef(id) => Expr::Var {
                name: self.var(id, renames),
            },
            Default(t) => Expr::Default { ty: self.ty(t)? },
            If { cond, then, els } => Expr::IfExpr {
                cond: sub(cond)?,
                then: sub(then)?,
                else_: sub(els)?,
            },
            EltRef { expr, elt, .. } => Expr::Field {
                expr: sub(expr)?,
                name: elt.clone(),
            },
            EnumRef { ty, elt } => Expr::EnumMember {
                ty: self.ty(ty)?,
                member: elt.clone(),
            },
            Tuple(args) | VectorLit(args) => Expr::Tuple {
                elements: args
                    .iter()
                    .map(|a| match a {
                        nir::TupleArg::Single(e) => self.expr(e, renames),
                        nir::TupleArg::Spread { len, expr } => Ok(Expr::Spread {
                            length: *len,
                            expr: sub(expr)?,
                        }),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            },
            TupleRef { expr, index } => Expr::Index {
                expr: sub(expr)?,
                index: as_usize(*index, "a tuple index")?,
            },
            TupleSlice {
                ty,
                expr,
                index,
                len,
            } => Expr::TupleSlice {
                expr: sub(expr)?,
                index: as_usize(*index, "a tuple-slice index")?,
                length: as_usize(*len, "a tuple-slice length")?,
                ty: self.ty(ty)?,
            },
            VectorRef { expr, index, .. } => Expr::VectorIndex {
                expr: sub(expr)?,
                index: sub(index)?,
            },
            VectorSlice {
                ty,
                expr,
                index,
                len,
            } => Expr::VectorSlice {
                expr: sub(expr)?,
                index: sub(index)?,
                length: as_usize(*len, "a vector-slice length")?,
                ty: self.ty(ty)?,
            },
            BytesRef { expr, index, .. } => Expr::BytesIndex {
                expr: sub(expr)?,
                index: sub(index)?,
            },
            BytesSlice {
                expr, index, len, ..
            } => Expr::BytesSlice {
                expr: sub(expr)?,
                index: sub(index)?,
                length: as_usize(*len, "a bytes-slice length")?,
            },
            Add { left, right, .. } => Expr::Add {
                left: sub(left)?,
                right: sub(right)?,
            },
            Sub { left, right, .. } => Expr::Sub {
                left: sub(left)?,
                right: sub(right)?,
            },
            Mul { left, right, .. } => Expr::Mul {
                left: sub(left)?,
                right: sub(right)?,
            },
            Eq { left, right, .. } => Expr::Eq {
                left: sub(left)?,
                right: sub(right)?,
            },
            Neq { left, right, .. } => Expr::Neq {
                left: sub(left)?,
                right: sub(right)?,
            },
            Lt { left, right, .. } => Expr::Lt {
                left: sub(left)?,
                right: sub(right)?,
            },
            Le { left, right, .. } => Expr::Le {
                left: sub(left)?,
                right: sub(right)?,
            },
            Gt { left, right, .. } => Expr::Gt {
                left: sub(left)?,
                right: sub(right)?,
            },
            Ge { left, right, .. } => Expr::Ge {
                left: sub(left)?,
                right: sub(right)?,
            },
            Map { len, fun, args } => Expr::Map {
                length: as_usize(*len, "a map length")?,
                fun: self.fun(fun, renames)?,
                args: args
                    .iter()
                    .map(|a| self.expr(&a.expr, renames))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Fold {
                len,
                fun,
                init,
                args,
                ..
            } => Expr::Fold {
                length: as_usize(*len, "a fold length")?,
                fun: self.fun(fun, renames)?,
                init: sub(init)?,
                args: args
                    .iter()
                    .map(|a| self.expr(&a.expr, renames))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Call { name, args } => {
                let args = subs(args)?;
                match self.callee(name)? {
                    Callee::Circuit(c) => Expr::CallPure {
                        name: self.circuit_names[c.name.0.as_str()].clone(),
                        args,
                        result_type: self.ty(&c.result_type)?,
                    },
                    Callee::NativeCircuit(n) => Expr::CallPure {
                        name: n.name.name().to_string(),
                        args,
                        result_type: self.ty(&n.result_type)?,
                    },
                    Callee::NativeWitness(n) => Expr::CallWitness {
                        name: n.name.name().to_string(),
                        args,
                        result_type: self.ty(&n.result_type)?,
                    },
                    Callee::Witness(w) => Expr::CallWitness {
                        name: w.name.name().to_string(),
                        args,
                        result_type: self.ty(&w.result_type)?,
                    },
                }
            }
            New { ty, elements } => Expr::New {
                ty: self.ty(ty)?,
                elements: subs(elements)?,
            },
            Seq(items) => {
                // A nested sequence: bind the discarded values so evaluation
                // order stays.
                let (last, init) = items
                    .split_last()
                    .ok_or_else(|| NormalizedError("empty seq".into()))?;
                let mut out = self.expr(last, renames)?;
                for (n, item) in init.iter().enumerate().rev() {
                    out = Expr::LetExpr {
                        bindings: vec![Stmt::Let {
                            name: format!("__seq_{n}"),
                            value: self.expr(item, renames)?,
                        }],
                        body: Box::new(out),
                    };
                }
                out
            }
            LetStar { bindings, body } => Expr::LetExpr {
                bindings: bindings
                    .iter()
                    .map(|(arg, value)| {
                        Ok(Stmt::Let {
                            name: self.var(&arg.name, renames),
                            value: self.expr(value, renames)?,
                        })
                    })
                    .collect::<Result<Vec<_>, NormalizedError>>()?,
                body: sub(body)?,
            },
            Assert { expr, message } => Expr::Assert {
                expr: sub(expr)?,
                message: message.clone(),
            },
            FieldToBytes {
                len,
                field_type,
                expr,
            } => {
                if !matches!(
                    field_type,
                    nir::FieldType::Native | nir::FieldType::Scalar(nir::Curve::Jubjub)
                ) {
                    return unsupported("field-to-bytes on a secp256k1 field");
                }
                Expr::FieldToBytes {
                    length: *len,
                    expr: sub(expr)?,
                }
            }
            CastFromBytes { ty, len, expr } => Expr::Cast {
                expr: sub(expr)?,
                from: TypeRef::Bytes {
                    length: as_usize(*len, "a Bytes length")?,
                },
                to: self.ty(ty)?,
            },
            VectorToBytes { len, expr } => Expr::VectorToBytes {
                length: *len,
                expr: sub(expr)?,
            },
            BytesToVector { len, expr } => Expr::BytesToVector {
                length: *len,
                expr: sub(expr)?,
            },
            CastFromEnum { ty, from, expr } | CastToEnum { ty, from, expr } => Expr::Cast {
                expr: sub(expr)?,
                from: self.ty(from)?,
                to: self.ty(ty)?,
            },
            CastToField {
                field_type,
                from,
                expr,
            } => {
                if !matches!(
                    field_type,
                    nir::FieldType::Native | nir::FieldType::Scalar(nir::Curve::Jubjub)
                ) {
                    return unsupported("a cast to a secp256k1 field");
                }
                Expr::Cast {
                    expr: sub(expr)?,
                    from: self.ty(from)?,
                    to: TypeRef::Field,
                }
            }
            CastFromField {
                maxval,
                field_type,
                expr,
            } => {
                if !matches!(
                    field_type,
                    nir::FieldType::Native | nir::FieldType::Scalar(nir::Curve::Jubjub)
                ) {
                    return unsupported("a cast from a secp256k1 field");
                }
                Expr::Cast {
                    expr: sub(expr)?,
                    from: TypeRef::Field,
                    to: TypeRef::Uint {
                        maxval: maxval.to_string(),
                    },
                }
            }
            SafeCast { ty, from, expr } => Expr::Cast {
                expr: sub(expr)?,
                from: self.ty(from)?,
                to: self.ty(ty)?,
            },
            DowncastUnsigned {
                from_maxval,
                to_maxval,
                expr,
            } => Expr::Cast {
                expr: sub(expr)?,
                from: TypeRef::Uint {
                    maxval: from_maxval.to_string(),
                },
                to: TypeRef::Uint {
                    maxval: to_maxval.to_string(),
                },
            },
            ContractCall {
                circuit,
                receiver,
                contract_type,
                args,
            } => Expr::ContractCall {
                circuit: circuit.clone(),
                contract: sub(receiver)?,
                contract_type: self.ty(contract_type)?,
                args: subs(args)?,
            },
            Emit { .. } => return unsupported("events (emit)"),
            PublicLedger {
                result_type,
                instructions,
                ..
            } => Expr::LedgerQuery {
                ops: instructions
                    .iter()
                    .map(|i| self.instruction(i, renames))
                    .collect::<Result<Vec<_>, _>>()?,
                result_type: self.ty(result_type)?,
            },
            Return(inner) => self.expr(inner, renames)?,
        })
    }

    fn fun(&self, f: &nir::Fun, renames: &HashMap<String, String>) -> Result<Fun, NormalizedError> {
        Ok(match f {
            nir::Fun::Ref(id) => Fun::Named {
                call: match self.callee(id)? {
                    Callee::Circuit(c) => self.circuit_names[c.name.0.as_str()].clone(),
                    _ => id.name().to_string(),
                },
            },
            nir::Fun::Circuit {
                arguments, body, ..
            } => {
                // The inline function's parameters shadow the enclosing
                // bindings; qualify any that collide with an outer name.
                let mut renames = renames.clone();
                for a in arguments {
                    let base = a.name.name();
                    let emitted = if renames.values().any(|v| v == base) {
                        format!("{}.{}", base, uniq_suffix(&a.name.0))
                    } else {
                        base.to_string()
                    };
                    renames.insert(a.name.0.clone(), emitted);
                }
                Fun::Inline {
                    params: self.params(arguments, &renames)?,
                    body: Box::new(self.expr(body, &renames)?),
                }
            }
        })
    }

    // -- ledger instructions --------------------------------------------

    fn instruction(
        &self,
        i: &nir::Instruction,
        renames: &HashMap<String, String>,
    ) -> Result<LedgerOp, NormalizedError> {
        let arg = |name: &str| i.arg(name);
        let flag = |name: &str| matches!(arg(name), Some(nir::Operand::Bool(true)));
        let int = |name: &str| -> Result<u64, NormalizedError> {
            match arg(name) {
                Some(nir::Operand::Int(n)) => u64::try_from(n)
                    .map_err(|_| NormalizedError(format!("{}: {name} out of range", i.op))),
                _ => Err(NormalizedError(format!("{}: missing integer {name}", i.op))),
            }
        };
        let small = |name: &str| -> Result<u8, NormalizedError> {
            u8::try_from(int(name)?)
                .map_err(|_| NormalizedError(format!("{}: {name} out of u8 range", i.op)))
        };
        let wide = |name: &str| -> Result<u32, NormalizedError> {
            u32::try_from(int(name)?)
                .map_err(|_| NormalizedError(format!("{}: {name} out of u32 range", i.op)))
        };
        Ok(match i.op.as_str() {
            "idx" => {
                let Some(nir::Operand::List(path)) = arg("path") else {
                    return unsupported("idx without a path list");
                };
                LedgerOp::Idx {
                    cached: flag("cached"),
                    push_path: flag("pushPath"),
                    path: path
                        .iter()
                        .map(|p| self.path_entry(p, renames))
                        .collect::<Result<Vec<_>, _>>()?,
                }
            }
            "push" => LedgerOp::Push {
                storage: flag("storage"),
                value: self.operand(
                    arg("value").ok_or_else(|| NormalizedError("push without value".into()))?,
                    renames,
                )?,
            },
            "addi" => LedgerOp::Addi {
                immediate: self.operand(
                    arg("immediate")
                        .ok_or_else(|| NormalizedError("addi without immediate".into()))?,
                    renames,
                )?,
            },
            "ins" => LedgerOp::Ins {
                cached: flag("cached"),
                n: small("n")?,
            },
            "dup" => LedgerOp::Dup {
                n: small("n").unwrap_or(0),
            },
            "swap" => LedgerOp::Swap {
                n: small("n").unwrap_or(0),
            },
            "popeq" => LedgerOp::Popeq {
                cached: flag("cached"),
            },
            "rem" => LedgerOp::Rem {
                cached: flag("cached"),
            },
            "noop" => LedgerOp::Noop { n: wide("n")? },
            "branch" => LedgerOp::Branch {
                skip: wide("skip")?,
            },
            "member" => LedgerOp::Member,
            "root" => LedgerOp::Root,
            "eq" => LedgerOp::Eq,
            "ckpt" => LedgerOp::Ckpt,
            "neg" => LedgerOp::Neg,
            "add" => LedgerOp::Add,
            other => return unsupported(&format!("VM instruction {other}")),
        })
    }

    fn path_entry(
        &self,
        p: &nir::Operand,
        renames: &HashMap<String, String>,
    ) -> Result<PathEntry, NormalizedError> {
        Ok(match p {
            nir::Operand::Align { value, bytes } => PathEntry::Value {
                value: value.to_string(),
                ty: TypeRef::Uint {
                    maxval: max_for_bytes(*bytes),
                },
            },
            nir::Operand::Stack => PathEntry::Stack,
            nir::Operand::Expr(e) => match e.as_ref() {
                nir::Expr::VarRef(id) => PathEntry::Var {
                    name: self.var(id, renames),
                },
                other => PathEntry::Expr {
                    expr: Box::new(self.expr(other, renames)?),
                },
            },
            other => return unsupported(&format!("path element {other:?}")),
        })
    }

    /// A `push` / `addi` operand in the internal encoding: a path element
    /// (`tag`), an expression (`op`), a structured state value (`state`), or
    /// a computed value (`vm`).
    fn operand(
        &self,
        o: &nir::Operand,
        renames: &HashMap<String, String>,
    ) -> Result<Value, NormalizedError> {
        use nir::Operand::*;
        let expr_value = |e: &nir::Expr| -> Result<Value, NormalizedError> {
            serde_json::to_value(self.expr(e, renames)?)
                .map_err(|e| NormalizedError(format!("serialize an operand expression: {e}")))
        };
        Ok(match o {
            Int(n) => Value::Number(Number::from_string_unchecked(n.to_string())),
            Bool(b) => json!(b),
            Str(s) => json!(s),
            Align { value, bytes } => json!({
                "tag": "value",
                "value": value.to_string(),
                "type": { "type-name": "Uint", "maxval": max_for_bytes(*bytes) },
            }),
            Stack => json!({ "tag": "stack" }),
            Void => Value::Null,
            // The count is the integer inside; the wrapper carries no width.
            ValueToInt(inner) => self.operand(inner, renames)?,
            // A cell is represented by its contents alone.
            StateValue(nir::StateValue::Cell(inner) | nir::StateValue::Adt(inner)) => {
                self.operand(inner, renames)?
            }
            StateValue(nir::StateValue::Null) => Value::Null,
            StateValue(nir::StateValue::Array(items)) => json!({
                "state": "array",
                "values": items
                    .iter()
                    .map(|v| self.operand(v, renames))
                    .collect::<Result<Vec<_>, NormalizedError>>()?,
            }),
            StateValue(nir::StateValue::Map(entries)) => json!({
                "state": "map",
                "entries": entries
                    .iter()
                    .map(|(k, v)| {
                        Ok(json!({
                            "key": self.operand(k, renames)?,
                            "value": self.operand(v, renames)?,
                        }))
                    })
                    .collect::<Result<Vec<_>, NormalizedError>>()?,
            }),
            StateValue(nir::StateValue::MerkleTree { depth, entries }) => json!({
                "state": "merkle-tree",
                "depth": depth,
                "entries": entries
                    .iter()
                    .map(|(k, v)| {
                        Ok(json!({
                            "key": self.operand(k, renames)?,
                            "value": self.operand(v, renames)?,
                        }))
                    })
                    .collect::<Result<Vec<_>, NormalizedError>>()?,
            }),
            Null(v) => json!({ "vm": "null", "value": self.operand(v, renames)? }),
            MaxSizeof(v) => json!({ "vm": "max-sizeof", "value": self.operand(v, renames)? }),
            LeafHash(v) => json!({ "vm": "leaf-hash", "value": self.operand(v, renames)? }),
            CoinCommit(coin, recipient) => json!({
                "vm": "coin-commit",
                "coin": self.operand(coin, renames)?,
                "recipient": self.operand(recipient, renames)?,
            }),
            AlignedConcat(items) => json!({
                "vm": "aligned-concat",
                "values": items
                    .iter()
                    .map(|v| self.operand(v, renames))
                    .collect::<Result<Vec<_>, NormalizedError>>()?,
            }),
            Expr(e) => expr_value(e)?,
            List(_) => return unsupported("a bare operand list outside idx"),
        })
    }
}

// -- free helpers -------------------------------------------------------

fn strip_alias(t: &nir::Type) -> &nir::Type {
    match t {
        nir::Type::Alias { ty, .. } => strip_alias(ty),
        other => other,
    }
}

fn uniq_suffix(full: &str) -> &str {
    full.rsplit('.').next().unwrap_or(full)
}

fn max_for_bytes(bytes: u64) -> String {
    let max = (num_bigint::BigUint::from(1u8) << (8 * bytes)) - 1u8;
    max.to_string()
}

fn as_usize<T: Copy + TryInto<usize> + std::fmt::Display>(
    n: T,
    what: &str,
) -> Result<usize, NormalizedError> {
    n.try_into()
        .map_err(|_| NormalizedError(format!("{what} out of range: {n}")))
}

/// Every binder in an expression tree: let bindings and inline-function
/// parameters (circuit arguments are collected by the caller).
fn collect_binders<'a>(e: &'a nir::Expr, out: &mut Vec<&'a nir::Ident>) {
    use nir::Expr::*;
    let mut go = |x: &'a nir::Expr| collect_binders(x, out);
    match e {
        LetStar { bindings, body } => {
            for (arg, value) in bindings {
                out.push(&arg.name);
                collect_binders(value, out);
            }
            collect_binders(body, out);
        }
        Map { fun, args, .. } => {
            if let nir::Fun::Circuit {
                arguments, body, ..
            } = fun
            {
                for a in arguments {
                    out.push(&a.name);
                }
                collect_binders(body, out);
            }
            for a in args {
                collect_binders(&a.expr, out);
            }
        }
        Fold {
            fun, init, args, ..
        } => {
            if let nir::Fun::Circuit {
                arguments, body, ..
            } = fun
            {
                for a in arguments {
                    out.push(&a.name);
                }
                collect_binders(body, out);
            }
            collect_binders(init, out);
            for a in args {
                collect_binders(&a.expr, out);
            }
        }
        Quote(_) | VarRef(_) | Default(_) | EnumRef { .. } => {}
        If { cond, then, els } => {
            go(cond);
            go(then);
            go(els);
        }
        EltRef { expr, .. }
        | TupleRef { expr, .. }
        | TupleSlice { expr, .. }
        | VectorToBytes { expr, .. }
        | BytesToVector { expr, .. }
        | FieldToBytes { expr, .. }
        | CastFromBytes { expr, .. }
        | CastFromEnum { expr, .. }
        | CastToEnum { expr, .. }
        | CastToField { expr, .. }
        | CastFromField { expr, .. }
        | SafeCast { expr, .. }
        | DowncastUnsigned { expr, .. }
        | Assert { expr, .. }
        | Return(expr) => go(expr),
        VectorRef { expr, index, .. }
        | VectorSlice { expr, index, .. }
        | BytesRef { expr, index, .. }
        | BytesSlice { expr, index, .. } => {
            go(expr);
            go(index);
        }
        Add { left, right, .. }
        | Sub { left, right, .. }
        | Mul { left, right, .. }
        | Lt { left, right, .. }
        | Le { left, right, .. }
        | Gt { left, right, .. }
        | Ge { left, right, .. }
        | Eq { left, right, .. }
        | Neq { left, right, .. } => {
            go(left);
            go(right);
        }
        Tuple(args) | VectorLit(args) => {
            for a in args {
                match a {
                    nir::TupleArg::Single(x) | nir::TupleArg::Spread { expr: x, .. } => go(x),
                }
            }
        }
        Call { args, .. } | New { elements: args, .. } => {
            for a in args {
                go(a);
            }
        }
        Seq(items) => {
            for i in items {
                go(i);
            }
        }
        ContractCall { receiver, args, .. } => {
            go(receiver);
            for a in args {
                go(a);
            }
        }
        Emit { payload, .. } => go(payload),
        PublicLedger {
            path,
            args,
            instructions,
            ..
        } => {
            for p in path {
                if let nir::PathElement::Computed { expr, .. } = p {
                    go(expr);
                }
            }
            for a in args {
                go(a);
            }
            for i in instructions {
                for (_, op) in &i.args {
                    collect_operand_binders(op, out);
                }
            }
        }
    }
}

fn collect_operand_binders<'a>(o: &'a nir::Operand, out: &mut Vec<&'a nir::Ident>) {
    use nir::Operand::*;
    match o {
        Expr(e) => collect_binders(e, out),
        ValueToInt(x) | Null(x) | MaxSizeof(x) | LeafHash(x) => collect_operand_binders(x, out),
        CoinCommit(a, b) => {
            collect_operand_binders(a, out);
            collect_operand_binders(b, out);
        }
        AlignedConcat(xs) | List(xs) => {
            for x in xs {
                collect_operand_binders(x, out);
            }
        }
        StateValue(sv) => match sv {
            nir::StateValue::Cell(x) | nir::StateValue::Adt(x) => collect_operand_binders(x, out),
            nir::StateValue::Array(xs) => {
                for x in xs {
                    collect_operand_binders(x, out);
                }
            }
            nir::StateValue::Map(entries) | nir::StateValue::MerkleTree { entries, .. } => {
                for (k, v) in entries {
                    collect_operand_binders(k, out);
                    collect_operand_binders(v, out);
                }
            }
            nir::StateValue::Null => {}
        },
        Int(_) | Bool(_) | Str(_) | Align { .. } | Stack | Void => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> ContractInfo {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        parse_normalized(&path).expect("normalized artifact loads")
    }

    #[test]
    fn loads_the_bboard_artifact() {
        let info =
            fixture("../../../tests/conformance/fixtures/bboard/compiler/normalized-ir.sexp");
        assert!(info.circuits.iter().all(|c| c.ir.is_some()));
        let names: Vec<_> = info.circuits.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"post") && names.contains(&"public_key"));
        let message = info
            .ledger
            .iter()
            .find(|f| f.name == "message")
            .expect("message");
        let Some(TypeRef::Struct { elements, .. }) = &message.element_type else {
            panic!("message should be a struct-typed cell")
        };
        assert_eq!(elements.len(), 2);
        // public_key is exported and called, so it is also a helper.
        assert!(info.helpers.iter().any(|h| h.name == "public_key"));
    }

    #[test]
    fn loads_the_gateway_artifact() {
        let info = fixture("../../../tests/fixtures/compiled/gateway/compiler/normalized-ir.sexp");
        assert_eq!(info.circuits.len(), 6);
        assert_eq!(info.ledger.len(), 10);
        let threshold = info.ledger.iter().find(|f| f.name == "threshold").unwrap();
        assert_eq!(threshold.index_usize(), Some(0));
        assert_eq!(threshold.storage, crate::types::StorageKind::Cell);
        let egress = info
            .ledger
            .iter()
            .find(|f| f.name == "egress_jobs")
            .unwrap();
        assert_eq!(egress.storage, crate::types::StorageKind::Map);
        assert!(egress.key.is_some() && egress.value.is_some());
        let counter = info
            .ledger
            .iter()
            .find(|f| f.name == "next_job_id")
            .unwrap();
        assert_eq!(counter.storage, crate::types::StorageKind::Counter);
    }

    #[test]
    fn wide_uint_bounds_survive_the_embedded_wire() {
        let info = fixture("../../../tests/conformance/fixtures/loops/compiler/normalized-ir.sexp");
        // The loops corpus adds Uint<64> values; the intermediate bound is
        // above u64 and must survive the JSON wire the generated bindings
        // embed (serialize at macro time, re-parse at run time).
        let json = serde_json::to_string(&info.circuits[0].ir).unwrap();
        let back: Option<CircuitIrBody> = serde_json::from_str(&json).unwrap();
        assert!(back.is_some());
    }
}
