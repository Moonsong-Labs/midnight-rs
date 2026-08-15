//! Load a `normalized-ir.sexp` artifact into [`ContractInfo`].
//!
//! The normalized artifact is the compiler's analyzed IR in its own
//! vocabulary. This module converts it into the crate's internal JSON
//! encoding and feeds the existing deserializers, so the interpreter and the
//! code generator run unchanged. The conversion mirrors the retired JSON
//! emitter's mapping, and the conformance suite holds the result to the same
//! goldens.
//!
//! Naming: the artifact prints identifiers as `%sym.uniq`. A binder whose
//! symbol is unique within its body keeps the bare symbol, so signatures and
//! generated bindings read as the source does; binders that share a symbol
//! (compiler temporaries, shadowed locals) are qualified as `sym.uniq` to
//! keep binding and reference sites in agreement.

use std::collections::HashMap;
use std::path::Path;

use compact_normalized_ir as nir;
use serde_json::{Map, Number, Value, json};

use crate::types::ContractInfo;

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

/// The internal JSON encoding of an artifact, for consumers that inspect
/// the raw tree next to the typed [`ContractInfo`].
pub fn contract_info_value_from_str(text: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let ir = nir::parse_str(text).map_err(|e| {
        NormalizedError(
            e.to_string()
                .trim_start_matches("normalized-ir: ")
                .to_string(),
        )
    })?;
    Ok(contract_info_value(&ir)?)
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
    let value = contract_info_value(&ir)?;
    // Through the string form, never `from_value`: a Uint bound exceeds u64
    // and only this path round-trips it exactly.
    Ok(serde_json::from_str(&value.to_string())?)
}

/// The whole artifact as the internal `contract-info` JSON encoding.
pub fn contract_info_value(ir: &nir::NormalizedIr) -> Result<Value, NormalizedError> {
    let cx = Context::new(ir);

    let mut circuits = Vec::new();
    for (export_name, id) in &ir.exports {
        let Some(c) = cx.circuits.get(id.0.as_str()) else {
            continue; // an exported ledger field
        };
        circuits.push(json!({
            "name": export_name,
            "pure": c.pure,
            "proof": c.proof,
            "arguments": cx.arguments(&c.arguments, &cx.body_renames(c))?,
            "result-type": cx.ty(&c.result_type)?,
            "ir": { "body": cx.body(c)? },
        }));
    }

    let mut helpers = Vec::new();
    for c in ir.circuits() {
        helpers.push(json!({
            "name": cx.circuit_names[c.name.0.as_str()],
            "params": cx.arguments(&c.arguments, &cx.body_renames(c))?,
            "body": cx.body(c)?,
        }));
    }

    let mut witnesses = Vec::new();
    for w in ir.elements.iter().filter_map(|e| match e {
        nir::ProgramElement::Witness(w) => Some(w),
        _ => None,
    }) {
        let renames = HashMap::new(); // witness signatures bind no locals
        witnesses.push(json!({
            "name": w.name.name(),
            "arguments": cx.arguments(&w.arguments, &renames)?,
            "result-type": cx.ty(&w.result_type)?,
        }));
    }

    let ledger = match ir.ledger() {
        Some(decl) => decl
            .fields
            .iter()
            .map(|f| cx.ledger_field(f))
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };

    let contracts: Vec<Value> = ir
        .contract_types
        .iter()
        .filter_map(|t| match t {
            nir::Type::Contract { name, .. } => Some(Value::String(name.clone())),
            _ => None,
        })
        .collect();

    Ok(json!({
        "compiler-version": ir.compiler_version,
        "language-version": ir.language_version,
        "runtime-version": ir.runtime_version,
        "circuits": circuits,
        "witnesses": witnesses,
        "contracts": contracts,
        "ledger": ledger,
        "helpers": helpers,
    }))
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

    fn arguments(
        &self,
        args: &[nir::Argument],
        renames: &HashMap<String, String>,
    ) -> Result<Value, NormalizedError> {
        Ok(Value::Array(
            args.iter()
                .map(|a| Ok(json!({ "name": self.var(&a.name, renames), "type": self.ty(&a.ty)? })))
                .collect::<Result<Vec<_>, NormalizedError>>()?,
        ))
    }

    // -- types ----------------------------------------------------------

    fn ty(&self, t: &nir::Type) -> Result<Value, NormalizedError> {
        use nir::Type::*;
        Ok(match t {
            Boolean => json!({ "type-name": "Boolean" }),
            Field(nir::FieldType::Native) => json!({ "type-name": "Field" }),
            // The interpreter computes Jubjub scalars in the native field,
            // as artifacts did before the compiler distinguished them.
            Field(nir::FieldType::Scalar(nir::Curve::Jubjub)) => {
                json!({ "type-name": "Field" })
            }
            Field(_) => return unsupported("secp256k1 field type"),
            // The runtime's EC values ride the wire spelling artifacts used
            // before the compiler made the point type native.
            Point(nir::Curve::Jubjub) => {
                json!({ "type-name": "Opaque", "tsType": "JubjubPoint" })
            }
            Point(nir::Curve::Secp256k1) => return unsupported("a secp256k1 point type"),
            Unsigned(maxval) => json!({ "type-name": "Uint", "maxval": big(maxval) }),
            Bytes(len) => json!({ "type-name": "Bytes", "length": len }),
            Opaque(ts) => json!({ "type-name": "Opaque", "tsType": ts }),
            Vector { len, ty } => {
                json!({ "type-name": "Vector", "length": len, "type": self.ty(ty)? })
            }
            Tuple(types) => json!({
                "type-name": "Tuple",
                "types": types.iter().map(|t| self.ty(t)).collect::<Result<Vec<_>, _>>()?,
            }),
            Struct { name, fields } => json!({
                "type-name": "Struct",
                "name": name,
                "elements": fields
                    .iter()
                    .map(|(n, t)| Ok(json!({ "name": n, "type": self.ty(t)? })))
                    .collect::<Result<Vec<_>, NormalizedError>>()?,
            }),
            Enum { name, variants } => {
                json!({ "type-name": "Enum", "name": name, "elements": variants })
            }
            Alias { nominal, name, ty } => {
                if *nominal {
                    json!({ "type-name": "Alias", "name": name, "type": self.ty(ty)? })
                } else {
                    self.ty(ty)?
                }
            }
            Contract { name, .. } => json!({ "type-name": "Contract", "name": name }),
            // The element type of an empty vector; no value of it exists,
            // so it maps to the unit type.
            Unknown => json!({ "type-name": "Tuple", "types": [] }),
            Adt { name, .. } => {
                return unsupported(&format!("ADT type {name} in a value position"));
            }
            TypeVar(v) => return unsupported(&format!("type variable {v}")),
        })
    }

    fn ledger_field(&self, f: &nir::LedgerBinding) -> Result<Value, NormalizedError> {
        let mut obj = Map::new();
        obj.insert("name".into(), json!(f.name.name()));
        let index = if f.path.len() == 1 {
            json!(f.path[0])
        } else {
            json!(f.path)
        };
        obj.insert("index".into(), index);
        obj.insert("exported".into(), json!(f.exported));
        let nir::Type::Adt { name, args } = strip_alias(&f.ty) else {
            return unsupported("a ledger field whose type is not a storage kind");
        };
        let kind = name.strip_prefix("__compact_").unwrap_or(name);
        obj.insert("storage".into(), json!(kind));
        let arg_ty = |i: usize| -> Result<Value, NormalizedError> {
            match &args[i] {
                nir::AdtArg::Type(t) => self.ty(t),
                nir::AdtArg::Nat(n) => Ok(json!(n)),
            }
        };
        match kind {
            "Cell" | "Set" | "List" => {
                obj.insert("type".into(), arg_ty(0)?);
            }
            "Counter" | "Kernel" => {}
            "Map" => {
                obj.insert("key".into(), arg_ty(0)?);
                obj.insert("value".into(), arg_ty(1)?);
            }
            "MerkleTree" | "HistoricMerkleTree" => {
                obj.insert("depth".into(), arg_ty(0)?);
                obj.insert("type".into(), arg_ty(1)?);
            }
            other => return unsupported(&format!("ledger storage kind {other}")),
        }
        Ok(Value::Object(obj))
    }

    // -- bodies ---------------------------------------------------------

    /// A circuit body: a `seq` of expression statements whose last value is
    /// the return value.
    fn body(&self, c: &nir::Circuit) -> Result<Value, NormalizedError> {
        let renames = self.body_renames(c);
        let stmts: Vec<&nir::Expr> = match &c.body {
            nir::Expr::Seq(items) => items.iter().collect(),
            other => vec![other],
        };
        Ok(json!({
            "op": "seq",
            "stmts": stmts
                .iter()
                .map(|e| Ok(json!({ "op": "expr-stmt", "expr": self.expr(e, &renames)? })))
                .collect::<Result<Vec<_>, NormalizedError>>()?,
        }))
    }

    fn expr(
        &self,
        e: &nir::Expr,
        renames: &HashMap<String, String>,
    ) -> Result<Value, NormalizedError> {
        use nir::Expr::*;
        let sub = |e: &nir::Expr| self.expr(e, renames);
        let binop = |op: &str, l: &nir::Expr, r: &nir::Expr| -> Result<Value, NormalizedError> {
            Ok(json!({ "op": op, "left": self.expr(l, renames)?, "right": self.expr(r, renames)? }))
        };
        Ok(match e {
            Quote(lit) => match lit {
                nir::Literal::Bool(b) => json!({
                    "op": "lit",
                    "type": { "type-name": "Boolean" },
                    "value": if *b { "true" } else { "false" },
                }),
                nir::Literal::Int(i) => json!({
                    "op": "lit",
                    "type": { "type-name": "Field" },
                    "value": i.to_string(),
                }),
                nir::Literal::Bytes(b) => json!({
                    "op": "lit",
                    "type": { "type-name": "Bytes", "length": b.len() },
                    "value": b.iter().map(|x| format!("{x:02X}")).collect::<String>(),
                }),
            },
            VarRef(id) => json!({ "op": "var", "name": self.var(id, renames) }),
            Default(t) => json!({ "op": "default", "type": self.ty(t)? }),
            If { cond, then, els } => json!({
                "op": "if-expr", "cond": sub(cond)?, "then": sub(then)?, "else": sub(els)?,
            }),
            EltRef { expr, elt, .. } => json!({ "op": "field", "expr": sub(expr)?, "name": elt }),
            EnumRef { ty, elt } => {
                json!({ "op": "enum-member", "type": self.ty(ty)?, "member": elt })
            }
            Tuple(args) | VectorLit(args) => json!({
                "op": "tuple",
                "elements": args
                    .iter()
                    .map(|a| match a {
                        nir::TupleArg::Single(e) => sub(e),
                        nir::TupleArg::Spread { len, expr } => {
                            Ok(json!({ "op": "spread", "length": len, "expr": sub(expr)? }))
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            TupleRef { expr, index } => {
                json!({ "op": "index", "expr": sub(expr)?, "index": index })
            }
            TupleSlice {
                ty,
                expr,
                index,
                len,
            } => json!({
                "op": "tuple-slice", "expr": sub(expr)?, "index": index, "length": len,
                "type": self.ty(ty)?,
            }),
            VectorRef { expr, index, .. } => {
                json!({ "op": "vector-index", "expr": sub(expr)?, "index": sub(index)? })
            }
            VectorSlice {
                ty,
                expr,
                index,
                len,
            } => json!({
                "op": "vector-slice", "expr": sub(expr)?, "index": sub(index)?, "length": len,
                "type": self.ty(ty)?,
            }),
            BytesRef { expr, index, .. } => {
                json!({ "op": "bytes-index", "expr": sub(expr)?, "index": sub(index)? })
            }
            BytesSlice {
                expr, index, len, ..
            } => json!({
                "op": "bytes-slice", "expr": sub(expr)?, "index": sub(index)?, "length": len,
            }),
            Add { left, right, .. } => binop("add", left, right)?,
            Sub { left, right, .. } => binop("sub", left, right)?,
            Mul { left, right, .. } => binop("mul", left, right)?,
            Eq { left, right, .. } => binop("eq", left, right)?,
            Neq { left, right, .. } => binop("neq", left, right)?,
            Lt { left, right, .. } => binop("lt", left, right)?,
            Le { left, right, .. } => binop("le", left, right)?,
            Gt { left, right, .. } => binop("gt", left, right)?,
            Ge { left, right, .. } => binop("ge", left, right)?,
            Map { len, fun, args } => json!({
                "op": "map", "length": len, "fun": self.fun(fun, renames)?,
                "args": args.iter().map(|a| sub(&a.expr)).collect::<Result<Vec<_>, _>>()?,
            }),
            Fold {
                len,
                fun,
                init,
                args,
                ..
            } => json!({
                "op": "fold", "length": len, "fun": self.fun(fun, renames)?,
                "init": sub(init)?,
                "args": args.iter().map(|a| sub(&a.expr)).collect::<Result<Vec<_>, _>>()?,
            }),
            Call { name, args } => {
                let args = args.iter().map(sub).collect::<Result<Vec<_>, _>>()?;
                let (op, callee_name, result) = match self.callee(name)? {
                    Callee::Circuit(c) => (
                        "call-pure",
                        self.circuit_names[c.name.0.as_str()].clone(),
                        self.ty(&c.result_type)?,
                    ),
                    Callee::NativeCircuit(n) => (
                        "call-pure",
                        n.name.name().to_string(),
                        self.ty(&n.result_type)?,
                    ),
                    Callee::NativeWitness(n) => (
                        "call-witness",
                        n.name.name().to_string(),
                        self.ty(&n.result_type)?,
                    ),
                    Callee::Witness(w) => (
                        "call-witness",
                        w.name.name().to_string(),
                        self.ty(&w.result_type)?,
                    ),
                };
                json!({ "op": op, "name": callee_name, "args": args, "result-type": result })
            }
            New { ty, elements } => json!({
                "op": "new", "type": self.ty(ty)?,
                "elements": elements.iter().map(sub).collect::<Result<Vec<_>, _>>()?,
            }),
            Seq(items) => {
                // A nested sequence: bind the discarded values so evaluation
                // order stays.
                let (last, init) = items
                    .split_last()
                    .ok_or_else(|| NormalizedError("empty seq".into()))?;
                let mut out = sub(last)?;
                for (n, item) in init.iter().enumerate().rev() {
                    out = json!({
                        "op": "let-expr",
                        "bindings": [
                            { "op": "let", "name": format!("__seq_{n}"), "value": sub(item)? }
                        ],
                        "body": out,
                    });
                }
                out
            }
            LetStar { bindings, body } => json!({
                "op": "let-expr",
                "bindings": bindings
                    .iter()
                    .map(|(arg, value)| {
                        Ok(json!({
                            "op": "let",
                            "name": self.var(&arg.name, renames),
                            "value": sub(value)?,
                        }))
                    })
                    .collect::<Result<Vec<_>, NormalizedError>>()?,
                "body": sub(body)?,
            }),
            Assert { expr, message } => {
                json!({ "op": "assert", "expr": sub(expr)?, "message": message })
            }
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
                json!({ "op": "field-to-bytes", "length": len, "expr": sub(expr)? })
            }
            CastFromBytes { ty, len, expr } => json!({
                "op": "cast", "expr": sub(expr)?,
                "from": { "type-name": "Bytes", "length": len },
                "to": self.ty(ty)?,
            }),
            VectorToBytes { len, expr } => {
                json!({ "op": "vector-to-bytes", "length": len, "expr": sub(expr)? })
            }
            BytesToVector { len, expr } => {
                json!({ "op": "bytes-to-vector", "length": len, "expr": sub(expr)? })
            }
            CastFromEnum { ty, from, expr } | CastToEnum { ty, from, expr } => json!({
                "op": "cast", "expr": sub(expr)?, "from": self.ty(from)?, "to": self.ty(ty)?,
            }),
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
                json!({
                    "op": "cast", "expr": sub(expr)?, "from": self.ty(from)?,
                    "to": { "type-name": "Field" },
                })
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
                json!({
                    "op": "cast", "expr": sub(expr)?,
                    "from": { "type-name": "Field" },
                    "to": { "type-name": "Uint", "maxval": big(maxval) },
                })
            }
            SafeCast { ty, from, expr } => json!({
                "op": "cast", "expr": sub(expr)?, "from": self.ty(from)?, "to": self.ty(ty)?,
            }),
            DowncastUnsigned {
                from_maxval,
                to_maxval,
                expr,
            } => json!({
                "op": "cast", "expr": sub(expr)?,
                "from": { "type-name": "Uint", "maxval": big(from_maxval) },
                "to": { "type-name": "Uint", "maxval": big(to_maxval) },
            }),
            ContractCall {
                circuit,
                receiver,
                contract_type,
                args,
            } => json!({
                "op": "contract-call", "circuit": circuit, "contract": sub(receiver)?,
                "contract-type": self.ty(contract_type)?,
                "args": args.iter().map(sub).collect::<Result<Vec<_>, _>>()?,
            }),
            Emit { .. } => return unsupported("events (emit)"),
            PublicLedger {
                result_type,
                instructions,
                ..
            } => json!({
                "op": "ledger-query",
                "ops": instructions
                    .iter()
                    .map(|i| self.instruction(i, renames))
                    .collect::<Result<Vec<_>, _>>()?,
                "result-type": self.ty(result_type)?,
            }),
            Return(inner) => sub(inner)?,
        })
    }

    fn fun(
        &self,
        f: &nir::Fun,
        renames: &HashMap<String, String>,
    ) -> Result<Value, NormalizedError> {
        Ok(match f {
            nir::Fun::Ref(id) => match self.callee(id)? {
                Callee::Circuit(c) => json!({ "call": self.circuit_names[c.name.0.as_str()] }),
                _ => json!({ "call": id.name() }),
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
                json!({
                    "params": self.arguments(arguments, &renames)?,
                    "body": self.expr(body, &renames)?,
                })
            }
        })
    }

    // -- ledger instructions --------------------------------------------

    fn instruction(
        &self,
        i: &nir::Instruction,
        renames: &HashMap<String, String>,
    ) -> Result<Value, NormalizedError> {
        let arg = |name: &str| i.arg(name);
        let flag = |name: &str| matches!(arg(name), Some(nir::Operand::Bool(true)));
        let int = |name: &str| -> Result<u64, NormalizedError> {
            match arg(name) {
                Some(nir::Operand::Int(n)) => u64::try_from(n)
                    .map_err(|_| NormalizedError(format!("{}: {name} out of range", i.op))),
                _ => Err(NormalizedError(format!("{}: missing integer {name}", i.op))),
            }
        };
        Ok(match i.op.as_str() {
            "idx" => {
                let Some(nir::Operand::List(path)) = arg("path") else {
                    return unsupported("idx without a path list");
                };
                json!({
                    "op": "idx",
                    "cached": flag("cached"),
                    "push-path": flag("pushPath"),
                    "path": path
                        .iter()
                        .map(|p| self.path_entry(p, renames))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            }
            "push" => json!({
                "op": "push",
                "storage": flag("storage"),
                "value": self.operand(
                    arg("value").ok_or_else(|| NormalizedError("push without value".into()))?,
                    renames,
                )?,
            }),
            "addi" => json!({
                "op": "addi",
                "immediate": self.operand(
                    arg("immediate")
                        .ok_or_else(|| NormalizedError("addi without immediate".into()))?,
                    renames,
                )?,
            }),
            "ins" => json!({ "op": "ins", "cached": flag("cached"), "n": int("n")? }),
            "dup" => json!({ "op": "dup", "n": int("n").unwrap_or(0) }),
            "swap" => json!({ "op": "swap", "n": int("n").unwrap_or(0) }),
            "popeq" => json!({ "op": "popeq", "cached": flag("cached") }),
            "rem" => json!({ "op": "rem", "cached": flag("cached") }),
            "noop" => json!({ "op": "noop", "n": int("n")? }),
            "branch" => json!({ "op": "branch", "skip": int("skip")? }),
            "member" | "root" | "eq" | "ckpt" | "neg" | "add" => json!({ "op": i.op }),
            other => return unsupported(&format!("VM instruction {other}")),
        })
    }

    fn path_entry(
        &self,
        p: &nir::Operand,
        renames: &HashMap<String, String>,
    ) -> Result<Value, NormalizedError> {
        Ok(match p {
            nir::Operand::Align { value, bytes } => json!({
                "tag": "value",
                "value": value.to_string(),
                "type": { "type-name": "Uint", "maxval": max_for_bytes(*bytes) },
            }),
            nir::Operand::Stack => json!({ "tag": "stack" }),
            nir::Operand::Expr(e) => match e.as_ref() {
                nir::Expr::VarRef(id) => json!({ "tag": "var", "name": self.var(id, renames) }),
                other => json!({ "tag": "expr", "expr": self.expr(other, renames)? }),
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
        Ok(match o {
            Int(n) => serde_json::Value::Number(Number::from_string_unchecked(n.to_string())),
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
                    .collect::<Result<Vec<_>, _>>()?,
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
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Expr(e) => self.expr(e, renames)?,
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

fn big(n: &num_bigint::BigUint) -> Value {
    Value::Number(Number::from_string_unchecked(n.to_string()))
}

fn max_for_bytes(bytes: u64) -> Value {
    let max = (num_bigint::BigUint::from(1u8) << (8 * bytes)) - 1u8;
    big(&max)
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
        let Some(crate::types::TypeNode::Struct { elements, .. }) = &message.element_type else {
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
    fn wide_uint_bounds_survive() {
        let info = fixture("../../../tests/conformance/fixtures/loops/compiler/normalized-ir.sexp");
        // The loops corpus adds Uint<64> values; the intermediate bound is
        // above u64 and must round-trip through the bridge.
        let json = serde_json::to_string(&info.circuits[0].ir).unwrap();
        drop(json);
    }
}
