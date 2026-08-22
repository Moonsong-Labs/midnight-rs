//! Load an `analyzed-ir.sexp` artifact into [`ContractInfo`].
//!
//! The artifact is the compiler's analyzed IR in its own
//! vocabulary, and this module keeps that vocabulary. It groups the
//! artifact's elements into the shapes the generator asks for (exported
//! circuits, helper circuits, witnesses, ledger fields) and refuses any type
//! or expression no consumer can execute. Nothing is re-encoded, so the
//! interpreter and the code generator run on one representation.
//!
//! Naming: the artifact prints identifiers as `%sym.uniq`. The model carries
//! them whole; a consumer that wants the source-level name calls
//! [`ir::Ident::name`].

use std::collections::HashMap;
use std::path::Path;

use crate::types::{Circuit, ContractInfo, FieldIndex, LedgerField, StorageKind};
use compact_analyzed_ir as ir;

/// A conversion failure: a construct the internal IR cannot represent yet
/// (events, foreign fields, curve points) or a malformed artifact.
#[derive(Debug)]
pub struct ArtifactError(String);

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "analyzed-ir: {}", self.0)
    }
}

impl std::error::Error for ArtifactError {}

fn unsupported<T>(what: &str) -> Result<T, ArtifactError> {
    Err(ArtifactError(format!(
        "unsupported by the interpreter IR: {what}"
    )))
}

pub fn load(path: &Path) -> Result<ContractInfo, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    load_str(&content)
}

pub fn load_str(text: &str) -> Result<ContractInfo, Box<dyn std::error::Error>> {
    let artifact = ir::parse_str(text).map_err(|e| {
        // The reader's Display already carries the format name.
        ArtifactError(
            e.to_string()
                .trim_start_matches("analyzed-ir: ")
                .to_string(),
        )
    })?;
    Ok(from_program(&artifact)?)
}

/// The whole artifact as the crate's typed model.
pub fn from_program(artifact: &ir::AnalyzedIr) -> Result<ContractInfo, ArtifactError> {
    let cx = Context::new(artifact);

    let mut circuits = Vec::new();
    for (export_name, id) in &artifact.exports {
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
    for c in artifact.circuits() {
        check_circuit(c)?;
        helpers.push(c.clone());
    }

    let natives: Vec<ir::Native> = artifact
        .elements
        .iter()
        .filter_map(|e| match e {
            ir::ProgramElement::Native(n) => Some(n.clone()),
            _ => None,
        })
        .collect();

    let mut witnesses = Vec::new();
    for w in artifact.elements.iter().filter_map(|e| match e {
        ir::ProgramElement::Witness(w) => Some(w),
        _ => None,
    }) {
        for a in &w.arguments {
            check_type(&a.ty)?;
        }
        check_type(&w.result_type)?;
        witnesses.push(w.clone());
    }

    let ledger = match artifact.ledger() {
        Some(decl) => decl
            .fields
            .iter()
            .map(|f| cx.ledger_field(f))
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };

    let contracts: Vec<String> = artifact
        .contract_types
        .iter()
        .filter_map(|t| match t {
            ir::Type::Contract { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();

    Ok(ContractInfo {
        compiler_version: artifact.compiler_version.clone(),
        language_version: artifact.language_version.clone(),
        runtime_version: artifact.runtime_version.clone(),
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
    circuits: HashMap<&'a str, &'a ir::Circuit>,
}

impl<'a> Context<'a> {
    fn new(artifact: &'a ir::AnalyzedIr) -> Self {
        let mut circuits = HashMap::new();
        for e in &artifact.elements {
            if let ir::ProgramElement::Circuit(c) = e {
                circuits.insert(c.name.0.as_str(), c);
            }
        }
        Context { circuits }
    }

    // -- types ----------------------------------------------------------

    /// Types pass through unchanged; this only refuses the ones no
    /// consumer can handle, so a bad artifact fails at load with a named
    /// error instead of mid-execution.
    fn ty(&self, t: &ir::Type) -> Result<ir::Type, ArtifactError> {
        check_type(t)?;
        Ok(t.clone())
    }

    fn ledger_field(&self, f: &ir::LedgerBinding) -> Result<LedgerField, ArtifactError> {
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
        let ir::Type::Adt { name, args } = strip_alias(&f.ty) else {
            return unsupported("a ledger field whose type is not a storage kind");
        };
        let kind = name.strip_prefix("__compact_").unwrap_or(name);
        let arg_ty = |i: usize| -> Result<ir::Type, ArtifactError> {
            match &args[i] {
                ir::AdtArg::Type(t) => self.ty(t),
                ir::AdtArg::Nat(n) => unsupported(&format!("a numeric type argument {n}")),
            }
        };
        let arg_nat = |i: usize| -> Result<u64, ArtifactError> {
            match &args[i] {
                ir::AdtArg::Nat(n) => Ok(*n),
                ir::AdtArg::Type(_) => unsupported("a type where a depth was expected"),
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

fn strip_alias(t: &ir::Type) -> &ir::Type {
    match t {
        ir::Type::Alias { ty, .. } => strip_alias(ty),
        other => other,
    }
}

/// Refuse a type no consumer can handle, naming it. Keeping this at load
/// time is what lets the interpreter assume every type it meets is one it
/// can execute.
fn check_type(t: &ir::Type) -> Result<(), ArtifactError> {
    match t {
        ir::Type::Field(ft)
            if !matches!(
                ft,
                ir::FieldType::Native | ir::FieldType::Scalar(ir::Curve::Jubjub)
            ) =>
        {
            unsupported("a secp256k1 field type")
        }
        ir::Type::Point(ir::Curve::Secp256k1) => unsupported("a secp256k1 point type"),
        ir::Type::Adt { name, .. } => unsupported(&format!("ADT type {name} in a value position")),
        ir::Type::TypeVar(v) => unsupported(&format!("type variable {v}")),
        ir::Type::Vector { ty, .. } => check_type(ty),
        ir::Type::Alias { ty, .. } => check_type(ty),
        ir::Type::Tuple(types) => types.iter().try_for_each(check_type),
        ir::Type::Struct { fields, .. } => fields.iter().try_for_each(|(_, t)| check_type(t)),
        _ => Ok(()),
    }
}

fn as_usize<T: Copy + TryInto<usize> + std::fmt::Display>(
    n: T,
    what: &str,
) -> Result<usize, ArtifactError> {
    n.try_into()
        .map_err(|_| ArtifactError(format!("{what} out of range: {n}")))
}

/// Every binder in an expression tree: let bindings and inline-function
/// parameters (circuit arguments are collected by the caller).
/// Refuse a circuit the interpreter cannot execute, naming the construct.
/// Running this at load is what lets execution assume every form it meets is
/// one it handles.
fn check_circuit(c: &ir::Circuit) -> Result<(), ArtifactError> {
    for a in &c.arguments {
        check_type(&a.ty)?;
    }
    check_type(&c.result_type)?;
    check_expr(&c.body)
}

/// Walk a circuit body and refuse anything the interpreter cannot execute.
/// Visiting every variant is the point: a form added to the language shows up
/// here as a non-exhaustive match, not as silent mis-execution.
fn check_expr(e: &ir::Expr) -> Result<(), ArtifactError> {
    use ir::Expr::*;
    let each = |es: &[ir::Expr]| es.iter().try_for_each(check_expr);
    let args = |args: &[ir::MapArg]| args.iter().try_for_each(|a| check_expr(&a.expr));
    let tuple_args = |ta: &[ir::TupleArg]| {
        ta.iter().try_for_each(|a| match a {
            ir::TupleArg::Single(x) | ir::TupleArg::Spread { expr: x, .. } => check_expr(x),
        })
    };
    let fun = |f: &ir::Fun| match f {
        ir::Fun::Ref(_) => Ok(()),
        ir::Fun::Circuit {
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
            check_type(&ir::Type::Field(field_type.clone()))?;
            check_type(from)?;
            check_expr(expr)
        }
        CastFromField {
            field_type, expr, ..
        } => {
            check_type(&ir::Type::Field(field_type.clone()))?;
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
            op_class,
            result_type,
            args: a,
            path,
            instructions,
            ..
        } => {
            // The runtime checks the coin commitment before it runs the
            // instructions, and that check is not one of them, so executing
            // these instructions alone would build a different transcript.
            if let ir::OpClass::CoinCheck { name, .. } = op_class {
                return unsupported(&format!("ledger operation class {name}"));
            }
            check_type(result_type)?;
            path.iter().try_for_each(|p| match p {
                ir::PathElement::Index(_) => Ok(()),
                ir::PathElement::Computed { ty, expr } => {
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

fn check_operand(o: &ir::Operand) -> Result<(), ArtifactError> {
    use ir::Operand::*;
    match o {
        Expr(e) => check_expr(e),
        ValueToInt(x) | LeafHash(x) => check_operand(x),
        Null(t) | MaxSizeof(t) => check_type(t),
        CoinCommit(a, b) | Add(a, b) => {
            check_operand(a)?;
            check_operand(b)
        }
        AlignedConcat(xs) | List(xs) => xs.iter().try_for_each(check_operand),
        StateValue(sv) => match sv {
            ir::StateValue::Cell(x) => check_operand(x),
            ir::StateValue::Adt(x, t) => check_operand(x).and_then(|()| check_type(t)),
            ir::StateValue::Array(xs) => xs.iter().try_for_each(check_operand),
            ir::StateValue::Map(entries) | ir::StateValue::MerkleTree { entries, .. } => entries
                .iter()
                .try_for_each(|(k, v)| check_operand(k).and_then(|()| check_operand(v))),
            ir::StateValue::Null => Ok(()),
        },
        Int(_) | Bool(_) | Str(_) | Align { .. } | Stack | Void => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> ContractInfo {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        load(&path).expect("the artifact loads")
    }

    /// The runtime checks the coin commitment before it runs the
    /// instructions, and that check is not one of them, so the interpreter
    /// must not execute the operation until it does the check itself.
    #[test]
    fn refuses_a_coin_check_operation() {
        let src = r#"(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
          (runtime-version "0.18.107") (exports (stash . %stash.0)) (contract-types)
          (circuit %stash.0 (exported #t) (pure #f) (proof #t) () (ttuple)
            (public-ledger %vault.1 (update-with-coin-check 0 1) (0) writeCoin (ttuple)
              (instructions (ins (cached #t) (n 1))))))"#;
        let e = load_str(src).expect_err("a coin-check operation is refused");
        assert!(
            e.to_string().contains("update-with-coin-check"),
            "the error names the class: {e}"
        );
    }

    #[test]
    fn loads_the_bboard_artifact() {
        let info = fixture("../../../tests/conformance/fixtures/bboard/compiler/analyzed-ir.sexp");
        let names: Vec<_> = info.circuits.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"post") && names.contains(&"public_key"));
        let message = info
            .ledger
            .iter()
            .find(|f| f.name == "message")
            .expect("message");
        let Some(ir::Type::Struct { fields, .. }) = &message.element_type else {
            panic!("message should be a struct-typed cell")
        };
        assert_eq!(fields.len(), 2);
        // public_key is exported and called, so it is also a helper.
        assert!(info.helpers.iter().any(|h| h.name.name() == "public_key"));
    }

    #[test]
    fn loads_the_gateway_artifact() {
        let info = fixture("../../../tests/fixtures/compiled/gateway/compiler/analyzed-ir.sexp");
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
        let ops = fixture("../../../tests/conformance/fixtures/ops/compiler/analyzed-ir.sexp");
        assert!(
            format!("{ops:?}").contains("36893488147419103230"),
            "the sum of two Uint<64> bounds should survive as digits"
        );

        let mint = fixture("../../../tests/fixtures/compiled/mint-probe/compiler/analyzed-ir.sexp");
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
        let info = fixture("../../../tests/fixtures/compiled/mint-probe/compiler/analyzed-ir.sexp");
        let dumped = format!("{info:?}");
        let dup = |n: u8| format!(r#"Instruction {{ op: Dup, args: [("n", Int({n}))] }}"#);
        assert!(dumped.contains(&dup(1)), "mint-probe has an arity-1 dup");
        assert!(dumped.contains(&dup(2)), "mint-probe has an arity-2 dup");
        assert!(
            !dumped.contains(&dup(0)),
            "no dup should lose its arity to the default"
        );
    }
}
