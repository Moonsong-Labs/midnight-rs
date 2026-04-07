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

use compact_codegen::ir::{CircuitIrBody, Expr, HelperDef, LedgerOp, PathEntry, Stmt};

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
pub trait WitnessProvider {
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
) -> Result<ExecutionResult, InterpreterError> {
    execute_with_owned(ir, state.clone(), args, witnesses, helpers)
}

/// Execute a circuit IR body, consuming the contract state to avoid cloning.
///
/// Identical to [`execute_with`] but takes `state` by value.
/// Use this when the caller does not need the original state after execution.
pub fn execute_with_owned(
    ir: &CircuitIrBody,
    state: ContractState<InMemoryDB>,
    args: &[(&str, Value)],
    witnesses: &dyn WitnessProvider,
    helpers: &[HelperDef],
) -> Result<ExecutionResult, InterpreterError> {
    let mut locals = HashMap::new();
    for (name, value) in args {
        locals.insert(name.to_string(), value.clone());
    }

    let helper_map: HashMap<String, &HelperDef> =
        helpers.iter().map(|h| (h.name.clone(), h)).collect();

    let mut ctx = ExecContext {
        state,
        locals,
        reads: Vec::new(),
        gather_ops: Vec::new(),
        witnesses: Some(witnesses),
        helpers: helper_map,
    };

    exec_stmt(&mut ctx, &ir.body)?;

    Ok(ExecutionResult {
        state: ctx.state,
        reads: ctx.reads,
        gather_ops: ctx.gather_ops,
    })
}

/// Execute a circuit IR body against a contract state (no args, no witnesses).
pub fn execute(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
) -> Result<ExecutionResult, InterpreterError> {
    execute_with(ir, state, &[], &NoWitnesses, &[])
}

struct ExecContext<'a> {
    state: ContractState<InMemoryDB>,
    locals: HashMap<String, Value>,
    reads: Vec<AlignedValue>,
    gather_ops: Vec<Op<ResultModeGather, InMemoryDB>>,
    witnesses: Option<&'a dyn WitnessProvider>,
    helpers: HashMap<String, &'a HelperDef>,
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
            let val = eval_expr(ctx, value)?;
            ctx.locals.insert(name.clone(), val);
            Ok(())
        }
        Stmt::ExprStmt { expr } => {
            eval_expr(ctx, expr)?;
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

fn eval_expr(ctx: &mut ExecContext, expr: &Expr) -> Result<Value, InterpreterError> {
    match expr {
        Expr::Var { name } => ctx
            .locals
            .get(name)
            .cloned()
            .ok_or_else(|| InterpreterError::UndefinedVariable(name.clone())),

        Expr::Lit { value, .. } => {
            // Try to parse as integer, fall back to string/void
            if value.is_empty() {
                Ok(Value::Void)
            } else if let Ok(n) = value.parse::<u128>() {
                Ok(Value::Integer(n))
            } else if value == "true" {
                Ok(Value::Bool(true))
            } else if value == "false" {
                Ok(Value::Bool(false))
            } else {
                Ok(Value::Void)
            }
        }

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

        Expr::New { ty: _ } => Ok(Value::Void),

        Expr::Cast { expr, .. } => {
            // Type cast — pass through for now
            eval_expr(ctx, expr)
        }

        Expr::Field { expr, name } => {
            let val = eval_expr(ctx, expr)?;
            match &val {
                Value::Struct(fields) => fields.get(name).cloned().ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "struct has no field '{name}', available: {:?}",
                        fields.keys().collect::<Vec<_>>()
                    ))
                }),
                // Common patterns for non-struct values
                Value::Bool(_) => Ok(val),
                Value::Integer(n) => match name.as_str() {
                    "is_some" => Ok(Value::Bool(*n != 0)),
                    _ => Ok(val),
                },
                Value::AlignedValue(_) => match name.as_str() {
                    "is_some" => Ok(Value::Bool(true)),
                    "value" => Ok(val),
                    _ => Err(InterpreterError::TypeError(format!(
                        "field access .{name} on AlignedValue"
                    ))),
                },
                Value::Void => match name.as_str() {
                    "is_some" => Ok(Value::Bool(false)),
                    _ => Ok(Value::Void),
                },
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
    for (param, val) in helper.params.iter().zip(args.iter()) {
        ctx.locals.insert(param.name.clone(), val.clone());
    }
    exec_stmt(ctx, &helper.body)?;
    let result = if let Some(ref result_expr) = helper.result {
        eval_expr(ctx, result_expr)?
    } else {
        Value::Void
    };
    ctx.locals = saved_locals;
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
                    Value::StateValue(_) | Value::Struct(_) | Value::Tuple(_) => {
                        // Complex values can't be directly hashed via binary_repr.
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
        "disclose" => {
            // disclose(value) — mark value as public (no-op for execution)
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
    match eval_expr(ctx, expr)? {
        Value::Integer(n) => Ok(n),
        Value::Bool(b) => Ok(if b { 1 } else { 0 }),
        other => Err(InterpreterError::TypeError(format!(
            "expected integer, got {other:?}"
        ))),
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
                    .all(|(k, v)| y.get(k).is_some_and(|v2| values_equal(v, v2)))
        }
        _ => false,
    }
}

fn is_truthy(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::Integer(n) => *n != 0,
        Value::Void => false,
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
                        // null means push an empty/void cell
                        StateValue::from(AlignedValue::from(()))
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
                        let val = eval_expr(ctx, &expr)?;
                        val.to_state_value()
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
                        let val = eval_expr(ctx, &expr)?;
                        val.to_state_value()
                    } else {
                        parse_push_value(value)
                    }
                };
                ops.push(Op::Push {
                    storage: *storage,
                    value: sv,
                });
            }
            LedgerOp::Popeq => {
                ops.push(Op::Popeq {
                    cached: false,
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
                let val = eval_expr(ctx, value)?;
                ops.push(Op::Push {
                    storage: true,
                    value: val.to_state_value(),
                });
            }
            LedgerOp::Noop { n } => {
                ops.push(Op::Noop { n: *n });
            }
        }
    }

    // Record the ops for transcript construction
    ctx.gather_ops.extend(ops.iter().cloned());

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
        let tuple = Value::Tuple(inputs.iter().copied().map(|fr| {
            Value::AlignedValue(AlignedValue::from(fr))
        }).collect());
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
