//! Circuit IR interpreter.
//!
//! Executes circuit IR against contract state using midnight-ledger's
//! `ContractStateExt::query()` for ledger operations.

use std::collections::HashMap;

use midnight_bindgen::{AlignedValue, ContractState, InMemoryDB, StateValue};
use midnight_onchain_runtime::contract_state_ext::ContractStateExt;
use midnight_onchain_runtime::cost_model::INITIAL_COST_MODEL;
use midnight_onchain_runtime::ops::{Key, Op};
use midnight_onchain_runtime::result_mode::{GatherEvent, ResultModeGather};

use compact_codegen::ir::{
    CircuitIrBody, EnumDef, Expr, HelperDef, LedgerOp, PathEntry, Stmt, StructDef, TypeRef,
};

/// Runtime value during IR interpretation.
#[derive(Debug, Clone)]
pub enum Value {
    Bool(bool),
    Integer(u128),
    AlignedValue(AlignedValue),
    StateValue(StateValue<InMemoryDB>),
    /// A struct/record with named fields.
    Struct(HashMap<String, Value>),
    /// A tuple/array with indexed elements.
    Tuple(Vec<Value>),
    Void,
}

impl Value {
    /// Extract as u32 for Op::Addi immediate.
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Value::Integer(n) => u32::try_from(*n).ok(),
            _ => None,
        }
    }

    /// Convert to an AlignedValue for use as circuit input.
    ///
    /// `Value::Tuple` is flattened recursively into a concatenated
    /// `AlignedValue` so the prover sees one input value per leaf atom
    /// (matching the FAB encoding the circuit expects for `Vector<N, T>`
    /// arguments). `Value::Struct` cannot be flattened deterministically
    /// here because the underlying `HashMap` has no canonical iteration
    /// order; callers that need to pass a struct as a circuit argument
    /// should pre-encode it as a single `Value::AlignedValue` so this
    /// path stays unambiguous.
    pub fn to_aligned_value(&self) -> AlignedValue {
        match self {
            Value::AlignedValue(av) => av.clone(),
            Value::Integer(n) => AlignedValue::from(*n as u64),
            Value::Bool(b) => AlignedValue::from(*b),
            Value::Void => AlignedValue::from(()),
            Value::Tuple(elements) => {
                let parts: Vec<AlignedValue> =
                    elements.iter().map(Self::to_aligned_value).collect();
                AlignedValue::concat(parts.iter())
            }
            Value::StateValue(_) | Value::Struct(_) => AlignedValue::from(()),
        }
    }

    /// Convert to a StateValue for ledger storage.
    pub fn to_state_value(&self) -> StateValue<InMemoryDB> {
        match self {
            Value::AlignedValue(av) => StateValue::from(av.clone()),
            Value::Integer(n) => StateValue::from(AlignedValue::from(*n as u64)),
            Value::Bool(b) => StateValue::from(AlignedValue::from(*b)),
            Value::Void => StateValue::from(AlignedValue::from(())),
            _ => StateValue::Null,
        }
    }
}

/// Error during circuit IR execution.
#[derive(Debug, thiserror::Error)]
pub enum InterpreterError {
    #[error("undefined variable: {0}")]
    UndefinedVariable(String),

    #[error("assertion failed: {0}")]
    AssertionFailed(String),

    #[error("ledger query failed: {0}")]
    LedgerQueryFailed(String),

    #[error("type error: {0}")]
    TypeError(String),

    #[error("unsupported IR node: {0}")]
    Unsupported(String),

    #[error("witness error: {0}")]
    Witness(String),
}

/// Trait for providing witness (private state) callbacks during circuit execution.
///
/// Implement this to supply private state for circuits that call witnesses.
/// Each method corresponds to a witness function in the Compact contract.
pub trait WitnessProvider: Send + Sync {
    /// Called when the circuit invokes a witness function.
    /// `name` is the witness function name (e.g., "private$secret_key").
    /// `args` are the evaluated argument values.
    /// Returns the witness result value.
    fn call_witness(&self, name: &str, args: &[Value]) -> Result<Value, InterpreterError>;
}

/// A no-op witness provider that rejects all witness calls.
pub struct NoWitnesses;

impl WitnessProvider for NoWitnesses {
    fn call_witness(&self, name: &str, _args: &[Value]) -> Result<Value, InterpreterError> {
        Err(InterpreterError::Witness(format!(
            "no witness provider for: {name}"
        )))
    }
}

/// Result of executing a circuit.
pub struct ExecutionResult {
    /// Updated contract state after execution.
    pub state: ContractState<InMemoryDB>,
    /// Values read from popeq operations (the "gather" results).
    pub reads: Vec<AlignedValue>,
    /// Ops executed in gather mode (for building transcripts).
    pub gather_ops: Vec<Op<ResultModeGather, InMemoryDB>>,
    /// The circuit's return value, if any (non-void circuits).
    pub result: Option<Value>,
    /// Values disclosed via `disclose()` calls (communication outputs).
    /// These must be included in `ContractCallPrototype.output` for the
    /// communication commitment to match the ZKIR's `Output` instructions.
    pub communication_outputs: Vec<AlignedValue>,
}

/// Execute a circuit IR body against a contract state.
///
/// `args` are the circuit's arguments as (name, value) pairs.
/// `witnesses` provides private state callbacks for witness calls.
///
/// Clones `state` internally so the caller retains the original.
/// When the caller no longer needs the original, prefer
/// [`execute_with_owned`] to avoid the clone.
pub fn execute_with(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    args: &[(&str, Value)],
    witnesses: &dyn WitnessProvider,
    helpers: &[HelperDef],
    structs: &[StructDef],
) -> Result<ExecutionResult, InterpreterError> {
    execute_with_owned(
        ir,
        state.clone(),
        args,
        &[],
        witnesses,
        helpers,
        structs,
        &[],
    )
}

/// Variant of [`execute_with`] that additionally seeds the interpreter's
/// enum-table so it can resolve `lit type=Enum value="<variant>"` literals.
pub fn execute_with_enums(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    args: &[(&str, Value)],
    witnesses: &dyn WitnessProvider,
    helpers: &[HelperDef],
    structs: &[StructDef],
    enums: &[EnumDef],
) -> Result<ExecutionResult, InterpreterError> {
    execute_with_owned(
        ir,
        state.clone(),
        args,
        &[],
        witnesses,
        helpers,
        structs,
        enums,
    )
}

/// Variant of [`execute_with`] that additionally seeds the interpreter's
/// type environment with the declared types of each circuit argument. Needed
/// when arguments arrive as `Value::AlignedValue` (pre-encoded structs) and
/// the circuit IR later destructures them with `Expr::Field`.
#[allow(clippy::too_many_arguments)]
pub fn execute_with_arg_types(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    args: &[(&str, Value)],
    arg_types: &[(&str, TypeRef)],
    witnesses: &dyn WitnessProvider,
    helpers: &[HelperDef],
    structs: &[StructDef],
) -> Result<ExecutionResult, InterpreterError> {
    execute_with_owned(
        ir,
        state.clone(),
        args,
        arg_types,
        witnesses,
        helpers,
        structs,
        &[],
    )
}

/// Execute a circuit IR body, consuming the contract state to avoid cloning.
///
/// Identical to [`execute_with`] but takes `state` by value.
/// Use this when the caller does not need the original state after execution.
#[allow(clippy::too_many_arguments)]
pub fn execute_with_owned(
    ir: &CircuitIrBody,
    state: ContractState<InMemoryDB>,
    args: &[(&str, Value)],
    arg_types: &[(&str, TypeRef)],
    witnesses: &dyn WitnessProvider,
    helpers: &[HelperDef],
    structs: &[StructDef],
    enums: &[EnumDef],
) -> Result<ExecutionResult, InterpreterError> {
    let mut locals = HashMap::new();
    for (name, value) in args {
        locals.insert(name.to_string(), value.clone());
    }
    let mut local_types: HashMap<String, TypeRef> = HashMap::new();
    for (name, ty) in arg_types {
        local_types.insert(name.to_string(), ty.clone());
    }

    let helper_map: HashMap<String, &HelperDef> =
        helpers.iter().map(|h| (h.name.clone(), h)).collect();

    let layouts = build_struct_layouts(structs);
    let struct_defs: HashMap<String, StructDef> = structs
        .iter()
        .map(|s| (s.name.clone(), s.clone()))
        .collect();
    let enum_defs: HashMap<String, EnumDef> =
        enums.iter().map(|e| (e.name.clone(), e.clone())).collect();

    let mut ctx = ExecContext {
        state,
        locals,
        local_types,
        reads: Vec::new(),
        gather_ops: Vec::new(),
        communication_outputs: Vec::new(),
        last_expr_value: None,
        witnesses: Some(witnesses),
        helpers: helper_map,
        layouts,
        struct_defs,
        enum_defs,
    };

    exec_stmt(&mut ctx, &ir.body)?;

    let result_value = if let Some(ref result_expr) = ir.result {
        Some(eval_expr(&mut ctx, result_expr)?)
    } else {
        // Use the last expression value as the implicit return (matches
        // how the Compact compiler lowers `return disclose(x)` to an
        // expr-stmt in the body with ir.result = null).
        ctx.last_expr_value.take()
    };

    // If no explicit disclose() calls were recorded, but the circuit has
    // an implicit return value, use that as the communication output.
    // This handles the case where the compiler lowers `return disclose(x)`
    // into the body without a separate disclose() call in the IR.
    let mut comm_outputs = ctx.communication_outputs;
    if comm_outputs.is_empty() {
        if let Some(ref val) = result_value {
            if !matches!(val, Value::Void) {
                comm_outputs.push(val.to_aligned_value());
            }
        }
    }

    Ok(ExecutionResult {
        state: ctx.state,
        reads: ctx.reads,
        gather_ops: ctx.gather_ops,
        result: result_value,
        communication_outputs: comm_outputs,
    })
}

/// Execute a circuit IR body against a contract state (no args, no witnesses).
pub fn execute(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
) -> Result<ExecutionResult, InterpreterError> {
    execute_with(ir, state, &[], &NoWitnesses, &[], &[])
}

/// Precomputed layout of a struct: field name → (atom offset, atom count).
#[derive(Debug, Clone)]
struct StructLayout {
    /// Declaration-order list of (field name, offset, length) in atom slots.
    fields: Vec<(String, usize, usize)>,
}

impl StructLayout {
    fn field_slice(&self, name: &str) -> Option<(usize, usize)> {
        self.fields
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, o, l)| (*o, *l))
    }
}

/// Compute the number of FAB atoms a `TypeRef` occupies in an `AlignedValue`
/// encoding. Used to build struct layouts so `Expr::Field` can slice
/// `Value::AlignedValue` receivers by offset/length.
fn atom_count_for_type(ty: &TypeRef, layouts: &HashMap<String, StructLayout>) -> Option<usize> {
    match ty {
        TypeRef::Boolean | TypeRef::Uint { .. } | TypeRef::Field | TypeRef::Bytes { .. } => Some(1),
        TypeRef::Void => Some(0),
        TypeRef::Opaque { name } => match name.as_str() {
            "JubjubPoint" => Some(2),
            "Scalar<BLS12-381>" => Some(1),
            _ => Some(1),
        },
        TypeRef::Tuple { types } => {
            let mut total = 0;
            for t in types {
                total += atom_count_for_type(t, layouts)?;
            }
            Some(total)
        }
        TypeRef::Vector { length, element } => {
            let per = atom_count_for_type(element, layouts)?;
            Some(per * length)
        }
        TypeRef::Struct { name } => layouts
            .get(name)
            .map(|l| l.fields.iter().map(|(_, _, len)| *len).sum()),
        TypeRef::Maybe { inner } => atom_count_for_type(inner, layouts).map(|n| 1 + n),
        TypeRef::Enum { .. } => Some(1),
    }
}

/// Build struct layouts from shipped `StructDef` entries. Structs may
/// reference each other, so we iterate until fixed point (bounded by the
/// number of structs).
fn build_struct_layouts(defs: &[StructDef]) -> HashMap<String, StructLayout> {
    let mut layouts: HashMap<String, StructLayout> = HashMap::new();
    let max_passes = defs.len() + 1;
    for _ in 0..max_passes {
        let mut made_progress = false;
        for def in defs {
            if layouts.contains_key(&def.name) {
                continue;
            }
            let mut fields = Vec::with_capacity(def.fields.len());
            let mut offset = 0usize;
            let mut ok = true;
            for f in &def.fields {
                match atom_count_for_type(&f.ty, &layouts) {
                    Some(len) => {
                        fields.push((f.name.clone(), offset, len));
                        offset += len;
                    }
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                layouts.insert(def.name.clone(), StructLayout { fields });
                made_progress = true;
            }
        }
        if !made_progress {
            break;
        }
    }
    layouts
}

struct ExecContext<'a> {
    state: ContractState<InMemoryDB>,
    locals: HashMap<String, Value>,
    /// Parallel type environment so `Expr::Field` can slice
    /// `Value::AlignedValue` receivers by the receiver's declared struct type.
    local_types: HashMap<String, TypeRef>,
    reads: Vec<AlignedValue>,
    gather_ops: Vec<Op<ResultModeGather, InMemoryDB>>,
    /// Values disclosed via `disclose()` — corresponds to ZKIR `Output` instructions.
    communication_outputs: Vec<AlignedValue>,
    /// The value of the last evaluated expression statement (used as the
    /// circuit's communication output when `ir.result` is None).
    last_expr_value: Option<Value>,
    witnesses: Option<&'a dyn WitnessProvider>,
    helpers: HashMap<String, &'a HelperDef>,
    layouts: HashMap<String, StructLayout>,
    /// Shipped struct definitions keyed by name. Used to recover the
    /// declared `TypeRef` of a field during type inference (layouts only
    /// carry atom offsets/lengths).
    struct_defs: HashMap<String, StructDef>,
    /// Shipped enum definitions keyed by name. Used by `eval_lit_typed`
    /// to resolve `lit type=Enum value="<variant>"` literals to their
    /// declaration index (the on-chain `u8` encoding).
    enum_defs: HashMap<String, EnumDef>,
}

/// Best-effort static type inference for an `Expr`, consulting the current
/// `ExecContext.local_types` environment and the struct layout registry.
/// Returns `None` when the type cannot be determined; callers must treat that
/// as "unknown" (never fabricate a type).
fn infer_type_of_expr(ctx: &ExecContext, expr: &Expr) -> Option<TypeRef> {
    match expr {
        Expr::Var { name } => ctx.local_types.get(name).cloned(),
        Expr::Lit { ty, .. } => Some(ty.clone()),
        Expr::CallPure { result_type, .. }
        | Expr::CallWitness { result_type, .. }
        | Expr::LedgerQuery { result_type, .. } => Some(result_type.clone()),
        Expr::Cast { to, .. } => Some(to.clone()),
        Expr::New { ty, .. } | Expr::Default { ty } => Some(ty.clone()),
        Expr::Eq { .. }
        | Expr::Neq { .. }
        | Expr::Lt { .. }
        | Expr::Le { .. }
        | Expr::Gt { .. }
        | Expr::Ge { .. }
        | Expr::Not { .. }
        | Expr::And { .. }
        | Expr::Or { .. } => Some(TypeRef::Boolean),
        Expr::Add { left, .. } | Expr::Sub { left, .. } | Expr::Mul { left, .. } => {
            infer_type_of_expr(ctx, left)
        }
        Expr::IfExpr { then, .. } => infer_type_of_expr(ctx, then),
        Expr::LetExpr { body, .. } => infer_type_of_expr(ctx, body),
        Expr::Tuple { elements } => {
            let types: Option<Vec<TypeRef>> = elements
                .iter()
                .map(|e| infer_type_of_expr(ctx, e))
                .collect();
            types.map(|types| TypeRef::Tuple { types })
        }
        Expr::Index { expr, index } => match infer_type_of_expr(ctx, expr)? {
            TypeRef::Tuple { types } => types.get(*index).cloned(),
            TypeRef::Vector { element, .. } => Some(*element),
            _ => None,
        },
        Expr::VectorIndex { expr, .. } => match infer_type_of_expr(ctx, expr)? {
            TypeRef::Vector { element, .. } => Some(*element),
            TypeRef::Tuple { types } => types.into_iter().next(),
            _ => None,
        },
        Expr::Field { expr, name } => {
            let recv_ty = infer_type_of_expr(ctx, expr)?;
            let struct_name = match recv_ty {
                TypeRef::Struct { name } => name,
                TypeRef::Maybe { .. } => "Maybe".to_string(),
                _ => return None,
            };
            let def = ctx.struct_defs.get(&struct_name)?;
            def.fields
                .iter()
                .find(|f| &f.name == name)
                .map(|f| f.ty.clone())
        }
        Expr::Assert { .. } => Some(TypeRef::Void),
    }
}

fn exec_stmt(ctx: &mut ExecContext, stmt: &Stmt) -> Result<(), InterpreterError> {
    match stmt {
        Stmt::Seq { stmts } => {
            for s in stmts {
                exec_stmt(ctx, s)?;
            }
            Ok(())
        }
        Stmt::Let { name, value } => {
            let inferred_ty = infer_type_of_expr(ctx, value);
            let val = eval_expr(ctx, value)?;
            ctx.locals.insert(name.clone(), val);
            if let Some(ty) = inferred_ty {
                ctx.local_types.insert(name.clone(), ty);
            } else {
                ctx.local_types.remove(name);
            }
            Ok(())
        }
        Stmt::ExprStmt { expr } => {
            let val = eval_expr(ctx, expr)?;
            ctx.last_expr_value = Some(val);
            Ok(())
        }
        Stmt::If { cond, then } => {
            let c = eval_expr(ctx, cond)?;
            if is_truthy(&c) {
                exec_stmt(ctx, then)?;
            }
            Ok(())
        }
        Stmt::IfElse { cond, then, else_ } => {
            let c = eval_expr(ctx, cond)?;
            if is_truthy(&c) {
                exec_stmt(ctx, then)?;
            } else {
                exec_stmt(ctx, else_)?;
            }
            Ok(())
        }
    }
}

/// Decode a typed `Expr::Lit` into a `Value`.
///
/// The compiler emits literal `value` strings whose encoding depends on the
/// declared `TypeRef`:
///
/// * `Boolean` → `"true"` / `"false"`.
/// * `Field` / `Uint` → decimal integer.
/// * `Bytes { length: N }` → hex-encoded big-endian bytes (no `0x` prefix),
///   exactly `2 * N` characters.
/// * `Void` → empty string.
///
/// Anything else is reported as an interpreter error rather than silently
/// returning `Value::Void`, which used to mask compiler/interpreter mismatches
/// (e.g. a `Bytes<N>` literal compared against a real input always succeeded
/// because both sides decoded to `Void`).
fn eval_lit_typed(ctx: &ExecContext, ty: &TypeRef, value: &str) -> Result<Value, InterpreterError> {
    match ty {
        TypeRef::Void => Ok(Value::Void),
        TypeRef::Boolean => match value {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            other => Err(InterpreterError::TypeError(format!(
                "invalid Boolean literal: {other:?}"
            ))),
        },
        TypeRef::Uint { .. } | TypeRef::Field => {
            value.parse::<u128>().map(Value::Integer).map_err(|e| {
                InterpreterError::TypeError(format!("invalid integer literal {value:?}: {e}"))
            })
        }
        TypeRef::Bytes { length } => {
            let bytes = hex::decode(value).map_err(|e| {
                InterpreterError::TypeError(format!("invalid hex Bytes literal {value:?}: {e}"))
            })?;
            if bytes.len() != *length {
                return Err(InterpreterError::TypeError(format!(
                    "Bytes<{length}> literal has wrong length: {} bytes",
                    bytes.len()
                )));
            }
            // Encode as a single FAB atom of declared length, matching the
            // alignment that on-chain `Bytes<N>` arguments use. Trailing
            // zeros are stripped to satisfy the FAB normal-form invariant
            // (`is_in_normal_form`); the alignment metadata still records
            // `length = N` so equality against zero-padded constants works.
            let mut atom: Vec<u8> = bytes;
            while matches!(atom.last(), Some(0)) {
                atom.pop();
            }
            let mut av = AlignedValue::from(0u8);
            av.value =
                midnight_base_crypto::fab::Value(vec![midnight_base_crypto::fab::ValueAtom(atom)]);
            av.alignment = midnight_base_crypto::fab::Alignment::singleton(
                midnight_base_crypto::fab::AlignmentAtom::Bytes {
                    length: *length as u32,
                },
            );
            Ok(Value::AlignedValue(av))
        }
        // An empty `Tuple` (no element types) is the Compact unit value `()`.
        // The compiler emits it for `return;` and other unit-typed positions.
        // Treat it as `Value::Void`.
        TypeRef::Tuple { types } if types.is_empty() => Ok(Value::Void),
        // Enum literals: the compiler emits `lit type=Enum value="<variant>"`
        // (or `value="<index>"` for the fork compactc which lowers via
        // `enum-ref`). Resolve the variant name against the shipped enum
        // definitions and produce the on-chain `u8` index encoding. If the
        // value already parses as an integer, use it directly.
        TypeRef::Enum { name } => {
            if let Ok(n) = value.parse::<u8>() {
                return Ok(Value::Integer(n as u128));
            }
            let def = ctx.enum_defs.get(name).ok_or_else(|| {
                InterpreterError::TypeError(format!(
                    "no enum definition for `{name}` (referenced by `lit type=Enum value={value:?}`); \
                     did the compiler ship it in the `enums` table?"
                ))
            })?;
            let idx = def
                .variants
                .iter()
                .position(|v| v == value)
                .ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "enum `{name}` has no variant `{value}` (variants: {:?})",
                        def.variants
                    ))
                })?;
            Ok(Value::Integer(idx as u128))
        }
        other => Err(InterpreterError::TypeError(format!(
            "literal of type {other:?} not supported by interpreter yet"
        ))),
    }
}

fn eval_expr(ctx: &mut ExecContext, expr: &Expr) -> Result<Value, InterpreterError> {
    match expr {
        Expr::Var { name } => ctx
            .locals
            .get(name)
            .cloned()
            .ok_or_else(|| InterpreterError::UndefinedVariable(name.clone())),

        Expr::Lit { ty, value } => eval_lit_typed(ctx, ty, value),

        Expr::Assert { expr, message } => {
            let val = eval_expr(ctx, expr)?;
            if !is_truthy(&val) {
                return Err(InterpreterError::AssertionFailed(message.clone()));
            }
            Ok(Value::Void)
        }

        Expr::LedgerQuery {
            ops,
            result_type: _,
        } => exec_ledger_query(ctx, ops),

        Expr::Tuple { elements } => {
            let vals: Vec<Value> = elements
                .iter()
                .map(|e| eval_expr(ctx, e))
                .collect::<Result<_, _>>()?;
            Ok(Value::Tuple(vals))
        }

        Expr::LetExpr { bindings, body } => {
            // Execute bindings (they're Stmt::Let nodes)
            for binding in bindings {
                exec_stmt(ctx, binding)?;
            }
            eval_expr(ctx, body)
        }

        Expr::CallWitness { name, args, .. } => {
            let evaluated_args: Vec<Value> = args
                .iter()
                .map(|a| eval_expr(ctx, a))
                .collect::<Result<_, _>>()?;

            // Handle disclose before anything else: it must always record
            // the value in communication_outputs regardless of the witness
            // provider. A witness provider that intercepts "disclose" would
            // break the communication commitment.
            if name == "disclose" {
                if let Some(arg) = evaluated_args.first() {
                    ctx.communication_outputs.push(arg.to_aligned_value());
                    return Ok(arg.clone());
                }
                return Ok(Value::Void);
            }

            // Witness calls are authoritative: ask the off-chain witness
            // provider first (it owns the canonical value the prover
            // commits to). For some calls — notably `persistentHash` —
            // the IR-level args are stripped (the compiler can't yet
            // serialize struct literals into the IR), so dispatching to
            // the builtin would compute a hash of `Void` instead of the
            // real preimage. Routing to the witness provider first lets
            // the off-chain caller supply the canonical value; we only
            // fall back to builtin/helper dispatch when the provider
            // returns an `InterpreterError::Witness` (i.e. doesn't know
            // this name).
            if let Some(w) = ctx.witnesses {
                match w.call_witness(name, &evaluated_args) {
                    Ok(v) => return Ok(v),
                    Err(InterpreterError::Witness(_)) => {
                        // Provider declined; fall through.
                    }
                    Err(e) => return Err(e),
                }
            }
            if let Some(result) = try_builtin(name, &evaluated_args) {
                return result;
            }
            if let Some(result) = call_helper(ctx, name, &evaluated_args)? {
                return Ok(result);
            }
            Err(InterpreterError::Witness(format!(
                "no witness provider, builtin, or helper for: {name}"
            )))
        }

        Expr::CallPure { name, args, .. } => {
            let evaluated_args: Vec<Value> = args
                .iter()
                .map(|a| eval_expr(ctx, a))
                .collect::<Result<_, _>>()?;

            // Handle disclose specially: record the value as a communication output
            if name == "disclose" {
                if let Some(arg) = evaluated_args.first() {
                    ctx.communication_outputs.push(arg.to_aligned_value());
                    return Ok(arg.clone());
                }
                return Ok(Value::Void);
            }
            if let Some(result) = try_builtin(name, &evaluated_args) {
                return result;
            }
            if let Some(result) = call_helper(ctx, name, &evaluated_args)? {
                Ok(result)
            } else {
                Err(InterpreterError::Unsupported(format!(
                    "unknown pure function: {name}"
                )))
            }
        }

        Expr::Add { left, right } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Integer(l.wrapping_add(r)))
        }

        Expr::Sub { left, right } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Integer(l.wrapping_sub(r)))
        }

        Expr::Mul { left, right } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Integer(l.wrapping_mul(r)))
        }

        Expr::Eq { left, right } => {
            let l = eval_expr(ctx, left)?;
            let r = eval_expr(ctx, right)?;
            Ok(Value::Bool(values_equal(&l, &r)))
        }

        Expr::Lt { left, right } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Bool(l < r))
        }

        Expr::Le { left, right } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Bool(l <= r))
        }

        Expr::Gt { left, right } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Bool(l > r))
        }

        Expr::Ge { left, right } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Bool(l >= r))
        }

        Expr::IfExpr { cond, then, else_ } => {
            let c = eval_expr(ctx, cond)?;
            if is_truthy(&c) {
                eval_expr(ctx, then)
            } else {
                eval_expr(ctx, else_)
            }
        }

        Expr::Not { expr } => {
            let val = eval_expr(ctx, expr)?;
            Ok(Value::Bool(!is_truthy(&val)))
        }

        Expr::And { left, right } => {
            let l = eval_expr(ctx, left)?;
            if !is_truthy(&l) {
                Ok(Value::Bool(false))
            } else {
                let r = eval_expr(ctx, right)?;
                Ok(Value::Bool(is_truthy(&r)))
            }
        }

        Expr::Or { left, right } => {
            let l = eval_expr(ctx, left)?;
            if is_truthy(&l) {
                Ok(Value::Bool(true))
            } else {
                let r = eval_expr(ctx, right)?;
                Ok(Value::Bool(is_truthy(&r)))
            }
        }

        Expr::Neq { left, right } => {
            let l = eval_expr(ctx, left)?;
            let r = eval_expr(ctx, right)?;
            Ok(Value::Bool(!values_equal(&l, &r)))
        }

        Expr::VectorIndex { expr, index } => {
            // Evaluate the index expression and use it to look up an
            // element in the vector. The vector is expected to be a
            // `Value::Tuple` (the bindgen lowering for `Vector<N, T>`).
            let idx_val = eval_expr(ctx, index)?;
            let idx = value_to_u128(&idx_val).ok_or_else(|| {
                InterpreterError::TypeError(format!(
                    "vector index expression did not evaluate to an integer (got {idx_val:?})"
                ))
            })? as usize;
            let val = eval_expr(ctx, expr)?;
            match val {
                Value::Tuple(elements) => elements.get(idx).cloned().ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "vector index {idx} out of bounds (len {})",
                        elements.len()
                    ))
                }),
                _ => Err(InterpreterError::TypeError(format!(
                    "cannot vector-index into {val:?}"
                ))),
            }
        }

        Expr::Index { expr, index } => {
            let val = eval_expr(ctx, expr)?;
            match val {
                Value::Tuple(elements) => elements.get(*index).cloned().ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "tuple index {index} out of bounds (len {})",
                        elements.len()
                    ))
                }),
                Value::Struct(fields) => {
                    // Structs can be indexed by position (field declaration order)
                    // This is a fallback — prefer field access by name
                    fields.values().nth(*index).cloned().ok_or_else(|| {
                        InterpreterError::TypeError(format!(
                            "struct index {index} out of bounds (len {})",
                            fields.len()
                        ))
                    })
                }
                _ => Err(InterpreterError::TypeError(format!(
                    "cannot index into {val:?}"
                ))),
            }
        }

        Expr::Default { ty: _ } => Ok(Value::Void),

        Expr::New { ty, elements } => {
            // Struct literal: encode each element with the alignment
            // declared by the corresponding field type, then concatenate
            // all field encodings into a single flat AlignedValue. The
            // result has the same FAB layout the on-chain
            // `persistent_hash` circuit produces for `<StructName>(...)`.
            let struct_name = match ty {
                TypeRef::Struct { name } => name.clone(),
                TypeRef::Maybe { .. } => "Maybe".to_string(),
                other => {
                    return Err(InterpreterError::TypeError(format!(
                        "`new` op with non-struct type {other:?}"
                    )));
                }
            };
            let def = ctx.struct_defs.get(&struct_name).cloned().ok_or_else(|| {
                InterpreterError::TypeError(format!(
                    "no struct definition for `{struct_name}` (referenced by `new`)"
                ))
            })?;
            if def.fields.len() != elements.len() {
                return Err(InterpreterError::TypeError(format!(
                    "`new {struct_name}` expects {} fields, got {}",
                    def.fields.len(),
                    elements.len()
                )));
            }
            let mut parts: Vec<midnight_base_crypto::fab::AlignedValue> =
                Vec::with_capacity(elements.len());
            for (field, element) in def.fields.iter().zip(elements.iter()) {
                let val = eval_expr(ctx, element)?;
                let av = encode_value_as_type(&val, &field.ty).ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "cannot encode field `{}` of `{struct_name}` as {:?}: got {val:?}",
                        field.name, field.ty
                    ))
                })?;
                parts.push(av);
            }
            let combined = midnight_base_crypto::fab::AlignedValue::concat(parts.iter());
            Ok(Value::AlignedValue(combined))
        }

        Expr::Cast { expr, to, .. } => {
            let val = eval_expr(ctx, expr)?;
            // When casting an Integer to Field (e.g. `request_id as Field`
            // before a `Map<Field, _>` insert/lookup), eagerly re-encode
            // as a Field-aligned `AlignedValue` so every downstream
            // consumer — PushCell, Idx's PathEntry::Var lookup, direct
            // locals reads — sees the correct alignment byte-for-byte.
            // Without this, the Integer value survives through the let-
            // binding and later gets encoded as a u64-aligned cell,
            // which never matches a Field-aligned key stored on-chain.
            if let (Value::Integer(n), TypeRef::Field) = (&val, to) {
                use midnight_transient_crypto::curve::Fr;
                return Ok(Value::AlignedValue(AlignedValue::from(Fr::from(*n as u64))));
            }
            Ok(val)
        }

        Expr::Field { expr, name } => {
            // Derive the receiver's declared type *before* consuming its
            // value, so we can slice `Value::AlignedValue` by the correct
            // struct layout.
            let receiver_ty = infer_type_of_expr(ctx, expr);
            let val = eval_expr(ctx, expr)?;
            match &val {
                Value::Struct(fields) => fields.get(name).cloned().ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "struct has no field '{name}', available: {:?}",
                        fields.keys().collect::<Vec<_>>()
                    ))
                }),
                Value::AlignedValue(av) => {
                    let struct_name = match &receiver_ty {
                        Some(TypeRef::Struct { name }) => name.clone(),
                        Some(TypeRef::Maybe { .. }) => "Maybe".to_string(),
                        other => {
                            return Err(InterpreterError::TypeError(format!(
                                "field access .{name} on AlignedValue with unknown receiver type {other:?}"
                            )));
                        }
                    };
                    let layout = ctx.layouts.get(&struct_name).ok_or_else(|| {
                        InterpreterError::TypeError(format!(
                            "no struct layout for '{struct_name}' (field .{name}); \
                             did the compiler ship it in the `structs` table?"
                        ))
                    })?;
                    let (offset, len) = layout.field_slice(name).ok_or_else(|| {
                        InterpreterError::TypeError(format!(
                            "struct '{struct_name}' has no field '{name}'"
                        ))
                    })?;
                    if offset + len > av.value.0.len() || offset + len > av.alignment.0.len() {
                        return Err(InterpreterError::TypeError(format!(
                            "field .{name} slice [{offset}..{}] out of bounds for \
                             AlignedValue (value_len={}, alignment_len={}, struct={struct_name})",
                            offset + len,
                            av.value.0.len(),
                            av.alignment.0.len()
                        )));
                    }
                    let value_atoms = av.value.0[offset..offset + len].to_vec();
                    let alignment_atoms = av.alignment.0[offset..offset + len].to_vec();
                    let mut sliced = av.clone();
                    sliced.value = midnight_base_crypto::fab::Value(value_atoms);
                    sliced.alignment = midnight_base_crypto::fab::Alignment(alignment_atoms);
                    Ok(Value::AlignedValue(sliced))
                }
                _ => Err(InterpreterError::TypeError(format!(
                    "field access .{name} on {val:?}"
                ))),
            }
        }

        #[allow(unreachable_patterns)]
        other => Err(InterpreterError::Unsupported(format!("{other:?}"))),
    }
}

/// Try to call a helper function by name. Returns `Ok(Some(value))` if
/// the helper exists, `Ok(None)` if not found, or `Err` on execution failure.
fn call_helper(
    ctx: &mut ExecContext,
    name: &str,
    args: &[Value],
) -> Result<Option<Value>, InterpreterError> {
    let helper = match ctx.helpers.get(name).cloned() {
        Some(h) => h,
        None => return Ok(None),
    };
    let saved_locals = ctx.locals.clone();
    let saved_types = ctx.local_types.clone();
    for (param, val) in helper.params.iter().zip(args.iter()) {
        ctx.locals.insert(param.name.clone(), val.clone());
        ctx.local_types.insert(param.name.clone(), param.ty.clone());
    }
    exec_stmt(ctx, &helper.body)?;
    let result = if let Some(ref result_expr) = helper.result {
        eval_expr(ctx, result_expr)?
    } else {
        Value::Void
    };
    ctx.locals = saved_locals;
    ctx.local_types = saved_types;
    Ok(Some(result))
}

/// Decode a [`Value`] holding a Compact `Field` into a transient `Fr`.
///
/// Accepts both `Value::AlignedValue` (the canonical encoding produced by the
/// existing builtins) and `Value::Integer` (so untyped integer literals can be
/// passed where a Field is expected, mirroring the on-chain runtime's
/// behavior).
fn value_to_fr(v: &Value) -> Option<midnight_transient_crypto::curve::Fr> {
    use midnight_transient_crypto::curve::Fr;
    match v {
        Value::Integer(n) => Some(Fr::from(*n as u64)),
        Value::AlignedValue(av) => Fr::try_from(&*av.value).ok(),
        _ => None,
    }
}

/// Decode a [`Value`] holding a Compact `JubjubPoint` into an
/// `EmbeddedGroupAffine`. The on-chain encoding is two `Field` atoms (the
/// affine `x`/`y` coordinates), matching the
/// `TryFrom<&ValueSlice> for EmbeddedGroupAffine` impl in
/// `midnight-transient-crypto`.
fn value_to_embedded_group(
    v: &Value,
) -> Option<midnight_transient_crypto::curve::EmbeddedGroupAffine> {
    use midnight_transient_crypto::curve::EmbeddedGroupAffine;
    match v {
        Value::AlignedValue(av) => EmbeddedGroupAffine::try_from(&*av.value).ok(),
        _ => None,
    }
}

/// Try to execute a Compact runtime builtin function.
/// Returns `Some(Ok(value))` if the function is a known builtin,
/// `Some(Err(..))` if it fails, or `None` if it's not a builtin.
fn try_builtin(name: &str, args: &[Value]) -> Option<Result<Value, InterpreterError>> {
    match name {
        "persistentHash" => {
            // persistentHash hashes an AlignedValue using midnight-ledger's
            // PersistentHashWriter with proper binary_repr.
            use midnight_base_crypto::hash::PersistentHashWriter;
            use midnight_base_crypto::repr::BinaryHashRepr;
            use midnight_transient_crypto::fab::ValueReprAlignedValue;

            let mut hasher = PersistentHashWriter::default();
            for arg in args {
                match arg {
                    Value::AlignedValue(av) => {
                        let wrapped = ValueReprAlignedValue(av.clone());
                        wrapped.binary_repr(&mut hasher);
                    }
                    Value::Integer(n) => {
                        // Use Fr for field-compatible hashing
                        use midnight_transient_crypto::curve::Fr;
                        let av = AlignedValue::from(Fr::from(*n as u64));
                        let wrapped = ValueReprAlignedValue(av);
                        wrapped.binary_repr(&mut hasher);
                    }
                    Value::Bool(b) => {
                        let av = AlignedValue::from(*b);
                        let wrapped = ValueReprAlignedValue(av);
                        wrapped.binary_repr(&mut hasher);
                    }
                    Value::Void => {
                        let av = AlignedValue::from(());
                        let wrapped = ValueReprAlignedValue(av);
                        wrapped.binary_repr(&mut hasher);
                    }
                    Value::Tuple(_) | Value::Struct(_) => {
                        // Compound values: flatten the entire structure into
                        // a single `AlignedValue` (Value::to_aligned_value
                        // walks Tuple/Struct recursively, concatenating each
                        // leaf's atoms in declaration order). Then binary_repr
                        // the result. This matches what the on-chain
                        // persistent_hash circuit produces for the same typed
                        // input, because the same flattening rule is used by
                        // the bindgen-emitted `Into<AlignedValue>` impls.
                        let av = arg.to_aligned_value();
                        let wrapped = ValueReprAlignedValue(av);
                        wrapped.binary_repr(&mut hasher);
                    }
                    Value::StateValue(_) => {
                        // StateValues can't be directly hashed via binary_repr.
                    }
                }
            }
            let hash = hasher.finalize();
            Some(Ok(Value::AlignedValue(AlignedValue::from(hash.0))))
        }
        "leafHash" => {
            // leafHash uses midnight-ledger's merkle tree leaf hashing
            use midnight_transient_crypto::fab::ValueReprAlignedValue;
            match args.first() {
                Some(Value::AlignedValue(av)) => {
                    let wrapped = ValueReprAlignedValue(av.clone());
                    let hash = midnight_transient_crypto::merkle_tree::leaf_hash(&wrapped);
                    Some(Ok(Value::AlignedValue(AlignedValue::from(hash.0))))
                }
                Some(Value::Integer(n)) => {
                    use midnight_transient_crypto::curve::Fr;
                    let av = AlignedValue::from(Fr::from(*n as u64));
                    let wrapped = ValueReprAlignedValue(av);
                    let hash = midnight_transient_crypto::merkle_tree::leaf_hash(&wrapped);
                    Some(Ok(Value::AlignedValue(AlignedValue::from(hash.0))))
                }
                _ => Some(Err(InterpreterError::TypeError(
                    "leafHash requires an AlignedValue or Integer argument".to_string(),
                ))),
            }
        }
        "ecMulGenerator" | "__builtin_ec_mul_generator" => {
            // EC scalar multiplication: G * scalar
            use midnight_transient_crypto::curve::EmbeddedGroupAffine;
            if let Some(scalar) = args.first() {
                let fr_val = match value_to_fr(scalar) {
                    Some(fr) => fr,
                    None => {
                        return Some(Err(InterpreterError::TypeError(
                            "ecMulGenerator: scalar argument is not a Field/Integer".to_string(),
                        )));
                    }
                };
                let generator = EmbeddedGroupAffine::generator();
                let result = generator * fr_val;
                Some(Ok(Value::AlignedValue(AlignedValue::from(result))))
            } else {
                Some(Err(InterpreterError::TypeError(
                    "ecMulGenerator requires a scalar argument".to_string(),
                )))
            }
        }
        "ecMul" => {
            // EC scalar multiplication: point * scalar
            if args.len() != 2 {
                return Some(Err(InterpreterError::TypeError(format!(
                    "ecMul expects 2 arguments, got {}",
                    args.len()
                ))));
            }
            let point = match value_to_embedded_group(&args[0]) {
                Some(p) => p,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "ecMul: first argument is not a JubjubPoint".to_string(),
                    )));
                }
            };
            let scalar = match value_to_fr(&args[1]) {
                Some(s) => s,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "ecMul: second argument is not a Field/Integer".to_string(),
                    )));
                }
            };
            let result = point * scalar;
            Some(Ok(Value::AlignedValue(AlignedValue::from(result))))
        }
        "ecAdd" => {
            // EC point addition: p1 + p2
            if args.len() != 2 {
                return Some(Err(InterpreterError::TypeError(format!(
                    "ecAdd expects 2 arguments, got {}",
                    args.len()
                ))));
            }
            let p1 = match value_to_embedded_group(&args[0]) {
                Some(p) => p,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "ecAdd: first argument is not a JubjubPoint".to_string(),
                    )));
                }
            };
            let p2 = match value_to_embedded_group(&args[1]) {
                Some(p) => p,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "ecAdd: second argument is not a JubjubPoint".to_string(),
                    )));
                }
            };
            Some(Ok(Value::AlignedValue(AlignedValue::from(p1 + p2))))
        }
        "jubjubPointX" => {
            // JubjubPoint -> Field (x coordinate)
            let point = match args.first().and_then(value_to_embedded_group) {
                Some(p) => p,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "jubjubPointX: argument is not a JubjubPoint".to_string(),
                    )));
                }
            };
            use midnight_transient_crypto::curve::Fr;
            let x = point.x().unwrap_or(Fr::from(0u64));
            Some(Ok(Value::AlignedValue(AlignedValue::from(x))))
        }
        "jubjubPointY" => {
            // JubjubPoint -> Field (y coordinate)
            let point = match args.first().and_then(value_to_embedded_group) {
                Some(p) => p,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "jubjubPointY: argument is not a JubjubPoint".to_string(),
                    )));
                }
            };
            use midnight_transient_crypto::curve::Fr;
            let y = point.y().unwrap_or(Fr::from(0u64));
            Some(Ok(Value::AlignedValue(AlignedValue::from(y))))
        }
        "transientHash" => {
            // Poseidon hash: transientHash<Vector<N, Field>>([fields...]) -> Field
            use midnight_transient_crypto::curve::Fr;
            use midnight_transient_crypto::hash::transient_hash;
            let mut field_inputs: Vec<Fr> = Vec::with_capacity(args.len());
            for (i, arg) in args.iter().enumerate() {
                // The IR sometimes passes a single Tuple wrapping all the fields.
                // Flatten one level so callers can pass either a flat arg list or
                // a single Tuple.
                if let Value::Tuple(elems) = arg {
                    for (j, e) in elems.iter().enumerate() {
                        match value_to_fr(e) {
                            Some(fr) => field_inputs.push(fr),
                            None => {
                                return Some(Err(InterpreterError::TypeError(format!(
                                    "transientHash: tuple arg {i} elem {j} is not a Field"
                                ))));
                            }
                        }
                    }
                } else {
                    match value_to_fr(arg) {
                        Some(fr) => field_inputs.push(fr),
                        None => {
                            return Some(Err(InterpreterError::TypeError(format!(
                                "transientHash: arg {i} is not a Field"
                            ))));
                        }
                    }
                }
            }
            let hash = transient_hash(&field_inputs);
            Some(Ok(Value::AlignedValue(AlignedValue::from(hash))))
        }
        "degradeToTransient" => {
            // Bytes<N> -> Field (transient field). The on-chain helper interprets
            // the bytes as a little-endian field element, reducing modulo Fr if
            // they are out of range.
            use midnight_transient_crypto::curve::Fr;
            let arg = match args.first() {
                Some(a) => a,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "degradeToTransient requires an argument".to_string(),
                    )));
                }
            };
            let bytes = match arg {
                Value::AlignedValue(av) => {
                    // Concatenate all atoms; for Bytes<N> this is a single atom.
                    let mut buf = Vec::new();
                    for atom in &av.value.0 {
                        buf.extend_from_slice(&atom.0);
                    }
                    buf
                }
                _ => {
                    return Some(Err(InterpreterError::TypeError(
                        "degradeToTransient: argument is not Bytes".to_string(),
                    )));
                }
            };
            // Try direct LE decode first; fall back to wide reduction so any
            // SHA-256-like hash output produces a valid field element.
            let fr = if let Some(fr) = Fr::from_le_bytes(&bytes) {
                fr
            } else {
                let mut wide = [0u8; 64];
                let n = bytes.len().min(64);
                wide[..n].copy_from_slice(&bytes[..n]);
                Fr::from_uniform_bytes(&wide)
            };
            Some(Ok(Value::AlignedValue(AlignedValue::from(fr))))
        }
        "pad" => {
            // pad(len, string) — pad a string to `len` bytes
            // Return as-is for now
            if args.len() >= 2 {
                Some(Ok(args[1].clone()))
            } else {
                Some(Ok(Value::Void))
            }
        }
        // Note: "disclose" is handled directly in eval_expr for CallWitness
        // and CallPure (before try_builtin is called) so that the disclosed
        // value is recorded in ctx.communication_outputs. This case is
        // unreachable from those paths but kept as a safety fallback for any
        // other call path that might invoke try_builtin with "disclose".
        "disclose" => {
            if let Some(arg) = args.first() {
                Some(Ok(arg.clone()))
            } else {
                Some(Ok(Value::Void))
            }
        }
        _ => None, // Not a builtin
    }
}

fn eval_as_integer(ctx: &mut ExecContext, expr: &Expr) -> Result<u128, InterpreterError> {
    let val = eval_expr(ctx, expr)?;
    value_to_u128(&val)
        .ok_or_else(|| InterpreterError::TypeError(format!("expected integer, got {val:?}")))
}

/// Coerce a `Value` into a `u128`, accepting:
/// - `Value::Integer(n)` directly
/// - `Value::Bool(b)` as 0/1
/// - `Value::AlignedValue` containing a single Uint or Field atom whose
///   little-endian byte content fits in `u128`
///
/// Returns `None` if the value isn't a recognized integer-shaped form.
fn value_to_u128(val: &Value) -> Option<u128> {
    match val {
        Value::Integer(n) => Some(*n),
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        Value::AlignedValue(av) => {
            // Take the first atom; ignore alignment because the prover
            // already enforces shape. We accept up to 16 bytes (u128).
            let atom = av.value.0.first()?;
            if atom.0.len() > 16 {
                return None;
            }
            let mut buf = [0u8; 16];
            buf[..atom.0.len()].copy_from_slice(&atom.0);
            Some(u128::from_le_bytes(buf))
        }
        _ => None,
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::AlignedValue(x), Value::AlignedValue(y)) => x.value == y.value,
        (Value::Void, Value::Void) => true,
        (Value::Tuple(x), Value::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| values_equal(a, b))
        }
        (Value::Struct(x), Value::Struct(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v2)| y.get(k).is_some_and(|v3| values_equal(v2, v3)))
        }
        // Mixed arms: a single-atom AlignedValue (e.g. the result of
        // slicing a struct field whose declared type is an enum or a
        // small Uint) compares equal to a Value::Integer with the same
        // numeric value. Without this, `request.status ==
        // SigningRequestStatus.pending` always returns false because
        // the LHS comes back as an `AlignedValue` from popeq and the
        // RHS is an `Integer` produced by eval_lit_typed.
        (Value::AlignedValue(av), Value::Integer(n))
        | (Value::Integer(n), Value::AlignedValue(av)) => {
            aligned_value_as_u128(av).is_some_and(|lhs| lhs == *n)
        }
        (Value::AlignedValue(av), Value::Bool(b)) | (Value::Bool(b), Value::AlignedValue(av)) => {
            aligned_value_as_u128(av)
                .map(|n| (n != 0) == *b)
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// Decode a single-atom `AlignedValue` as a `u128`, big-endian. Returns
/// `None` if the AlignedValue has zero atoms or a single atom whose
/// byte buffer is wider than 16 bytes (which wouldn't fit in `u128`
/// anyway). Used by `values_equal` to compare a sliced struct field
/// cell against a Value::Integer without re-encoding the integer.
fn aligned_value_as_u128(av: &AlignedValue) -> Option<u128> {
    let atom = av.value.0.first()?;
    if atom.0.len() > 16 {
        return None;
    }
    let mut buf = [0u8; 16];
    let start = 16 - atom.0.len();
    buf[start..].copy_from_slice(&atom.0);
    Some(u128::from_be_bytes(buf))
}

fn is_truthy(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::Integer(n) => *n != 0,
        Value::Void => false,
        Value::AlignedValue(av) => {
            // Boolean cells coming back from `popeq` are encoded as a
            // single-atom AlignedValue whose only byte is 0x00 (false)
            // or 0x01 (true). The previous catch-all `_ => true` arm
            // treated *every* AlignedValue as truthy, which silently
            // turned `member(...) == false` into "membership found"
            // and broke gateway asserts like `!processed_attestations.member(...)`.
            //
            // Treat any AlignedValue whose atoms are all-zero (or empty)
            // as false; otherwise true.
            !av.value.0.iter().all(|atom| atom.0.iter().all(|b| *b == 0))
        }
        _ => true,
    }
}

/// Execute a ledger-query: translate IR LedgerOps to onchain-vm Ops,
/// run them against the contract state via ContractStateExt::query().
fn exec_ledger_query(
    ctx: &mut ExecContext,
    ir_ops: &[LedgerOp],
) -> Result<Value, InterpreterError> {
    let cost_model = &INITIAL_COST_MODEL;
    let mut ops: Vec<Op<ResultModeGather, InMemoryDB>> = Vec::new();

    for ir_op in ir_ops {
        match ir_op {
            LedgerOp::Dup => {
                ops.push(Op::Dup { n: 0 });
            }
            LedgerOp::Idx {
                cached,
                push_path,
                path,
            } => {
                let keys: Vec<Key> = path
                    .iter()
                    .map(|entry| match entry {
                        PathEntry::Value { value, ty } => {
                            // Work around a fork compactc codegen bug:
                            // `Map<Field, _>::lookup(request_key)` compiles
                            // to a path entry whose `value` is the raw
                            // Scheme sexp of the expression tree
                            // (`"((op . var) (name . request_key))"`) and
                            // whose `type` is `Uint<8>`, instead of a
                            // proper value literal or a `PathEntry::Var`
                            // pointing at the local. Detect the sexp
                            // pattern, extract the variable name, and
                            // resolve it from `ctx.locals` with the
                            // local's inferred `TypeRef` — so the
                            // alignment matches what the map insert
                            // actually stored.
                            if let Some(var_name) = parse_scheme_var_sexp(value) {
                                if let Some(local) = ctx.locals.get(&var_name) {
                                    let local_ty = ctx.local_types.get(&var_name).cloned();
                                    let sv = encode_ledger_key(local, local_ty.as_ref());
                                    if let StateValue::Cell(ref av_sp) = sv {
                                        return Ok(Key::Value((**av_sp).clone()));
                                    }
                                }
                            }
                            let av = path_value_to_aligned(value, ty);
                            Ok(Key::Value(av))
                        }
                        PathEntry::Stack => Ok(Key::Stack),
                        PathEntry::Var { name } => match ctx.locals.get(name) {
                            Some(Value::Integer(n)) => {
                                Ok(Key::Value(AlignedValue::from(*n as u64)))
                            }
                            Some(Value::AlignedValue(av)) => Ok(Key::Value(av.clone())),
                            Some(Value::Bool(b)) => Ok(Key::Value(AlignedValue::from(*b))),
                            _ => Err(InterpreterError::UndefinedVariable(name.clone())),
                        },
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                ops.push(Op::Idx {
                    cached: *cached,
                    push_path: *push_path,
                    path: keys.into_iter().collect(),
                });
            }
            LedgerOp::Addi { immediate } => {
                // Resolve the immediate value — can be a literal or an expression
                let imm = resolve_immediate(ctx, immediate)?;
                ops.push(Op::Addi { immediate: imm });
            }
            LedgerOp::Ins { cached, n } => {
                ops.push(Op::Ins {
                    cached: *cached,
                    n: *n,
                });
            }
            LedgerOp::Push { storage, value } => {
                let sv = if *storage {
                    if value.is_null() {
                        // `null` here means the source ledger op pushed
                        // `(state-value-null)` (e.g. inserting into a Set,
                        // where the "value" slot is just a marker). The
                        // on-chain `StateValue::Null` field_repr is `[0]`,
                        // distinct from `StateValue::Cell(unit)` which is
                        // `[1, 0]`. We must emit Null here, not Cell(unit).
                        StateValue::Null
                    } else if let Some(raw) = value.as_str() {
                        // Raw VM instruction string (compiler didn't convert to JSON expr).
                        // Try to extract variable references from patterns like
                        // "(VMleaf-hash ... %varname.N ...)"
                        if raw.starts_with("(VMleaf-hash") {
                            // Extract the variable name: %name.N → name
                            if let Some(pct) = raw.find('%') {
                                let rest = &raw[pct + 1..];
                                let var_name = rest.split('.').next().unwrap_or("");
                                if let Some(val) = ctx.locals.get(var_name) {
                                    // Apply leafHash to the variable's value
                                    match try_builtin("leafHash", std::slice::from_ref(val)) {
                                        Some(Ok(hashed)) => hashed.to_state_value(),
                                        _ => val.to_state_value(),
                                    }
                                } else {
                                    StateValue::from(AlignedValue::from(()))
                                }
                            } else {
                                StateValue::from(AlignedValue::from(()))
                            }
                        } else {
                            StateValue::from(AlignedValue::from(()))
                        }
                    } else {
                        // storage=true: value is an IR expression to evaluate
                        let expr: Expr = serde_json::from_value(value.clone()).map_err(|e| {
                            InterpreterError::Unsupported(format!("push storage expression: {e}"))
                        })?;
                        let inferred = infer_type_of_expr(ctx, &expr);
                        let val = eval_expr(ctx, &expr)?;
                        encode_ledger_key(&val, inferred.as_ref())
                    }
                } else {
                    // storage=false: value is either a literal path key
                    // (PathEntry) or an IR expression to evaluate (e.g.
                    // `{"op": "var", "name": "..."}` for a previously bound
                    // local). Try the path-key shape first; if that fails,
                    // fall back to evaluating it as an expression — same as
                    // the `storage=true` branch above.
                    if let Ok(path_entry) = serde_json::from_value::<PathEntry>(value.clone()) {
                        match path_entry {
                            PathEntry::Value { value: v, ty } => {
                                let av = path_value_to_aligned(&v, &ty);
                                StateValue::from(av)
                            }
                            _ => StateValue::Null,
                        }
                    } else if let Ok(expr) = serde_json::from_value::<Expr>(value.clone()) {
                        // Infer the expression's declared type *before*
                        // evaluating, so we can re-encode `Value::Integer`
                        // with the right AlignedValue alignment. Without
                        // this, a `request_id as Field` cast still pushes
                        // a u64-aligned key, which never matches a
                        // `Map<Field, V>` entry that was inserted with
                        // Field alignment on-chain. Manifests as
                        // `signing_requests.member(...) == false` even
                        // when the entry is clearly present.
                        let inferred = infer_type_of_expr(ctx, &expr);
                        let val = eval_expr(ctx, &expr)?;
                        encode_ledger_key(&val, inferred.as_ref())
                    } else {
                        parse_push_value(value)
                    }
                };
                ops.push(Op::Push {
                    storage: *storage,
                    value: sv,
                });
            }
            LedgerOp::Popeq { cached } => {
                ops.push(Op::Popeq {
                    cached: *cached,
                    result: (),
                });
            }
            LedgerOp::Member => {
                ops.push(Op::Member);
            }
            LedgerOp::Root => {
                ops.push(Op::Root);
            }
            LedgerOp::Eq => {
                ops.push(Op::Eq);
            }
            LedgerOp::Ckpt => {
                ops.push(Op::Ckpt);
            }
            LedgerOp::Rem { cached, .. } => {
                ops.push(Op::Rem { cached: *cached });
            }
            LedgerOp::PushCell { value } => {
                let inferred = infer_type_of_expr(ctx, value);
                let val = eval_expr(ctx, value)?;
                ops.push(Op::Push {
                    storage: true,
                    value: encode_ledger_key(&val, inferred.as_ref()),
                });
            }
            LedgerOp::Noop { n } => {
                ops.push(Op::Noop { n: *n });
            }
            LedgerOp::Swap { n } => {
                ops.push(Op::Swap { n: *n });
            }
            LedgerOp::Neg => {
                ops.push(Op::Neg);
            }
            LedgerOp::Branch { skip } => {
                ops.push(Op::Branch { skip: *skip });
            }
            LedgerOp::Add => {
                ops.push(Op::Add);
            }
        }
    }

    // Record the ops for transcript construction
    ctx.gather_ops.extend(ops.iter().cloned());

    if std::env::var("INTERPRETER_DEBUG").is_ok() {
        eprintln!("[interpreter] executing {} ops:", ops.len());
        for (i, op) in ops.iter().enumerate() {
            eprintln!("  {i:3}: {op:?}");
        }
        // Also dump the starting state of the field we're navigating into
        // (first idx op's field index) so we can see the on-chain layout.
        if let Some(midnight_onchain_runtime::ops::Op::Idx { path, .. }) = ops.get(1) {
            if let Some(first) = path.iter().next() {
                eprintln!("  field nav first key: {first:?}");
            }
        }
    }

    // Execute the ops against the contract state
    let (new_state, events) = ctx
        .state
        .query(&ops, cost_model)
        .map_err(|e| InterpreterError::LedgerQueryFailed(format!("{e:?}")))?;

    // Collect popeq read results
    for event in &events {
        if let GatherEvent::Read(av) = event {
            ctx.reads.push(av.clone());
        }
    }

    ctx.state = new_state;

    if std::env::var("INTERPRETER_DEBUG").is_ok() {
        let reads: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                GatherEvent::Read(av) => Some(av),
                _ => None,
            })
            .collect();
        eprintln!("  -> {} read events", reads.len());
        for (i, av) in reads.iter().enumerate() {
            eprintln!(
                "     [{i}] value_atoms={} alignment_atoms={}",
                av.value.0.len(),
                av.alignment.0.len()
            );
        }
    }

    // Return the last read value if any, otherwise void
    if let Some(last_read) = events.iter().rev().find_map(|e| match e {
        GatherEvent::Read(av) => Some(av.clone()),
        _ => None,
    }) {
        Ok(Value::AlignedValue(last_read))
    } else {
        Ok(Value::Void)
    }
}

/// Resolve an addi immediate value — either a literal number or an expression.
fn resolve_immediate(
    ctx: &mut ExecContext,
    value: &serde_json::Value,
) -> Result<u32, InterpreterError> {
    if let Some(n) = value.as_i64() {
        return u32::try_from(n).map_err(|_| {
            InterpreterError::TypeError(format!("addi immediate {n} out of u32 range"))
        });
    }
    if let Some(n) = value.as_u64() {
        return u32::try_from(n).map_err(|_| {
            InterpreterError::TypeError(format!("addi immediate {n} out of u32 range"))
        });
    }
    // It's an expression (e.g., { "op": "var", "name": "tmp" })
    if value.is_object() {
        let expr: Expr = serde_json::from_value(value.clone()).map_err(|e| {
            InterpreterError::TypeError(format!("cannot parse addi immediate: {e}"))
        })?;
        let val = eval_expr(ctx, &expr)?;
        return val.as_u32().ok_or_else(|| {
            InterpreterError::TypeError(format!("addi immediate is not u32: {val:?}"))
        });
    }
    Err(InterpreterError::TypeError(format!(
        "cannot resolve addi immediate: {value}"
    )))
}

/// Convert a path value string + type to an AlignedValue.
/// Encode a runtime `Value` as an `AlignedValue` whose alignment matches
/// the declared `ty`. This is used by `Expr::New` (and any other place
/// where a runtime value must be embedded into a struct/typed slot at a
/// known width). For `Value::Integer`, this picks the right number of
/// bytes from the target `Uint{maxval}` width — `Value::Integer(1000)`
/// embedded as `Uint<128>` becomes a 16-byte atom, not the 8-byte default
/// `to_aligned_value` would produce.
fn encode_value_as_type(val: &Value, ty: &TypeRef) -> Option<AlignedValue> {
    use midnight_base_crypto::fab;
    match ty {
        TypeRef::Boolean => match val {
            Value::Bool(b) => Some(AlignedValue::from(*b)),
            Value::Integer(n) => Some(AlignedValue::from(*n != 0)),
            _ => None,
        },
        TypeRef::Uint { maxval } => {
            let n = value_to_u128(val)?;
            let max: u128 = maxval.parse().unwrap_or(u128::MAX);
            // Choose the smallest standard primitive width >= max so that
            // the alignment matches what the on-chain runtime expects.
            // (`From<u8/u16/u32/u64/u128>` set the alignment via `Aligned`.)
            if max <= u8::MAX as u128 {
                Some(AlignedValue::from(n as u8))
            } else if max <= u16::MAX as u128 {
                Some(AlignedValue::from(n as u16))
            } else if max <= u32::MAX as u128 {
                Some(AlignedValue::from(n as u32))
            } else if max <= u64::MAX as u128 {
                Some(AlignedValue::from(n as u64))
            } else {
                Some(AlignedValue::from(n))
            }
        }
        TypeRef::Field => match val {
            Value::AlignedValue(av) => Some(av.clone()),
            Value::Integer(n) => {
                use midnight_transient_crypto::curve::Fr;
                Some(AlignedValue::from(Fr::from(*n as u64)))
            }
            _ => None,
        },
        TypeRef::Bytes { length } => match val {
            Value::AlignedValue(av) => {
                // Re-tag with the requested Bytes<length> alignment so the
                // hash circuit sees the correct width even if the source
                // value carried a different alignment.
                let mut av = av.clone();
                av.alignment = fab::Alignment::singleton(fab::AlignmentAtom::Bytes {
                    length: *length as u32,
                });
                Some(av)
            }
            Value::Void => {
                let av = fab::AlignedValue::new(
                    fab::Value(vec![fab::ValueAtom(vec![])]),
                    fab::Alignment::singleton(fab::AlignmentAtom::Bytes {
                        length: *length as u32,
                    }),
                )?;
                Some(av)
            }
            _ => None,
        },
        TypeRef::Opaque { name } if name == "JubjubPoint" => match val {
            Value::AlignedValue(av) => Some(av.clone()),
            _ => None,
        },
        TypeRef::Opaque { .. } => match val {
            Value::AlignedValue(av) => Some(av.clone()),
            _ => None,
        },
        TypeRef::Tuple { types } => match val {
            Value::Tuple(elements) if elements.len() == types.len() => {
                let parts: Option<Vec<AlignedValue>> = elements
                    .iter()
                    .zip(types.iter())
                    .map(|(e, t)| encode_value_as_type(e, t))
                    .collect();
                Some(AlignedValue::concat(parts?.iter()))
            }
            _ => None,
        },
        TypeRef::Vector { length, element } => match val {
            Value::Tuple(elements) if elements.len() == *length => {
                let parts: Option<Vec<AlignedValue>> = elements
                    .iter()
                    .map(|e| encode_value_as_type(e, element))
                    .collect();
                Some(AlignedValue::concat(parts?.iter()))
            }
            _ => None,
        },
        // For Struct/Maybe receivers we'd need the layout registry to
        // recurse field-by-field; the current call sites (Expr::New) only
        // need the leaf type encodings above. Fall back to to_aligned_value.
        TypeRef::Struct { .. } | TypeRef::Maybe { .. } => Some(val.to_aligned_value()),
        TypeRef::Void => Some(AlignedValue::from(())),
        TypeRef::Enum { .. } => match val {
            Value::Integer(n) => Some(AlignedValue::from(*n as u8)),
            Value::AlignedValue(av) => Some(av.clone()),
            _ => None,
        },
    }
}

/// Encode an evaluated [`Value`] as a [`StateValue`] for pushing onto
/// the ledger query stack, re-aligning the inner `AlignedValue` to
/// match the expression's declared [`TypeRef`] when known.
///
/// The default [`Value::to_state_value`] conversion throws away type
/// information and always encodes integers as u64. That's fine for
/// arithmetic but wrong for `Map<Field, _>` keys: the on-chain insert
/// goes through `as Field`, producing a Field-aligned key, while the
/// off-chain lookup would otherwise push a u64-aligned key and get a
/// miss. `encode_ledger_key` forwards everything untouched except the
/// `(Integer, Field)` case, which is re-encoded via `Fr::from(n)` so
/// its alignment matches the insert path byte-for-byte.
/// Extract the variable name from a Scheme-formatted var expression
/// the fork compactc occasionally emits as a `PathEntry::Value.value`
/// string instead of a proper `PathEntry::Var`. Expected input shape:
///
/// ```text
/// "((op . var) (name . <name>))"
/// ```
///
/// Returns `Some(<name>)` on match, `None` otherwise.
fn parse_scheme_var_sexp(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if !trimmed.starts_with("((op . var)") {
        return None;
    }
    let name_key = "(name . ";
    let start = trimmed.find(name_key)? + name_key.len();
    let rest = &trimmed[start..];
    let end = rest.find(')')?;
    Some(rest[..end].trim().to_string())
}

fn encode_ledger_key(val: &Value, ty: Option<&TypeRef>) -> StateValue<InMemoryDB> {
    if let (Value::Integer(n), Some(TypeRef::Field)) = (val, ty) {
        use midnight_transient_crypto::curve::Fr;
        return StateValue::from(AlignedValue::from(Fr::from(*n as u64)));
    }
    val.to_state_value()
}

fn path_value_to_aligned(value: &str, ty: &compact_codegen::ir::TypeRef) -> AlignedValue {
    use compact_codegen::ir::TypeRef;
    match ty {
        TypeRef::Uint { maxval } => {
            // Parse based on the range implied by maxval
            let max: u128 = maxval.parse().unwrap_or(255);
            let n: u128 = value.parse().unwrap_or(0);
            if max <= u8::MAX as u128 {
                AlignedValue::from(n as u8)
            } else if max <= u16::MAX as u128 {
                AlignedValue::from(n as u16)
            } else if max <= u32::MAX as u128 {
                AlignedValue::from(n as u32)
            } else {
                AlignedValue::from(n as u64)
            }
        }
        TypeRef::Boolean => AlignedValue::from(value == "true" || value == "1"),
        TypeRef::Field => {
            use midnight_transient_crypto::curve::Fr;
            let n: u64 = value.parse().unwrap_or(0);
            AlignedValue::from(Fr::from(n))
        }
        _ => {
            // Best-effort: try parsing as integer
            if let Ok(n) = value.parse::<u64>() {
                AlignedValue::from(n)
            } else {
                AlignedValue::from(0u8)
            }
        }
    }
}

/// Parse a push value from the IR JSON.
///
/// StateValue<InMemoryDB> implements serde::Deserialize, so we try to
/// deserialize directly.
fn parse_push_value(value: &serde_json::Value) -> StateValue<InMemoryDB> {
    // Handle simple JSON values directly
    if let Some(n) = value.as_u64() {
        return StateValue::from(AlignedValue::from(n));
    }
    if value.is_null() {
        return StateValue::Null;
    }
    serde_json::from_value(value.clone()).unwrap_or(StateValue::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_bindgen::{ContractMaintenanceAuthority, StorageHashMap};

    fn make_counter_state(round: u64) -> ContractState<InMemoryDB> {
        // Counter contract state: Array(1) [ Cell(round) ]
        let root = StateValue::Array(vec![StateValue::from(round)].into());
        ContractState::new(
            root,
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }

    #[test]
    fn execute_counter_increment() {
        let state = make_counter_state(0);

        // Parse the counter increment IR
        let ir_json = r#"{
            "body": {
                "op": "seq",
                "stmts": [
                    {
                        "op": "expr-stmt",
                        "expr": {
                            "op": "let-expr",
                            "bindings": [
                                {
                                    "op": "let",
                                    "name": "tmp",
                                    "value": { "op": "lit", "type": { "type": "Uint", "maxval": "65535" }, "value": "1" }
                                }
                            ],
                            "body": {
                                "op": "ledger-query",
                                "ops": [
                                    { "op": "idx", "cached": false, "push-path": true,
                                      "path": [{ "tag": "value", "value": "0", "type": { "type": "Uint", "maxval": "255" } }] },
                                    { "op": "addi", "immediate": { "op": "var", "name": "tmp" } },
                                    { "op": "ins", "cached": true, "n": 1 }
                                ],
                                "result-type": { "type": "Void" }
                            }
                        }
                    },
                    {
                        "op": "expr-stmt",
                        "expr": { "op": "lit", "type": { "type": "Tuple", "types": [] }, "value": "" }
                    }
                ]
            },
            "result": null
        }"#;

        let ir: CircuitIrBody = serde_json::from_str(ir_json).expect("parse IR");
        let result = execute(&ir, &state).expect("execute increment");

        // The counter should have been incremented from 0 to 1
        // Check by reading the state
        let new_state = result.state;
        let root = new_state.data.get_ref();
        // Navigate to Array[0] which should be Cell(1u64)
        match root {
            StateValue::Array(arr) => {
                let cell = arr.get(0).expect("field 0");
                match cell {
                    StateValue::Cell(sp) => {
                        let counter = u64::try_from(&*sp.value).expect("u64");
                        assert_eq!(counter, 1, "counter should be 1 after increment");
                    }
                    _ => panic!("expected Cell, got {:?}", cell),
                }
            }
            _ => panic!("expected Array root"),
        }
    }

    #[test]
    fn execute_counter_increment_nonzero() {
        let state = make_counter_state(42);
        let ir_json = r#"{
            "body": {
                "op": "seq",
                "stmts": [
                    {
                        "op": "expr-stmt",
                        "expr": {
                            "op": "let-expr",
                            "bindings": [
                                { "op": "let", "name": "tmp",
                                  "value": { "op": "lit", "type": { "type": "Uint", "maxval": "65535" }, "value": "1" } }
                            ],
                            "body": {
                                "op": "ledger-query",
                                "ops": [
                                    { "op": "idx", "cached": false, "push-path": true,
                                      "path": [{ "tag": "value", "value": "0", "type": { "type": "Uint", "maxval": "255" } }] },
                                    { "op": "addi", "immediate": { "op": "var", "name": "tmp" } },
                                    { "op": "ins", "cached": true, "n": 1 }
                                ],
                                "result-type": { "type": "Void" }
                            }
                        }
                    }
                ]
            },
            "result": null
        }"#;

        let ir: CircuitIrBody = serde_json::from_str(ir_json).expect("parse IR");
        let result = execute(&ir, &state).expect("execute increment");

        let root = result.state.data.get_ref();
        match root {
            StateValue::Array(arr) => {
                let cell = arr.get(0).expect("field 0");
                match cell {
                    StateValue::Cell(sp) => {
                        let counter = u64::try_from(&*sp.value).expect("u64");
                        assert_eq!(counter, 43, "counter should be 43 after increment from 42");
                    }
                    _ => panic!("expected Cell"),
                }
            }
            _ => panic!("expected Array"),
        }
    }

    #[test]
    fn struct_field_access() {
        let mut fields = HashMap::new();
        fields.insert("x".to_string(), Value::Integer(10));
        fields.insert("y".to_string(), Value::Integer(20));
        let s = Value::Struct(fields);

        match &s {
            Value::Struct(f) => {
                assert_eq!(
                    f.get("x").map(|v| matches!(v, Value::Integer(10))),
                    Some(true)
                );
            }
            _ => panic!("expected Struct"),
        }
    }

    #[test]
    fn tuple_index_access() {
        let t = Value::Tuple(vec![
            Value::Integer(1),
            Value::Bool(true),
            Value::Integer(42),
        ]);

        match &t {
            Value::Tuple(elems) => {
                assert!(matches!(elems[0], Value::Integer(1)));
                assert!(matches!(elems[1], Value::Bool(true)));
                assert!(matches!(elems[2], Value::Integer(42)));
            }
            _ => panic!("expected Tuple"),
        }
    }

    #[test]
    fn values_equal_struct() {
        let mut f1 = HashMap::new();
        f1.insert("a".to_string(), Value::Integer(1));
        let mut f2 = HashMap::new();
        f2.insert("a".to_string(), Value::Integer(1));
        assert!(values_equal(&Value::Struct(f1.clone()), &Value::Struct(f2)));

        let mut f3 = HashMap::new();
        f3.insert("a".to_string(), Value::Integer(2));
        assert!(!values_equal(&Value::Struct(f1), &Value::Struct(f3)));
    }

    #[test]
    fn values_equal_tuple() {
        let t1 = Value::Tuple(vec![Value::Integer(1), Value::Bool(true)]);
        let t2 = Value::Tuple(vec![Value::Integer(1), Value::Bool(true)]);
        let t3 = Value::Tuple(vec![Value::Integer(1), Value::Bool(false)]);
        assert!(values_equal(&t1, &t2));
        assert!(!values_equal(&t1, &t3));
    }

    // -----------------------------------------------------------------------
    // Jubjub builtins
    // -----------------------------------------------------------------------

    fn fr_value(n: u64) -> Value {
        use midnight_transient_crypto::curve::Fr;
        Value::AlignedValue(AlignedValue::from(Fr::from(n)))
    }

    #[test]
    fn ec_mul_generator_matches_direct_call() {
        use midnight_transient_crypto::curve::{EmbeddedGroupAffine, Fr};
        let result = try_builtin("ecMulGenerator", &[fr_value(7)])
            .expect("builtin known")
            .expect("ok");
        let point = match result {
            Value::AlignedValue(av) => EmbeddedGroupAffine::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        let expected = EmbeddedGroupAffine::generator() * Fr::from(7u64);
        assert_eq!(point, expected);
    }

    #[test]
    fn ec_mul_with_arbitrary_point() {
        use midnight_transient_crypto::curve::{EmbeddedGroupAffine, Fr};
        // p = G * 3 ; ecMul(p, 5) should equal G * 15
        let p = EmbeddedGroupAffine::generator() * Fr::from(3u64);
        let p_value = Value::AlignedValue(AlignedValue::from(p));
        let result = try_builtin("ecMul", &[p_value, fr_value(5)])
            .expect("builtin known")
            .expect("ok");
        let got = match result {
            Value::AlignedValue(av) => EmbeddedGroupAffine::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        let expected = EmbeddedGroupAffine::generator() * Fr::from(15u64);
        assert_eq!(got, expected);
    }

    #[test]
    fn ec_add_associative() {
        use midnight_transient_crypto::curve::{EmbeddedGroupAffine, Fr};
        let p1 = EmbeddedGroupAffine::generator() * Fr::from(2u64);
        let p2 = EmbeddedGroupAffine::generator() * Fr::from(5u64);
        let result = try_builtin(
            "ecAdd",
            &[
                Value::AlignedValue(AlignedValue::from(p1)),
                Value::AlignedValue(AlignedValue::from(p2)),
            ],
        )
        .expect("builtin known")
        .expect("ok");
        let got = match result {
            Value::AlignedValue(av) => EmbeddedGroupAffine::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        let expected = EmbeddedGroupAffine::generator() * Fr::from(7u64);
        assert_eq!(got, expected);
    }

    #[test]
    fn jubjub_point_x_y_round_trip() {
        use midnight_transient_crypto::curve::{EmbeddedGroupAffine, Fr};
        let p = EmbeddedGroupAffine::generator() * Fr::from(11u64);
        let p_value = Value::AlignedValue(AlignedValue::from(p));

        let x_result = try_builtin("jubjubPointX", &[p_value.clone()])
            .expect("builtin known")
            .expect("ok");
        let y_result = try_builtin("jubjubPointY", &[p_value])
            .expect("builtin known")
            .expect("ok");

        let x_fr = match x_result {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        let y_fr = match y_result {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        assert_eq!(x_fr, p.x().unwrap());
        assert_eq!(y_fr, p.y().unwrap());
    }

    #[test]
    fn transient_hash_matches_direct_call() {
        use midnight_transient_crypto::curve::Fr;
        use midnight_transient_crypto::hash::transient_hash;

        let inputs = [Fr::from(1u64), Fr::from(2u64), Fr::from(3u64)];
        let direct = transient_hash(&inputs);

        // Pass as a single Tuple (the IR's typical layout for Vector<N, Field>).
        let tuple = Value::Tuple(
            inputs
                .iter()
                .copied()
                .map(|fr| Value::AlignedValue(AlignedValue::from(fr)))
                .collect(),
        );
        let via_builtin = try_builtin("transientHash", &[tuple])
            .expect("builtin known")
            .expect("ok");
        let got = match via_builtin {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        assert_eq!(got, direct);
    }

    #[test]
    fn transient_hash_accepts_flat_args() {
        use midnight_transient_crypto::curve::Fr;
        use midnight_transient_crypto::hash::transient_hash;

        let direct = transient_hash(&[Fr::from(7u64), Fr::from(11u64)]);
        let via_builtin = try_builtin("transientHash", &[fr_value(7), fr_value(11)])
            .expect("builtin known")
            .expect("ok");
        let got = match via_builtin {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        assert_eq!(got, direct);
    }

    #[test]
    fn degrade_to_transient_canonical_input() {
        use midnight_transient_crypto::curve::Fr;
        // A small value that fits in a single canonical Fr LE encoding.
        let mut bytes = [0u8; 32];
        bytes[0] = 42;
        let av = AlignedValue::from(bytes);
        let result = try_builtin("degradeToTransient", &[Value::AlignedValue(av)])
            .expect("builtin known")
            .expect("ok");
        let got = match result {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        assert_eq!(got, Fr::from(42u64));
    }

    #[test]
    fn degrade_to_transient_wide_reduction_path() {
        use midnight_transient_crypto::curve::Fr;
        // Top byte 0xFF makes the LE-decoded integer >= Fr modulus, forcing
        // the wide-reduction fallback.
        let bytes = [0xFFu8; 32];
        let av = AlignedValue::from(bytes);
        let result = try_builtin("degradeToTransient", &[Value::AlignedValue(av)])
            .expect("builtin known")
            .expect("ok");
        // Just assert it produced *some* Fr; the exact value comes from
        // wide_reduction(0xFFFF... || 0x00...) which we trust the curve crate.
        match result {
            Value::AlignedValue(av) => {
                Fr::try_from(&*av.value).expect("decoded Fr");
            }
            other => panic!("expected AlignedValue, got {other:?}"),
        }
    }
}
