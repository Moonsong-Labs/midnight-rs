//! Circuit interpreter.
//!
//! Executes a circuit's analyzed-IR body ([`ir::Expr`]) against contract
//! state, using midnight-ledger's VM (`QueryContext::query`) for ledger
//! operations.

use std::collections::HashMap;

use midnight_onchain_runtime::context::QueryContext;
use midnight_onchain_runtime::cost_model::INITIAL_COST_MODEL;
use midnight_onchain_runtime::ops::{Key, Op};
use midnight_onchain_runtime::result_mode::{GatherEvent, ResultModeGather};
use midnight_typed_state::{AlignedValue, ContractState, InMemoryDB, StateValue};

use compact_codegen::ir::{self, Type};
use num_bigint::{BigInt, BigUint};

// Runtime primitives used by the tree-walk. Public callers reach these
// through `midnight_contract::runtime` (see lib.rs), not this module.
use compact_runtime::{
    CircuitZswapInput, CircuitZswapOutput, ExecutionResult, InterpreterError, NoWitnesses, Value,
    WitnessContext, WitnessNative, WitnessOutcome, WitnessProvider, integer_fallback_aligned,
};
// Value/builtin helpers used internally by the tree-walk (arithmetic,
// equality, encoding, builtin dispatch). Not re-exported: unlike the types
// above, generated code does not reference these by path.
use compact_runtime::{
    aligned_atom_to_u128, atom_count_for_type, bytes_aligned_value, encode_typed,
    layout_from_fields, try_builtin_typed, value_to_fr, value_to_u128,
};

/// Everything a circuit body needs from its program.
///
/// A body reaches outside itself only through `call`: to another circuit, to
/// a witness, or to a native (a runtime builtin). These three tables are how
/// such a name resolves.
pub struct Program<'a> {
    /// Every circuit in the artifact, indexed by its full identifier text
    /// (`ir::Ident.0`), which is unique by construction.
    pub circuits: HashMap<&'a str, &'a ir::Circuit>,
    /// Witness declarations, indexed by source name.
    pub witnesses: HashMap<&'a str, &'a ir::Witness>,
    /// Native declarations, indexed by source name.
    pub natives: HashMap<&'a str, &'a ir::Native>,
}

impl<'a> Program<'a> {
    pub fn new(
        circuits: &'a [ir::Circuit],
        witnesses: &'a [ir::Witness],
        natives: &'a [ir::Native],
    ) -> Self {
        Self {
            circuits: circuits.iter().map(|c| (c.name.0.as_str(), c)).collect(),
            witnesses: witnesses.iter().map(|w| (w.name.name(), w)).collect(),
            natives: natives.iter().map(|n| (n.name.name(), n)).collect(),
        }
    }

    /// The circuit with this identifier, if the program declares one.
    pub fn circuit(&self, id: &str) -> Option<&'a ir::Circuit> {
        self.circuits.get(id).copied()
    }

    /// What a `call` name resolves to.
    ///
    /// A circuit wins over everything else: a circuit that shadows a native's
    /// name is a distinct identifier, and the call site names it directly.
    fn callee(&self, id: &ir::Ident) -> Callee<'a> {
        if let Some(c) = self.circuits.get(id.0.as_str()).copied() {
            return Callee::Circuit(c);
        }
        match self.natives.get(id.name()) {
            Some(n) if n.class == "witness" => Callee::Witness,
            Some(_) => Callee::Pure,
            None if self.witnesses.contains_key(id.name()) => Callee::Witness,
            None => Callee::Pure,
        }
    }

    /// The callee's declared result type, which the call site's static type is.
    fn result_type(&self, id: &ir::Ident) -> Option<&'a Type> {
        if let Some(c) = self.circuits.get(id.0.as_str()).copied() {
            return Some(&c.result_type);
        }
        if let Some(n) = self.natives.get(id.name()).copied() {
            return Some(&n.result_type);
        }
        self.witnesses
            .get(id.name())
            .copied()
            .map(|w| &w.result_type)
    }
}

/// What a `call` name resolves to.
enum Callee<'a> {
    /// A circuit in this program: its body runs in place.
    Circuit(&'a ir::Circuit),
    /// A witness declaration, or a native declared in the witness class: the
    /// witness provider answers it and its value joins the private transcript.
    Witness,
    /// Anything else, which is a native: a runtime builtin computed in place.
    Pure,
}

/// Execute a circuit against a contract state.
///
/// `args` are the circuit's arguments as (name, value) pairs, keyed by the
/// argument's source name. `witnesses` provides private state callbacks for
/// witness calls.
///
/// Clones `state` internally so the caller retains the original.
/// When the caller no longer needs the original, prefer
/// [`execute_with_owned`] to avoid the clone.
pub fn execute_with(
    circuit: &ir::Circuit,
    program: &Program<'_>,
    state: &ContractState<InMemoryDB>,
    args: &[(&str, Value)],
    witnesses: &dyn WitnessProvider,
) -> Result<ExecutionResult, InterpreterError> {
    execute_with_owned(circuit, program, state.clone(), args, witnesses, None, None)
}

/// Execute a circuit, consuming the contract state to avoid cloning.
///
/// Identical to [`execute_with`] but takes `state` by value.
/// Use this when the caller does not need the original state after execution.
#[allow(clippy::too_many_arguments)]
pub fn execute_with_owned(
    circuit: &ir::Circuit,
    program: &Program<'_>,
    state: ContractState<InMemoryDB>,
    args: &[(&str, Value)],
    witnesses: &dyn WitnessProvider,
    witness_ctx: Option<&mut WitnessContext<'_>>,
    contract_address: Option<midnight_coin_structure::contract::ContractAddress>,
) -> Result<ExecutionResult, InterpreterError> {
    // The threading hook is the private-state buffer carried by `WitnessContext`.
    // If the caller supplied one, witness mutations land in the caller's buffer
    // and the post-call state is visible after this returns. If not, witnesses
    // mutate a `scratch` buffer whose contents are discarded when this returns
    // — witnesses still run either way (they take `&dyn WitnessProvider`
    // separately from the threading context).
    let mut scratch = Vec::new();
    let private_state: &mut Vec<u8> = match witness_ctx {
        Some(ctx) => ctx.private_state_mut(),
        None => &mut scratch,
    };

    // Arguments arrive keyed by their source name; the body refers to them by
    // their full identifier, which is what binds them.
    let mut locals = HashMap::new();
    let mut local_types: HashMap<String, Type> = HashMap::new();
    for argument in &circuit.arguments {
        check_type(&argument.ty)?;
        local_types.insert(argument.name.0.clone(), argument.ty.clone());
        if let Some((_, value)) = args.iter().find(|(name, _)| *name == argument.name.name()) {
            locals.insert(argument.name.0.clone(), value.clone());
        }
    }
    check_type(&circuit.result_type)?;

    let mut ctx = ExecContext {
        state,
        locals,
        local_types,
        reads: Vec::new(),
        gather_ops: Vec::new(),
        communication_outputs: Vec::new(),
        private_transcript_outputs: Vec::new(),
        zswap_outputs: Vec::new(),
        zswap_inputs: Vec::new(),
        witnesses: Some(witnesses),
        private_state,
        program,
        contract_address,
    };

    // The circuit's value is its body's value (the compiler lowers `return
    // disclose(x)` into the body's final expression).
    let result_value = Some(eval_expr(&mut ctx, &circuit.body)?);

    // If no explicit disclose() calls were recorded, but the circuit has
    // an implicit return value, use that as the communication output.
    // This handles the case where the compiler lowers `return disclose(x)`
    // into the body without a separate disclose() call.
    //
    // The encoding must match the circuit's declared result type: the
    // canonical runtime encodes the output through the result descriptor, so
    // a `Field`-returning circuit binds a field-aligned output even when the
    // value is small.
    let mut comm_outputs = ctx.communication_outputs;
    if comm_outputs.is_empty() {
        if let Some(ref val) = result_value {
            if !matches!(val, Value::Void) {
                comm_outputs.push(encode_typed(val, &circuit.result_type)?);
            }
        }
    }

    Ok(ExecutionResult {
        state: ctx.state,
        reads: ctx.reads,
        gather_ops: ctx.gather_ops,
        result: result_value,
        communication_outputs: comm_outputs,
        private_transcript_outputs: ctx.private_transcript_outputs,
        zswap_outputs: ctx.zswap_outputs,
        zswap_inputs: ctx.zswap_inputs,
    })
}

/// Context-aware execution used by the funded call path.
///
/// Threads the contract's loaded private state (via `ctx`) through every witness
/// call so a stateful witness can read and update it. After this returns, `ctx`'s
/// private-state buffer holds the post-call state, ready to persist.
#[allow(clippy::too_many_arguments)]
pub fn execute_with_context(
    circuit: &ir::Circuit,
    program: &Program<'_>,
    state: &ContractState<InMemoryDB>,
    args: &[(&str, Value)],
    ctx: &mut WitnessContext<'_>,
    witnesses: &dyn WitnessProvider,
) -> Result<ExecutionResult, InterpreterError> {
    execute_with_owned(
        circuit,
        program,
        state.clone(),
        args,
        witnesses,
        Some(ctx),
        None,
    )
}

/// Execute a circuit against a contract state (no args, no witnesses).
pub fn execute(
    circuit: &ir::Circuit,
    program: &Program<'_>,
    state: &ContractState<InMemoryDB>,
) -> Result<ExecutionResult, InterpreterError> {
    execute_with(circuit, program, state, &[], &NoWitnesses)
}

/// Refuse a type the interpreter cannot execute, naming it.
///
/// The compiler's language is wider than what runs off-chain: a foreign-curve
/// field or point has no value representation here, and an ADT or a type
/// variable is not a value type at all. Refusing them by name beats reading
/// one as if it were a native field.
fn check_type(ty: &Type) -> Result<(), InterpreterError> {
    let unsupported = |what: String| Err(InterpreterError::Unsupported(what));
    match ty {
        Type::Field(field_type) if !is_native_field_type(field_type) => {
            unsupported("a secp256k1 field type".to_string())
        }
        Type::Point(ir::Curve::Secp256k1) => unsupported("a secp256k1 point type".to_string()),
        Type::Adt { name, .. } => unsupported(format!("ADT type {name} in a value position")),
        Type::TypeVar(v) => unsupported(format!("type variable {v}")),
        Type::Vector { ty, .. } | Type::Alias { ty, .. } => check_type(ty),
        Type::Tuple(types) => types.iter().try_for_each(check_type),
        Type::Struct { fields, .. } => fields.iter().try_for_each(|(_, t)| check_type(t)),
        _ => Ok(()),
    }
}

/// Whether values of this field compute in the native scalar field. The
/// compiler distinguishes the Jubjub scalar field from the native one, but
/// they share a modulus, so both compute identically.
fn is_native_field_type(field_type: &ir::FieldType) -> bool {
    matches!(
        field_type,
        ir::FieldType::Native | ir::FieldType::Scalar(ir::Curve::Jubjub)
    )
}

/// The Compact `default<T>` value at its declared type.
///
/// The canonical runtime materializes defaults through the type's descriptor
/// (`CompactType*.toValue` of the zero value), so the FAB alignment is the
/// type's own: `default<Bytes<32>>` is an empty atom aligned `Bytes {32}`,
/// not the unit value. Only leaf and composite types with obvious zero
/// values are covered; anything else is an explicit error rather than a
/// silently misaligned encoding.
fn default_value(ty: &Type) -> Result<Value, InterpreterError> {
    use midnight_base_crypto::fab;
    match ty {
        Type::Boolean => Ok(Value::Bool(false)),
        Type::Unsigned(_) | Type::Enum { .. } => Ok(Value::Integer(0)),
        Type::Field(_) => Ok(Value::AlignedValue(AlignedValue::from(
            midnight_transient_crypto::curve::Fr::from(0u64),
        ))),
        Type::Bytes(length) => Ok(Value::AlignedValue(bytes_aligned_value(
            Vec::new(),
            ir_length(*length)?,
        )?)),
        // A curve point takes the opaque default: the interpreter carries an
        // EC value as an opaque atom.
        Type::Opaque(_) | Type::Point(_) => fab::AlignedValue::new(
            fab::Value(vec![fab::ValueAtom(Vec::new())]),
            fab::Alignment::singleton(fab::AlignmentAtom::Compress),
        )
        .map(Value::AlignedValue)
        .ok_or_else(|| {
            InterpreterError::TypeError("empty opaque default is unrepresentable".into())
        }),
        // Mirrors `ir::Expr::New`: each field's default encoded at its
        // declared type, concatenated into the struct's flat FAB encoding.
        Type::Struct { name, fields } => {
            let mut parts = Vec::with_capacity(fields.len());
            for (field_name, field_ty) in fields {
                let val = default_value(field_ty)?;
                let av = encode_typed(&val, field_ty).map_err(|e| {
                    InterpreterError::TypeError(format!(
                        "cannot encode default field `{field_name}` of `{name}`: {e}"
                    ))
                })?;
                parts.push(av);
            }
            Ok(Value::AlignedValue(fab::AlignedValue::concat(parts.iter())))
        }
        Type::Tuple(types) if types.is_empty() => Ok(Value::Void),
        Type::Tuple(types) => Ok(Value::Tuple(
            types
                .iter()
                .map(default_value)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Type::Vector { len, ty: element } => Ok(Value::Tuple(
            std::iter::repeat_with(|| default_value(element))
                .take(ir_length(*len)?)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        other => Err(InterpreterError::Unsupported(format!(
            "default<{other:?}> not supported by interpreter yet"
        ))),
    }
}

/// FAB-encode a circuit's argument list into the single input value the
/// prover binds (`ContractCallPrototype::input`).
///
/// Each argument is encoded at its declared type's width when `arg_types`
/// carries an entry for it: the canonical runtime routes arguments through
/// per-type descriptors, so a `Uint<32>` argument is a 4-byte atom even
/// though the interpreter's width-preserving fallback would pick 8 bytes.
/// Arguments without a declared type keep the fallback encoding.
pub fn encode_circuit_input(
    args: &[(&str, Value)],
    arg_types: &[(&str, Type)],
) -> Result<AlignedValue, InterpreterError> {
    if args.is_empty() {
        return Ok(AlignedValue::from(()));
    }
    let parts: Vec<AlignedValue> = args
        .iter()
        .map(
            |(name, value)| match arg_types.iter().find(|(n, _)| n == name) {
                Some((_, ty)) => encode_typed(value, ty),
                None => value.try_to_aligned_value(),
            },
        )
        .collect::<Result<_, _>>()?;
    Ok(AlignedValue::concat(parts.iter()))
}

struct ExecContext<'a> {
    state: ContractState<InMemoryDB>,
    /// Bound values, keyed by the binder's full identifier text.
    locals: HashMap<String, Value>,
    /// Parallel type environment so `ir::Expr::EltRef` can slice
    /// `Value::AlignedValue` receivers by the receiver's declared struct type.
    local_types: HashMap<String, Type>,
    reads: Vec<AlignedValue>,
    gather_ops: Vec<Op<ResultModeGather, InMemoryDB>>,
    /// Values disclosed via `disclose()` — corresponds to ZKIR `Output` instructions.
    communication_outputs: Vec<AlignedValue>,
    /// Witness return values in call order — the prover's private transcript
    /// outputs (ZKIR private inputs). Empty for witness-free circuits.
    private_transcript_outputs: Vec<AlignedValue>,
    /// Coins the circuit asked to create via `createZswapOutput`, in call
    /// order. Surfaced on `ExecutionResult` for the call/deploy path.
    zswap_outputs: Vec<CircuitZswapOutput>,
    /// Coins the circuit asked to spend via `createZswapInput`, in call order.
    /// Surfaced on `ExecutionResult` for the call/deploy path.
    zswap_inputs: Vec<CircuitZswapInput>,
    witnesses: Option<&'a dyn WitnessProvider>,
    /// Mutable private-state buffer threaded through witness calls.
    private_state: &'a mut Vec<u8>,
    /// The declarations a `call` resolves against.
    program: &'a Program<'a>,
    /// The address of the contract being executed, when known. Used to resolve
    /// `kernel.self()`: in the lowered circuit that reads the contract's own
    /// address from the VM **context** (`dup{n:2} idx[0] popeq`), but the
    /// portable IR drops the `dup` arity and the interpreter has no real
    /// context, so the read is resolved directly from this field. Required by
    /// contracts that mint shielded tokens (the coin color is
    /// `tokenType(domain_sep, self())`); `None` for paths that never call
    /// `kernel.self()`.
    contract_address: Option<midnight_coin_structure::contract::ContractAddress>,
}

/// Best-effort static type inference for an expression, consulting the current
/// `ExecContext.local_types` environment and the struct layout registry.
/// Returns `None` when the type cannot be determined; callers must treat that
/// as "unknown" (never fabricate a type).
///
/// This is deliberately narrower than the types the artifact carries: an
/// arithmetic node is typed as its left operand rather than as the widened
/// result, and an integer literal as a field element, because those are the
/// widths the encodings in flight were produced with.
fn infer_type_of_expr(ctx: &ExecContext, expr: &ir::Expr) -> Option<Type> {
    use ir::Expr as E;
    match expr {
        E::VarRef(id) => ctx.local_types.get(id.0.as_str()).cloned(),
        E::Quote(ir::Literal::Bool(_)) => Some(Type::Boolean),
        E::Quote(ir::Literal::Int(_)) => Some(Type::Field(ir::FieldType::Native)),
        E::Quote(ir::Literal::Bytes(bytes)) => Some(Type::Bytes(bytes.len() as u64)),
        E::Call { name, .. } => ctx.program.result_type(name).cloned(),
        E::PublicLedger { result_type, .. } => Some(result_type.clone()),
        E::New { ty, .. } | E::Default(ty) => Some(ty.clone()),
        E::Eq { .. }
        | E::Neq { .. }
        | E::Lt { .. }
        | E::Le { .. }
        | E::Gt { .. }
        | E::Ge { .. } => Some(Type::Boolean),
        E::Add { left, .. } | E::Sub { left, .. } | E::Mul { left, .. } => {
            infer_type_of_expr(ctx, left)
        }
        E::If { then, .. } => infer_type_of_expr(ctx, then),
        E::LetStar { body, .. } => infer_type_of_expr(ctx, body),
        E::Seq(items) => infer_type_of_expr(ctx, items.last()?),
        E::Return(inner) => infer_type_of_expr(ctx, inner),
        E::Tuple(args) | E::VectorLit(args) => {
            let mut types = Vec::with_capacity(args.len());
            for arg in args {
                match arg {
                    ir::TupleArg::Single(e) => types.push(infer_type_of_expr(ctx, e)?),
                    // A spread element contributes the element types of its
                    // (vector/tuple-typed) inner expression, `len` of them.
                    ir::TupleArg::Spread { len, expr } => match infer_type_of_expr(ctx, expr)? {
                        Type::Tuple(inner) if inner.len() as u64 == *len => {
                            types.extend(inner);
                        }
                        Type::Vector { len: n, ty } if n == *len => {
                            types.extend(std::iter::repeat_n(*ty, usize::try_from(n).ok()?));
                        }
                        _ => return None,
                    },
                }
            }
            Some(Type::Tuple(types))
        }
        E::TupleRef { expr, index } => match infer_type_of_expr(ctx, expr)? {
            Type::Tuple(types) => types.get(usize::try_from(*index).ok()?).cloned(),
            Type::Vector { ty, .. } => Some(*ty),
            _ => None,
        },
        E::VectorRef { expr, .. } => match infer_type_of_expr(ctx, expr)? {
            Type::Vector { ty, .. } => Some(*ty),
            Type::Tuple(types) => types.into_iter().next(),
            _ => None,
        },
        E::EltRef { expr, elt, .. } => {
            let recv_ty = infer_type_of_expr(ctx, expr)?;
            let Type::Struct { fields, .. } = recv_ty else {
                return None;
            };
            fields
                .iter()
                .find(|(n, _)| n == elt)
                .map(|(_, t)| t.clone())
        }
        E::Assert { .. } => Some(Type::unit()),
        // Conversion forms have statically known result types
        // (circuit-passes.ss types bytes->field as Field, field->bytes /
        // vector->bytes as Bytes<len>, bytes->vector as Vector<len, Uint<255>>).
        E::FieldToBytes { len, .. } | E::VectorToBytes { len, .. } => Some(Type::Bytes(*len)),
        E::BytesToVector { len, .. } => Some(Type::Vector {
            len: *len,
            ty: Box::new(byte_type()),
        }),
        E::CastFromBytes { ty, .. }
        | E::CastFromEnum { ty, .. }
        | E::CastToEnum { ty, .. }
        | E::SafeCast { ty, .. } => Some(ty.clone()),
        E::CastToField { .. } => Some(Type::Field(ir::FieldType::Native)),
        E::CastFromField { maxval, .. } => Some(Type::Unsigned(maxval.clone())),
        E::DowncastUnsigned { to_maxval, .. } => Some(Type::Unsigned(to_maxval.clone())),
        E::EnumRef { ty, .. } => Some(ty.clone()),
        E::TupleSlice { ty, .. } | E::VectorSlice { ty, .. } => Some(ty.clone()),
        E::BytesSlice { len, .. } => Some(Type::Bytes(*len)),
        E::BytesRef { .. } => Some(byte_type()),
        // A fold returns its accumulator, so it has the type the initial
        // value has. Without this a Field accumulator reaches the ledger as a
        // width-guessed integer cell instead of a field atom.
        E::Fold { init, .. } => infer_type_of_expr(ctx, init),
        // A map's element type is the callee's result type, which the node
        // does not carry, and a cross-contract call's result type is not
        // shipped either. Inference stays honest and says unknown.
        E::Map { .. } | E::ContractCall { .. } | E::Emit { .. } => None,
    }
}

/// A quoted literal's value.
///
/// The literal carries no type: an integer is a field element, which is the
/// widest numeric domain, unless it fits `u128`, which is the form the rest
/// of the interpreter carries a small number in.
fn eval_literal(literal: &ir::Literal) -> Result<Value, InterpreterError> {
    match literal {
        ir::Literal::Bool(b) => Ok(Value::Bool(*b)),
        ir::Literal::Int(n) => {
            if let Ok(small) = u128::try_from(n) {
                return Ok(Value::Integer(small));
            }
            // Wider than u128 (e.g. `JUBJUB_ORDER`, ~2^252): fold the decimal
            // digits into a full field element with Horner's method, reusing
            // `Fr`'s field arithmetic.
            use midnight_transient_crypto::curve::Fr;
            let mut acc = Fr::from(0u64).0;
            let ten = Fr::from(10u64).0;
            for ch in n.to_string().chars() {
                let digit = ch.to_digit(10).ok_or_else(|| {
                    InterpreterError::TypeError(format!("invalid Field literal {n}"))
                })?;
                acc = acc * ten + Fr::from(u64::from(digit)).0;
            }
            Ok(Value::AlignedValue(AlignedValue::from(Fr(acc))))
        }
        ir::Literal::Bytes(bytes) => Ok(Value::AlignedValue(bytes_aligned_value(
            bytes.clone(),
            bytes.len(),
        )?)),
    }
}

fn eval_expr(ctx: &mut ExecContext, expr: &ir::Expr) -> Result<Value, InterpreterError> {
    use ir::Expr as E;
    match expr {
        E::Quote(literal) => eval_literal(literal),

        E::VarRef(id) => ctx
            .locals
            .get(id.0.as_str())
            .cloned()
            .ok_or_else(|| InterpreterError::UndefinedVariable(id.0.clone())),

        E::Default(ty) => {
            check_type(ty)?;
            default_value(ty)
        }

        E::Assert { expr, message } => {
            let val = eval_expr(ctx, expr)?;
            if !is_truthy(&val) {
                return Err(InterpreterError::AssertionFailed(message.clone()));
            }
            Ok(Value::Void)
        }

        // `kernel.self()` lowers to a read of the contract's own address from
        // the VM *context* (`dup{n:2} idx[0] popeq`). These ops run through the
        // real VM (`exec_ledger_query` injects the supplied `contract_address`
        // into the `QueryContext`), so the read returns the right address *and*
        // the ops land in the transcript. The compiled circuit's proving key
        // expects that `dup/idx/popeq` sequence in the public transcript, so
        // skipping it (an earlier shortcut) produced a "public transcript input
        // mismatch" at prove time.
        E::PublicLedger {
            result_type,
            instructions,
            ..
        } => {
            check_type(result_type)?;
            exec_ledger_query(ctx, instructions)
        }

        // A sequence's value is its last item's; the earlier items run for
        // their effects.
        E::Seq(items) => {
            let mut value = Value::Void;
            for item in items {
                value = eval_expr(ctx, item)?;
            }
            Ok(value)
        }

        E::Return(inner) => eval_expr(ctx, inner),

        E::Tuple(args) | E::VectorLit(args) => {
            let mut vals: Vec<Value> = Vec::with_capacity(args.len());
            for arg in args {
                match arg {
                    ir::TupleArg::Single(e) => vals.push(eval_expr(ctx, e)?),
                    // A spread splices the elements of its (vector/tuple-valued)
                    // inner expression into the surrounding element list. The
                    // compiler attaches the contributed element count as `len`
                    // (analysis-passes.ss attaches the spread vector's length),
                    // so a count mismatch here is a compiler/interpreter
                    // disagreement, not a user error.
                    ir::TupleArg::Spread { len, expr } => {
                        let expected = ir_length(*len)?;
                        let inner = eval_expr(ctx, expr)?;
                        splice_spread(inner, expected, &mut vals)?;
                    }
                }
            }
            // The empty tuple is Compact's unit value, so it is Void rather
            // than an empty aggregate. A void circuit whose body ends in one
            // would otherwise bind a spurious communication output.
            if vals.is_empty() {
                return Ok(Value::Void);
            }
            Ok(Value::Tuple(vals))
        }

        E::LetStar { bindings, body } => {
            for (binder, value) in bindings {
                let inferred_ty = infer_type_of_expr(ctx, value);
                let val = eval_expr(ctx, value)?;
                ctx.locals.insert(binder.name.0.clone(), val);
                match inferred_ty {
                    Some(ty) => ctx.local_types.insert(binder.name.0.clone(), ty),
                    None => ctx.local_types.remove(binder.name.0.as_str()),
                };
            }
            eval_expr(ctx, body)
        }

        E::Call { name, args } => {
            let program = ctx.program;
            if let Some(result_type) = program.result_type(name) {
                check_type(result_type)?;
            }
            let values: Vec<Value> = args
                .iter()
                .map(|a| eval_expr(ctx, a))
                .collect::<Result<_, _>>()?;
            match program.callee(name) {
                Callee::Circuit(circuit) => call_circuit(ctx, circuit, &values),
                Callee::Witness => eval_witness_call(ctx, name.name(), args, values),
                Callee::Pure => eval_pure_call(ctx, name.name(), args, values),
            }
        }

        E::Add { left, right, .. } => eval_arith(ctx, left, right, ArithOp::Add),
        E::Sub { left, right, .. } => eval_arith(ctx, left, right, ArithOp::Sub),
        E::Mul { left, right, .. } => eval_arith(ctx, left, right, ArithOp::Mul),

        E::Eq { left, right, .. } => {
            let l = eval_expr(ctx, left)?;
            let r = eval_expr(ctx, right)?;
            Ok(Value::Bool(values_equal(&l, &r)))
        }

        E::Neq { left, right, .. } => {
            let l = eval_expr(ctx, left)?;
            let r = eval_expr(ctx, right)?;
            Ok(Value::Bool(!values_equal(&l, &r)))
        }

        E::Lt { left, right, .. } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Bool(l < r))
        }

        E::Le { left, right, .. } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Bool(l <= r))
        }

        E::Gt { left, right, .. } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Bool(l > r))
        }

        E::Ge { left, right, .. } => {
            let l = eval_as_integer(ctx, left)?;
            let r = eval_as_integer(ctx, right)?;
            Ok(Value::Bool(l >= r))
        }

        E::If { cond, then, els } => {
            let c = eval_expr(ctx, cond)?;
            if is_truthy(&c) {
                eval_expr(ctx, then)
            } else {
                eval_expr(ctx, els)
            }
        }

        E::VectorRef { expr, index, .. } => {
            // Evaluate the index expression and use it to look up an
            // element in the vector. The vector is expected to be a
            // `Value::Tuple` (the lowering for `Vector<N, T>`).
            let idx_val = eval_expr(ctx, index)?;
            let n = value_to_u128(&idx_val).ok_or_else(|| {
                InterpreterError::TypeError(format!(
                    "vector index expression did not evaluate to an integer (got {idx_val:?})"
                ))
            })?;
            // `as usize` would silently wrap an index like 2^64 + 1 to 1 on
            // 64-bit targets and read the wrong element; reject it instead.
            let idx = usize::try_from(n).map_err(|_| {
                InterpreterError::TypeError(format!(
                    "vector index {n} out of bounds (does not fit in usize)"
                ))
            })?;
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

        E::TupleRef { expr, index } => {
            let index = ir_length(*index)?;
            let val = eval_expr(ctx, expr)?;
            match val {
                Value::Tuple(elements) => elements.get(index).cloned().ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "tuple index {index} out of bounds (len {})",
                        elements.len()
                    ))
                }),
                Value::Struct(fields) => {
                    // Structs can be indexed by position (field declaration order)
                    // This is a fallback — prefer field access by name
                    fields.values().nth(index).cloned().ok_or_else(|| {
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

        E::New { ty, elements } => {
            check_type(ty)?;
            // Struct literal: encode each element with the alignment
            // declared by the corresponding field type, then concatenate
            // all field encodings into a single flat AlignedValue. The
            // result has the same FAB layout the on-chain
            // `persistent_hash` circuit produces for `<StructName>(...)`.
            let struct_name = match ty {
                Type::Struct { name, .. } => name.clone(),
                other => {
                    return Err(InterpreterError::TypeError(format!(
                        "`new` op with non-struct type {other:?}"
                    )));
                }
            };
            // The type carries its own field list, which is exact per
            // instantiation where a name is not: two instantiations of one
            // generic struct share a name and differ in layout.
            let fields: Vec<(String, Type)> = match ty {
                Type::Struct { fields, .. } => fields.clone(),
                other => {
                    return Err(InterpreterError::TypeError(format!(
                        "`new` op with non-struct type {other:?}"
                    )));
                }
            };
            if fields.len() != elements.len() {
                return Err(InterpreterError::TypeError(format!(
                    "`new {struct_name}` expects {} fields, got {}",
                    fields.len(),
                    elements.len()
                )));
            }
            let mut parts: Vec<midnight_base_crypto::fab::AlignedValue> =
                Vec::with_capacity(elements.len());
            for ((field_name, field_ty), element) in fields.iter().zip(elements.iter()) {
                let val = eval_expr(ctx, element)?;
                let av = encode_typed(&val, field_ty).map_err(|e| {
                    InterpreterError::TypeError(format!(
                        "cannot encode field `{field_name}` of `{struct_name}`: {e}"
                    ))
                })?;
                parts.push(av);
            }
            let combined = midnight_base_crypto::fab::AlignedValue::concat(parts.iter());
            Ok(Value::AlignedValue(combined))
        }

        E::CastFromBytes { ty, len, expr } => {
            check_type(ty)?;
            let val = eval_expr(ctx, expr)?;
            eval_cast(val, &Type::Bytes(*len), ty)
        }

        E::CastFromEnum { ty, from, expr } | E::CastToEnum { ty, from, expr } => {
            check_type(ty)?;
            check_type(from)?;
            let val = eval_expr(ctx, expr)?;
            eval_cast(val, from, ty)
        }

        E::CastToField {
            field_type,
            from,
            expr,
        } => {
            if !is_native_field_type(field_type) {
                return Err(InterpreterError::Unsupported(
                    "a cast to a secp256k1 field".to_string(),
                ));
            }
            check_type(from)?;
            let val = eval_expr(ctx, expr)?;
            eval_cast(val, from, &Type::Field(ir::FieldType::Native))
        }

        E::CastFromField {
            maxval,
            field_type,
            expr,
        } => {
            if !is_native_field_type(field_type) {
                return Err(InterpreterError::Unsupported(
                    "a cast from a secp256k1 field".to_string(),
                ));
            }
            let val = eval_expr(ctx, expr)?;
            eval_cast(
                val,
                &Type::Field(ir::FieldType::Native),
                &Type::Unsigned(maxval.clone()),
            )
        }

        E::SafeCast { ty, from, expr } => {
            check_type(ty)?;
            check_type(from)?;
            let val = eval_expr(ctx, expr)?;
            eval_cast(val, from, ty)
        }

        E::DowncastUnsigned {
            from_maxval,
            to_maxval,
            expr,
        } => {
            let val = eval_expr(ctx, expr)?;
            eval_cast(
                val,
                &Type::Unsigned(from_maxval.clone()),
                &Type::Unsigned(to_maxval.clone()),
            )
        }

        E::EltRef { expr, elt, .. } => {
            // Derive the receiver's declared type *before* consuming its
            // value, so we can slice `Value::AlignedValue` by the correct
            // struct layout.
            let receiver_ty = infer_type_of_expr(ctx, expr);
            let val = eval_expr(ctx, expr)?;
            match &val {
                Value::Struct(fields) => fields.get(elt).cloned().ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "struct has no field '{elt}', available: {:?}",
                        fields.keys().collect::<Vec<_>>()
                    ))
                }),
                Value::AlignedValue(av) => {
                    let struct_name = match &receiver_ty {
                        Some(Type::Struct { name, .. }) => name.clone(),
                        other => {
                            return Err(InterpreterError::TypeError(format!(
                                "field access .{elt} on AlignedValue with unknown receiver type {other:?}"
                            )));
                        }
                    };
                    // The type determines its own layout, which a name cannot:
                    // two instantiations of one generic struct share a name and
                    // differ in layout.
                    let Some(Type::Struct { fields, .. }) = &receiver_ty else {
                        return Err(InterpreterError::TypeError(format!(
                            "field access .{elt} on a value whose type is not a struct"
                        )));
                    };
                    let layout = layout_from_fields(fields).ok_or_else(|| {
                        InterpreterError::TypeError(format!(
                            "cannot compute the layout of '{struct_name}' for field .{elt}"
                        ))
                    })?;
                    let (offset, len) = match layout.field_slice(elt) {
                        Some(slice) => slice,
                        // `Either<A, B>.field`: the field lives on the live
                        // variant, not on `Either`, so descend via the
                        // `is_left` discriminant.
                        None => either_variant_field_slice(fields, av, elt)?,
                    };
                    if offset + len > av.value.0.len() || offset + len > av.alignment.0.len() {
                        return Err(InterpreterError::TypeError(format!(
                            "field .{elt} slice [{offset}..{}] out of bounds for \
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
                    "field access .{elt} on {val:?}"
                ))),
            }
        }

        // Field → Bytes<len>. Little-endian, zero-padded; values that
        // need more than `len` bytes are a range error, matching the
        // Compact runtime's `convertFieldToBytes` (casts.ts).
        E::FieldToBytes {
            len,
            field_type,
            expr,
        } => {
            if !is_native_field_type(field_type) {
                return Err(InterpreterError::Unsupported(
                    "field-to-bytes on a secp256k1 field".to_string(),
                ));
            }
            let val = eval_expr(ctx, expr)?;
            let fr = value_to_fr(&val).ok_or_else(|| {
                InterpreterError::TypeError(format!(
                    "field-to-bytes expects a Field value, got {val:?}"
                ))
            })?;
            let len = ir_length(*len)?;
            let mut bytes = fr.as_le_bytes();
            // Trim trailing zeros here (not just in bytes_aligned_value) so
            // the width check below sees the value's true byte length.
            while matches!(bytes.last(), Some(0)) {
                bytes.pop();
            }
            if bytes.len() > len {
                return Err(InterpreterError::TypeError(format!(
                    "range error: Field value {fr:?} does not fit into {len} bytes"
                )));
            }
            Ok(Value::AlignedValue(bytes_aligned_value(bytes, len)?))
        }

        // Bytes<len> → Vector<len, Uint<8>>. Element i is byte i. The
        // TypeScript lowering is `Array.from(bytes, BigInt)`
        // (typescript-passes.ss). Each element is encoded as a 1-byte atom
        // so downstream typed consumers (hashes, stores) see the on-chain
        // Vector<N, Uint<8>> layout.
        E::BytesToVector { len, expr } => {
            let val = eval_expr(ctx, expr)?;
            let len = ir_length(*len)?;
            let bytes = value_to_byte_string(&val, len)?;
            let elements = (0..len)
                .map(|i| {
                    let b = bytes.get(i).copied().unwrap_or(0);
                    Value::AlignedValue(AlignedValue::from(b))
                })
                .collect();
            Ok(Value::Tuple(elements))
        }

        // Vector<len, Uint<8>> → Bytes<len>. Element i becomes byte i. The
        // TypeScript lowering is `Uint8Array.from(vector, Number)`
        // (typescript-passes.ss). The type checker guarantees Uint<=255
        // elements (circuit-passes.ss); anything wider here is a bug.
        E::VectorToBytes { len, expr } => {
            let val = eval_expr(ctx, expr)?;
            let len = ir_length(*len)?;
            let bytes = vector_value_to_bytes(&val, len)?;
            Ok(Value::AlignedValue(bytes_aligned_value(bytes, len)?))
        }

        // Cross-contract calls are a later feature; fail with a purposeful
        // message naming the call target instead of a Debug dump.
        E::ContractCall {
            circuit,
            receiver,
            contract_type,
            ..
        } => {
            check_type(contract_type)?;
            let target = match receiver.as_ref() {
                ir::Expr::VarRef(id) => id.name().to_string(),
                _ => match contract_type {
                    Type::Contract { name, .. }
                    | Type::Struct { name, .. }
                    | Type::Opaque(name) => name.clone(),
                    _ => "<contract>".to_string(),
                },
            };
            Err(InterpreterError::Unsupported(format!(
                "cross-contract calls are not implemented yet (call to {target}.{circuit})"
            )))
        }

        // The on-chain encoding of an enum is its member's index in the
        // declaration order the type carries.
        E::EnumRef { ty, elt } => {
            check_type(ty)?;
            let Type::Enum { name, variants } = ty else {
                return Err(InterpreterError::TypeError(format!(
                    "enum-member `{elt}` has non-enum type {ty:?}"
                )));
            };
            let index = variants.iter().position(|v| v == elt).ok_or_else(|| {
                InterpreterError::TypeError(format!(
                    "enum {name} has no member `{elt}` (has {variants:?})"
                ))
            })?;
            Ok(Value::Integer(index as u128))
        }

        // A bounded loop over `len` elements, building a tuple. Each
        // argument is evaluated once, as in the lowering this mirrors.
        E::Map { len, fun, args } => {
            let arg_types: Vec<Option<Type>> = args
                .iter()
                .map(|a| infer_type_of_expr(ctx, &a.expr))
                .collect();
            let arg_values: Vec<Value> = args
                .iter()
                .map(|a| eval_expr(ctx, &a.expr))
                .collect::<Result<_, _>>()?;
            let len = ir_length(*len)?;
            let mut out = Vec::with_capacity(len);
            for i in 0..len {
                let mut call_args = Vec::with_capacity(arg_values.len());
                for (v, t) in arg_values.iter().zip(arg_types.iter()) {
                    call_args.push(loop_element(v, t.as_ref(), i)?);
                }
                out.push(apply_fun(ctx, fun, &call_args)?);
            }
            Ok(Value::Tuple(out))
        }

        // The same, threading an accumulator. The accumulator is the callee's
        // first argument and the iteration runs from 0 upwards.
        E::Fold {
            len,
            fun,
            init,
            args,
            ..
        } => {
            let arg_types: Vec<Option<Type>> = args
                .iter()
                .map(|a| infer_type_of_expr(ctx, &a.expr))
                .collect();
            let mut acc = eval_expr(ctx, init)?;
            let arg_values: Vec<Value> = args
                .iter()
                .map(|a| eval_expr(ctx, &a.expr))
                .collect::<Result<_, _>>()?;
            for i in 0..ir_length(*len)? {
                let mut call_args = Vec::with_capacity(arg_values.len() + 1);
                call_args.push(acc);
                for (v, t) in arg_values.iter().zip(arg_types.iter()) {
                    call_args.push(loop_element(v, t.as_ref(), i)?);
                }
                acc = apply_fun(ctx, fun, &call_args)?;
            }
            Ok(acc)
        }

        // One byte of a Bytes value.
        E::BytesRef { expr, index, .. } => {
            let val = eval_expr(ctx, expr)?;
            let i = value_to_u128(&eval_expr(ctx, index)?)
                .ok_or_else(|| InterpreterError::TypeError("byte index is not an integer".into()))?
                as usize;
            let bytes = bytes_of(&val)?;
            bytes
                .get(i)
                .map(|b| Value::Integer(*b as u128))
                .ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "byte {i} is out of range for a {}-byte value",
                        bytes.len()
                    ))
                })
        }

        // A run of `len` elements from a tuple or vector, taken from a
        // constant offset. The operand type gives the element widths.
        E::TupleSlice {
            ty,
            expr,
            index,
            len,
        } => {
            check_type(ty)?;
            let val = eval_expr(ctx, expr)?;
            slice_elements(&val, ty, ir_length(*index)?, ir_length(*len)?)
        }

        // The same, with the offset given by an expression. The compiler
        // requires that expression to reduce to a constant, and evaluates the
        // operand before it.
        E::VectorSlice {
            ty,
            expr,
            index,
            len,
        } => {
            check_type(ty)?;
            let val = eval_expr(ctx, expr)?;
            let start = value_to_u128(&eval_expr(ctx, index)?).ok_or_else(|| {
                InterpreterError::TypeError("slice index is not an integer".into())
            })? as usize;
            slice_elements(&val, ty, start, ir_length(*len)?)
        }

        // A run of `len` bytes; the result is Bytes<len>.
        E::BytesSlice {
            expr, index, len, ..
        } => {
            let val = eval_expr(ctx, expr)?;
            let start = value_to_u128(&eval_expr(ctx, index)?).ok_or_else(|| {
                InterpreterError::TypeError("slice index is not an integer".into())
            })? as usize;
            let len = ir_length(*len)?;
            let bytes = bytes_of(&val)?;
            if start + len > bytes.len() {
                return Err(InterpreterError::TypeError(format!(
                    "slice [{start}..{}] runs past a {}-byte value",
                    start + len,
                    bytes.len()
                )));
            }
            Ok(Value::AlignedValue(bytes_aligned_value(
                bytes[start..start + len].to_vec(),
                len,
            )?))
        }

        E::Emit { .. } => Err(InterpreterError::Unsupported("events (emit)".to_string())),
    }
}

/// The value of a cast.
///
/// Only two conversions change the value; every other cast is a change of
/// static type over the same runtime representation.
fn eval_cast(val: Value, from: &Type, to: &Type) -> Result<Value, InterpreterError> {
    use midnight_transient_crypto::curve::Fr;

    // Bytes<length> → Field. Byte 0 is the least significant byte and
    // values >= the field modulus are rejected, not reduced — matching
    // the Compact runtime's `convertBytesToField` (casts.ts). At the
    // FAB level both Bytes and Field atoms are zero-trimmed
    // little-endian bytes, so this is a reinterpretation plus a range
    // check.
    if let (Type::Bytes(length), Type::Field(_)) = (from.resolved(), to.resolved()) {
        let bytes = value_to_byte_string(&val, ir_length(*length)?)?;
        let fr = Fr::from_le_bytes(&bytes).ok_or_else(|| {
            InterpreterError::TypeError(format!(
                "range error: byte string {} exceeds the maximum value of the Field type",
                hex::encode(&bytes)
            ))
        })?;
        return Ok(Value::AlignedValue(AlignedValue::from(fr)));
    }

    // When casting an Integer to Field (e.g. `request_id as Field`
    // before a `Map<Field, _>` insert/lookup), eagerly re-encode
    // as a Field-aligned `AlignedValue` so every downstream
    // consumer sees the correct alignment byte-for-byte. The consumers are
    // a push operand, an idx path key, and a direct read of the local. Without this, the Integer
    // value survives through the let-binding and later gets encoded as a
    // u64-aligned cell, which never matches a Field-aligned key stored
    // on-chain.
    if let (Value::Integer(n), Type::Field(_)) = (&val, to) {
        // `From<u128> for Fr` is exact (midnight-curves
        // `Scalar::from_u128`); never narrow through u64 here.
        return Ok(Value::AlignedValue(AlignedValue::from(Fr::from(*n))));
    }
    Ok(val)
}

/// Run a circuit's body as a pure call: parameters bound positionally, the
/// body's value returned.
///
/// The parameters shadow the caller's bindings rather than replacing them;
/// identifiers are unique program-wide, so nothing else is reachable anyway.
fn call_circuit(
    ctx: &mut ExecContext,
    circuit: &ir::Circuit,
    args: &[Value],
) -> Result<Value, InterpreterError> {
    for param in &circuit.arguments {
        check_type(&param.ty)?;
    }
    let saved_locals = ctx.locals.clone();
    let saved_types = ctx.local_types.clone();
    for (param, val) in circuit.arguments.iter().zip(args.iter()) {
        ctx.locals.insert(param.name.0.clone(), val.clone());
        ctx.local_types
            .insert(param.name.0.clone(), param.ty.clone());
    }
    let result = eval_expr(ctx, &circuit.body);
    ctx.locals = saved_locals;
    ctx.local_types = saved_types;
    result
}

/// A call to a native the runtime computes in place: a builtin.
fn eval_pure_call(
    ctx: &mut ExecContext,
    name: &str,
    arg_exprs: &[ir::Expr],
    values: Vec<Value>,
) -> Result<Value, InterpreterError> {
    // Handle disclose specially: record the value as a communication output.
    if name == "disclose" {
        return disclose(ctx, values);
    }
    let arg_types: Vec<Option<Type>> = arg_exprs
        .iter()
        .map(|a| infer_type_of_expr(ctx, a))
        .collect();
    match try_builtin_typed(name, &values, &arg_types) {
        Some(result) => result,
        None => Err(InterpreterError::Unsupported(format!(
            "unknown pure function: {name}"
        ))),
    }
}

/// A call the witness provider answers.
fn eval_witness_call(
    ctx: &mut ExecContext,
    name: &str,
    arg_exprs: &[ir::Expr],
    values: Vec<Value>,
) -> Result<Value, InterpreterError> {
    // Handle disclose before anything else: it must always record the value
    // in communication_outputs regardless of the witness provider. A witness
    // provider that intercepts "disclose" would break the communication
    // commitment.
    if name == "disclose" {
        return disclose(ctx, values);
    }

    // The Compact "witness" native primitives (see [`WitnessNative`]).
    // These are effectful and have no witness-provider/builtin entry, so the
    // interpreter handles them inline here. The match is exhaustive: adding a
    // `WitnessNative` variant forces a decision. `createZswapOutput` records
    // no ledger effect of its own (the mint/spend/receive effects are separate
    // ledger ops); it marks "attach a Zswap output for this coin here", so we
    // capture its `(coin, recipient)` args for the call/deploy path to build
    // the corresponding `Output` in the transaction's Zswap offer.
    if let Some(native) = WitnessNative::from_name(name) {
        match native {
            WitnessNative::CreateZswapOutput => {
                let mut it = values.into_iter();
                return match (it.next(), it.next()) {
                    (Some(coin), Some(recipient)) => {
                        ctx.zswap_outputs
                            .push(CircuitZswapOutput { coin, recipient });
                        Ok(Value::Void)
                    }
                    _ => Err(InterpreterError::TypeError(
                        "createZswapOutput expects (coin, recipient) arguments".to_string(),
                    )),
                };
            }
            // The spend counterpart of `createZswapOutput`: like it, records
            // no ledger effect of its own (the spend/nullifier effects are
            // separate ledger ops), so we capture the coin arg for the
            // call/deploy path to build the `Input` / `Transient` in the
            // transaction's Zswap offer.
            WitnessNative::CreateZswapInput => {
                return match values.into_iter().next() {
                    Some(coin) => {
                        ctx.zswap_inputs.push(CircuitZswapInput { coin });
                        Ok(Value::Void)
                    }
                    None => Err(InterpreterError::TypeError(
                        "createZswapInput expects a (coin) argument".to_string(),
                    )),
                };
            }
            // Not yet implemented; see the coverage table in
            // docs/compact-natives.md.
            WitnessNative::OwnPublicKey => {
                return Err(InterpreterError::Witness(format!(
                    "unimplemented Compact witness native: {name}"
                )));
            }
        }
    }

    // Witness calls are authoritative: ask the off-chain witness provider
    // first (it owns the canonical value the prover commits to). For some
    // calls (notably `persistentHash`) the IR-level args are stripped (the
    // compiler can't yet serialize struct literals into the IR), so
    // dispatching to the builtin would compute a hash of `Void` instead of
    // the real preimage. Routing to the witness provider first lets the
    // off-chain caller supply the canonical value; we only fall back to
    // builtin dispatch when the provider returns `WitnessOutcome::Unknown`
    // (i.e. it has no witness with this name). Every `Err` is a genuine
    // witness failure and propagates. It must never reroute to a builtin. A
    // failing provider whose name collides with one (e.g. `persistentHash`)
    // would otherwise "succeed" with the wrong inputs.
    if let Some(w) = ctx.witnesses {
        // Scope the WitnessContext's borrow of `ctx` so we can record the
        // result into `ctx.private_transcript_outputs` afterward.
        let outcome = {
            let mut wctx = WitnessContext::new(&mut *ctx.private_state);
            w.call_witness(&mut wctx, name, &values)
        };
        match outcome? {
            WitnessOutcome::Value(v) => {
                // Capture the witness's private value as a private transcript
                // output, in call order, for the prover.
                ctx.private_transcript_outputs
                    .push(v.try_to_aligned_value()?);
                return Ok(v);
            }
            WitnessOutcome::Unknown => {
                // Provider doesn't know the name; fall through.
            }
        }
    }
    let arg_types: Vec<Option<Type>> = arg_exprs
        .iter()
        .map(|a| infer_type_of_expr(ctx, a))
        .collect();
    if let Some(result) = try_builtin_typed(name, &values, &arg_types) {
        return result;
    }
    Err(InterpreterError::Witness(format!(
        "no witness provider or builtin for: {name}"
    )))
}

/// Record a disclosed value as a communication output and pass it through.
fn disclose(ctx: &mut ExecContext, values: Vec<Value>) -> Result<Value, InterpreterError> {
    match values.into_iter().next() {
        Some(arg) => {
            ctx.communication_outputs.push(arg.try_to_aligned_value()?);
            Ok(arg)
        }
        None => Ok(Value::Void),
    }
}

/// Convert an IR-level element count or index (`u64`) to `usize`, rejecting
/// values the host cannot index instead of wrapping.
fn ir_length(length: u64) -> Result<usize, InterpreterError> {
    usize::try_from(length)
        .map_err(|_| InterpreterError::TypeError(format!("length {length} does not fit in usize")))
}

/// The element type of a byte string, `Uint<0..255>`: what indexing or
/// iterating a `Bytes<N>` yields.
fn byte_type() -> Type {
    Type::Unsigned(BigUint::from(u8::MAX))
}

/// Splice the elements of a spread's inner value into a tuple constructor's
/// element list. The inner value is a Compact vector/tuple, which at runtime
/// arrives either as a structured `Value::Tuple` or flattened into an
/// `AlignedValue` (one atom per leaf element — circuit arguments and popeq
/// reads). `expected` is the element count the compiler attached to the
/// spread; a mismatch means the value's shape disagrees with its static type.
fn splice_spread(
    inner: Value,
    expected: usize,
    out: &mut Vec<Value>,
) -> Result<(), InterpreterError> {
    use midnight_base_crypto::fab;
    match inner {
        Value::Tuple(els) => {
            if els.len() != expected {
                return Err(InterpreterError::TypeError(format!(
                    "spread of length {expected} got a tuple with {} elements",
                    els.len()
                )));
            }
            out.extend(els);
            Ok(())
        }
        Value::AlignedValue(av) => {
            let atoms = &av.value.0;
            let segments = &av.alignment.0;
            if atoms.len() != expected || segments.len() != expected {
                return Err(InterpreterError::TypeError(format!(
                    "spread of length {expected} got an AlignedValue with {} atoms \
                     ({} alignment segments); only flat one-atom-per-element \
                     vectors can be spliced",
                    atoms.len(),
                    segments.len()
                )));
            }
            for (atom, segment) in atoms.iter().zip(segments.iter()) {
                let fab::AlignmentSegment::Atom(a) = segment else {
                    return Err(InterpreterError::TypeError(
                        "spread over an AlignedValue with non-atom alignment \
                         (e.g. Maybe) is not supported"
                            .to_string(),
                    ));
                };
                let single = fab::AlignedValue::new(
                    fab::Value(vec![atom.clone()]),
                    fab::Alignment::singleton(*a),
                )
                .ok_or_else(|| {
                    InterpreterError::TypeError(
                        "spread element does not satisfy its alignment".to_string(),
                    )
                })?;
                out.push(Value::AlignedValue(single));
            }
            Ok(())
        }
        other => Err(InterpreterError::TypeError(format!(
            "cannot spread non-vector value {other:?}"
        ))),
    }
}

/// Extract the raw byte string of a `Bytes<length>` value. FAB stores it as
/// a single zero-trimmed atom (byte 0 first), so the returned vector may be
/// shorter than `length`; the missing trailing bytes are zero.
fn value_to_byte_string(val: &Value, length: usize) -> Result<Vec<u8>, InterpreterError> {
    match val {
        Value::AlignedValue(av) if av.value.0.len() == 1 => {
            let atom = &av.value.0[0];
            if atom.0.len() > length {
                return Err(InterpreterError::TypeError(format!(
                    "byte string of {} bytes is wider than Bytes<{length}>",
                    atom.0.len()
                )));
            }
            Ok(atom.0.clone())
        }
        other => Err(InterpreterError::TypeError(format!(
            "expected a Bytes<{length}> value, got {other:?}"
        ))),
    }
}

/// Decode a `Vector<length, Uint<8>>` value into its byte string (element i
/// → byte i). Accepts the structured `Value::Tuple` form and the flattened
/// one-atom-per-element `AlignedValue` form.
fn vector_value_to_bytes(val: &Value, length: usize) -> Result<Vec<u8>, InterpreterError> {
    let byte_of = |v: u128| -> Result<u8, InterpreterError> {
        u8::try_from(v).map_err(|_| {
            InterpreterError::TypeError(format!(
                "vector-to-bytes element {v} exceeds 255 (expected Uint<8> elements)"
            ))
        })
    };
    match val {
        Value::Tuple(els) => {
            if els.len() != length {
                return Err(InterpreterError::TypeError(format!(
                    "vector-to-bytes of length {length} got {} elements",
                    els.len()
                )));
            }
            els.iter()
                .map(|e| {
                    let n = value_to_u128(e).ok_or_else(|| {
                        InterpreterError::TypeError(format!(
                            "vector-to-bytes element is not an integer: {e:?}"
                        ))
                    })?;
                    byte_of(n)
                })
                .collect()
        }
        Value::AlignedValue(av) if av.value.0.len() == length => av
            .value
            .0
            .iter()
            .map(|atom| {
                if atom.0.len() > 1 {
                    return Err(InterpreterError::TypeError(format!(
                        "vector-to-bytes element atom of {} bytes exceeds 255 \
                         (expected Uint<8> elements)",
                        atom.0.len()
                    )));
                }
                Ok(atom.0.first().copied().unwrap_or(0))
            })
            .collect(),
        other => Err(InterpreterError::TypeError(format!(
            "expected a Vector<{length}, Uint<8>> value, got {other:?}"
        ))),
    }
}

/// The bytes behind a `Bytes<N>` value, which is a single atom.
fn bytes_of(val: &Value) -> Result<Vec<u8>, InterpreterError> {
    match val {
        Value::AlignedValue(av) => av
            .value
            .0
            .first()
            .map(|atom| atom.0.clone())
            .ok_or_else(|| InterpreterError::TypeError("empty bytes value".to_string())),
        other => Err(InterpreterError::TypeError(format!(
            "expected a bytes value, got {other:?}"
        ))),
    }
}

/// Extract element `i` of a loop argument.
///
/// The lowering this mirrors indexes a tuple or vector positionally and takes
/// a byte from a `Bytes` value (circuit-passes.ss, `Map-Argument`). A value
/// that arrives already flattened is sliced by its element stride, which needs
/// the element type.
fn loop_element(arg: &Value, arg_ty: Option<&Type>, i: usize) -> Result<Value, InterpreterError> {
    match arg {
        Value::Tuple(elements) => elements.get(i).cloned().ok_or_else(|| {
            InterpreterError::TypeError(format!(
                "loop index {i} is out of range for a {}-element argument",
                elements.len()
            ))
        }),
        Value::AlignedValue(av) => {
            let element_ty = match arg_ty {
                Some(Type::Vector { ty, .. }) => (**ty).clone(),
                Some(Type::Tuple(types)) => types.get(i).cloned().ok_or_else(|| {
                    InterpreterError::TypeError(format!("loop index {i} is out of range"))
                })?,
                Some(Type::Bytes(_)) => byte_type(),
                other => {
                    return Err(InterpreterError::TypeError(format!(
                        "cannot index a loop argument of type {other:?}"
                    )));
                }
            };
            // A Bytes argument yields one byte per iteration.
            if matches!(arg_ty, Some(Type::Bytes(_))) {
                let byte = av
                    .value
                    .0
                    .first()
                    .and_then(|atom| atom.0.get(i))
                    .copied()
                    .ok_or_else(|| {
                        InterpreterError::TypeError(format!("byte {i} is out of range"))
                    })?;
                return Ok(Value::Integer(byte as u128));
            }
            let stride = atom_count_for_type(&element_ty).ok_or_else(|| {
                InterpreterError::TypeError(format!(
                    "cannot determine the element width of {element_ty:?}"
                ))
            })?;
            let start = i * stride;
            if start + stride > av.value.0.len() {
                return Err(InterpreterError::TypeError(format!(
                    "loop element {i} runs past the argument (len {})",
                    av.value.0.len()
                )));
            }
            Ok(Value::AlignedValue(
                midnight_base_crypto::fab::AlignedValue {
                    value: midnight_base_crypto::fab::Value(
                        av.value.0[start..start + stride].to_vec(),
                    ),
                    alignment: midnight_base_crypto::fab::Alignment(
                        av.alignment.0[start..start + stride].to_vec(),
                    ),
                },
            ))
        }
        other => Err(InterpreterError::TypeError(format!(
            "a loop argument must be a tuple, vector or bytes value, got {other:?}"
        ))),
    }
}

/// Take `length` elements starting at `start` from a tuple- or vector-typed
/// value, mirroring the element-wise lowering the compiler applies.
fn slice_elements(
    val: &Value,
    operand_ty: &Type,
    start: usize,
    length: usize,
) -> Result<Value, InterpreterError> {
    let mut out = Vec::with_capacity(length);
    for k in 0..length {
        out.push(loop_element(val, Some(operand_ty), start + k)?);
    }
    Ok(Value::Tuple(out))
}

/// Apply a loop callee to one iteration's arguments.
fn apply_fun(
    ctx: &mut ExecContext,
    fun: &ir::Fun,
    args: &[Value],
) -> Result<Value, InterpreterError> {
    match fun {
        ir::Fun::Ref(id) => match ctx.program.circuits.get(id.0.as_str()).copied() {
            Some(circuit) => call_circuit(ctx, circuit, args),
            None => Err(InterpreterError::TypeError(format!(
                "loop calls `{}`, which is not a circuit in the program",
                id.0
            ))),
        },
        ir::Fun::Circuit {
            arguments, body, ..
        } => {
            if arguments.len() != args.len() {
                return Err(InterpreterError::TypeError(format!(
                    "loop callee takes {} parameters but got {} arguments",
                    arguments.len(),
                    args.len()
                )));
            }
            for param in arguments {
                check_type(&param.ty)?;
            }
            // An inline callee closes over the enclosing scope: a loop body
            // reads the circuit's own arguments, and the generated TypeScript
            // passes a closure that captures them. So the parameters shadow
            // the caller's locals rather than replacing them.
            let saved_locals = ctx.locals.clone();
            let saved_types = ctx.local_types.clone();
            for (param, val) in arguments.iter().zip(args.iter()) {
                ctx.locals.insert(param.name.0.clone(), val.clone());
                ctx.local_types
                    .insert(param.name.0.clone(), param.ty.clone());
            }
            let result = eval_expr(ctx, body);
            ctx.locals = saved_locals;
            ctx.local_types = saved_types;
            result
        }
    }
}

fn eval_as_integer(ctx: &mut ExecContext, expr: &ir::Expr) -> Result<u128, InterpreterError> {
    let val = eval_expr(ctx, expr)?;
    value_to_u128(&val)
        .ok_or_else(|| InterpreterError::TypeError(format!("expected integer, got {val:?}")))
}

#[derive(Clone, Copy)]
enum ArithOp {
    Add,
    Sub,
    Mul,
}

/// Evaluate `left <op> right`.
///
/// When both operands fit `u128` this keeps the historical wrapping integer
/// arithmetic. When either operand is a wider `Field` element — e.g. a Poseidon
/// hash output feeding an on-circuit mod-`r` reduction like
/// `c = c_native - challenge_quotient * JUBJUB_ORDER` — it falls back to field
/// arithmetic over `Fr`, matching what the compiled circuit computes. Without
/// this, `eval_as_integer` rejects the full-width operand as "expected integer".
fn eval_arith(
    ctx: &mut ExecContext,
    left: &ir::Expr,
    right: &ir::Expr,
    op: ArithOp,
) -> Result<Value, InterpreterError> {
    use midnight_transient_crypto::curve::Fr;

    let lv = eval_expr(ctx, left)?;
    let rv = eval_expr(ctx, right)?;

    if let (Some(l), Some(r)) = (value_to_u128(&lv), value_to_u128(&rv)) {
        let n = match op {
            ArithOp::Add => l.wrapping_add(r),
            ArithOp::Sub => l.wrapping_sub(r),
            ArithOp::Mul => l.wrapping_mul(r),
        };
        return Ok(Value::Integer(n));
    }

    let to_fr = |v: &Value| -> Result<Fr, InterpreterError> {
        value_to_fr(v).ok_or_else(|| {
            InterpreterError::TypeError(format!("expected a Field or integer operand, got {v:?}"))
        })
    };
    // `Fr` wraps `midnight_curves::Fq` (the field). It has no direct `std::ops`,
    // so operate on the inner scalar to reuse midnight-curves' field arithmetic.
    let (l, r) = (to_fr(&lv)?.0, to_fr(&rv)?.0);
    let f = match op {
        ArithOp::Add => Fr(l + r),
        ArithOp::Sub => Fr(l - r),
        ArithOp::Mul => Fr(l * r),
    };
    Ok(Value::AlignedValue(AlignedValue::from(f)))
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
        // RHS is an `Integer` produced by an enum reference.
        (Value::AlignedValue(av), Value::Integer(n))
        | (Value::Integer(n), Value::AlignedValue(av)) => {
            aligned_atom_to_u128(av).is_some_and(|lhs| lhs == *n)
        }
        (Value::AlignedValue(av), Value::Bool(b)) | (Value::Bool(b), Value::AlignedValue(av)) => {
            aligned_atom_to_u128(av)
                .map(|n| (n != 0) == *b)
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// `Either<A, B>.field` where the compiler folded the ternary away: the field
/// lives on the live variant, not on `Either` itself, so `Either` is asked for
/// a field it does not carry. Recover the slice by reading the `is_left`
/// discriminant from the receiver's atoms and descending into the live
/// variant's own layout. Returns the `(offset, len)` of `field` within the
/// `Either`'s `AlignedValue`, matching what the real circuit's ternary computes.
fn either_variant_field_slice(
    fields: &[(String, Type)],
    av: &AlignedValue,
    field: &str,
) -> Result<(usize, usize), InterpreterError> {
    let resolve = || -> Option<(usize, usize)> {
        let layout = layout_from_fields(fields)?;
        let (disc_off, disc_len) = layout.field_slice("is_left")?;
        let (left_off, _) = layout.field_slice("left")?;
        let (right_off, _) = layout.field_slice("right")?;

        let mut disc = av.clone();
        disc.value = midnight_base_crypto::fab::Value(
            av.value.0.get(disc_off..disc_off + disc_len)?.to_vec(),
        );
        disc.alignment = midnight_base_crypto::fab::Alignment(
            av.alignment.0.get(disc_off..disc_off + disc_len)?.to_vec(),
        );
        let is_left = is_truthy(&Value::AlignedValue(disc));

        let (variant_field, variant_off) = if is_left {
            ("left", left_off)
        } else {
            ("right", right_off)
        };
        // The variant's own type carries its fields, so the descent needs no
        // lookup by name.
        let variant_ty = fields
            .iter()
            .find(|(name, _)| name == variant_field)
            .map(|(_, ty)| ty)?;
        let Type::Struct {
            fields: variant_fields,
            ..
        } = variant_ty.resolved()
        else {
            return None;
        };
        let (sub_off, sub_len) = layout_from_fields(variant_fields)?.field_slice(field)?;
        Some((variant_off + sub_off, sub_len))
    };
    resolve().ok_or_else(|| {
        InterpreterError::TypeError(format!("no field '{field}' on the live Either variant"))
    })
}

fn is_truthy(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::Integer(n) => *n != 0,
        Value::Void => false,
        Value::AlignedValue(av) => {
            // Boolean cells coming back from `popeq` are encoded as a
            // single-atom AlignedValue whose only byte is 0x00 (false)
            // or 0x01 (true). A catch-all "every AlignedValue is truthy"
            // would silently turn `member(...) == false` into "membership
            // found" and break asserts like `!processed.member(...)`.
            //
            // Treat any AlignedValue whose atoms are all-zero (or empty)
            // as false; otherwise true.
            !av.value.0.iter().all(|atom| atom.0.iter().all(|b| *b == 0))
        }
        _ => true,
    }
}

/// Execute a public-ledger operation: translate its VM instructions to
/// onchain-vm `Op`s and run them through the VM `QueryContext` (with the
/// contract's real address injected, so `kernel.self()`'s `dup{n:2} idx[0]
/// popeq` context read returns the right address and lands in the transcript
/// the proving key expects).
fn exec_ledger_query(
    ctx: &mut ExecContext,
    instructions: &[ir::Instruction],
) -> Result<Value, InterpreterError> {
    let cost_model = &INITIAL_COST_MODEL;
    let mut ops: Vec<Op<ResultModeGather, InMemoryDB>> = Vec::new();
    for instruction in instructions {
        ops.push(build_op(ctx, instruction)?);
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

    // Execute the ops against the contract state.
    //
    // `ContractStateExt::query` builds the VM `QueryContext` with a zero
    // `address`, which breaks `kernel.self()` (it reads the contract's own
    // address out of the VM *context* via `dup{n:2} idx[0] popeq`). Build the
    // context directly so we can inject the real `contract_address` when it is
    // known. The address only matters for context reads; for everything else
    // a zero default is identical to what `ContractStateExt::query` used.
    let address = ctx.contract_address.unwrap_or_default();
    let qc = QueryContext::new(ctx.state.data.clone(), address);
    let res = qc
        .query::<ResultModeGather>(&ops, None, cost_model)
        .map_err(|e| InterpreterError::LedgerQueryFailed(format!("{e:?}")))?;
    let events = res.events;
    let new_state = ContractState {
        data: res.context.state,
        ..ctx.state.clone()
    };

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

/// One expanded VM instruction as the op the VM runs.
///
/// The instruction set is open, so an instruction this does not implement is
/// refused by name rather than skipped. Operands that carry an expression are
/// evaluated here, in instruction order.
fn build_op(
    ctx: &mut ExecContext,
    i: &ir::Instruction,
) -> Result<Op<ResultModeGather, InMemoryDB>, InterpreterError> {
    Ok(match i.op.as_str() {
        "idx" => {
            let Some(ir::Operand::List(path)) = i.arg("path") else {
                return Err(InterpreterError::Unsupported(
                    "idx without a path list".to_string(),
                ));
            };
            let mut keys: Vec<Key> = Vec::with_capacity(path.len());
            for entry in path {
                keys.push(path_key(ctx, entry)?);
            }
            Op::Idx {
                cached: flag(i, "cached"),
                push_path: flag(i, "pushPath"),
                path: keys.into_iter().collect(),
            }
        }
        "push" => Op::Push {
            storage: flag(i, "storage"),
            value: push_value(ctx, operand(i, "value")?)?,
        },
        "addi" => Op::Addi {
            immediate: resolve_immediate(ctx, operand(i, "immediate")?)?,
        },
        "ins" => Op::Ins {
            cached: flag(i, "cached"),
            n: u8_arg(i, "n")?,
        },
        "dup" => Op::Dup {
            n: u8_arg(i, "n").unwrap_or(0),
        },
        "swap" => Op::Swap {
            n: u8_arg(i, "n").unwrap_or(0),
        },
        "popeq" => Op::Popeq {
            cached: flag(i, "cached"),
            result: (),
        },
        "rem" => Op::Rem {
            cached: flag(i, "cached"),
        },
        "noop" => Op::Noop {
            n: u32_arg(i, "n")?,
        },
        "branch" => Op::Branch {
            skip: u32_arg(i, "skip")?,
        },
        "member" => Op::Member,
        "root" => Op::Root,
        "eq" => Op::Eq,
        "ckpt" => Op::Ckpt,
        "neg" => Op::Neg,
        "add" => Op::Add,
        other => {
            return Err(InterpreterError::Unsupported(format!(
                "VM instruction {other}"
            )));
        }
    })
}

fn flag(i: &ir::Instruction, name: &str) -> bool {
    matches!(i.arg(name), Some(ir::Operand::Bool(true)))
}

fn operand<'a>(i: &'a ir::Instruction, name: &str) -> Result<&'a ir::Operand, InterpreterError> {
    i.arg(name)
        .ok_or_else(|| InterpreterError::TypeError(format!("{}: missing operand {name}", i.op)))
}

fn u64_arg(i: &ir::Instruction, name: &str) -> Result<u64, InterpreterError> {
    match i.arg(name) {
        Some(ir::Operand::Int(n)) => u64::try_from(n)
            .map_err(|_| InterpreterError::TypeError(format!("{}: {name} out of range", i.op))),
        _ => Err(InterpreterError::TypeError(format!(
            "{}: missing integer {name}",
            i.op
        ))),
    }
}

fn u8_arg(i: &ir::Instruction, name: &str) -> Result<u8, InterpreterError> {
    u8::try_from(u64_arg(i, name)?)
        .map_err(|_| InterpreterError::TypeError(format!("{}: {name} out of u8 range", i.op)))
}

fn u32_arg(i: &ir::Instruction, name: &str) -> Result<u32, InterpreterError> {
    u32::try_from(u64_arg(i, name)?)
        .map_err(|_| InterpreterError::TypeError(format!("{}: {name} out of u32 range", i.op)))
}

/// One element of an `idx` path.
fn path_key(ctx: &mut ExecContext, entry: &ir::Operand) -> Result<Key, InterpreterError> {
    match entry {
        ir::Operand::Align { value, bytes } => Ok(Key::Value(literal_key(value, *bytes)?)),
        ir::Operand::Stack => Ok(Key::Stack),
        ir::Operand::Expr(e) => match e.as_ref() {
            // Resolve the local and encode it with its declared type when
            // known, so the key's alignment matches what the on-chain insert
            // produced (an Integer local of type Uint<16> must become a 2-byte
            // key, not the type-less 8-byte default).
            ir::Expr::VarRef(id) => match ctx.locals.get(id.0.as_str()) {
                Some(val @ (Value::Integer(_) | Value::AlignedValue(_) | Value::Bool(_))) => {
                    let local_ty = ctx.local_types.get(id.0.as_str());
                    match encode_ledger_key(val, local_ty)? {
                        StateValue::Cell(ref av) => Ok(Key::Value((**av).clone())),
                        other => Err(InterpreterError::TypeError(format!(
                            "variable `{}` did not encode to a cell key (got {other:?})",
                            id.0
                        ))),
                    }
                }
                _ => Err(InterpreterError::UndefinedVariable(id.0.clone())),
            },
            // Encode the computed value with its inferred type, so the key's
            // alignment matches what the insert that stored it produced.
            other => {
                let ty = infer_type_of_expr(ctx, other);
                let val = eval_expr(ctx, other)?;
                match encode_ledger_key(&val, ty.as_ref())? {
                    StateValue::Cell(ref av) => Ok(Key::Value((**av).clone())),
                    other => Err(InterpreterError::TypeError(format!(
                        "a computed ledger path element did not encode to a cell key \
                         (got {other:?})"
                    ))),
                }
            }
        },
        other => Err(InterpreterError::Unsupported(format!(
            "path element {other:?}"
        ))),
    }
}

/// The value a `push` puts on the query stack.
fn push_value(
    ctx: &mut ExecContext,
    o: &ir::Operand,
) -> Result<StateValue<InMemoryDB>, InterpreterError> {
    match reduce_operand(o) {
        // A pushed `(state-value-null)` (e.g. inserting into a Set, where the
        // "value" slot is just a marker). The on-chain `StateValue::Null`
        // field_repr is `[0]`, distinct from `StateValue::Cell(unit)` which is
        // `[1, 0]`.
        VmArg::Null | VmArg::Stack => Ok(StateValue::Null),
        // Infer the expression's declared type *before* evaluating, so a
        // `Value::Integer` is re-encoded with the right alignment. Without
        // this, a `request_id as Field` cast still pushes a u64-aligned key,
        // which never matches a `Map<Field, V>` entry that was inserted with
        // Field alignment on-chain.
        VmArg::Expr(e) => {
            let inferred = infer_type_of_expr(ctx, e);
            let val = eval_expr(ctx, e)?;
            encode_ledger_key(&val, inferred.as_ref())
        }
        VmArg::Literal { value, bytes } => Ok(StateValue::from(literal_key(value, bytes)?)),
        VmArg::Int(n) => {
            let n = u64::try_from(n).map_err(|_| {
                InterpreterError::TypeError(format!("push integer {n} out of u64 range"))
            })?;
            Ok(StateValue::from(AlignedValue::from(n)))
        }
        VmArg::Bool(b) => Ok(StateValue::from(AlignedValue::from(b))),
        VmArg::Vm(computed) => operand_aligned(ctx, computed).map(StateValue::from),
        other => Err(InterpreterError::Unsupported(format!(
            "push of a {} operand",
            other.kind()
        ))),
    }
}

/// Encode an operand as the [`AlignedValue`] a ledger key is built from.
///
/// Separate from [`push_value`] because a computed operand nests: an
/// `aligned-concat` joins the encodings of its own operands, which may be
/// computed in turn.
fn operand_aligned(
    ctx: &mut ExecContext,
    o: &ir::Operand,
) -> Result<AlignedValue, InterpreterError> {
    match reduce_operand(o) {
        VmArg::Expr(e) => {
            let inferred = infer_type_of_expr(ctx, e);
            let val = eval_expr(ctx, e)?;
            match (&val, inferred.as_ref()) {
                (Value::Integer(_), Some(ty)) => encode_typed(&val, ty),
                _ => value_aligned(&val),
            }
        }
        VmArg::Literal { value, bytes } => literal_key(value, bytes),
        VmArg::Int(n) => {
            let n = u64::try_from(n).map_err(|_| {
                InterpreterError::TypeError(format!("push integer {n} out of u64 range"))
            })?;
            Ok(AlignedValue::from(n))
        }
        VmArg::Bool(b) => Ok(AlignedValue::from(b)),
        // The ledger keys a token balance by its colour joined to the
        // recipient, so every unshielded token operation pushes one of these.
        VmArg::Vm(ir::Operand::AlignedConcat(parts)) => {
            let encoded = parts
                .iter()
                .map(|part| operand_aligned(ctx, part))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AlignedValue::concat(encoded.iter()))
        }
        _ => Err(InterpreterError::Unsupported(format!(
            "{o:?} as an aligned value"
        ))),
    }
}

/// The [`AlignedValue`] behind an evaluated [`Value`].
///
/// Mirrors `Value::to_state_value`, which wraps the same encoding in a
/// `StateValue`; a concatenation needs the value itself.
fn value_aligned(val: &Value) -> Result<AlignedValue, InterpreterError> {
    match val {
        Value::AlignedValue(av) => Ok(av.clone()),
        Value::Integer(n) => Ok(integer_fallback_aligned(*n)),
        Value::Bool(b) => Ok(AlignedValue::from(*b)),
        Value::Void => Ok(AlignedValue::from(())),
        other => Err(InterpreterError::Unsupported(format!(
            "aligned encoding of {other:?}"
        ))),
    }
}

/// Resolve an addi immediate value — either a literal number or an expression.
fn resolve_immediate(ctx: &mut ExecContext, o: &ir::Operand) -> Result<u32, InterpreterError> {
    match reduce_operand(o) {
        VmArg::Int(n) => u32::try_from(n).map_err(|_| {
            InterpreterError::TypeError(format!("addi immediate {n} out of u32 range"))
        }),
        VmArg::Expr(e) => {
            let val = eval_expr(ctx, e)?;
            val.as_u32().ok_or_else(|| {
                InterpreterError::TypeError(format!("addi immediate is not u32: {val:?}"))
            })
        }
        other => Err(InterpreterError::TypeError(format!(
            "cannot resolve a {} addi immediate",
            other.kind()
        ))),
    }
}

/// A `push` / `addi` operand reduced to what the interpreter acts on. The
/// wrappers the ledger DSL prints around a value (`value->int`, a cell, an
/// ADT) carry no width of their own, so they reduce to their contents.
enum VmArg<'a> {
    Int(&'a BigInt),
    Bool(bool),
    Str,
    Null,
    /// `(align value bytes)`: an aligned constant.
    Literal {
        value: &'a BigUint,
        bytes: u64,
    },
    Stack,
    Expr(&'a ir::Expr),
    State,
    Vm(&'a ir::Operand),
    List,
}

impl VmArg<'_> {
    fn kind(&self) -> &'static str {
        match self {
            VmArg::Int(_) => "integer",
            VmArg::Bool(_) => "boolean",
            VmArg::Str => "string",
            VmArg::Null => "null",
            VmArg::Literal { .. } | VmArg::Stack => "path-key",
            VmArg::Expr(_) => "expression",
            VmArg::State => "structured state-value",
            VmArg::Vm(_) => "vm-computed",
            VmArg::List => "operand list",
        }
    }
}

fn reduce_operand(o: &ir::Operand) -> VmArg<'_> {
    use ir::Operand as O;
    match o {
        O::Int(n) => VmArg::Int(n),
        O::Bool(b) => VmArg::Bool(*b),
        O::Str(_) => VmArg::Str,
        O::Align { value, bytes } => VmArg::Literal {
            value,
            bytes: *bytes,
        },
        O::Stack => VmArg::Stack,
        O::Void => VmArg::Null,
        O::ValueToInt(inner) => reduce_operand(inner),
        O::StateValue(ir::StateValue::Cell(inner) | ir::StateValue::Adt(inner, _)) => {
            reduce_operand(inner)
        }
        O::StateValue(ir::StateValue::Null) => VmArg::Null,
        O::StateValue(_) => VmArg::State,
        O::Null(_)
        | O::MaxSizeof(_)
        | O::Add(..)
        | O::LeafHash(_)
        | O::CoinCommit(..)
        | O::AlignedConcat(_) => VmArg::Vm(o),
        O::Expr(e) => VmArg::Expr(e),
        O::List(_) => VmArg::List,
    }
}

/// Encode an evaluated [`Value`] as a [`StateValue`] for pushing onto
/// the ledger query stack, re-aligning integers to the expression's
/// declared [`Type`] when known.
///
/// The default [`Value::to_state_value`] conversion throws away type
/// information and encodes integers at the u64 width. That's fine for
/// arithmetic but wrong wherever the on-chain encoding is width-sensitive
/// (e.g. `Map<Field, _>` or `Map<Uint<16>, _>` keys): the insert path
/// produces a key with the declared type's alignment, while a u64-aligned
/// off-chain key would never match it. When the declared type is known,
/// integers are routed through [`encode_typed`] so the alignment matches
/// the insert path byte-for-byte; out-of-range integers error instead of
/// wrapping. Everything else (pre-encoded `AlignedValue`s, booleans,
/// type-less integers) keeps the `to_state_value` behavior.
fn encode_ledger_key(
    val: &Value,
    ty: Option<&Type>,
) -> Result<StateValue<InMemoryDB>, InterpreterError> {
    match (val, ty) {
        (Value::Integer(_), Some(ty)) => encode_typed(val, ty).map(StateValue::from),
        _ => Ok(val.to_state_value()),
    }
}

/// The `AlignedValue` of an `(align value bytes)` constant: a literal ledger
/// key, encoded at the width the instruction declares.
fn literal_key(value: &BigUint, bytes: u64) -> Result<AlignedValue, InterpreterError> {
    path_value_to_aligned(&value.to_string(), &Type::Unsigned(max_for_bytes(bytes)))
}

fn max_for_bytes(bytes: u64) -> BigUint {
    (BigUint::from(1u8) << (8 * bytes)) - 1u8
}

/// Convert a literal path value string + declared type to an `AlignedValue`,
/// delegating the width-sensitive encoding to [`encode_typed`].
fn path_value_to_aligned(value: &str, ty: &Type) -> Result<AlignedValue, InterpreterError> {
    match ty {
        Type::Boolean => Ok(AlignedValue::from(value == "true" || value == "1")),
        Type::Unsigned(_) | Type::Field(_) | Type::Enum { .. } => {
            let n: u128 = value.parse().map_err(|e| {
                InterpreterError::TypeError(format!(
                    "invalid integer path literal {value:?} for {ty:?}: {e}"
                ))
            })?;
            encode_typed(&Value::Integer(n), ty)
        }
        // Refuse rather than guess: these are not value types, and
        // `check_type` rejects them.
        Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => Err(InterpreterError::Unsupported(
            format!("a path key of type {ty:?}"),
        )),
        _ => {
            // Best-effort fallback for types the compiler is not expected
            // to emit as literal path keys: parse as an integer and use the
            // type-less width rules (see `integer_fallback_aligned`).
            if let Ok(n) = value.parse::<u128>() {
                Ok(integer_fallback_aligned(n))
            } else {
                Ok(AlignedValue::from(0u8))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use compact_runtime::try_builtin;
    use ir::FieldType;
    use midnight_typed_state::{ContractMaintenanceAuthority, StorageHashMap};

    fn field() -> Type {
        Type::Field(FieldType::Native)
    }

    fn uint(maxval: &str) -> Type {
        Type::Unsigned(maxval.parse().expect("a decimal maxval"))
    }

    fn vector(len: u64, ty: Type) -> Type {
        Type::Vector {
            len,
            ty: Box::new(ty),
        }
    }

    fn ident(text: &str) -> ir::Ident {
        ir::Ident(text.to_string())
    }

    fn var(name: &str) -> ir::Expr {
        ir::Expr::VarRef(ident(name))
    }

    fn int(n: u128) -> ir::Expr {
        ir::Expr::Quote(ir::Literal::Int(BigInt::from(n)))
    }

    fn bytes(hex_bytes: &str) -> ir::Expr {
        ir::Expr::Quote(ir::Literal::Bytes(
            hex::decode(hex_bytes).expect("hex digits"),
        ))
    }

    fn single(expr: ir::Expr) -> ir::TupleArg {
        ir::TupleArg::Single(expr)
    }

    fn spread(len: u64, expr: ir::Expr) -> ir::TupleArg {
        ir::TupleArg::Spread { len, expr }
    }

    fn argument(name: &str, ty: Type) -> ir::Argument {
        ir::Argument {
            name: ident(name),
            ty,
        }
    }

    fn circuit(arguments: Vec<ir::Argument>, result_type: Type, body: ir::Expr) -> ir::Circuit {
        ir::Circuit {
            name: ident("%test.0"),
            exported: true,
            pure: false,
            proof: true,
            arguments,
            result_type,
            body,
        }
    }

    fn instruction(op: &str, args: Vec<(&str, ir::Operand)>) -> ir::Instruction {
        ir::Instruction {
            op: op.to_string(),
            args: args
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect(),
        }
    }

    fn make_counter_state(round: u64) -> ContractState<InMemoryDB> {
        // Counter contract state: Array(1) [ Cell(round) ]
        let root = StateValue::Array(vec![StateValue::from(round)].into());
        ContractState::new(
            root,
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }

    /// Execute a circuit over the counter fixture state.
    fn execute_in(
        circuit: &ir::Circuit,
        program: &Program<'_>,
        args: &[(&str, Value)],
    ) -> Result<ExecutionResult, InterpreterError> {
        let state = make_counter_state(0);
        execute_with(circuit, program, &state, args, &NoWitnesses)
    }

    /// Execute a circuit that calls nothing, and take its value.
    fn run(circuit: &ir::Circuit, args: &[(&str, Value)]) -> Result<Value, InterpreterError> {
        let program = Program::new(&[], &[], &[]);
        execute_in(circuit, &program, args).map(|r| r.result.expect("a result value"))
    }

    /// Evaluate one expression as a whole circuit body.
    fn eval(body: ir::Expr, result_type: Type) -> Result<Value, InterpreterError> {
        run(&circuit(Vec::new(), result_type, body), &[])
    }

    /// The counter contract's `increment` body: add 1 to the round cell.
    fn increment_round() -> ir::Expr {
        ir::Expr::LetStar {
            bindings: vec![(argument("%tmp.1", uint("65535")), int(1))],
            body: Box::new(ir::Expr::PublicLedger {
                op_class: ir::OpClass::Plain("update".into()),
                field: ident("%round.2"),
                path: Vec::new(),
                op: "increment".to_string(),
                result_type: Type::unit(),
                instructions: vec![
                    instruction(
                        "idx",
                        vec![
                            ("cached", ir::Operand::Bool(false)),
                            ("pushPath", ir::Operand::Bool(true)),
                            (
                                "path",
                                ir::Operand::List(vec![ir::Operand::Align {
                                    value: BigUint::from(0u8),
                                    bytes: 1,
                                }]),
                            ),
                        ],
                    ),
                    instruction(
                        "addi",
                        vec![(
                            "immediate",
                            ir::Operand::ValueToInt(Box::new(ir::Operand::Expr(Box::new(var(
                                "%tmp.1",
                            ))))),
                        )],
                    ),
                    instruction(
                        "ins",
                        vec![
                            ("cached", ir::Operand::Bool(true)),
                            ("n", ir::Operand::Int(BigInt::from(1))),
                        ],
                    ),
                ],
                args: Vec::new(),
            }),
        }
    }

    fn counter_cell(state: &ContractState<InMemoryDB>) -> u64 {
        match state.data.get_ref() {
            StateValue::Array(arr) => match arr.get(0).expect("field 0") {
                StateValue::Cell(sp) => u64::try_from(&*sp.value).expect("u64"),
                other => panic!("expected Cell, got {other:?}"),
            },
            other => panic!("expected Array root, got {other:?}"),
        }
    }

    #[test]
    fn execute_counter_increment() {
        let state = make_counter_state(0);
        let circ = circuit(
            Vec::new(),
            Type::unit(),
            ir::Expr::Seq(vec![increment_round(), ir::Expr::Tuple(Vec::new())]),
        );
        let program = Program::new(&[], &[], &[]);
        let result = execute(&circ, &program, &state).expect("execute increment");
        assert_eq!(counter_cell(&result.state), 1, "counter should be 1");
    }

    #[test]
    fn execute_counter_increment_nonzero() {
        let state = make_counter_state(42);
        let circ = circuit(Vec::new(), Type::unit(), increment_round());
        let program = Program::new(&[], &[], &[]);
        let result = execute(&circ, &program, &state).expect("execute increment");
        assert_eq!(counter_cell(&result.state), 43, "counter should be 43");
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

    #[test]
    fn values_equal_decodes_fab_atoms_little_endian() {
        use midnight_base_crypto::fab;
        // FAB atoms are zero-trimmed little-endian bytes (`ValueAtom`
        // conversions in midnight-base-crypto fab/conversions.rs): the atom
        // [0x2C, 0x01] is 300. A big-endian decode would read 0x2C01 = 11265
        // and silently flip equality results, e.g. a `popeq` read of a
        // Cell<Uint<16>> holding 300 compared against an integer literal.
        let av = fab::AlignedValue::new(
            fab::Value(vec![fab::ValueAtom(vec![0x2C, 0x01])]),
            fab::Alignment::singleton(fab::AlignmentAtom::Bytes { length: 2 }),
        )
        .unwrap();
        // Sanity: this is exactly the FAB encoding of 300u16.
        assert_eq!(av, AlignedValue::from(300u16));
        assert!(values_equal(
            &Value::AlignedValue(av.clone()),
            &Value::Integer(300)
        ));
        assert!(values_equal(
            &Value::Integer(300),
            &Value::AlignedValue(av.clone())
        ));
        assert!(!values_equal(
            &Value::AlignedValue(av),
            &Value::Integer(11265)
        ));
    }

    #[test]
    fn arithmetic_falls_back_to_field_for_wide_operands() {
        use midnight_transient_crypto::curve::Fr;
        use midnight_transient_crypto::hash::transient_hash;

        let as_fr = |v: &Value| -> Fr {
            match v {
                Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
                Value::Integer(n) => Fr::from(*n),
                other => panic!("not a field value: {other:?}"),
            }
        };

        // A Poseidon output is a full-width field element (wider than u128) — the
        // shape that fed the "expected integer" failure in a gateway committee
        // signature's mod-r reduction `c_native - challenge_quotient * order`.
        let c_native = transient_hash(&[Fr::from(1u64), Fr::from(2u64), Fr::from(3u64)]);
        let order = transient_hash(&[Fr::from(9u64)]);

        // Subtraction with a wide operand must evaluate over Fr, not reject it.
        let sub = run(
            &circuit(
                vec![argument("a", field()), argument("b", field())],
                field(),
                ir::Expr::Sub {
                    ty: field(),
                    left: Box::new(var("a")),
                    right: Box::new(var("b")),
                },
            ),
            &[
                ("a", Value::AlignedValue(AlignedValue::from(c_native))),
                ("b", fr_value(5)),
            ],
        )
        .expect("field subtraction must not reject a wide operand");
        assert_eq!(as_fr(&sub), Fr(c_native.0 - Fr::from(5u64).0));

        // The full mod-r reduction shape, all field arithmetic.
        let c = run(
            &circuit(
                vec![
                    argument("c", field()),
                    argument("q", field()),
                    argument("o", field()),
                ],
                field(),
                ir::Expr::Sub {
                    ty: field(),
                    left: Box::new(var("c")),
                    right: Box::new(ir::Expr::Mul {
                        ty: field(),
                        left: Box::new(var("q")),
                        right: Box::new(var("o")),
                    }),
                },
            ),
            &[
                ("c", Value::AlignedValue(AlignedValue::from(c_native))),
                ("q", fr_value(3)),
                ("o", Value::AlignedValue(AlignedValue::from(order))),
            ],
        )
        .expect("field reduction must evaluate");
        assert_eq!(as_fr(&c), Fr(c_native.0 - Fr::from(3u64).0 * order.0));

        // Operands that fit u128 keep the historical integer semantics.
        let int_sum = run(
            &circuit(
                vec![argument("a", field()), argument("b", field())],
                field(),
                ir::Expr::Add {
                    ty: field(),
                    left: Box::new(var("a")),
                    right: Box::new(var("b")),
                },
            ),
            &[("a", Value::Integer(2)), ("b", Value::Integer(3))],
        )
        .expect("integer add");
        assert!(
            matches!(int_sum, Value::Integer(5)),
            "operands that fit u128 keep integer semantics, got {int_sum:?}"
        );
    }

    #[test]
    fn large_field_literal_parses_as_field_element() {
        use midnight_transient_crypto::curve::Fr;

        // JUBJUB_ORDER (~2^252) exceeds u128, so it folds into a field element
        // instead of parsing as an integer.
        let order = "6554484396890773809930967563523245729705921265872317281365359162392183254199";
        let literal = ir::Expr::Quote(ir::Literal::Int(
            order.parse::<BigInt>().expect("a decimal literal"),
        ));
        let result = eval(literal, field()).expect("a Field literal wider than u128 must parse");
        let got = match result {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected a Field AlignedValue, got {other:?}"),
        };

        // Independently: the parsed element equals JUBJUB_ORDER's little-endian bytes.
        let order_le: [u8; 32] = [
            0xb7, 0x2c, 0xf7, 0xd6, 0x5e, 0x0e, 0x97, 0xd0, 0x82, 0x10, 0xc8, 0xcc, 0x93, 0x20,
            0x68, 0xa6, 0x00, 0x3b, 0x34, 0x01, 0x01, 0x3b, 0x67, 0x06, 0xa9, 0xaf, 0x33, 0x65,
            0xea, 0xb4, 0x7d, 0x0e,
        ];
        assert_eq!(got, Fr::from_le_bytes(&order_le).unwrap());

        // Small literals still carry as integer values.
        let small = eval(int(7), field()).unwrap();
        assert!(matches!(small, Value::Integer(7)));
    }

    // -----------------------------------------------------------------------
    // Names, calls and sequencing
    // -----------------------------------------------------------------------

    #[test]
    fn arguments_bind_by_source_name() {
        // The caller keys arguments by the source name; the body refers to the
        // full identifier, and that is what the binding is keyed by.
        let circ = circuit(
            vec![argument("%round.7", uint("65535"))],
            uint("65535"),
            var("%round.7"),
        );
        let value = run(&circ, &[("round", Value::Integer(9))]).expect("the argument binds");
        assert!(values_equal(&value, &Value::Integer(9)), "got {value:?}");

        // An argument the caller did not supply is undefined, not defaulted.
        let err = run(&circ, &[("other", Value::Integer(9))]).expect_err("no value for `round`");
        assert!(
            matches!(err, InterpreterError::UndefinedVariable(ref name) if name == "%round.7"),
            "expected an undefined-variable error, got {err:?}"
        );
    }

    #[test]
    fn a_call_runs_the_callee_circuit_body() {
        // `double(x) = x + x`, called with 21.
        let callee = ir::Circuit {
            name: ident("%double.3"),
            exported: false,
            pure: true,
            proof: false,
            arguments: vec![argument("%x.4", uint("255"))],
            result_type: uint("255"),
            body: ir::Expr::Add {
                ty: uint("255"),
                left: Box::new(var("%x.4")),
                right: Box::new(var("%x.4")),
            },
        };
        let circuits = vec![callee];
        let program = Program::new(&circuits, &[], &[]);
        let circ = circuit(
            Vec::new(),
            uint("255"),
            ir::Expr::Call {
                name: ident("%double.3"),
                args: vec![int(21)],
            },
        );
        let result = execute_in(&circ, &program, &[])
            .expect("the call runs")
            .result
            .expect("a result value");
        assert!(values_equal(&result, &Value::Integer(42)), "got {result:?}");
    }

    #[test]
    fn a_circuit_shadowing_a_builtin_name_wins() {
        // A circuit named `persistentHash` is a distinct identifier, and the
        // call site names it: the builtin must not intercept it.
        let callee = ir::Circuit {
            name: ident("%persistentHash.5"),
            exported: false,
            pure: true,
            proof: false,
            arguments: Vec::new(),
            result_type: uint("255"),
            body: int(7),
        };
        let circuits = vec![callee];
        let program = Program::new(&circuits, &[], &[]);
        let circ = circuit(
            Vec::new(),
            uint("255"),
            ir::Expr::Call {
                name: ident("%persistentHash.5"),
                args: Vec::new(),
            },
        );
        let result = execute_in(&circ, &program, &[])
            .expect("the circuit runs")
            .result
            .expect("a result value");
        assert!(
            values_equal(&result, &Value::Integer(7)),
            "the circuit must win over the builtin, got {result:?}"
        );
    }

    #[test]
    fn a_native_call_reaches_the_builtin() {
        use midnight_transient_crypto::curve::Fr;
        use midnight_transient_crypto::hash::transient_hash;

        let natives = vec![ir::Native {
            type_arguments: Vec::new(),
            name: ident("%transientHash.6"),
            entry: "__compactRuntime.transientHash".to_string(),
            class: "circuit".to_string(),
            arguments: vec![argument("%value.7", field())],
            result_type: field(),
        }];
        let program = Program::new(&[], &[], &natives);
        let circ = circuit(
            Vec::new(),
            field(),
            ir::Expr::Call {
                name: ident("%transientHash.6"),
                args: vec![int(3)],
            },
        );
        let result = execute_in(&circ, &program, &[])
            .expect("the builtin runs")
            .result
            .expect("a result value");
        let got = match result {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        assert_eq!(got, transient_hash(&[Fr::from(3u64)]));
    }

    #[test]
    fn a_witness_call_records_a_private_transcript_output() {
        struct Secret;
        impl WitnessProvider for Secret {
            fn call_witness(
                &self,
                _ctx: &mut WitnessContext<'_>,
                name: &str,
                _args: &[Value],
            ) -> Result<WitnessOutcome, InterpreterError> {
                assert_eq!(name, "secret_key", "witnesses are called by source name");
                Ok(WitnessOutcome::Value(Value::Integer(5)))
            }
        }

        let witnesses = vec![ir::Witness {
            name: ident("%secret_key.8"),
            arguments: Vec::new(),
            result_type: uint("255"),
        }];
        let program = Program::new(&[], &witnesses, &[]);
        let circ = circuit(
            Vec::new(),
            uint("255"),
            ir::Expr::Call {
                name: ident("%secret_key.8"),
                args: Vec::new(),
            },
        );
        let state = make_counter_state(0);
        let result = execute_with(&circ, &program, &state, &[], &Secret).expect("the witness runs");
        assert_eq!(
            result.private_transcript_outputs.len(),
            1,
            "a witness value joins the private transcript"
        );
        assert!(values_equal(
            &result.result.expect("a result value"),
            &Value::Integer(5)
        ));
    }

    #[test]
    fn a_sequence_runs_every_item_and_takes_the_last() {
        let state = make_counter_state(0);
        let circ = circuit(
            Vec::new(),
            uint("255"),
            ir::Expr::Seq(vec![increment_round(), increment_round(), int(3)]),
        );
        let program = Program::new(&[], &[], &[]);
        let result = execute(&circ, &program, &state).expect("execute the sequence");
        assert_eq!(counter_cell(&result.state), 2, "both items ran");
        assert!(values_equal(
            &result.result.expect("a result value"),
            &Value::Integer(3)
        ));
    }

    #[test]
    fn an_emit_is_unsupported() {
        let err = eval(
            ir::Expr::Emit {
                event_version: 1,
                event_tag: 2,
                len: 0,
                payload: Box::new(ir::Expr::Tuple(Vec::new())),
                instructions: Vec::new(),
            },
            Type::unit(),
        )
        .expect_err("events are not executable");
        assert!(
            matches!(err, InterpreterError::Unsupported(_)),
            "expected Unsupported, got {err:?}"
        );
    }

    #[test]
    fn a_secp256k1_type_is_refused_by_name() {
        let err = eval(
            ir::Expr::Default(Type::Field(FieldType::Scalar(ir::Curve::Secp256k1))),
            Type::unit(),
        )
        .expect_err("a foreign field is not executable");
        match err {
            InterpreterError::Unsupported(ref msg) => {
                assert!(msg.contains("secp256k1"), "error should name it: {msg}")
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
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

        let x_result = try_builtin("jubjubPointX", std::slice::from_ref(&p_value))
            .expect("builtin known")
            .expect("ok");
        let y_result = try_builtin("jubjubPointY", std::slice::from_ref(&p_value))
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

    /// The `persistentCommit` builtin must reproduce the ledger's own
    /// `ContractAddress::custom_shielded_token_type`, which is how a minted
    /// coin's color (token type) is derived: `persistentCommit((domain_sep,
    /// self().bytes), "midnight:derive_token\0..")`. If these disagree the
    /// minted coin's token type won't match what the chain records and the
    /// recipient's wallet won't recognise the coin.
    #[test]
    fn persistent_commit_matches_custom_shielded_token_type() {
        use midnight_base_crypto::hash::HashOutput;
        use midnight_coin_structure::contract::ContractAddress;

        let domain_sep = [0x11u8; 32];
        let address = ContractAddress(HashOutput([0xABu8; 32]));

        // Ledger-side derivation (the on-chain truth).
        let expected = address.custom_shielded_token_type(HashOutput(domain_sep)).0;

        // Interpreter-side: persistentCommit((domain_sep, address.bytes),
        // "midnight:derive_token\0..").
        let inner_domain = *b"midnight:derive_token\0\0\0\0\0\0\0\0\0\0\0";
        let value = Value::Tuple(vec![
            Value::AlignedValue(AlignedValue::from(domain_sep)),
            Value::AlignedValue(AlignedValue::from(address.0.0)),
        ]);
        let opening = Value::AlignedValue(AlignedValue::from(inner_domain));

        let via_builtin = try_builtin("persistentCommit", &[value, opening])
            .expect("persistentCommit is a known builtin")
            .expect("persistentCommit succeeds");
        let got = match via_builtin {
            Value::AlignedValue(av) => {
                let atom = &av.value.0[0];
                let mut b = [0u8; 32];
                b[..atom.0.len()].copy_from_slice(&atom.0);
                b
            }
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        assert_eq!(
            got, expected.0,
            "persistentCommit must match ContractAddress::custom_shielded_token_type"
        );
    }

    #[test]
    fn transient_commit_matches_direct_call() {
        use midnight_transient_crypto::curve::Fr;
        use midnight_transient_crypto::fab::ValueReprAlignedValue;
        use midnight_transient_crypto::hash::transient_commit;
        let value = Value::AlignedValue(AlignedValue::from([0x11u8; 32]));
        let got = match try_builtin("transientCommit", &[value.clone(), fr_value(42)])
            .expect("builtin known")
            .expect("ok")
        {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        let expected = transient_commit(
            &ValueReprAlignedValue(value.try_to_aligned_value().unwrap()),
            Fr::from(42u64),
        );
        assert_eq!(got, expected);
    }

    #[test]
    fn upgrade_from_transient_matches_direct_call() {
        use midnight_transient_crypto::curve::Fr;
        use midnight_transient_crypto::hash::upgrade_from_transient;
        let got = match try_builtin("upgradeFromTransient", &[fr_value(7)])
            .expect("builtin known")
            .expect("ok")
        {
            Value::AlignedValue(av) => {
                let atom = &av.value.0[0];
                let mut b = [0u8; 32];
                b[..atom.0.len()].copy_from_slice(&atom.0);
                b
            }
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        assert_eq!(got, upgrade_from_transient(Fr::from(7u64)).0);
    }

    #[test]
    fn hash_to_curve_matches_direct_call() {
        use midnight_transient_crypto::curve::EmbeddedGroupAffine;
        use midnight_transient_crypto::fab::ValueReprAlignedValue;
        use midnight_transient_crypto::hash::hash_to_curve;
        let value = Value::AlignedValue(AlignedValue::from([0x09u8; 32]));
        let got = match try_builtin("hashToCurve", std::slice::from_ref(&value))
            .expect("builtin known")
            .expect("ok")
        {
            Value::AlignedValue(av) => EmbeddedGroupAffine::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        let expected = hash_to_curve(&ValueReprAlignedValue(
            value.try_to_aligned_value().unwrap(),
        ));
        assert_eq!(got, expected);
    }

    #[test]
    fn construct_jubjub_point_rebuilds_the_generator() {
        use midnight_transient_crypto::curve::EmbeddedGroupAffine;
        let g = EmbeddedGroupAffine::generator();
        let x = g.x().unwrap();
        let y = g.y().unwrap();
        let got = match try_builtin(
            "constructJubjubPoint",
            &[
                Value::AlignedValue(AlignedValue::from(x)),
                Value::AlignedValue(AlignedValue::from(y)),
            ],
        )
        .expect("builtin known")
        .expect("ok")
        {
            Value::AlignedValue(av) => EmbeddedGroupAffine::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        assert_eq!(got, g);
    }

    /// Every Compact native primitive must be dispatched by the interpreter,
    /// either implemented (`try_builtin` arm or [`WitnessNative`]) or explicitly
    /// listed as known-unimplemented. A new native that is neither fails here
    /// instead of surfacing as a runtime miss deep in a circuit. See
    /// `docs/compact-natives.md`.
    #[test]
    fn every_compact_native_is_handled_or_known_unimplemented() {
        // The `declare-native-entry` names from the compiler's
        // tools/compact-compiler/compiler/midnight-natives.ss, transcribed so
        // the test does not depend on the (CI-absent) compiler submodule. The
        // cross-check below re-derives this list from the submodule when present.
        const EXPECTED: &[&str] = &[
            // circuit (pure) natives
            "transientHash",
            "transientCommit",
            "persistentHash",
            "persistentCommit",
            "degradeToTransient",
            "upgradeFromTransient",
            "keccak256",
            "jubjubPointX",
            "jubjubPointY",
            "ecAdd",
            "ecMul",
            "ecMulGenerator",
            "hashToCurve",
            "constructJubjubPoint",
            "jubjubScalarFromNative",
            // witness natives
            "ownPublicKey",
            "createZswapInput",
            "createZswapOutput",
        ];
        // Natives with no upstream primitive to bind to yet. Recognized witness
        // natives are NOT here: they are dispatched by `WitnessNative` and count
        // as handled — `createZswapInput`/`createZswapOutput` capture their coin
        // args, `ownPublicKey` still errors explicitly. See docs/compact-natives.md.
        const KNOWN_UNIMPLEMENTED: &[&str] = &["keccak256", "jubjubScalarFromNative"];

        for name in EXPECTED {
            let handled =
                WitnessNative::from_name(name).is_some() || try_builtin(name, &[]).is_some();
            let known_unimplemented = KNOWN_UNIMPLEMENTED.contains(name);
            assert!(
                handled || known_unimplemented,
                "Compact native `{name}` is neither implemented (try_builtin/WitnessNative) nor \
                 listed as known-unimplemented. Implement it or add it to KNOWN_UNIMPLEMENTED \
                 (and update docs/compact-natives.md)."
            );
            // Keep KNOWN_UNIMPLEMENTED honest: a native that is now implemented
            // must be removed from the list, otherwise the allowlist silently
            // goes stale and the docs drift.
            assert!(
                !(handled && known_unimplemented),
                "Compact native `{name}` is now implemented but still in \
                 KNOWN_UNIMPLEMENTED. Remove it from the list (and update \
                 docs/compact-natives.md)."
            );
        }

        // When the compiler submodule is checked out (developer machines, not
        // CI), re-derive the native list from source and assert it matches
        // EXPECTED, so a compiler bump that adds or removes a native fails here.
        let natives_ss = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tools/compact-compiler/compiler/midnight-natives.ss"
        );
        if let Ok(src) = std::fs::read_to_string(natives_ss) {
            let mut from_source: Vec<String> = src
                .lines()
                .filter_map(|l| l.trim().strip_prefix("(declare-native-entry "))
                .filter_map(|rest| rest.split_whitespace().nth(1))
                .map(str::to_string)
                .collect();
            from_source.sort();
            from_source.dedup();
            let mut expected: Vec<String> = EXPECTED.iter().map(|s| s.to_string()).collect();
            expected.sort();
            assert_eq!(
                from_source, expected,
                "midnight-natives.ss changed: update EXPECTED and docs/compact-natives.md"
            );
        }
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

    /// The stdlib hands `transientHash` its input as a struct, so the digest has
    /// to follow the declared field order. The value carries a map, whose own
    /// order is unspecified, so this pins the type as the source of truth: the
    /// struct hashes exactly as the flat sequence does.
    #[test]
    fn transient_hash_orders_a_struct_by_its_declared_type() {
        use compact_codegen::ir::{FieldType, Type};
        use midnight_transient_crypto::curve::Fr;
        use midnight_transient_crypto::hash::transient_hash;

        let field = || Type::Field(FieldType::Native);
        let ty = Type::Struct {
            name: "JubjubSchnorrHashInput".to_string(),
            fields: vec![
                ("annX".to_string(), field()),
                ("annY".to_string(), field()),
                (
                    "msg".to_string(),
                    Type::Vector {
                        len: 1,
                        ty: Box::new(field()),
                    },
                ),
            ],
        };

        // Insertion order deliberately differs from the declared order.
        let mut fields = std::collections::HashMap::new();
        fields.insert("msg".to_string(), Value::Tuple(vec![fr_value(3)]));
        fields.insert("annY".to_string(), fr_value(2));
        fields.insert("annX".to_string(), fr_value(1));

        let got = match try_builtin_typed("transientHash", &[Value::Struct(fields)], &[Some(ty)])
            .expect("builtin known")
            .expect("ok")
        {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };

        assert_eq!(
            got,
            transient_hash(&[Fr::from(1u64), Fr::from(2u64), Fr::from(3u64)]),
            "a struct must hash as its declared fields in order, flattening nested vectors"
        );
    }

    /// A struct nested in a vector is encoded from the argument's own type, so
    /// the whole argument goes to `encode_typed` in one piece. Encoding element
    /// by element would hand each element the vector's type and reject it.
    #[test]
    fn transient_hash_orders_a_struct_nested_in_a_vector() {
        use compact_codegen::ir::{FieldType, Type};
        use midnight_transient_crypto::curve::Fr;
        use midnight_transient_crypto::hash::transient_hash;

        let field = || Type::Field(FieldType::Native);
        let element = Type::Struct {
            name: "Pair".to_string(),
            fields: vec![("x".to_string(), field()), ("y".to_string(), field())],
        };
        let ty = Type::Vector {
            len: 1,
            ty: Box::new(element),
        };

        let mut pair = std::collections::HashMap::new();
        pair.insert("y".to_string(), fr_value(9));
        pair.insert("x".to_string(), fr_value(8));
        let arg = Value::Tuple(vec![Value::Struct(pair)]);

        let got = match try_builtin_typed("transientHash", &[arg], &[Some(ty)])
            .expect("builtin known")
            .expect("a struct inside a vector must hash")
        {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };

        assert_eq!(got, transient_hash(&[Fr::from(8u64), Fr::from(9u64)]));
    }

    /// Without the declared type there is no field order or width to follow.
    /// Hashing the map's own order would produce a digest that merely looks
    /// valid, so the builtin refuses instead.
    #[test]
    fn transient_hash_refuses_a_struct_with_no_declared_type() {
        let mut fields = std::collections::HashMap::new();
        fields.insert("annX".to_string(), fr_value(1));

        let result = try_builtin("transientHash", &[Value::Struct(fields)]).expect("builtin known");
        let message = result
            .expect_err("a struct with no type must not hash")
            .to_string();
        assert!(
            message.contains("without its declared type"),
            "the error should name the missing type, got: {message}"
        );
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

    // -----------------------------------------------------------------------
    // Integer width: values above u64::MAX must never be truncated
    // -----------------------------------------------------------------------

    /// 2^64 as an `Fr`, computed from u64 limbs only — an independent path
    /// that cannot share a bug with `From<u128> for Fr`.
    fn fr_two_pow_64() -> midnight_transient_crypto::curve::Fr {
        use midnight_transient_crypto::curve::Fr;
        Fr::from(u64::MAX) + Fr::from(1u64)
    }

    #[test]
    fn value_to_fr_is_exact_above_u64() {
        use midnight_transient_crypto::curve::Fr;
        let k = 999u64;
        let n = (1u128 << 64) + k as u128;
        let expected = fr_two_pow_64() + Fr::from(k);
        assert_eq!(value_to_fr(&Value::Integer(n)), Some(expected));
    }

    #[test]
    fn transient_hash_field_arg_above_u64_matches_direct() {
        use midnight_transient_crypto::curve::Fr;
        use midnight_transient_crypto::hash::transient_hash;

        let n = (1u128 << 64) + 7;
        let expected_fr = fr_two_pow_64() + Fr::from(7u64);
        let direct = transient_hash(&[expected_fr]);

        let via_builtin = try_builtin("transientHash", &[Value::Integer(n)])
            .expect("builtin known")
            .expect("ok");
        let got = match via_builtin {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        assert_eq!(got, direct);
    }

    #[test]
    fn persistent_hash_integer_above_u64_matches_direct() {
        use midnight_base_crypto::hash::PersistentHashWriter;
        use midnight_base_crypto::repr::BinaryHashRepr;
        use midnight_transient_crypto::curve::Fr;
        use midnight_transient_crypto::fab::ValueReprAlignedValue;

        let n = (1u128 << 64) + 3;
        // Expected hash computed through an independent conversion path
        // (u64 limb arithmetic), then the same hashing primitives the
        // builtin uses.
        let expected_fr = fr_two_pow_64() + Fr::from(3u64);
        let mut hasher = PersistentHashWriter::default();
        ValueReprAlignedValue(AlignedValue::from(expected_fr)).binary_repr(&mut hasher);
        let expected = hasher.finalize();

        let via_builtin = try_builtin("persistentHash", &[Value::Integer(n)])
            .expect("builtin known")
            .expect("ok");
        match via_builtin {
            Value::AlignedValue(av) => assert_eq!(av, AlignedValue::from(expected.0)),
            other => panic!("expected AlignedValue, got {other:?}"),
        }
    }

    #[test]
    fn typeless_fallback_keeps_u64_width_and_widens_above() {
        // Values that fit u64 must keep the historical Bytes{8} alignment
        // (byte-compatibility with existing encodings).
        assert_eq!(
            Value::Integer(5).try_to_aligned_value().unwrap(),
            AlignedValue::from(5u64)
        );
        assert_eq!(
            Value::Integer(u64::MAX as u128)
                .try_to_aligned_value()
                .unwrap(),
            AlignedValue::from(u64::MAX)
        );
        // Values above u64::MAX must be encoded wide, not truncated.
        let big = u64::MAX as u128 + 1;
        assert_eq!(
            Value::Integer(big).try_to_aligned_value().unwrap(),
            AlignedValue::from(big)
        );
        let decoded = match Value::Integer(big).to_state_value() {
            StateValue::Cell(ref sp) => u128::try_from(&*sp.value).expect("decode u128"),
            other => panic!("expected Cell, got {other:?}"),
        };
        assert_eq!(decoded, big);
    }

    #[test]
    fn typed_uint_encode_roundtrips_above_u64() {
        let big = (1u128 << 64) + 12345;
        let ty = Type::Unsigned(BigUint::from(u128::MAX));
        let av = encode_typed(&Value::Integer(big), &ty).expect("encode");
        // Byte-for-byte the u128 encoding (atom + Bytes{16} alignment)...
        assert_eq!(av, AlignedValue::from(big));
        // ...and decodes back without losing the high bits.
        assert_eq!(u128::try_from(&*av.value).expect("decode"), big);
    }

    #[test]
    fn typed_uint_encode_rejects_out_of_range() {
        let ty = uint("255");
        assert!(encode_typed(&Value::Integer(255), &ty).is_ok());
        let err = encode_typed(&Value::Integer(300), &ty).expect_err("out of range");
        assert!(matches!(err, InterpreterError::TypeError(_)));
        // Enum indices are u8 on-chain; anything wider must error too.
        let err = encode_typed(
            &Value::Integer(300),
            &Type::Enum {
                name: "Whatever".to_string(),
                variants: Vec::new(),
            },
        )
        .expect_err("enum index out of range");
        assert!(matches!(err, InterpreterError::TypeError(_)));
    }

    #[test]
    fn typed_uint_encode_uses_declared_width() {
        // The ladder must match the bindgen-emitted encoders: Uint<=65535>
        // is a u16 (2-byte) atom, not the type-less 8-byte default.
        let ty = uint("65535");
        let av = encode_typed(&Value::Integer(7), &ty).expect("encode");
        assert_eq!(av, AlignedValue::from(7u16));

        // encode_ledger_key routes typed integers through the same encoder.
        let sv = encode_ledger_key(&Value::Integer(7), Some(&ty)).expect("encode key");
        match sv {
            StateValue::Cell(ref sp) => assert_eq!((**sp).clone(), AlignedValue::from(7u16)),
            other => panic!("expected Cell, got {other:?}"),
        }
    }

    #[test]
    fn typed_uint_encode_uses_the_exact_byte_width() {
        // The width is `ceil(bits(maxval) / 8)`, the same rule compactc applies
        // when it emits the runtime descriptor, so it is not restricted to
        // primitive sizes. `u64::MAX` is 64 bits and so still 8 bytes, but one
        // above it needs 65 bits and therefore 9, not the 16 a
        // u8/u16/u32/u64/u128 ladder would round up to.
        let ty = Type::Unsigned(BigUint::from(u64::MAX));
        let av = encode_typed(&Value::Integer(7), &ty).expect("encode");
        assert_eq!(av, AlignedValue::from(7u64));

        let ty = Type::Unsigned(BigUint::from(u64::MAX as u128 + 1));
        let av = encode_typed(&Value::Integer(7), &ty).expect("encode");
        assert_eq!(av, bytes_aligned_value(vec![7], 9).expect("9-byte atom"));
    }

    #[test]
    fn path_value_field_literal_above_u64_is_exact() {
        use midnight_transient_crypto::curve::Fr;
        let n = (1u128 << 64) + 5;
        let av = path_value_to_aligned(&n.to_string(), &field()).expect("encode");
        let expected = fr_two_pow_64() + Fr::from(5u64);
        assert_eq!(av, AlignedValue::from(expected));
    }

    #[test]
    fn an_aligned_path_key_takes_the_declared_byte_width() {
        // `(align 3 2)` is the two-byte encoding of 3, which is a different
        // ledger key from the one-byte encoding.
        assert_eq!(
            literal_key(&BigUint::from(3u8), 2).expect("encode"),
            AlignedValue::from(3u16)
        );
        assert_eq!(
            literal_key(&BigUint::from(3u8), 1).expect("encode"),
            AlignedValue::from(3u8)
        );
    }

    #[test]
    fn create_zswap_input_is_captured() {
        // `createZswapInput(coin)` records no ledger effect; the interpreter
        // captures its coin arg into `zswap_inputs` for the call/deploy path to
        // build the `Input` / `Transient`. Here the coin is passed as a
        // struct-encoded `QualifiedShieldedCoinInfo` value.
        let natives = vec![ir::Native {
            type_arguments: Vec::new(),
            name: ident("%createZswapInput.9"),
            entry: "__compactRuntime.createZswapInput".to_string(),
            class: "witness".to_string(),
            arguments: vec![argument("%coin.10", Type::Unknown)],
            result_type: Type::unit(),
        }];
        let program = Program::new(&[], &[], &natives);
        let circ = circuit(
            vec![argument("coin", Type::Unknown)],
            Type::unit(),
            ir::Expr::Call {
                name: ident("%createZswapInput.9"),
                args: vec![var("coin")],
            },
        );

        let nonce = [3u8; 32];
        let color = [4u8; 32];
        let value: u128 = 500;
        let mt_index: u64 = 7;
        let coin = Value::AlignedValue(AlignedValue::concat(
            [
                AlignedValue::from(nonce),
                AlignedValue::from(color),
                AlignedValue::from(value),
                AlignedValue::from(mt_index),
            ]
            .iter(),
        ));

        let result =
            execute_in(&circ, &program, &[("coin", coin)]).expect("execute createZswapInput");
        assert_eq!(
            result.zswap_inputs.len(),
            1,
            "createZswapInput must capture exactly one coin"
        );
    }

    #[test]
    fn vector_index_beyond_usize_errors() {
        // 2^64 + 1 truncated `as usize` on 64-bit would wrap to 1 and silently
        // read element 1; it must error with the offending index instead.
        let circ = circuit(
            vec![argument("v", vector(2, uint("255")))],
            uint("255"),
            ir::Expr::VectorRef {
                ty: uint("255"),
                expr: Box::new(var("v")),
                index: Box::new(int(18446744073709551617)),
            },
        );
        let vector_value = Value::Tuple(vec![Value::Integer(10), Value::Integer(20)]);
        let err = match run(&circ, &[("v", vector_value)]) {
            Err(e) => e,
            Ok(value) => panic!("index 2^64 + 1 must not wrap to element 1, got {value:?}"),
        };
        match err {
            InterpreterError::TypeError(ref msg) => assert!(
                msg.contains("18446744073709551617"),
                "error must name the index value, got: {msg}"
            ),
            other => panic!("expected TypeError, got {other:?}"),
        }
    }

    #[test]
    fn degrade_to_transient_drops_top_byte() {
        use midnight_transient_crypto::curve::Fr;
        // `degrade_to_transient` is `field_vec()[1]` = the low 31 bytes as an Fr;
        // the 32nd (top) byte is dropped. A plain little-endian decode of all 32
        // bytes would fold that byte in, so a non-zero top byte is the case that
        // distinguishes the two — and it must not affect the result.
        let mut bytes = [0u8; 32];
        bytes[0] = 7;
        bytes[31] = 0x1e;
        let av = AlignedValue::from(bytes);
        let result = try_builtin("degradeToTransient", &[Value::AlignedValue(av)])
            .expect("builtin known")
            .expect("ok");
        let got = match result {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };
        assert_eq!(got, Fr::from(7u64));
    }

    // -----------------------------------------------------------------------
    // Spread + Bytes/Field/Vector conversion forms
    //
    // The runtime semantics asserted here follow the compiler's own TypeScript
    // runtime (`tools/compact-compiler/runtime/src/casts.ts`): little-endian
    // byte order, zero padding, and rejection (not reduction) on range
    // overflow.
    // -----------------------------------------------------------------------

    #[test]
    fn spread_splices_tuple_value_into_constructor() {
        let v = Value::Tuple(vec![Value::Integer(2), Value::Integer(3)]);
        let circ = circuit(
            vec![argument("v", vector(2, uint("255")))],
            vector(4, uint("255")),
            ir::Expr::Tuple(vec![single(int(1)), spread(2, var("v")), single(int(4))]),
        );
        let result = run(&circ, &[("v", v)]).expect("eval");
        match result {
            Value::Tuple(els) => {
                assert_eq!(els.len(), 4, "spread must splice, not nest: {els:?}");
                for (el, want) in els.iter().zip([1u128, 2, 3, 4]) {
                    assert!(
                        values_equal(el, &Value::Integer(want)),
                        "expected {want}, got {el:?}"
                    );
                }
            }
            other => panic!("expected Tuple, got {other:?}"),
        }
    }

    #[test]
    fn spread_splits_flattened_aligned_value() {
        // A Vector<2, Uint<8>> that arrives flattened as a 2-atom AlignedValue
        // (e.g. a circuit argument or a popeq read).
        let av = AlignedValue::concat([AlignedValue::from(7u8), AlignedValue::from(9u8)].iter());
        let circ = circuit(
            vec![argument("v", vector(2, uint("255")))],
            vector(2, uint("255")),
            ir::Expr::VectorLit(vec![spread(2, var("v"))]),
        );
        let result = run(&circ, &[("v", Value::AlignedValue(av))]).expect("eval");
        match result {
            Value::Tuple(els) => {
                assert_eq!(els.len(), 2);
                assert!(values_equal(&els[0], &Value::Integer(7)), "{:?}", els[0]);
                assert!(values_equal(&els[1], &Value::Integer(9)), "{:?}", els[1]);
            }
            other => panic!("expected Tuple, got {other:?}"),
        }
    }

    #[test]
    fn spread_length_mismatch_errors() {
        let v = Value::Tuple(vec![Value::Integer(2)]);
        let circ = circuit(
            vec![argument("v", vector(1, uint("255")))],
            vector(2, uint("255")),
            ir::Expr::Tuple(vec![spread(2, var("v"))]),
        );
        let err = run(&circ, &[("v", v)]).expect_err("length mismatch must error");
        assert!(
            err.to_string().contains("spread"),
            "error should mention spread: {err}"
        );
    }

    /// The reachable bytes-to-field conversion: `cast-from-bytes` to Field.
    fn bytes_to_field(len: u64, hex_bytes: &str) -> ir::Expr {
        ir::Expr::CastFromBytes {
            ty: field(),
            len,
            expr: Box::new(bytes(hex_bytes)),
        }
    }

    #[test]
    fn bytes_to_field_is_little_endian() {
        use midnight_transient_crypto::curve::Fr;
        // Bytes<4> = [0x2A, 0x01, 0x00, 0x00]; byte 0 is the least
        // significant (casts.ts convertBytesToField), so the value is
        // 0x2A + 0x01·256 = 298.
        let result = eval(bytes_to_field(4, "2a010000"), field()).expect("eval");
        match result {
            Value::AlignedValue(av) => {
                assert_eq!(Fr::try_from(&*av.value).expect("Fr"), Fr::from(298u64));
            }
            other => panic!("expected AlignedValue, got {other:?}"),
        }
    }

    #[test]
    fn bytes_to_field_rejects_values_above_field_modulus() {
        // 32 bytes of 0xFF = 2^256 - 1, above the BLS12-381 scalar modulus.
        // The Compact runtime rejects (convertBytesToField throws a range
        // error); it does not reduce mod p.
        let err = eval(bytes_to_field(32, &"ff".repeat(32)), field())
            .expect_err("over-modulus bytes must error");
        assert!(
            matches!(err, InterpreterError::TypeError(_)),
            "expected TypeError, got {err:?}"
        );
        assert!(
            err.to_string().contains("exceeds"),
            "error should mention exceeding the Field range: {err}"
        );
    }

    #[test]
    fn bytes_to_field_boundary_at_the_modulus() {
        use midnight_transient_crypto::curve::Fr;
        // p - 1 (the largest field element) must be accepted; exactly p
        // (the modulus itself) must be rejected — the range check is
        // strict, not off-by-one.
        let p_minus_1 = -Fr::from(1u64);
        let mut le = p_minus_1.as_le_bytes();
        le.resize(32, 0);

        let result =
            eval(bytes_to_field(32, &hex::encode(&le)), field()).expect("p - 1 must be accepted");
        match result {
            Value::AlignedValue(av) => {
                assert_eq!(Fr::try_from(&*av.value).expect("Fr"), p_minus_1);
            }
            other => panic!("expected AlignedValue, got {other:?}"),
        }

        // Increment the little-endian byte string to get exactly p.
        let mut p = le;
        for b in &mut p {
            let (incremented, carry) = b.overflowing_add(1);
            *b = incremented;
            if !carry {
                break;
            }
        }
        let err = eval(bytes_to_field(32, &hex::encode(&p)), field())
            .expect_err("exactly p must be rejected");
        assert!(
            err.to_string().contains("exceeds"),
            "error should mention exceeding the Field range: {err}"
        );
    }

    #[test]
    fn bytes_to_field_empty_bytes_is_zero() {
        use midnight_transient_crypto::curve::Fr;
        // Bytes<0> (the empty byte string) converts to the Field value 0,
        // matching Fr::from_le_bytes(&[]).
        let result = eval(bytes_to_field(0, ""), field()).expect("eval");
        match result {
            Value::AlignedValue(av) => {
                assert_eq!(Fr::try_from(&*av.value).expect("Fr"), Fr::from(0u64));
            }
            other => panic!("expected AlignedValue, got {other:?}"),
        }
    }

    fn field_to_bytes(len: u64, expr: ir::Expr) -> ir::Expr {
        ir::Expr::FieldToBytes {
            len,
            field_type: FieldType::Native,
            expr: Box::new(expr),
        }
    }

    #[test]
    fn field_to_bytes_is_little_endian_and_bytes_aligned() {
        use midnight_base_crypto::fab;
        // 298 → LE bytes [0x2A, 0x01], logically zero-padded to Bytes<32>
        // (casts.ts convertFieldToBytes). The expected value is built from
        // FAB primitives directly so the test does not validate the
        // production encoder against itself.
        let result = eval(field_to_bytes(32, int(298)), Type::Bytes(32)).expect("eval");
        let expected = fab::AlignedValue::new(
            fab::Value(vec![fab::ValueAtom(vec![0x2A, 0x01])]),
            fab::Alignment::singleton(fab::AlignmentAtom::Bytes { length: 32 }),
        )
        .unwrap();
        match result {
            Value::AlignedValue(av) => assert_eq!(av, expected),
            other => panic!("expected AlignedValue, got {other:?}"),
        }
    }

    #[test]
    fn field_to_bytes_round_trips_through_bytes_to_field() {
        use midnight_transient_crypto::curve::Fr;
        let expr = ir::Expr::CastFromBytes {
            ty: field(),
            len: 32,
            expr: Box::new(field_to_bytes(32, int(12345678901234567890))),
        };
        let result = eval(expr, field()).expect("eval");
        match result {
            Value::AlignedValue(av) => {
                assert_eq!(
                    Fr::try_from(&*av.value).expect("Fr"),
                    Fr::from(12345678901234567890u128)
                );
            }
            other => panic!("expected AlignedValue, got {other:?}"),
        }
    }

    #[test]
    fn field_to_bytes_rejects_values_wider_than_target() {
        // 298 needs two bytes; Bytes<1> must be a range error (casts.ts
        // convertFieldToBytes: "does not fit into n bytes").
        let err = eval(field_to_bytes(1, int(298)), Type::Bytes(1))
            .expect_err("too-wide value must error");
        assert!(
            err.to_string().contains("fit"),
            "error should mention the value not fitting: {err}"
        );
    }

    #[test]
    fn field_to_bytes_on_a_foreign_field_is_unsupported() {
        let expr = ir::Expr::FieldToBytes {
            len: 32,
            field_type: FieldType::Scalar(ir::Curve::Secp256k1),
            expr: Box::new(int(1)),
        };
        let err = eval(expr, Type::Bytes(32)).expect_err("a foreign field is not executable");
        assert!(
            matches!(err, InterpreterError::Unsupported(_)),
            "expected Unsupported, got {err:?}"
        );
    }

    #[test]
    fn bytes_to_vector_yields_bytes_in_order() {
        // Element i of the vector is byte i of the byte string
        // (typescript-passes.ss lowers bytes->vector to `Array.from(expr, BigInt)`).
        let expr = ir::Expr::BytesToVector {
            len: 4,
            expr: Box::new(bytes("01020300")),
        };
        let result = eval(expr, vector(4, uint("255"))).expect("eval");
        match result {
            Value::Tuple(els) => {
                assert_eq!(els.len(), 4);
                for (el, want) in els.iter().zip([1u128, 2, 3, 0]) {
                    assert!(
                        values_equal(el, &Value::Integer(want)),
                        "expected {want}, got {el:?}"
                    );
                }
            }
            other => panic!("expected Tuple, got {other:?}"),
        }
    }

    #[test]
    fn vector_to_bytes_collects_elements_in_order() {
        use midnight_base_crypto::fab;
        let v = Value::Tuple(vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(0),
        ]);
        let circ = circuit(
            vec![argument("v", vector(4, uint("255")))],
            Type::Bytes(4),
            ir::Expr::VectorToBytes {
                len: 4,
                expr: Box::new(var("v")),
            },
        );
        let result = run(&circ, &[("v", v)]).expect("eval");
        // Trailing zero is trimmed by FAB normalization; alignment stays Bytes<4>.
        let expected = fab::AlignedValue::new(
            fab::Value(vec![fab::ValueAtom(vec![1, 2, 3])]),
            fab::Alignment::singleton(fab::AlignmentAtom::Bytes { length: 4 }),
        )
        .unwrap();
        match result {
            Value::AlignedValue(av) => assert_eq!(av, expected),
            other => panic!("expected AlignedValue, got {other:?}"),
        }
    }

    #[test]
    fn vector_to_bytes_rejects_elements_above_255() {
        let v = Value::Tuple(vec![Value::Integer(256)]);
        let circ = circuit(
            vec![argument("v", vector(1, uint("65535")))],
            Type::Bytes(1),
            ir::Expr::VectorToBytes {
                len: 1,
                expr: Box::new(var("v")),
            },
        );
        let err = run(&circ, &[("v", v)]).expect_err("element > 255 must error");
        assert!(
            err.to_string().contains("255"),
            "error should mention the byte bound: {err}"
        );
    }

    #[test]
    fn bytes_to_vector_round_trips_through_vector_to_bytes() {
        use midnight_base_crypto::fab;
        let expr = ir::Expr::VectorToBytes {
            len: 3,
            expr: Box::new(ir::Expr::BytesToVector {
                len: 3,
                expr: Box::new(bytes("aabb00")),
            }),
        };
        let result = eval(expr, Type::Bytes(3)).expect("eval");
        let expected = fab::AlignedValue::new(
            fab::Value(vec![fab::ValueAtom(vec![0xAA, 0xBB])]),
            fab::Alignment::singleton(fab::AlignmentAtom::Bytes { length: 3 }),
        )
        .unwrap();
        match result {
            Value::AlignedValue(av) => assert_eq!(av, expected),
            other => panic!("expected AlignedValue, got {other:?}"),
        }
    }

    #[test]
    fn push_of_an_aligned_concat_joins_its_parts() {
        // The ledger keys an unshielded token balance by the colour joined to
        // the recipient, so every unshielded token operation pushes one of
        // these. Before it was supported, such a contract could not be called
        // at all.
        let program = Program::new(&[], &[], &[]);
        let mut private_state = Vec::new();
        let mut ctx = test_ctx(&program, &mut private_state, HashMap::new());

        let part = |v: u64| ir::Operand::Align {
            value: BigUint::from(v),
            bytes: 8,
        };
        let concat = ir::Operand::AlignedConcat(vec![part(7), part(9)]);

        let joined = operand_aligned(&mut ctx, &concat).expect("push an aligned-concat");
        let expected = AlignedValue::concat(
            [
                operand_aligned(&mut ctx, &part(7)).unwrap(),
                operand_aligned(&mut ctx, &part(9)).unwrap(),
            ]
            .iter(),
        );
        assert_eq!(joined, expected);

        // Order matters: the colour and the recipient are not interchangeable,
        // and a swapped join would key a different balance.
        let reversed = ir::Operand::AlignedConcat(vec![part(9), part(7)]);
        assert_ne!(
            joined,
            operand_aligned(&mut ctx, &reversed).expect("push the reversed concat"),
            "the parts must join in order"
        );

        // And it is reachable through the push path itself, not only directly.
        assert_eq!(
            push_value(&mut ctx, &concat).expect("push"),
            StateValue::from(expected)
        );
    }

    #[test]
    fn encode_typed_opaque_default_is_empty_compress_atom() {
        use midnight_base_crypto::fab;
        // `default<Opaque<"string">>` (Value::Void) must encode as the empty
        // string: one empty atom with Compress alignment (compact-types.ts
        // CompactTypeOpaqueString).
        let av = encode_typed(&Value::Void, &Type::Opaque("string".to_string()))
            .expect("encode default opaque");
        let expected = fab::AlignedValue::new(
            fab::Value(vec![fab::ValueAtom(Vec::new())]),
            fab::Alignment::singleton(fab::AlignmentAtom::Compress),
        )
        .unwrap();
        assert_eq!(av, expected);
    }

    #[test]
    fn default_of_a_struct_concats_its_field_defaults() {
        use midnight_base_crypto::fab::AlignedValue;

        // `default<ContractAddress>` is what `left<ZswapCoinPublicKey,
        // ContractAddress>(recipient)` materializes for the Either's unused
        // arm; a two-field struct pins the field ordering of the concat.
        let contract_address = Type::Struct {
            name: "ContractAddress".to_string(),
            fields: vec![("bytes".to_string(), Type::Bytes(32))],
        };
        let uint_ty = uint("18446744073709551615");
        let pair_ty = Type::Struct {
            name: "Pair".to_string(),
            fields: vec![
                ("address".to_string(), contract_address.clone()),
                ("amount".to_string(), uint_ty.clone()),
            ],
        };

        let expected_bytes = {
            let value = default_value(&Type::Bytes(32)).unwrap();
            encode_typed(&value, &Type::Bytes(32)).unwrap()
        };

        let address = default_value(&contract_address).expect("struct default");
        let Value::AlignedValue(address) = address else {
            panic!("expected AlignedValue, got {address:?}");
        };
        assert_eq!(address, expected_bytes.clone());

        // Nested structs recurse, and fields concatenate in declaration order.
        let pair = default_value(&pair_ty).expect("nested struct default");
        let Value::AlignedValue(pair) = pair else {
            panic!("expected AlignedValue, got {pair:?}");
        };
        let expected_amount = {
            let value = default_value(&uint_ty).unwrap();
            encode_typed(&value, &uint_ty).unwrap()
        };
        let expected_pair = AlignedValue::concat([expected_bytes, expected_amount].iter());
        assert_eq!(pair, expected_pair);
    }

    #[test]
    fn contract_call_unsupported_names_target() {
        let expr = ir::Expr::ContractCall {
            circuit: "do_thing".to_string(),
            receiver: Box::new(var("other_contract")),
            contract_type: Type::unit(),
            args: Vec::new(),
        };
        let err = eval(expr, Type::unit()).expect_err("contract-call must be unsupported");
        assert!(
            matches!(err, InterpreterError::Unsupported(_)),
            "expected Unsupported, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains(
                "cross-contract calls are not implemented yet (call to other_contract.do_thing)"
            ),
            "error must name the called contract and circuit, got: {msg}"
        );
    }

    /// Build a minimal `ExecContext` over the counter fixture state for
    /// type-inference tests. `local_types` is the only knob these tests vary.
    fn test_ctx<'a>(
        program: &'a Program<'a>,
        private_state: &'a mut Vec<u8>,
        local_types: HashMap<String, Type>,
    ) -> ExecContext<'a> {
        ExecContext {
            state: make_counter_state(0),
            locals: HashMap::new(),
            local_types,
            reads: Vec::new(),
            gather_ops: Vec::new(),
            communication_outputs: Vec::new(),
            private_transcript_outputs: Vec::new(),
            zswap_outputs: Vec::new(),
            zswap_inputs: Vec::new(),
            witnesses: None,
            private_state,
            program,
            contract_address: None,
        }
    }

    #[test]
    fn either_field_access_slices_the_live_variant() {
        let bytes_field = || ("bytes".to_string(), Type::Bytes(32));
        let variant = |name: &str| Type::Struct {
            name: name.into(),
            fields: vec![bytes_field()],
        };
        let either_fields = vec![
            ("is_left".to_string(), Type::Boolean),
            ("left".to_string(), variant("ZswapCoinPublicKey")),
            ("right".to_string(), variant("ContractAddress")),
        ];

        // Three atoms: the `is_left` discriminant, `left.bytes`, `right.bytes`.
        let either = |is_left: bool| {
            AlignedValue::concat(
                [
                    AlignedValue::from(is_left),
                    AlignedValue::from(1u64),
                    AlignedValue::from(2u64),
                ]
                .iter(),
            )
        };

        // `is_left` selects the live variant: `left.bytes` at atom offset 1,
        // `right.bytes` at atom offset 2.
        assert_eq!(
            either_variant_field_slice(&either_fields, &either(true), "bytes").unwrap(),
            (1, 1)
        );
        assert_eq!(
            either_variant_field_slice(&either_fields, &either(false), "bytes").unwrap(),
            (2, 1)
        );
        // A field carried by neither variant is still an error.
        assert!(either_variant_field_slice(&either_fields, &either(true), "nope").is_err());
    }

    #[test]
    fn infer_types_of_conversion_forms() {
        let program = Program::new(&[], &[], &[]);
        let mut ps = Vec::new();
        let ctx = test_ctx(&program, &mut ps, HashMap::new());

        let b2f = ir::Expr::CastFromBytes {
            ty: field(),
            len: 32,
            expr: Box::new(var("x")),
        };
        assert!(matches!(
            infer_type_of_expr(&ctx, &b2f),
            Some(Type::Field(_))
        ));

        let f2b = field_to_bytes(32, var("x"));
        assert!(matches!(
            infer_type_of_expr(&ctx, &f2b),
            Some(Type::Bytes(32))
        ));

        let b2v = ir::Expr::BytesToVector {
            len: 4,
            expr: Box::new(var("x")),
        };
        match infer_type_of_expr(&ctx, &b2v) {
            Some(Type::Vector { len: 4, ty }) => {
                assert_eq!(*ty, uint("255"));
            }
            other => panic!("expected Vector<4, Uint<255>>, got {other:?}"),
        }

        let v2b = ir::Expr::VectorToBytes {
            len: 4,
            expr: Box::new(var("x")),
        };
        assert!(matches!(
            infer_type_of_expr(&ctx, &v2b),
            Some(Type::Bytes(4))
        ));
    }

    #[test]
    fn infer_type_of_tuple_with_spread_splices_inner_types() {
        let program = Program::new(&[], &[], &[]);
        let mut ps = Vec::new();
        let mut local_types = HashMap::new();
        local_types.insert("v".to_string(), vector(2, field()));
        let ctx = test_ctx(&program, &mut ps, local_types);
        let expr = ir::Expr::Tuple(vec![
            single(ir::Expr::Quote(ir::Literal::Bool(true))),
            spread(2, var("v")),
        ]);
        match infer_type_of_expr(&ctx, &expr) {
            Some(Type::Tuple(types)) => {
                assert_eq!(types.len(), 3, "spread must contribute 2 element types");
                assert!(matches!(types[0], Type::Boolean));
                assert!(matches!(types[1], Type::Field(_)));
                assert!(matches!(types[2], Type::Field(_)));
            }
            other => panic!("expected Tuple type, got {other:?}"),
        }
    }

    #[test]
    fn infer_type_of_a_call_is_the_callee_result_type() {
        let circuits = vec![ir::Circuit {
            name: ident("%helper.11"),
            exported: false,
            pure: true,
            proof: false,
            arguments: Vec::new(),
            result_type: Type::Bytes(32),
            body: bytes(&"00".repeat(32)),
        }];
        let witnesses = vec![ir::Witness {
            name: ident("%secret.12"),
            arguments: Vec::new(),
            result_type: uint("255"),
        }];
        let program = Program::new(&circuits, &witnesses, &[]);
        let mut ps = Vec::new();
        let ctx = test_ctx(&program, &mut ps, HashMap::new());

        let call = |name: &str| ir::Expr::Call {
            name: ident(name),
            args: Vec::new(),
        };
        assert!(matches!(
            infer_type_of_expr(&ctx, &call("%helper.11")),
            Some(Type::Bytes(32))
        ));
        assert_eq!(
            infer_type_of_expr(&ctx, &call("%secret.12")),
            Some(uint("255"))
        );
        assert_eq!(infer_type_of_expr(&ctx, &call("%nobody.13")), None);
    }
}
