//! Circuit IR interpreter.
//!
//! Executes circuit IR against contract state using midnight-ledger's
//! `ContractStateExt::query()` for ledger operations.

use std::collections::HashMap;

use midnight_bindgen_runtime::{AlignedValue, ContractState, InMemoryDB, StateValue};
use midnight_onchain_runtime::context::QueryContext;
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
            Value::Integer(n) => integer_fallback_aligned(*n),
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
            Value::Integer(n) => StateValue::from(integer_fallback_aligned(*n)),
            Value::Bool(b) => StateValue::from(AlignedValue::from(*b)),
            Value::Void => StateValue::from(AlignedValue::from(())),
            _ => StateValue::Null,
        }
    }
}

/// Width-preserving FAB encoding for an integer with no type information.
///
/// FAB atoms are zero-trimmed little-endian bytes (`ValueAtom::normalize` in
/// midnight-base-crypto, and `From<u64>` forwards through `From<u128>`), so
/// the atom bytes are identical for every integer width — only the declared
/// `AlignmentAtom::Bytes { length }` differs. That width is *not* cosmetic:
/// `AlignedValue`'s `Eq`/`Hash` include the alignment (so on-chain `Map` keys
/// of different widths are different keys) and `persistentHash` zero-pads
/// each atom to the declared width (`ValueAtom::binary_repr_unchecked` in
/// midnight-transient-crypto), so `Bytes{8}` and `Bytes{16}` encodings of the
/// same number hash differently.
///
/// Therefore: values that fit `u64` keep the historical 8-byte alignment so
/// every existing encoding (witness transcript outputs, circuit-argument
/// flattening, type-less ledger pushes) stays byte-for-byte identical.
/// Values above `u64::MAX` are encoded at the 16-byte width — the width the
/// type-aware encoder ([`encode_typed`]) picks for every `Uint` bound that
/// can hold such a value — instead of being silently truncated as before.
fn integer_fallback_aligned(n: u128) -> AlignedValue {
    match u64::try_from(n) {
        Ok(small) => AlignedValue::from(small),
        Err(_) => AlignedValue::from(n),
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

    /// A genuine witness-level failure: the provider knew the name but could
    /// not produce a value (key store unreachable, corrupt private state,
    /// argument conversion failure, ...), or nothing — provider, builtin, or
    /// helper — could handle a witness call at all. Always aborts execution;
    /// "the provider doesn't implement this name" is NOT an error, it's
    /// [`WitnessOutcome::Unknown`].
    #[error("witness error: {0}")]
    Witness(String),
}

/// Mutable context handed to each witness call during circuit execution.
///
/// A witness reads the contract's current private state, computes its value,
/// and may mutate the private state in place. The mutated state is what the SDK
/// persists after a successful call (see `docs/private-state.md`).
///
/// The private state is opaque bytes; the witness owns its encoding. When no
/// `PrivateStateProvider` is attached the buffer starts empty and lives only
/// for the duration of the call.
pub struct WitnessContext<'a> {
    private_state: &'a mut Vec<u8>,
}

impl<'a> WitnessContext<'a> {
    /// Wrap a mutable private-state buffer.
    pub fn new(private_state: &'a mut Vec<u8>) -> Self {
        Self { private_state }
    }

    /// The contract's current private state as opaque bytes (empty if unset).
    pub fn private_state(&self) -> &[u8] {
        self.private_state
    }

    /// Mutable access to the private-state buffer, to update it in place.
    pub fn private_state_mut(&mut self) -> &mut Vec<u8> {
        self.private_state
    }

    /// Replace the private state wholesale.
    pub fn set_private_state(&mut self, bytes: Vec<u8>) {
        *self.private_state = bytes;
    }
}

/// Outcome of dispatching a witness call to a [`WitnessProvider`].
///
/// Distinguishes "the provider doesn't implement this name" (a normal,
/// non-error outcome that lets the interpreter fall through to builtins and
/// IR helpers) from witness-level failures, which are `Err` on the
/// surrounding `Result` and always abort execution.
#[derive(Debug, Clone)]
pub enum WitnessOutcome {
    /// The provider handled the call and produced the witness value.
    Value(Value),
    /// The provider doesn't implement a witness with this name. The
    /// interpreter falls through to builtin and helper dispatch.
    Unknown,
}

/// Trait for providing witness (private state) callbacks during circuit execution.
///
/// Implement this to supply private state for circuits that call witnesses.
/// Each method corresponds to a witness function in the Compact contract.
pub trait WitnessProvider: Send + Sync {
    /// Called when the circuit invokes a witness function.
    ///
    /// `ctx` carries the mutable private state — read it to compute the
    /// witness value, and mutate it to record state changes that should
    /// survive to the next call. `name` is the witness function name
    /// (e.g. `"private$secret_key"`); `args` are the evaluated arguments.
    ///
    /// Return [`WitnessOutcome::Value`] when the call was handled, and
    /// [`WitnessOutcome::Unknown`] when this provider has no witness named
    /// `name` (the interpreter then falls through to builtins and helpers).
    /// Return `Err` only for genuine failures — a signer that is unreachable,
    /// undecodable private state, bad arguments — which abort the circuit;
    /// errors are never treated as "unknown name".
    fn call_witness(
        &self,
        ctx: &mut WitnessContext<'_>,
        name: &str,
        args: &[Value],
    ) -> Result<WitnessOutcome, InterpreterError>;
}

/// A no-op witness provider that reports every name as unknown.
pub struct NoWitnesses;

impl WitnessProvider for NoWitnesses {
    fn call_witness(
        &self,
        _ctx: &mut WitnessContext<'_>,
        _name: &str,
        _args: &[Value],
    ) -> Result<WitnessOutcome, InterpreterError> {
        Ok(WitnessOutcome::Unknown)
    }
}

/// A shielded coin the circuit asked to create on-chain via the
/// `createZswapOutput` kernel native (e.g. through `mintShieldedToken` or
/// `sendShielded`).
///
/// `createZswapOutput(coin, recipient)` records no ledger effect of its own
/// (the mint/spend/receive effects are separate `ledger-query` ops); it marks
/// "attach a Zswap output for this coin here". The interpreter captures the
/// raw arg `Value`s so the call/deploy path can build the corresponding
/// `Output` in the transaction's Zswap offer (optionally with a discovery
/// ciphertext keyed to the recipient's encryption public key).
#[derive(Debug, Clone)]
pub struct CircuitZswapOutput {
    /// The `ShieldedCoinInfo` the circuit constructed (nonce, color/type,
    /// value), as evaluated by the interpreter — a struct-encoded value.
    pub coin: Value,
    /// The `Either<ZswapCoinPublicKey, ContractAddress>` recipient the circuit
    /// passed, as evaluated by the interpreter.
    pub recipient: Value,
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
    /// Witness return values, in call order — the prover's private transcript
    /// outputs (the ZKIR's private inputs). These must be set on
    /// `ContractCallPrototype.private_transcript_outputs`, or proving a
    /// witness-using circuit fails with "ran out of private transcript outputs".
    /// Empty for witness-free circuits.
    pub private_transcript_outputs: Vec<AlignedValue>,
    /// Coins the circuit asked to create on-chain via `createZswapOutput`
    /// (shielded mints / sends), in call order. The call/deploy path turns
    /// each into an `Output` in the Zswap offer. Empty for circuits that
    /// don't create shielded outputs.
    pub zswap_outputs: Vec<CircuitZswapOutput>,
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
        None,
        helpers,
        structs,
        &[],
        None,
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
        None,
        helpers,
        structs,
        enums,
        None,
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
        None,
        helpers,
        structs,
        &[],
        None,
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
    witness_ctx: Option<&mut WitnessContext<'_>>,
    helpers: &[HelperDef],
    structs: &[StructDef],
    enums: &[EnumDef],
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
        Some(ctx) => &mut *ctx.private_state,
        None => &mut scratch,
    };

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
        private_transcript_outputs: Vec::new(),
        zswap_outputs: Vec::new(),
        last_expr_value: None,
        witnesses: Some(witnesses),
        private_state,
        helpers: helper_map,
        layouts,
        struct_defs,
        enum_defs,
        contract_address,
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
        private_transcript_outputs: ctx.private_transcript_outputs,
        zswap_outputs: ctx.zswap_outputs,
    })
}

/// Context-aware execution used by the funded call path.
///
/// Threads the contract's loaded private state (via `ctx`) through every witness
/// call so a stateful witness can read and update it. After this returns, `ctx`'s
/// private-state buffer holds the post-call state, ready to persist.
#[allow(clippy::too_many_arguments)]
pub fn execute_with_context(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    args: &[(&str, Value)],
    ctx: &mut WitnessContext<'_>,
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
        Some(ctx),
        helpers,
        structs,
        enums,
        None,
    )
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
    /// Witness return values in call order — the prover's private transcript
    /// outputs (ZKIR private inputs). Empty for witness-free circuits.
    private_transcript_outputs: Vec<AlignedValue>,
    /// Coins the circuit asked to create via `createZswapOutput`, in call
    /// order. Surfaced on `ExecutionResult` for the call/deploy path.
    zswap_outputs: Vec<CircuitZswapOutput>,
    /// The value of the last evaluated expression statement (used as the
    /// circuit's communication output when `ir.result` is None).
    last_expr_value: Option<Value>,
    witnesses: Option<&'a dyn WitnessProvider>,
    /// Mutable private-state buffer threaded through witness calls.
    private_state: &'a mut Vec<u8>,
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
            let mut types = Vec::with_capacity(elements.len());
            for e in elements {
                // A spread element contributes the element types of its
                // (vector/tuple-typed) inner expression, `length` of them.
                if let Expr::Spread { length, expr } = e {
                    match infer_type_of_expr(ctx, expr)? {
                        TypeRef::Tuple { types: inner } if inner.len() as u64 == *length => {
                            types.extend(inner);
                        }
                        TypeRef::Vector { length: l, element } if l as u64 == *length => {
                            types.extend(std::iter::repeat_n(*element, l));
                        }
                        _ => return None,
                    }
                } else {
                    types.push(infer_type_of_expr(ctx, e)?);
                }
            }
            Some(TypeRef::Tuple { types })
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
        // Conversion forms have statically known result types
        // (circuit-passes.ss types bytes->field as Field, field->bytes /
        // vector->bytes as Bytes<len>, bytes->vector as Vector<len, Uint<255>>).
        Expr::BytesToField { .. } => Some(TypeRef::Field),
        Expr::FieldToBytes { length, .. } | Expr::VectorToBytes { length, .. } => {
            usize::try_from(*length)
                .ok()
                .map(|length| TypeRef::Bytes { length })
        }
        Expr::BytesToVector { length, .. } => {
            usize::try_from(*length).ok().map(|length| TypeRef::Vector {
                length,
                element: Box::new(TypeRef::Uint {
                    maxval: "255".to_string(),
                }),
            })
        }
        // A bare `spread` is not a value (it only contributes elements to a
        // surrounding tuple constructor, handled in the `Expr::Tuple` arm
        // above), and `contract-call` result types are not shipped in the IR.
        // Returning `None` keeps the contract ("unknown means unknown")
        // without fabricating a type.
        Expr::Spread { .. } | Expr::ContractCall { .. } => None,
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
        TypeRef::Uint { maxval } => {
            let n = value.parse::<u128>().map_err(|e| {
                InterpreterError::TypeError(format!("invalid integer literal {value:?}: {e}"))
            })?;
            check_uint_range(n, maxval)?;
            Ok(Value::Integer(n))
        }
        TypeRef::Field => value.parse::<u128>().map(Value::Integer).map_err(|e| {
            InterpreterError::TypeError(format!("invalid integer literal {value:?}: {e}"))
        }),
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
            Ok(Value::AlignedValue(bytes_aligned_value(bytes, *length)?))
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

        Expr::LedgerQuery { ops, .. } => {
            // `kernel.self()` lowers to a read of the contract's own address
            // from the VM *context* (`dup{n:2} idx[0] popeq`). We execute these
            // ops through the real VM (`exec_ledger_query` injects the supplied
            // `contract_address` into the `QueryContext`), so the read returns
            // the right address *and* the ops land in the transcript — the
            // compiled circuit's proving key expects that `dup/idx/popeq`
            // sequence in the public transcript, so skipping it (an earlier
            // shortcut) produced a "public transcript input mismatch" at prove
            // time.
            exec_ledger_query(ctx, ops)
        }

        Expr::Tuple { elements } => {
            let mut vals: Vec<Value> = Vec::with_capacity(elements.len());
            for e in elements {
                // `spread` is only meaningful as a direct child of a
                // tuple/vector constructor: it splices the elements of its
                // (vector/tuple-valued) inner expression into the surrounding
                // element list. The compiler attaches the contributed element
                // count as `length` (analysis-passes.ss attaches the spread
                // vector's length), so a count mismatch here is a compiler/
                // interpreter disagreement, not a user error.
                if let Expr::Spread { length, expr } = e {
                    let expected = ir_length(*length)?;
                    let inner = eval_expr(ctx, expr)?;
                    splice_spread(inner, expected, &mut vals)?;
                } else {
                    vals.push(eval_expr(ctx, e)?);
                }
            }
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

            // `createZswapOutput(coin, recipient)` is a kernel native that
            // records no ledger effect of its own (the mint/spend/receive
            // effects are separate `ledger-query` ops); it marks "attach a
            // Zswap output for this coin here". Capture the `(coin, recipient)`
            // args so the call/deploy path can build the corresponding `Output`
            // in the transaction's Zswap offer, and return unit. It must never
            // reach the witness provider / builtin / helper dispatch (there is
            // no such name), which would otherwise error.
            if name == "createZswapOutput" {
                let mut it = evaluated_args.into_iter();
                match (it.next(), it.next()) {
                    (Some(coin), Some(recipient)) => {
                        ctx.zswap_outputs
                            .push(CircuitZswapOutput { coin, recipient });
                        return Ok(Value::Void);
                    }
                    _ => {
                        return Err(InterpreterError::TypeError(
                            "createZswapOutput expects (coin, recipient) arguments".to_string(),
                        ));
                    }
                }
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
            // returns `WitnessOutcome::Unknown` (i.e. it has no witness
            // with this name). Every `Err` is a genuine witness failure
            // and propagates — it must never reroute to a builtin, or a
            // failing provider whose name collides with one (e.g.
            // `persistentHash`) would "succeed" with the wrong inputs.
            if let Some(w) = ctx.witnesses {
                // Scope the WitnessContext's borrow of `ctx` so we can record
                // the result into `ctx.private_transcript_outputs` afterward.
                let outcome = {
                    let mut wctx = WitnessContext {
                        private_state: &mut *ctx.private_state,
                    };
                    w.call_witness(&mut wctx, name, &evaluated_args)
                };
                match outcome? {
                    WitnessOutcome::Value(v) => {
                        // Capture the witness's private value as a private
                        // transcript output, in call order, for the prover.
                        ctx.private_transcript_outputs.push(v.to_aligned_value());
                        return Ok(v);
                    }
                    WitnessOutcome::Unknown => {
                        // Provider doesn't know the name; fall through.
                    }
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
                let av = encode_typed(&val, &field.ty).map_err(|e| {
                    InterpreterError::TypeError(format!(
                        "cannot encode field `{}` of `{struct_name}`: {e}",
                        field.name
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
                // `From<u128> for Fr` is exact (midnight-curves
                // `Scalar::from_u128`); never narrow through u64 here.
                return Ok(Value::AlignedValue(AlignedValue::from(Fr::from(*n))));
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

        // A `spread` reaching eval_expr directly was not spliced by a
        // surrounding `Expr::Tuple` constructor — the only position the
        // compiler emits it in (save-contract-info-passes.ss emits `spread`
        // exclusively inside `tuple` element lists).
        Expr::Spread { length, .. } => Err(InterpreterError::TypeError(format!(
            "spread (length {length}) outside a tuple/vector constructor"
        ))),

        // Bytes<length> → Field. Byte 0 is the least significant byte and
        // values >= the field modulus are rejected, not reduced — matching
        // the Compact runtime's `convertBytesToField` (casts.ts). At the FAB
        // level both Bytes and Field atoms are zero-trimmed little-endian
        // bytes, so this is a reinterpretation plus a range check.
        Expr::BytesToField { length, expr } => {
            use midnight_transient_crypto::curve::Fr;
            let val = eval_expr(ctx, expr)?;
            let length = ir_length(*length)?;
            let bytes = value_to_byte_string(&val, length)?;
            let fr = Fr::from_le_bytes(&bytes).ok_or_else(|| {
                InterpreterError::TypeError(format!(
                    "range error: byte string {} exceeds the maximum value of the Field type",
                    hex::encode(&bytes)
                ))
            })?;
            Ok(Value::AlignedValue(AlignedValue::from(fr)))
        }

        // Field → Bytes<length>. Little-endian, zero-padded; values that
        // need more than `length` bytes are a range error — matching the
        // Compact runtime's `convertFieldToBytes` (casts.ts).
        Expr::FieldToBytes { length, expr } => {
            let val = eval_expr(ctx, expr)?;
            let fr = value_to_fr(&val).ok_or_else(|| {
                InterpreterError::TypeError(format!(
                    "field-to-bytes expects a Field value, got {val:?}"
                ))
            })?;
            let length = ir_length(*length)?;
            let mut bytes = fr.as_le_bytes();
            // Trim trailing zeros here (not just in bytes_aligned_value) so
            // the width check below sees the value's true byte length.
            while matches!(bytes.last(), Some(0)) {
                bytes.pop();
            }
            if bytes.len() > length {
                return Err(InterpreterError::TypeError(format!(
                    "range error: Field value {fr:?} does not fit into {length} bytes"
                )));
            }
            Ok(Value::AlignedValue(bytes_aligned_value(bytes, length)?))
        }

        // Bytes<length> → Vector<length, Uint<8>>. Element i is byte i —
        // the TypeScript lowering is `Array.from(bytes, BigInt)`
        // (typescript-passes.ss). Each element is encoded as a 1-byte atom
        // so downstream typed consumers (hashes, stores) see the on-chain
        // Vector<N, Uint<8>> layout.
        Expr::BytesToVector { length, expr } => {
            let val = eval_expr(ctx, expr)?;
            let length = ir_length(*length)?;
            let bytes = value_to_byte_string(&val, length)?;
            let elements = (0..length)
                .map(|i| {
                    let b = bytes.get(i).copied().unwrap_or(0);
                    Value::AlignedValue(AlignedValue::from(b))
                })
                .collect();
            Ok(Value::Tuple(elements))
        }

        // Vector<length, Uint<8>> → Bytes<length>. Element i becomes byte i —
        // the TypeScript lowering is `Uint8Array.from(vector, Number)`
        // (typescript-passes.ss). The type checker guarantees Uint<=255
        // elements (circuit-passes.ss); anything wider here is a bug.
        Expr::VectorToBytes { length, expr } => {
            let val = eval_expr(ctx, expr)?;
            let length = ir_length(*length)?;
            let bytes = vector_value_to_bytes(&val, length)?;
            Ok(Value::AlignedValue(bytes_aligned_value(bytes, length)?))
        }

        // Cross-contract calls are a later feature; fail with a purposeful
        // message naming the call target instead of a Debug dump.
        Expr::ContractCall {
            circuit,
            contract,
            contract_type,
            ..
        } => {
            let target = match contract.as_ref() {
                Expr::Var { name } => name.clone(),
                _ => match contract_type {
                    TypeRef::Struct { name } | TypeRef::Opaque { name } => name.clone(),
                    _ => "<contract>".to_string(),
                },
            };
            Err(InterpreterError::Unsupported(format!(
                "cross-contract calls are not implemented yet (call to {target}.{circuit})"
            )))
        }
    }
}

/// Convert an IR-level element count (`u64`) to `usize`, rejecting values the
/// host cannot index instead of wrapping.
fn ir_length(length: u64) -> Result<usize, InterpreterError> {
    usize::try_from(length)
        .map_err(|_| InterpreterError::TypeError(format!("length {length} does not fit in usize")))
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
        // Exact u128 → Fr conversion (`Scalar::from_u128`); a `u64` cast
        // here would silently drop the high bits of wide integers feeding
        // hashes and EC scalar ops.
        Value::Integer(n) => Some(Fr::from(*n)),
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

/// Interpret a value as a 32-byte `HashOutput` (e.g. a `Bytes<32>` opening or
/// domain separator). FAB atoms are zero-trimmed, so a shorter atom is
/// right-padded with zeros to 32 bytes.
fn value_to_hash_output(
    v: &Value,
) -> Result<midnight_base_crypto::hash::HashOutput, InterpreterError> {
    let av = v.to_aligned_value();
    let atom = av.value.0.first().ok_or_else(|| {
        InterpreterError::TypeError("expected a 32-byte value, got an empty AlignedValue".into())
    })?;
    if atom.0.len() > 32 {
        return Err(InterpreterError::TypeError(format!(
            "expected a 32-byte value, got a {}-byte atom",
            atom.0.len()
        )));
    }
    let mut bytes = [0u8; 32];
    bytes[..atom.0.len()].copy_from_slice(&atom.0);
    Ok(midnight_base_crypto::hash::HashOutput(bytes))
}

/// Try to execute a Compact runtime builtin function.
/// Returns `Some(Ok(value))` if the function is a known builtin,
/// `Some(Err(..))` if it fails, or `None` if it's not a builtin.
fn try_builtin(name: &str, args: &[Value]) -> Option<Result<Value, InterpreterError>> {
    match name {
        "persistentCommit" => {
            // persistentCommit(value, opening) = persistent_commit(value, opening):
            // a domain-separated commitment. The opening is written to the
            // hasher first, then the value (see base-crypto `persistent_commit`).
            // Used to derive a contract's custom shielded token type:
            // `tokenType(domain_sep, self()) = persistentCommit((domain_sep,
            // self().bytes), "midnight:derive_token\0..")`. Matching the
            // on-chain derivation exactly is what lets a minted coin's color
            // line up with the recipient's wallet sync.
            use midnight_base_crypto::hash::{HashOutput, persistent_commit};
            use midnight_transient_crypto::fab::ValueReprAlignedValue;

            let value = match args.first() {
                Some(v) => v,
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "persistentCommit expects (value, opening)".to_string(),
                    )));
                }
            };
            let opening = match args.get(1).map(value_to_hash_output) {
                Some(Ok(h)) => h,
                Some(Err(e)) => return Some(Err(e)),
                None => {
                    return Some(Err(InterpreterError::TypeError(
                        "persistentCommit expects an opening (domain separator) argument"
                            .to_string(),
                    )));
                }
            };
            // Flatten the value into a single AlignedValue (walks Tuple/Struct
            // in declaration order, matching the on-chain typed repr) and commit.
            let wrapped = ValueReprAlignedValue(value.to_aligned_value());
            let hash: HashOutput = persistent_commit(&wrapped, opening);
            Some(Ok(Value::AlignedValue(AlignedValue::from(hash.0))))
        }
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
                        // Use Fr for field-compatible hashing. Exact u128
                        // conversion — see `value_to_fr`.
                        use midnight_transient_crypto::curve::Fr;
                        let av = AlignedValue::from(Fr::from(*n));
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
                    // Exact u128 conversion — see `value_to_fr`.
                    let av = AlignedValue::from(Fr::from(*n));
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
        Value::AlignedValue(av) => aligned_atom_to_u128(av),
        _ => None,
    }
}

/// Decode the first atom of an `AlignedValue` as a `u128`. FAB atoms are
/// zero-trimmed *little-endian* bytes (`ValueAtom` conversions in
/// midnight-base-crypto fab/conversions.rs), so the atom [0x2C, 0x01] is 300.
/// The alignment is ignored because the prover already enforces shape.
/// Returns `None` for a zero-atom value or an atom wider than 16 bytes.
fn aligned_atom_to_u128(av: &AlignedValue) -> Option<u128> {
    let atom = av.value.0.first()?;
    if atom.0.len() > 16 {
        return None;
    }
    let mut buf = [0u8; 16];
    buf[..atom.0.len()].copy_from_slice(&atom.0);
    Some(u128::from_le_bytes(buf))
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

/// Execute a ledger-query: translate IR LedgerOps to onchain-vm Ops and run
/// them through the VM `QueryContext` (with the contract's real address
/// injected, so `kernel.self()`'s `dup{n:2} idx[0] popeq` context read returns
/// the right address and lands in the transcript the proving key expects).
fn exec_ledger_query(
    ctx: &mut ExecContext,
    ir_ops: &[LedgerOp],
) -> Result<Value, InterpreterError> {
    let cost_model = &INITIAL_COST_MODEL;
    let mut ops: Vec<Op<ResultModeGather, InMemoryDB>> = Vec::new();

    for ir_op in ir_ops {
        match ir_op {
            LedgerOp::Dup { n } => {
                ops.push(Op::Dup { n: *n });
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
                            // A stack-keyed `idx` (the key was pushed onto the
                            // VM stack, e.g. a coin commitment in the mint/spend
                            // effect ops) is emitted in the portable IR as a
                            // value literal `"stack"` typed `Uint<255>` instead
                            // of a proper stack tag (`{ tag: 'stack' }` in the
                            // VM ops). Map it back to `Key::Stack`.
                            if value == "stack" {
                                return Ok(Key::Stack);
                            }
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
                                    let sv = encode_ledger_key(local, local_ty.as_ref())?;
                                    if let StateValue::Cell(ref av_sp) = sv {
                                        return Ok(Key::Value((**av_sp).clone()));
                                    }
                                }
                            }
                            let av = path_value_to_aligned(value, ty)?;
                            Ok(Key::Value(av))
                        }
                        PathEntry::Stack => Ok(Key::Stack),
                        // Resolve the local and encode it with its declared
                        // type when known, so the key's alignment matches
                        // what the on-chain insert produced (an Integer
                        // local of type Uint<16> must become a 2-byte key,
                        // not the type-less 8-byte default).
                        PathEntry::Var { name } => match ctx.locals.get(name) {
                            Some(
                                val @ (Value::Integer(_) | Value::AlignedValue(_) | Value::Bool(_)),
                            ) => {
                                let local_ty = ctx.local_types.get(name);
                                match encode_ledger_key(val, local_ty)? {
                                    StateValue::Cell(ref av_sp) => {
                                        Ok(Key::Value((**av_sp).clone()))
                                    }
                                    other => Err(InterpreterError::TypeError(format!(
                                        "variable `{name}` did not encode to a cell key \
                                         (got {other:?})"
                                    ))),
                                }
                            }
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
                        encode_ledger_key(&val, inferred.as_ref())?
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
                                let av = path_value_to_aligned(&v, &ty)?;
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
                        encode_ledger_key(&val, inferred.as_ref())?
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
                    value: encode_ledger_key(&val, inferred.as_ref())?,
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

/// Parse a `Uint{maxval}` bound, check `n` against it, and return the parsed
/// bound so callers (e.g. [`encode_typed`]'s width ladder) reuse it instead of
/// re-parsing with a default that could drift from this one.
///
/// `maxval` is the decimal bound string shipped in the IR. Bounds wider than
/// `u128` fail to parse and are capped at `u128::MAX` — `Value::Integer`
/// cannot hold anything larger, so every representable value is in range.
fn check_uint_range(n: u128, maxval: &str) -> Result<u128, InterpreterError> {
    let max: u128 = maxval.parse().unwrap_or(u128::MAX);
    if n > max {
        return Err(InterpreterError::TypeError(format!(
            "integer {n} out of range for Uint with maxval {max}"
        )));
    }
    Ok(max)
}

/// Build a single-atom `AlignedValue` with `Bytes<length>` alignment from raw
/// bytes, trimming trailing zeros to satisfy the FAB normal-form invariant
/// (`is_in_normal_form`). The alignment metadata still records `length = N`
/// so equality against zero-padded constants works.
fn bytes_aligned_value(bytes: Vec<u8>, length: usize) -> Result<AlignedValue, InterpreterError> {
    use midnight_base_crypto::fab;
    let byte_len = bytes.len();
    let mut atom = bytes;
    while matches!(atom.last(), Some(0)) {
        atom.pop();
    }
    fab::AlignedValue::new(
        fab::Value(vec![fab::ValueAtom(atom)]),
        fab::Alignment::singleton(fab::AlignmentAtom::Bytes {
            length: length as u32,
        }),
    )
    .ok_or_else(|| {
        InterpreterError::TypeError(format!(
            "{byte_len} bytes do not fit a Bytes<{length}> alignment"
        ))
    })
}

/// Encode a runtime [`Value`] as an [`AlignedValue`] whose alignment matches
/// the declared [`TypeRef`]. This is the single type-aware FAB encoder:
/// `Expr::New` struct fields, ledger cell/key pushes ([`encode_ledger_key`]),
/// literal path keys ([`path_value_to_aligned`]) and `Idx` path variables all
/// route through here, so a new `TypeRef` variant only needs handling in one
/// place.
///
/// # Why the width matters
///
/// FAB atoms are zero-trimmed little-endian bytes, so the atom for a given
/// number is identical at every width; the declared width lives only in the
/// `AlignmentAtom::Bytes { length }`. That alignment participates in
/// `AlignedValue` equality/hashing (on-chain `Map` lookups compare the full
/// `AlignedValue`) and in `persistentHash`, which zero-pads each atom to the
/// declared width. The width ladder below (u8/u16/u32/u64/u128) must
/// therefore match the bindgen-emitted encoders (`uint_tokens` in
/// compact-codegen) byte-for-byte.
///
/// For `Value::Integer`, this picks the right number of bytes from the
/// target `Uint{maxval}` width — `Value::Integer(1000)` embedded as
/// `Uint<128>` becomes a 16-byte atom, not the 8-byte default
/// `to_aligned_value` would produce. Integers that exceed the declared bound
/// (e.g. 300 for `Uint{maxval: 255}`) are an error, never a silent wrap.
fn encode_typed(val: &Value, ty: &TypeRef) -> Result<AlignedValue, InterpreterError> {
    use midnight_base_crypto::fab;
    let unsupported =
        || InterpreterError::TypeError(format!("cannot encode value {val:?} as type {ty:?}"));
    match ty {
        TypeRef::Boolean => match val {
            Value::Bool(b) => Ok(AlignedValue::from(*b)),
            Value::Integer(n) => Ok(AlignedValue::from(*n != 0)),
            // A Boolean sliced out of a struct (e.g. `recipient.is_left`)
            // arrives as a single-byte `AlignedValue` (0x00/0x01); re-encode
            // it as a Boolean so it can flow into another struct field.
            Value::AlignedValue(av) => aligned_atom_to_u128(av)
                .map(|n| AlignedValue::from(n != 0))
                .ok_or_else(unsupported),
            _ => Err(unsupported()),
        },
        TypeRef::Uint { maxval } => {
            let n = value_to_u128(val).ok_or_else(unsupported)?;
            let max = check_uint_range(n, maxval)?;
            // Choose the smallest standard primitive width >= max so that
            // the alignment matches what the on-chain runtime expects.
            // (`From<u8/u16/u32/u64/u128>` set the alignment via `Aligned`.)
            // The `as` casts are lossless: `check_uint_range` guarantees
            // `n <= max`, and each branch requires `max` to fit the width.
            if max <= u8::MAX as u128 {
                Ok(AlignedValue::from(n as u8))
            } else if max <= u16::MAX as u128 {
                Ok(AlignedValue::from(n as u16))
            } else if max <= u32::MAX as u128 {
                Ok(AlignedValue::from(n as u32))
            } else if max <= u64::MAX as u128 {
                Ok(AlignedValue::from(n as u64))
            } else {
                // Bounds above u128 also land here (bindgen's `uint_tokens` falls back to
                // `Vec<u8>` instead, so the byte-for-byte claim above does not cover them),
                // but `Value::Integer` is u128 so no representable value can exceed 16 bytes.
                Ok(AlignedValue::from(n))
            }
        }
        TypeRef::Field => match val {
            Value::AlignedValue(av) => Ok(av.clone()),
            Value::Integer(n) => {
                use midnight_transient_crypto::curve::Fr;
                // Exact u128 → Fr conversion — see `value_to_fr`.
                Ok(AlignedValue::from(Fr::from(*n)))
            }
            _ => Err(unsupported()),
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
                Ok(av)
            }
            Value::Void => bytes_aligned_value(Vec::new(), *length),
            _ => Err(unsupported()),
        },
        TypeRef::Opaque { .. } => match val {
            Value::AlignedValue(av) => Ok(av.clone()),
            // `default<Opaque<...>>` (e.g. via `none<Opaque<"string">>()`)
            // evaluates to Void. The Compact runtime encodes opaque values
            // as a single Compress-aligned atom and their default as the
            // empty value (compact-types.ts: CompactTypeOpaqueString /
            // CompactTypeOpaqueUint8Array), i.e. an empty atom.
            Value::Void => fab::AlignedValue::new(
                fab::Value(vec![fab::ValueAtom(Vec::new())]),
                fab::Alignment::singleton(fab::AlignmentAtom::Compress),
            )
            .ok_or_else(unsupported),
            _ => Err(unsupported()),
        },
        TypeRef::Tuple { types } => match val {
            Value::Tuple(elements) if elements.len() == types.len() => {
                let parts: Vec<AlignedValue> = elements
                    .iter()
                    .zip(types.iter())
                    .map(|(e, t)| encode_typed(e, t))
                    .collect::<Result<_, _>>()?;
                Ok(AlignedValue::concat(parts.iter()))
            }
            _ => Err(unsupported()),
        },
        TypeRef::Vector { length, element } => match val {
            Value::Tuple(elements) if elements.len() == *length => {
                let parts: Vec<AlignedValue> = elements
                    .iter()
                    .map(|e| encode_typed(e, element))
                    .collect::<Result<_, _>>()?;
                Ok(AlignedValue::concat(parts.iter()))
            }
            _ => Err(unsupported()),
        },
        // For Struct/Maybe receivers we'd need the layout registry to
        // recurse field-by-field; the current call sites (Expr::New) only
        // need the leaf type encodings above. Fall back to to_aligned_value.
        TypeRef::Struct { .. } | TypeRef::Maybe { .. } => Ok(val.to_aligned_value()),
        TypeRef::Void => match val {
            Value::Void => Ok(AlignedValue::from(())),
            _ => Err(unsupported()),
        },
        TypeRef::Enum { .. } => match val {
            // On-chain enums encode as their u8 declaration index.
            Value::Integer(n) => {
                let idx = u8::try_from(*n).map_err(|_| {
                    InterpreterError::TypeError(format!(
                        "integer {n} out of range for enum (max 255)"
                    ))
                })?;
                Ok(AlignedValue::from(idx))
            }
            Value::AlignedValue(av) => Ok(av.clone()),
            _ => Err(unsupported()),
        },
    }
}

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

/// Encode an evaluated [`Value`] as a [`StateValue`] for pushing onto
/// the ledger query stack, re-aligning integers to the expression's
/// declared [`TypeRef`] when known.
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
    ty: Option<&TypeRef>,
) -> Result<StateValue<InMemoryDB>, InterpreterError> {
    match (val, ty) {
        (Value::Integer(_), Some(ty)) => encode_typed(val, ty).map(StateValue::from),
        _ => Ok(val.to_state_value()),
    }
}

/// Convert a literal path value string + declared type to an `AlignedValue`,
/// delegating the width-sensitive encoding to [`encode_typed`].
fn path_value_to_aligned(value: &str, ty: &TypeRef) -> Result<AlignedValue, InterpreterError> {
    match ty {
        TypeRef::Boolean => Ok(AlignedValue::from(value == "true" || value == "1")),
        TypeRef::Uint { .. } | TypeRef::Field | TypeRef::Enum { .. } => {
            let n: u128 = value.parse().map_err(|e| {
                InterpreterError::TypeError(format!(
                    "invalid integer path literal {value:?} for {ty:?}: {e}"
                ))
            })?;
            encode_typed(&Value::Integer(n), ty)
        }
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
    use midnight_bindgen_runtime::{ContractMaintenanceAuthority, StorageHashMap};

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
            Value::Integer(5).to_aligned_value(),
            AlignedValue::from(5u64)
        );
        assert_eq!(
            Value::Integer(u64::MAX as u128).to_aligned_value(),
            AlignedValue::from(u64::MAX)
        );
        // Values above u64::MAX must be encoded wide, not truncated.
        let big = u64::MAX as u128 + 1;
        assert_eq!(
            Value::Integer(big).to_aligned_value(),
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
        let ty = TypeRef::Uint {
            maxval: u128::MAX.to_string(),
        };
        let av = encode_typed(&Value::Integer(big), &ty).expect("encode");
        // Byte-for-byte the u128 encoding (atom + Bytes{16} alignment)...
        assert_eq!(av, AlignedValue::from(big));
        // ...and decodes back without losing the high bits.
        assert_eq!(u128::try_from(&*av.value).expect("decode"), big);
    }

    #[test]
    fn typed_uint_encode_rejects_out_of_range() {
        let ty = TypeRef::Uint {
            maxval: "255".to_string(),
        };
        assert!(encode_typed(&Value::Integer(255), &ty).is_ok());
        let err = encode_typed(&Value::Integer(300), &ty).expect_err("out of range");
        assert!(matches!(err, InterpreterError::TypeError(_)));
        // Enum indices are u8 on-chain; anything wider must error too.
        let err = encode_typed(
            &Value::Integer(300),
            &TypeRef::Enum {
                name: "Whatever".to_string(),
            },
        )
        .expect_err("enum index out of range");
        assert!(matches!(err, InterpreterError::TypeError(_)));
    }

    #[test]
    fn typed_uint_encode_uses_declared_width() {
        // The ladder must match the bindgen-emitted encoders: Uint<=65535>
        // is a u16 (2-byte) atom, not the type-less 8-byte default.
        let ty = TypeRef::Uint {
            maxval: "65535".to_string(),
        };
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
    fn typed_uint_encode_ladder_boundary_at_u64() {
        // The u64/u128 cutoff of the width ladder: maxval == u64::MAX stays on
        // the 8-byte rung, one above it moves to the 16-byte rung.
        let ty = TypeRef::Uint {
            maxval: u64::MAX.to_string(),
        };
        let av = encode_typed(&Value::Integer(7), &ty).expect("encode");
        assert_eq!(av, AlignedValue::from(7u64));

        let ty = TypeRef::Uint {
            maxval: (u64::MAX as u128 + 1).to_string(),
        };
        let av = encode_typed(&Value::Integer(7), &ty).expect("encode");
        assert_eq!(av, AlignedValue::from(7u128));
    }

    #[test]
    fn path_value_field_literal_above_u64_is_exact() {
        use midnight_transient_crypto::curve::Fr;
        let n = (1u128 << 64) + 5;
        let av = path_value_to_aligned(&n.to_string(), &TypeRef::Field).expect("encode");
        let expected = fr_two_pow_64() + Fr::from(5u64);
        assert_eq!(av, AlignedValue::from(expected));
    }

    #[test]
    fn uint_literal_out_of_range_errors() {
        let state = make_counter_state(0);
        let ir_json = r#"{
            "body": {
                "op": "expr-stmt",
                "expr": { "op": "lit", "type": { "type": "Uint", "maxval": "255" }, "value": "300" }
            },
            "result": null
        }"#;
        let ir: CircuitIrBody = serde_json::from_str(ir_json).expect("parse IR");
        let err = match execute(&ir, &state) {
            Err(e) => e,
            Ok(_) => panic!("literal 300 exceeds Uint<= 255> but execution succeeded"),
        };
        assert!(
            matches!(err, InterpreterError::TypeError(_)),
            "expected TypeError, got {err:?}"
        );
    }

    #[test]
    fn vector_index_beyond_usize_errors() {
        // 2^64 + 1 truncated `as usize` on 64-bit would wrap to 1 and silently
        // read element 1; it must error with the offending index instead.
        let state = make_counter_state(0);
        let ir_json = r#"{
            "body": {
                "op": "expr-stmt",
                "expr": {
                    "op": "vector-index",
                    "expr": { "op": "var", "name": "v" },
                    "index": { "op": "lit",
                               "type": { "type": "Uint", "maxval": "340282366920938463463374607431768211455" },
                               "value": "18446744073709551617" }
                }
            },
            "result": null
        }"#;
        let ir: CircuitIrBody = serde_json::from_str(ir_json).expect("parse IR");
        let vector = Value::Tuple(vec![Value::Integer(10), Value::Integer(20)]);
        let err = match execute_with(&ir, &state, &[("v", vector)], &NoWitnesses, &[], &[]) {
            Err(e) => e,
            Ok(res) => panic!(
                "index 2^64 + 1 must not wrap to element 1, got {:?}",
                res.result
            ),
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

    // -----------------------------------------------------------------------
    // Spread + Bytes/Field/Vector conversion IR forms
    //
    // The JSON shapes below mirror what the fork compiler's
    // `save-contract-info-passes.ss` emits for `spread`, `bytes-to-field`,
    // `field-to-bytes`, `bytes-to-vector` and `vector-to-bytes`; the runtime
    // semantics asserted here follow the compiler's own TypeScript runtime
    // (`tools/compact-compiler/runtime/src/casts.ts`): little-endian byte
    // order, zero padding, and rejection (not reduction) on range overflow.
    // -----------------------------------------------------------------------

    /// Evaluate a single IR expression (given as JSON) as the circuit's
    /// result expression, with `args` pre-seeded as locals.
    fn eval_expr_json(expr_json: &str, args: &[(&str, Value)]) -> Result<Value, InterpreterError> {
        let ir_json = format!(r#"{{"body": {{"op": "seq", "stmts": []}}, "result": {expr_json}}}"#);
        let ir: CircuitIrBody = serde_json::from_str(&ir_json).expect("parse IR");
        let state = make_counter_state(0);
        execute_with(&ir, &state, args, &NoWitnesses, &[], &[])
            .map(|r| r.result.expect("expression result"))
    }

    #[test]
    fn spread_splices_tuple_value_into_constructor() {
        let v = Value::Tuple(vec![Value::Integer(2), Value::Integer(3)]);
        let expr = r#"{
            "op": "tuple",
            "elements": [
                { "op": "lit", "type": { "type": "Uint", "maxval": "255" }, "value": "1" },
                { "op": "spread", "length": 2, "expr": { "op": "var", "name": "v" } },
                { "op": "lit", "type": { "type": "Uint", "maxval": "255" }, "value": "4" }
            ]
        }"#;
        let result = eval_expr_json(expr, &[("v", v)]).expect("eval");
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
        let expr = r#"{
            "op": "tuple",
            "elements": [
                { "op": "spread", "length": 2, "expr": { "op": "var", "name": "v" } }
            ]
        }"#;
        let result = eval_expr_json(expr, &[("v", Value::AlignedValue(av))]).expect("eval");
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
        let expr = r#"{
            "op": "tuple",
            "elements": [
                { "op": "spread", "length": 2, "expr": { "op": "var", "name": "v" } }
            ]
        }"#;
        let err = eval_expr_json(expr, &[("v", v)]).expect_err("length mismatch must error");
        assert!(
            err.to_string().contains("spread"),
            "error should mention spread: {err}"
        );
    }

    #[test]
    fn bare_spread_outside_constructor_errors() {
        let v = Value::Tuple(vec![Value::Integer(1), Value::Integer(2)]);
        let expr = r#"{ "op": "spread", "length": 2, "expr": { "op": "var", "name": "v" } }"#;
        let err = eval_expr_json(expr, &[("v", v)]).expect_err("bare spread must error");
        assert!(
            err.to_string().contains("spread"),
            "error should mention spread: {err}"
        );
    }

    #[test]
    fn bytes_to_field_is_little_endian() {
        use midnight_transient_crypto::curve::Fr;
        // Bytes<4> = [0x2A, 0x01, 0x00, 0x00]; byte 0 is the least
        // significant (casts.ts convertBytesToField), so the value is
        // 0x2A + 0x01·256 = 298.
        let expr = r#"{
            "op": "bytes-to-field", "length": 4,
            "expr": { "op": "lit", "type": { "type": "Bytes", "length": 4 }, "value": "2a010000" }
        }"#;
        let result = eval_expr_json(expr, &[]).expect("eval");
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
        let expr = format!(
            r#"{{
                "op": "bytes-to-field", "length": 32,
                "expr": {{ "op": "lit", "type": {{ "type": "Bytes", "length": 32 }}, "value": "{}" }}
            }}"#,
            "ff".repeat(32)
        );
        let err = eval_expr_json(&expr, &[]).expect_err("over-modulus bytes must error");
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
        let expr_for = |bytes: &[u8]| {
            format!(
                r#"{{
                    "op": "bytes-to-field", "length": 32,
                    "expr": {{ "op": "lit", "type": {{ "type": "Bytes", "length": 32 }}, "value": "{}" }}
                }}"#,
                hex::encode(bytes)
            )
        };

        let result = eval_expr_json(&expr_for(&le), &[]).expect("p - 1 must be accepted");
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
        let err = eval_expr_json(&expr_for(&p), &[]).expect_err("exactly p must be rejected");
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
        let expr = r#"{
            "op": "bytes-to-field", "length": 0,
            "expr": { "op": "lit", "type": { "type": "Bytes", "length": 0 }, "value": "" }
        }"#;
        let result = eval_expr_json(expr, &[]).expect("eval");
        match result {
            Value::AlignedValue(av) => {
                assert_eq!(Fr::try_from(&*av.value).expect("Fr"), Fr::from(0u64));
            }
            other => panic!("expected AlignedValue, got {other:?}"),
        }
    }

    #[test]
    fn field_to_bytes_is_little_endian_and_bytes_aligned() {
        use midnight_base_crypto::fab;
        // 298 → LE bytes [0x2A, 0x01], logically zero-padded to Bytes<32>
        // (casts.ts convertFieldToBytes). The expected value is built from
        // FAB primitives directly so the test does not validate the
        // production encoder against itself.
        let expr = r#"{
            "op": "field-to-bytes", "length": 32,
            "expr": { "op": "lit", "type": { "type": "Field" }, "value": "298" }
        }"#;
        let result = eval_expr_json(expr, &[]).expect("eval");
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
        let expr = r#"{
            "op": "bytes-to-field", "length": 32,
            "expr": {
                "op": "field-to-bytes", "length": 32,
                "expr": { "op": "lit", "type": { "type": "Field" }, "value": "12345678901234567890" }
            }
        }"#;
        let result = eval_expr_json(expr, &[]).expect("eval");
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
        let expr = r#"{
            "op": "field-to-bytes", "length": 1,
            "expr": { "op": "lit", "type": { "type": "Field" }, "value": "298" }
        }"#;
        let err = eval_expr_json(expr, &[]).expect_err("too-wide value must error");
        assert!(
            err.to_string().contains("fit"),
            "error should mention the value not fitting: {err}"
        );
    }

    #[test]
    fn bytes_to_vector_yields_bytes_in_order() {
        // Element i of the vector is byte i of the byte string
        // (typescript-passes.ss lowers bytes->vector to `Array.from(expr, BigInt)`).
        let expr = r#"{
            "op": "bytes-to-vector", "length": 4,
            "expr": { "op": "lit", "type": { "type": "Bytes", "length": 4 }, "value": "01020300" }
        }"#;
        let result = eval_expr_json(expr, &[]).expect("eval");
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
        let expr =
            r#"{ "op": "vector-to-bytes", "length": 4, "expr": { "op": "var", "name": "v" } }"#;
        let result = eval_expr_json(expr, &[("v", v)]).expect("eval");
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
        let expr =
            r#"{ "op": "vector-to-bytes", "length": 1, "expr": { "op": "var", "name": "v" } }"#;
        let err = eval_expr_json(expr, &[("v", v)]).expect_err("element > 255 must error");
        assert!(
            err.to_string().contains("255"),
            "error should mention the byte bound: {err}"
        );
    }

    #[test]
    fn bytes_to_vector_round_trips_through_vector_to_bytes() {
        use midnight_base_crypto::fab;
        let expr = r#"{
            "op": "vector-to-bytes", "length": 3,
            "expr": {
                "op": "bytes-to-vector", "length": 3,
                "expr": { "op": "lit", "type": { "type": "Bytes", "length": 3 }, "value": "aabb00" }
            }
        }"#;
        let result = eval_expr_json(expr, &[]).expect("eval");
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
    fn encode_typed_opaque_default_is_empty_compress_atom() {
        use midnight_base_crypto::fab;
        // `default<Opaque<"string">>` (Value::Void) must encode as the empty
        // string: one empty atom with Compress alignment (compact-types.ts
        // CompactTypeOpaqueString).
        let av = encode_typed(
            &Value::Void,
            &TypeRef::Opaque {
                name: "string".to_string(),
            },
        )
        .expect("encode default opaque");
        let expected = fab::AlignedValue::new(
            fab::Value(vec![fab::ValueAtom(Vec::new())]),
            fab::Alignment::singleton(fab::AlignmentAtom::Compress),
        )
        .unwrap();
        assert_eq!(av, expected);
    }

    #[test]
    fn contract_call_unsupported_names_target() {
        let expr = r#"{
            "op": "contract-call",
            "circuit": "do_thing",
            "contract": { "op": "var", "name": "other_contract" },
            "contract-type": { "type": "Void" },
            "args": []
        }"#;
        let err = eval_expr_json(expr, &[]).expect_err("contract-call must be unsupported");
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
    fn test_ctx(
        private_state: &mut Vec<u8>,
        local_types: HashMap<String, TypeRef>,
    ) -> ExecContext<'_> {
        ExecContext {
            state: make_counter_state(0),
            locals: HashMap::new(),
            local_types,
            reads: Vec::new(),
            gather_ops: Vec::new(),
            communication_outputs: Vec::new(),
            private_transcript_outputs: Vec::new(),
            zswap_outputs: Vec::new(),
            last_expr_value: None,
            witnesses: None,
            private_state,
            helpers: HashMap::new(),
            layouts: HashMap::new(),
            struct_defs: HashMap::new(),
            enum_defs: HashMap::new(),
            contract_address: None,
        }
    }

    #[test]
    fn infer_types_of_conversion_forms() {
        let mut ps = Vec::new();
        let ctx = test_ctx(&mut ps, HashMap::new());
        let parse = |s: &str| serde_json::from_str::<Expr>(s).expect("parse expr");

        let b2f = parse(r#"{"op":"bytes-to-field","length":32,"expr":{"op":"var","name":"x"}}"#);
        assert!(matches!(
            infer_type_of_expr(&ctx, &b2f),
            Some(TypeRef::Field)
        ));

        let f2b = parse(r#"{"op":"field-to-bytes","length":32,"expr":{"op":"var","name":"x"}}"#);
        assert!(matches!(
            infer_type_of_expr(&ctx, &f2b),
            Some(TypeRef::Bytes { length: 32 })
        ));

        let b2v = parse(r#"{"op":"bytes-to-vector","length":4,"expr":{"op":"var","name":"x"}}"#);
        match infer_type_of_expr(&ctx, &b2v) {
            Some(TypeRef::Vector { length: 4, element }) => {
                assert!(matches!(*element, TypeRef::Uint { ref maxval } if maxval == "255"));
            }
            other => panic!("expected Vector<4, Uint<255>>, got {other:?}"),
        }

        let v2b = parse(r#"{"op":"vector-to-bytes","length":4,"expr":{"op":"var","name":"x"}}"#);
        assert!(matches!(
            infer_type_of_expr(&ctx, &v2b),
            Some(TypeRef::Bytes { length: 4 })
        ));
    }

    #[test]
    fn infer_type_of_tuple_with_spread_splices_inner_types() {
        let mut ps = Vec::new();
        let mut local_types = HashMap::new();
        local_types.insert(
            "v".to_string(),
            TypeRef::Vector {
                length: 2,
                element: Box::new(TypeRef::Field),
            },
        );
        let ctx = test_ctx(&mut ps, local_types);
        let expr: Expr = serde_json::from_str(
            r#"{
                "op": "tuple",
                "elements": [
                    { "op": "lit", "type": { "type": "Boolean" }, "value": "true" },
                    { "op": "spread", "length": 2, "expr": { "op": "var", "name": "v" } }
                ]
            }"#,
        )
        .expect("parse expr");
        match infer_type_of_expr(&ctx, &expr) {
            Some(TypeRef::Tuple { types }) => {
                assert_eq!(types.len(), 3, "spread must contribute 2 element types");
                assert!(matches!(types[0], TypeRef::Boolean));
                assert!(matches!(types[1], TypeRef::Field));
                assert!(matches!(types[2], TypeRef::Field));
            }
            other => panic!("expected Tuple type, got {other:?}"),
        }
    }
}
