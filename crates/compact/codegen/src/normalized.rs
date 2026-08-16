//! Load a `normalized-ir.sexp` artifact into [`ContractInfo`].
//!
//! The normalized artifact is the compiler's analyzed IR in its own
//! vocabulary, and this module keeps that vocabulary. It groups the
//! artifact's elements into the shapes the generator asks for (exported
//! circuits, helper circuits, witnesses, ledger fields) and refuses any type
//! or expression no consumer can execute. Nothing is re-encoded, so the
//! interpreter and the code generator run on one representation.
//!
//! Naming: the artifact prints identifiers as `%sym.uniq`. The model carries
//! them whole; a consumer that wants the source-level name calls
//! [`nir::Ident::name`].

use std::collections::HashMap;
use std::path::Path;

use crate::types::{Circuit, ContractInfo, FieldIndex, LedgerField, StorageKind};
use compact_normalized_ir as nir;

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
        check_circuit(c)?;
        circuits.push(Circuit {
            name: export_name.clone(),
            def: (*c).clone(),
        });
    }

    // Every circuit is callable from an exported body, so all of them travel
    // with the contract.
    let mut helpers = Vec::new();
    for c in ir.circuits() {
        check_circuit(c)?;
        helpers.push(c.clone());
    }

    let natives: Vec<nir::Native> = ir
        .elements
        .iter()
        .filter_map(|e| match e {
            nir::ProgramElement::Native(n) => Some(n.clone()),
            _ => None,
        })
        .collect();

    let mut witnesses = Vec::new();
    for w in ir.elements.iter().filter_map(|e| match e {
        nir::ProgramElement::Witness(w) => Some(w),
        _ => None,
    }) {
        for a in &w.arguments {
            check_type(&a.ty)?;
        }
        check_type(&w.result_type)?;
        witnesses.push(w.clone());
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
        natives,
    })
}

struct Context<'a> {
    /// Full id text -> the circuit definition.
    circuits: HashMap<&'a str, &'a nir::Circuit>,
}

impl<'a> Context<'a> {
    fn new(ir: &'a nir::NormalizedIr) -> Self {
        let mut circuits = HashMap::new();
        for e in &ir.elements {
            if let nir::ProgramElement::Circuit(c) = e {
                circuits.insert(c.name.0.as_str(), c);
            }
        }
        Context { circuits }
    }

    // -- types ----------------------------------------------------------

    /// Types pass through unchanged; this only refuses the ones no
    /// consumer can handle, so a bad artifact fails at load with a named
    /// error instead of mid-execution.
    fn ty(&self, t: &nir::Type) -> Result<nir::Type, NormalizedError> {
        check_type(t)?;
        Ok(t.clone())
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
        let arg_ty = |i: usize| -> Result<nir::Type, NormalizedError> {
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
}

// -- free helpers -------------------------------------------------------

fn strip_alias(t: &nir::Type) -> &nir::Type {
    match t {
        nir::Type::Alias { ty, .. } => strip_alias(ty),
        other => other,
    }
}

/// Refuse a type no consumer can handle, naming it. Keeping this at load
/// time is what lets the interpreter assume every type it meets is one it
/// can execute.
fn check_type(t: &nir::Type) -> Result<(), NormalizedError> {
    match t {
        nir::Type::Field(ft)
            if !matches!(
                ft,
                nir::FieldType::Native | nir::FieldType::Scalar(nir::Curve::Jubjub)
            ) =>
        {
            unsupported("a secp256k1 field type")
        }
        nir::Type::Point(nir::Curve::Secp256k1) => unsupported("a secp256k1 point type"),
        nir::Type::Adt { name, .. } => unsupported(&format!("ADT type {name} in a value position")),
        nir::Type::TypeVar(v) => unsupported(&format!("type variable {v}")),
        nir::Type::Vector { ty, .. } => check_type(ty),
        nir::Type::Alias { ty, .. } => check_type(ty),
        nir::Type::Tuple(types) => types.iter().try_for_each(check_type),
        nir::Type::Struct { fields, .. } => fields.iter().try_for_each(|(_, t)| check_type(t)),
        _ => Ok(()),
    }
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
/// Refuse a circuit the interpreter cannot execute, naming the construct.
/// Running this at load is what lets execution assume every form it meets is
/// one it handles.
fn check_circuit(c: &nir::Circuit) -> Result<(), NormalizedError> {
    for a in &c.arguments {
        check_type(&a.ty)?;
    }
    check_type(&c.result_type)?;
    check_expr(&c.body)
}

/// Walk a circuit body and refuse anything the interpreter cannot execute.
/// Visiting every variant is the point: a form added to the language shows up
/// here as a non-exhaustive match, not as silent mis-execution.
fn check_expr(e: &nir::Expr) -> Result<(), NormalizedError> {
    use nir::Expr::*;
    let each = |es: &[nir::Expr]| es.iter().try_for_each(check_expr);
    let args = |args: &[nir::MapArg]| args.iter().try_for_each(|a| check_expr(&a.expr));
    let tuple_args = |ta: &[nir::TupleArg]| {
        ta.iter().try_for_each(|a| match a {
            nir::TupleArg::Single(x) | nir::TupleArg::Spread { expr: x, .. } => check_expr(x),
        })
    };
    let fun = |f: &nir::Fun| match f {
        nir::Fun::Ref(_) => Ok(()),
        nir::Fun::Circuit {
            arguments,
            result_type,
            body,
        } => {
            arguments.iter().try_for_each(|a| check_type(&a.ty))?;
            check_type(result_type)?;
            check_expr(body)
        }
    };
    match e {
        // Events ride the public transcript as VM ops; nothing replays them
        // off-chain yet.
        Emit { .. } => unsupported("events (emit)"),

        Quote(_) | VarRef(_) => Ok(()),
        Default(ty) => check_type(ty),
        EnumRef { ty, .. } => check_type(ty),

        If { cond, then, els } => {
            check_expr(cond)?;
            check_expr(then)?;
            check_expr(els)
        }
        EltRef { expr, .. } | TupleRef { expr, .. } | Return(expr) | Assert { expr, .. } => {
            check_expr(expr)
        }
        FieldToBytes { expr, .. } | VectorToBytes { expr, .. } | BytesToVector { expr, .. } => {
            check_expr(expr)
        }
        Tuple(ta) | VectorLit(ta) => tuple_args(ta),
        TupleSlice { ty, expr, .. } => {
            check_type(ty)?;
            check_expr(expr)
        }
        VectorRef { ty, expr, index } | BytesRef { ty, expr, index } => {
            check_type(ty)?;
            check_expr(expr)?;
            check_expr(index)
        }
        VectorSlice {
            ty, expr, index, ..
        }
        | BytesSlice {
            ty, expr, index, ..
        } => {
            check_type(ty)?;
            check_expr(expr)?;
            check_expr(index)
        }
        Add { ty, left, right } | Sub { ty, left, right } | Mul { ty, left, right } => {
            check_type(ty)?;
            check_expr(left)?;
            check_expr(right)
        }
        Eq { ty, left, right } | Neq { ty, left, right } => {
            check_type(ty)?;
            check_expr(left)?;
            check_expr(right)
        }
        Lt { left, right, .. }
        | Le { left, right, .. }
        | Gt { left, right, .. }
        | Ge { left, right, .. } => {
            check_expr(left)?;
            check_expr(right)
        }
        Map {
            fun: f, args: a, ..
        } => {
            fun(f)?;
            args(a)
        }
        Fold {
            fun: f,
            init,
            args: a,
            ..
        } => {
            fun(f)?;
            check_expr(init)?;
            args(a)
        }
        Call { args: a, .. } => each(a),
        New { ty, elements } => {
            check_type(ty)?;
            each(elements)
        }
        Seq(items) => each(items),
        LetStar { bindings, body } => {
            bindings.iter().try_for_each(|(arg, value)| {
                check_type(&arg.ty)?;
                check_expr(value)
            })?;
            check_expr(body)
        }
        CastFromBytes { ty, expr, .. } | SafeCast { ty, expr, .. } => {
            check_type(ty)?;
            check_expr(expr)
        }
        CastFromEnum { ty, from, expr } | CastToEnum { ty, from, expr } => {
            check_type(ty)?;
            check_type(from)?;
            check_expr(expr)
        }
        CastToField {
            field_type,
            from,
            expr,
        } => {
            check_type(&nir::Type::Field(field_type.clone()))?;
            check_type(from)?;
            check_expr(expr)
        }
        CastFromField {
            field_type, expr, ..
        } => {
            check_type(&nir::Type::Field(field_type.clone()))?;
            check_expr(expr)
        }
        DowncastUnsigned { expr, .. } => check_expr(expr),
        ContractCall {
            receiver,
            contract_type,
            args: a,
            ..
        } => {
            check_type(contract_type)?;
            check_expr(receiver)?;
            each(a)
        }
        PublicLedger {
            result_type,
            args: a,
            path,
            instructions,
            ..
        } => {
            check_type(result_type)?;
            path.iter().try_for_each(|p| match p {
                nir::PathElement::Index(_) => Ok(()),
                nir::PathElement::Computed { ty, expr } => {
                    check_type(ty)?;
                    check_expr(expr)
                }
            })?;
            each(a)?;
            instructions
                .iter()
                .try_for_each(|i| i.args.iter().try_for_each(|(_, o)| check_operand(o)))
        }
    }
}

fn check_operand(o: &nir::Operand) -> Result<(), NormalizedError> {
    use nir::Operand::*;
    match o {
        Expr(e) => check_expr(e),
        ValueToInt(x) | Null(x) | MaxSizeof(x) | LeafHash(x) => check_operand(x),
        CoinCommit(a, b) => {
            check_operand(a)?;
            check_operand(b)
        }
        AlignedConcat(xs) | List(xs) => xs.iter().try_for_each(check_operand),
        StateValue(sv) => match sv {
            nir::StateValue::Cell(x) | nir::StateValue::Adt(x) => check_operand(x),
            nir::StateValue::Array(xs) => xs.iter().try_for_each(check_operand),
            nir::StateValue::Map(entries) | nir::StateValue::MerkleTree { entries, .. } => entries
                .iter()
                .try_for_each(|(k, v)| check_operand(k).and_then(|()| check_operand(v))),
            nir::StateValue::Null => Ok(()),
        },
        Int(_) | Bool(_) | Str(_) | Align { .. } | Stack | Void => Ok(()),
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
        let names: Vec<_> = info.circuits.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"post") && names.contains(&"public_key"));
        let message = info
            .ledger
            .iter()
            .find(|f| f.name == "message")
            .expect("message");
        let Some(nir::Type::Struct { fields, .. }) = &message.element_type else {
            panic!("message should be a struct-typed cell")
        };
        assert_eq!(fields.len(), 2);
        // public_key is exported and called, so it is also a helper.
        assert!(info.helpers.iter().any(|h| h.name.name() == "public_key"));
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

    /// A `Uint` bound can exceed `u64`: adding two `Uint<64>` values reaches
    /// 2^65-2, and a `Uint<128>` field reaches 2^128-1. The bound is carried
    /// as digits, so no integer width truncates it.
    ///
    /// The whole parsed model is searched, because a bound can sit anywhere a
    /// type does (a signature, a cast, a ledger path key).
    #[test]
    fn a_uint_bound_above_u64_is_carried_exactly() {
        let ops = fixture("../../../tests/conformance/fixtures/ops/compiler/normalized-ir.sexp");
        assert!(
            format!("{ops:?}").contains("36893488147419103230"),
            "the sum of two Uint<64> bounds should survive as digits"
        );

        let mint =
            fixture("../../../tests/fixtures/compiled/mint-probe/compiler/normalized-ir.sexp");
        assert!(
            format!("{mint:?}").contains("340282366920938463463374607431768211455"),
            "a Uint<128> bound should survive as digits"
        );
    }

    /// A `dup` carries the stack arity its ledger-op template gave it. The
    /// interpreter replays it as `Op::Dup { n }`, so a dropped arity would
    /// duplicate the wrong stack slot.
    #[test]
    fn dup_ops_carry_their_stack_arity() {
        let info =
            fixture("../../../tests/fixtures/compiled/mint-probe/compiler/normalized-ir.sexp");
        let dumped = format!("{info:?}");
        let dup = |n: u8| format!(r#"Instruction {{ op: "dup", args: [("n", Int({n}))] }}"#);
        assert!(dumped.contains(&dup(1)), "mint-probe has an arity-1 dup");
        assert!(dumped.contains(&dup(2)), "mint-probe has an arity-2 dup");
        assert!(
            !dumped.contains(&dup(0)),
            "no dup should lose its arity to the default"
        );
    }
}
