//! Circuit interpreter.
//!
//! Executes a circuit's analyzed-IR body ([`ir::Expr`]) against contract
//! state, using midnight-ledger's VM (`QueryContext::query`) for ledger
//! operations.

use std::collections::HashMap;

use midnight_base_crypto::time::Timestamp;
use midnight_coin_structure::coin::PublicKey as CoinPublicKey;
use midnight_coin_structure::contract::ContractAddress;
use midnight_onchain_runtime::context::{CallContext, QueryContext};
use midnight_onchain_runtime::cost_model::INITIAL_COST_MODEL;
use midnight_onchain_runtime::ops::{Key, Op};
use midnight_onchain_runtime::result_mode::{GatherEvent, ResultModeGather};
use midnight_typed_state::{
    AlignedValue, ContractState, InMemoryDB, MerkleTree, StateValue, StorageArray, StorageHashMap,
};

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
    aligned_atom_to_u128, bytes_aligned_value, default_value, element_atom_range, element_count,
    element_type_at, encode_typed, ir_length, layout_from_fields, merkle_leaf_hash, slice_atoms,
    try_builtin_typed, value_to_fr, value_to_u128,
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
    /// Native declarations, indexed by their full identifier text. One source
    /// name can declare several natives: `ecAdd` on Jubjub points and `ecAdd`
    /// on secp256k1 points are distinct declarations.
    pub natives: HashMap<&'a str, &'a ir::Native>,
}

/// The prefix of a native's `entry`, the module the TypeScript runtime
/// exports the native from.
const RUNTIME_MODULE: &str = "__compactRuntime.";

impl<'a> Program<'a> {
    pub fn new(
        circuits: &'a [ir::Circuit],
        witnesses: &'a [ir::Witness],
        natives: &'a [ir::Native],
    ) -> Self {
        Self {
            circuits: circuits.iter().map(|c| (c.name.0.as_str(), c)).collect(),
            witnesses: witnesses.iter().map(|w| (w.name.name(), w)).collect(),
            natives: natives.iter().map(|n| (n.name.0.as_str(), n)).collect(),
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
        match self.natives.get(id.0.as_str()) {
            Some(n) if n.class == "witness" => Callee::Witness,
            Some(_) => Callee::Pure,
            None if self.witnesses.contains_key(id.name()) => Callee::Witness,
            None => Callee::Pure,
        }
    }

    /// The name the runtime dispatches a witness or pure call on.
    ///
    /// For a native this is its runtime symbol, which is unique per
    /// declaration (`ecAdd` on secp256k1 points is `secp256k1Add`). Anything
    /// else dispatches on its source name.
    fn runtime_name<'b>(&self, id: &'b ir::Ident) -> &'b str
    where
        'a: 'b,
    {
        match self.natives.get(id.0.as_str()) {
            Some(n) => n.entry.strip_prefix(RUNTIME_MODULE).unwrap_or(&n.entry),
            None => id.name(),
        }
    }

    /// The declared parameter types of the native or witness a call names, in
    /// argument order, which a builtin encodes its arguments at. An undeclared
    /// callee has none.
    fn argument_types(&self, id: &ir::Ident) -> Vec<Type> {
        let arguments = match self.natives.get(id.0.as_str()) {
            Some(n) => &n.arguments,
            None => match self.witnesses.get(id.name()) {
                Some(w) => &w.arguments,
                None => return Vec::new(),
            },
        };
        arguments.iter().map(|a| a.ty.clone()).collect()
    }

    /// The callee's declared result type, which the call site's static type is.
    fn result_type(&self, id: &ir::Ident) -> Option<&'a Type> {
        if let Some(c) = self.circuits.get(id.0.as_str()).copied() {
            return Some(&c.result_type);
        }
        if let Some(n) = self.natives.get(id.0.as_str()).copied() {
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

/// What a circuit runs with, apart from its program, state and arguments.
///
/// Callers start from [`Env::new`], which names the block time, and override
/// the other fields with struct update:
///
/// ```
/// use compact_interpreter::Env;
/// use compact_runtime::NoWitnesses;
/// use midnight_base_crypto::time::Timestamp;
///
/// let mut private_state = Vec::new();
/// let env = Env {
///     witnesses: &NoWitnesses,
///     private_state: Some(&mut private_state),
///     ..Env::new(Timestamp::from_secs(1_700_000_000))
/// };
/// ```
pub struct Env<'a> {
    /// The provider that answers the circuit's witness calls.
    pub witnesses: &'a dyn WitnessProvider,
    /// The private state that the witnesses read and update. After
    /// [`execute`] returns, the buffer holds the state after the call. `None`
    /// runs the witnesses on a scratch buffer.
    pub private_state: Option<&'a mut Vec<u8>>,
    /// The address that `kernel.self()` reads.
    pub address: ContractAddress,
    /// The time that the kernel clock checks compare against.
    pub block_time: Timestamp,
    /// The caller's coin public key, which `ownPublicKey()` returns. With
    /// `None`, a circuit that calls `ownPublicKey()` fails with
    /// [`InterpreterError::Witness`]. A default key would run the circuit as a
    /// key that the caller does not hold.
    pub coin_public_key: Option<CoinPublicKey>,
}

impl Env<'_> {
    /// No witnesses, a scratch private state, the zero address, no coin
    /// public key.
    pub fn new(block_time: Timestamp) -> Self {
        Self {
            witnesses: &NoWitnesses,
            private_state: None,
            address: ContractAddress::default(),
            block_time,
            coin_public_key: None,
        }
    }
}

// The private state can hold secret keys, so `Debug` leaves it out with the
// witness provider.
impl std::fmt::Debug for Env<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Env")
            .field("address", &self.address)
            .field("block_time", &self.block_time)
            .field("coin_public_key", &self.coin_public_key)
            .finish_non_exhaustive()
    }
}

/// Execute a circuit against a contract state.
///
/// `args` are the circuit's arguments as (name, value) pairs, keyed by the
/// argument's source name. `env` carries the witnesses, the private state,
/// the contract address, the block time and the caller's coin public key. The
/// kernel balance checks read the balance of `state`.
///
/// # Errors
///
/// An [`InterpreterError`] when the circuit fails: a failed `assert`, a
/// witness failure, a ledger operation that the VM rejects, or a construct
/// that the interpreter does not support.
pub fn execute(
    circuit: &ir::Circuit,
    program: &Program<'_>,
    state: ContractState<InMemoryDB>,
    args: &[(&str, Value)],
    env: Env<'_>,
) -> Result<ExecutionResult, InterpreterError> {
    let Env {
        witnesses,
        private_state,
        address,
        block_time,
        coin_public_key,
    } = env;
    let mut scratch = Vec::new();
    let private_state = private_state.unwrap_or(&mut scratch);
    let call_context = CallContext {
        own_address: address,
        tblock: block_time,
        balance: state.balance.clone(),
        ..CallContext::default()
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
        call_context,
        coin_public_key,
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
    if comm_outputs.is_empty()
        && let Some(ref val) = result_value
        && !matches!(val, Value::Void)
    {
        comm_outputs.push(encode_typed(val, &circuit.result_type)?);
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
    /// The results of the witness calls and of the witness-class natives, in
    /// call order. Surfaced as `ExecutionResult::private_transcript_outputs`.
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
    /// The `CallContext` of every ledger operation of this run, built once
    /// from the [`Env`] and the balance of the state. The clock checks read
    /// its `tblock` (slot 2), and the balance checks its `balance` (slot 5).
    call_context: CallContext<InMemoryDB>,
    coin_public_key: Option<CoinPublicKey>,
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
        E::TupleRef { expr, index } => {
            let operand = infer_type_of_expr(ctx, expr)?;
            element_type_at(&operand, usize::try_from(*index).ok()).cloned()
        }
        // The node carries the operand's declared type, which is the reliable
        // one: inference types an arithmetic node as its left operand, a
        // narrower width than the value in flight was encoded with.
        E::VectorRef { ty, expr, index } => {
            let i = const_index(index);
            element_type_at(ty, i).cloned().or_else(|| {
                let operand = infer_type_of_expr(ctx, expr)?;
                element_type_at(&operand, i).cloned()
            })
        }
        E::EltRef { expr, elt, .. } => {
            let recv_ty = infer_type_of_expr(ctx, expr)?;
            let Type::Struct { fields, .. } = recv_ty.resolved() else {
                return None;
            };
            fields
                .iter()
                .find(|(n, _)| n == elt)
                .map(|(_, t)| t.clone())
        }
        E::Assert { .. } | E::Emit { .. } => Some(Type::unit()),
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
        // The `ty` a slice carries is its OPERAND's type, the one eval slices
        // by; the result is the run of `len` elements taken out of it.
        E::TupleSlice { ty, index, len, .. } => {
            slice_result_type(ty, usize::try_from(*index).ok(), *len)
        }
        E::VectorSlice { ty, index, len, .. } => slice_result_type(ty, const_index(index), *len),
        E::BytesSlice { len, .. } => Some(Type::Bytes(*len)),
        E::BytesRef { .. } => Some(byte_type()),
        // A fold returns its accumulator, so it has the type the initial
        // value has. Without this a Field accumulator reaches the ledger as a
        // width-guessed integer cell instead of a field atom.
        E::Fold { init, .. } => infer_type_of_expr(ctx, init),
        // A map builds one element per iteration from its callee, so its type
        // is the callee's declared result type at the node's length.
        E::Map { len, fun, .. } => {
            let element = match fun {
                ir::Fun::Circuit { result_type, .. } => result_type.clone(),
                ir::Fun::Ref(id) => ctx.program.result_type(id)?.clone(),
            };
            Some(Type::Vector {
                len: *len,
                ty: Box::new(element),
            })
        }
        // A cross-contract call returns what the receiver's contract type
        // declares for the circuit it names.
        E::ContractCall {
            circuit,
            contract_type,
            ..
        } => match contract_type.resolved() {
            Type::Contract { circuits, .. } => circuits
                .iter()
                .find(|c| c.name == *circuit)
                .map(|c| c.result_type.clone()),
            _ => None,
        },
    }
}

/// The type of the run of `len` elements a slice takes from `operand_ty`.
///
/// A vector's elements are uniform, so the start does not change the answer.
/// A tuple's are not, so an unknown start leaves the type unknown.
fn slice_result_type(operand_ty: &Type, start: Option<usize>, len: u64) -> Option<Type> {
    match operand_ty.resolved() {
        Type::Vector { ty, .. } => Some(Type::Vector {
            len,
            ty: ty.clone(),
        }),
        Type::Tuple(types) => {
            let start = start?;
            let end = start.checked_add(usize::try_from(len).ok()?)?;
            Some(Type::Tuple(types.get(start..end)?.to_vec()))
        }
        _ => None,
    }
}

/// The value of an index expression the compiler reduced to a literal.
fn const_index(expr: &ir::Expr) -> Option<usize> {
    match expr {
        ir::Expr::Quote(ir::Literal::Int(n)) => usize::try_from(n).ok(),
        _ => None,
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
        // real VM (`exec_ledger_query` puts the `Env` address in the
        // `QueryContext`), so the read returns the right address *and*
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

        // The `push` operand carries the payload expression again, so
        // evaluating `payload` as well would put its ledger reads in the
        // transcript twice. The VM turns the pushed array into the event.
        E::Emit { instructions, .. } => {
            exec_ledger_query(ctx, instructions)?;
            Ok(Value::Void)
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
                // A right-hand side that inference cannot read (such as a
                // cross-contract call) still has the type the binder
                // declares.
                let ty = infer_type_of_expr(ctx, value).unwrap_or_else(|| binder.ty.clone());
                let val = eval_expr(ctx, value)?;
                ctx.locals.insert(binder.name.0.clone(), val);
                ctx.local_types.insert(binder.name.0.clone(), ty);
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
                Callee::Witness => eval_witness_call(ctx, name, values),
                Callee::Pure => eval_pure_call(ctx, name, values),
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

        // The operand is evaluated before the index, as every other indexed
        // and sliced node does: an index expression can read the ledger or
        // call a witness, and the transcript records those in evaluation
        // order.
        E::VectorRef { ty, expr, index } => {
            let val = eval_expr(ctx, expr)?;
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
            indexed_element(&val, Some(ty), idx, "vector")
        }

        E::TupleRef { expr, index } => {
            let index = ir_length(*index)?;
            // The node carries no type of its own, so a receiver that arrives
            // flattened is sliced by the layout inference gives it, the way
            // field access is.
            let receiver_ty = infer_type_of_expr(ctx, expr);
            let val = eval_expr(ctx, expr)?;
            match &val {
                // Structs can be indexed by position (field declaration order)
                // This is a fallback — prefer field access by name
                Value::Struct(fields) => fields.values().nth(index).cloned().ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "struct index {index} out of bounds (len {})",
                        fields.len()
                    ))
                }),
                _ => indexed_element(&val, receiver_ty.as_ref(), index, "tuple"),
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
                    let receiver_ty = receiver_ty.as_ref().map(Type::resolved);
                    let struct_name = match receiver_ty {
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
                    let Some(Type::Struct { fields, .. }) = receiver_ty else {
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
                    slice_atoms(av, offset..offset + len)
                        .map(Value::AlignedValue)
                        .ok_or_else(|| {
                            InterpreterError::TypeError(format!(
                                "field .{elt} takes atoms [{offset}..{}] of an AlignedValue \
                                 that does not carry one alignment atom per value atom \
                                 (value_len={}, alignment_len={}, struct={struct_name})",
                                offset + len,
                                av.value.0.len(),
                                av.alignment.0.len()
                            ))
                        })
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
            let arg_values: Vec<Value> = args
                .iter()
                .map(|a| eval_expr(ctx, &a.expr))
                .collect::<Result<_, _>>()?;
            let len = ir_length(*len)?;
            let mut out = Vec::with_capacity(len);
            for i in 0..len {
                let mut call_args = Vec::with_capacity(arg_values.len());
                for (v, a) in arg_values.iter().zip(args.iter()) {
                    call_args.push(indexed_element(v, Some(&a.ty), i, "loop")?);
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
            let mut acc = eval_expr(ctx, init)?;
            let arg_values: Vec<Value> = args
                .iter()
                .map(|a| eval_expr(ctx, &a.expr))
                .collect::<Result<_, _>>()?;
            for i in 0..ir_length(*len)? {
                let mut call_args = Vec::with_capacity(arg_values.len() + 1);
                call_args.push(acc);
                for (v, a) in arg_values.iter().zip(args.iter()) {
                    call_args.push(indexed_element(v, Some(&a.ty), i, "loop")?);
                }
                acc = apply_fun(ctx, fun, &call_args)?;
            }
            Ok(acc)
        }

        // One byte of a Bytes value.
        E::BytesRef { ty, expr, index } => {
            check_type(ty)?;
            let val = eval_expr(ctx, expr)?;
            let n = value_to_u128(&eval_expr(ctx, index)?).ok_or_else(|| {
                InterpreterError::TypeError("byte index is not an integer".into())
            })?;
            let i = usize::try_from(n).map_err(|_| {
                InterpreterError::TypeError(format!(
                    "byte index {n} out of bounds (does not fit in usize)"
                ))
            })?;
            let Type::Bytes(width) = ty.resolved() else {
                return Err(InterpreterError::TypeError(format!(
                    "byte reference has non-bytes operand type {ty:?}"
                )));
            };
            let width = ir_length(*width)?;
            if i >= width {
                return Err(InterpreterError::TypeError(format!(
                    "byte index {i} out of bounds for a {width}-byte value"
                )));
            }
            let bytes = value_to_byte_string(&val, width)?;
            Ok(Value::Integer(bytes.get(i).copied().unwrap_or(0) as u128))
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
            ty,
            expr,
            index,
            len,
        } => {
            check_type(ty)?;
            let val = eval_expr(ctx, expr)?;
            let n = value_to_u128(&eval_expr(ctx, index)?).ok_or_else(|| {
                InterpreterError::TypeError("slice index is not an integer".into())
            })?;
            let start = usize::try_from(n).map_err(|_| {
                InterpreterError::TypeError(format!(
                    "slice index {n} out of bounds (does not fit in usize)"
                ))
            })?;
            let len = ir_length(*len)?;
            let Type::Bytes(width) = ty.resolved() else {
                return Err(InterpreterError::TypeError(format!(
                    "byte slice has non-bytes operand type {ty:?}"
                )));
            };
            let width = ir_length(*width)?;
            if len > width || start > width - len {
                return Err(InterpreterError::TypeError(format!(
                    "slice of {len} bytes at {start} is out of bounds for a {width}-byte value"
                )));
            }
            let bytes = value_to_byte_string(&val, width)?;
            let slice = (start..start + len)
                .map(|i| bytes.get(i).copied().unwrap_or(0))
                .collect();
            Ok(Value::AlignedValue(bytes_aligned_value(slice, len)?))
        }
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
        let fr = Fr::from_le_bytes(bytes).ok_or_else(|| {
            InterpreterError::TypeError(format!(
                "range error: byte string {} exceeds the maximum value of the Field type",
                hex::encode(bytes)
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
    id: &ir::Ident,
    values: Vec<Value>,
) -> Result<Value, InterpreterError> {
    let program = ctx.program;
    let name = program.runtime_name(id);
    // Handle disclose specially: record the value as a communication output.
    if name == "disclose" {
        return disclose(ctx, values);
    }
    match try_builtin_typed(name, &values, &program.argument_types(id)) {
        Some(result) => result,
        None => Err(InterpreterError::Unsupported(format!(
            "unknown pure function: {name}"
        ))),
    }
}

/// A call the witness provider answers.
fn eval_witness_call(
    ctx: &mut ExecContext,
    id: &ir::Ident,
    values: Vec<Value>,
) -> Result<Value, InterpreterError> {
    let program = ctx.program;
    let name = program.runtime_name(id);
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
            // The prover reads the key as a private input, as it reads a
            // witness result.
            WitnessNative::OwnPublicKey => {
                let key = ctx.coin_public_key.ok_or_else(|| {
                    InterpreterError::Witness(
                        "ownPublicKey needs the caller's coin public key, and \
                         Env.coin_public_key is None"
                            .to_string(),
                    )
                })?;
                let key = AlignedValue::from(key.0);
                ctx.private_transcript_outputs.push(key.clone());
                return Ok(Value::AlignedValue(key));
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
    if let Some(result) = try_builtin_typed(name, &values, &program.argument_types(id)) {
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
/// a single zero-trimmed atom (byte 0 first), so the returned slice may be
/// shorter than `length`; the missing trailing bytes are zero.
fn value_to_byte_string(val: &Value, length: usize) -> Result<&[u8], InterpreterError> {
    match val {
        Value::AlignedValue(av) if av.value.0.len() == 1 => {
            let atom = &av.value.0[0];
            if atom.0.len() > length {
                return Err(InterpreterError::TypeError(format!(
                    "byte string of {} bytes is wider than Bytes<{length}>",
                    atom.0.len()
                )));
            }
            Ok(&atom.0)
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

/// Extract element `i` of a value that carries several.
///
/// The lowering this mirrors indexes a tuple positionally and takes a byte
/// from a `Bytes` value (circuit-passes.ss, `Map-Argument`). A value that
/// arrives already flattened is sliced by the atom range its declared type
/// gives element `i`, which needs that type.
///
/// `what` names the indexed construct in the error messages, so a failure
/// reports the construct the source wrote.
fn indexed_element(
    val: &Value,
    ty: Option<&Type>,
    i: usize,
    what: &str,
) -> Result<Value, InterpreterError> {
    match val {
        Value::Tuple(elements) => elements.get(i).cloned().ok_or_else(|| {
            InterpreterError::TypeError(format!(
                "{what} index {i} out of bounds (len {})",
                elements.len()
            ))
        }),
        Value::AlignedValue(av) => {
            let ty = ty.ok_or_else(|| {
                InterpreterError::TypeError(format!(
                    "cannot {what}-index a flattened value of unknown type"
                ))
            })?;
            // A Bytes value yields one byte per index, out of the single atom
            // that holds them all.
            if let Type::Bytes(length) = ty.resolved() {
                let length = ir_length(*length)?;
                if i >= length {
                    return Err(InterpreterError::TypeError(format!(
                        "{what} index {i} out of bounds for a {length}-byte value"
                    )));
                }
                // The FAB normal form trims trailing zero bytes, so a byte
                // past the payload is one the encoding dropped.
                let byte = av
                    .value
                    .0
                    .first()
                    .and_then(|atom| atom.0.get(i))
                    .copied()
                    .unwrap_or(0);
                return Ok(Value::Integer(byte as u128));
            }
            let range = element_atom_range(ty, i).ok_or_else(|| {
                InterpreterError::TypeError(match element_count(ty) {
                    None => format!("cannot {what}-index a flattened value of type {ty:?}"),
                    Some(len) if i >= len => {
                        format!("{what} index {i} out of bounds (len {len})")
                    }
                    Some(_) => format!("cannot lay out {what} element {i} of {ty:?}"),
                })
            })?;
            slice_atoms(av, range.clone())
                .map(Value::AlignedValue)
                .ok_or_else(|| {
                    InterpreterError::TypeError(format!(
                        "{what} index {i} takes atoms [{}..{}] of a flattened value that does \
                         not carry one alignment atom per value atom (value_len={}, \
                         alignment_len={})",
                        range.start,
                        range.end,
                        av.value.0.len(),
                        av.alignment.0.len()
                    ))
                })
        }
        other => Err(InterpreterError::TypeError(format!(
            "cannot {what}-index {other:?}"
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
        out.push(indexed_element(val, Some(operand_ty), start + k, "slice")?);
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

/// Execute a public-ledger operation or an `emit`: translate its VM
/// instructions to onchain-vm `Op`s and run them through the VM
/// `QueryContext` (on the run's `CallContext` and the `Env` address, so a
/// kernel read of the context, such as `kernel.self()`'s
/// `dup{n:2} idx[0] popeq`, returns the `Env` value and lands in the
/// transcript the proving key expects).
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
        if let Some(midnight_onchain_runtime::ops::Op::Idx { path, .. }) = ops.get(1)
            && let Some(first) = path.iter().next()
        {
            eprintln!("  field nav first key: {first:?}");
        }
    }

    // Execute the ops against the contract state. The context holds the
    // address twice, and the VM reads `QueryContext::address` for slot 0, so
    // both copies take the `Env` address.
    let qc = QueryContext {
        call_context: ctx.call_context.clone(),
        ..QueryContext::new(ctx.state.data.clone(), ctx.call_context.own_address)
    };
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
    use ir::OpName as N;

    Ok(match &i.op {
        N::Idx => {
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
        N::Push => Op::Push {
            storage: flag(i, "storage"),
            value: push_value(ctx, operand(i, "value")?)?,
        },
        N::Addi => Op::Addi {
            immediate: resolve_immediate(ctx, &i.op, operand(i, "immediate")?)?,
        },
        N::Subi => Op::Subi {
            immediate: resolve_immediate(ctx, &i.op, operand(i, "immediate")?)?,
        },
        N::Ins => Op::Ins {
            cached: flag(i, "cached"),
            n: u8_arg(i, "n")?,
        },
        N::Dup => Op::Dup {
            n: u8_arg(i, "n").unwrap_or(0),
        },
        N::Swap => Op::Swap {
            n: u8_arg(i, "n").unwrap_or(0),
        },
        N::Popeq => Op::Popeq {
            cached: flag(i, "cached"),
            result: (),
        },
        N::Rem => Op::Rem {
            cached: flag(i, "cached"),
        },
        N::Noop => Op::Noop {
            n: u32_arg(i, "n")?,
        },
        N::Branch => Op::Branch {
            skip: u32_arg(i, "skip")?,
        },
        N::Jmp => Op::Jmp {
            skip: u32_arg(i, "skip")?,
        },
        N::Concat => Op::Concat {
            cached: flag(i, "cached"),
            n: u32_arg(i, "n")?,
        },
        N::Member => Op::Member,
        N::Root => Op::Root,
        N::Eq => Op::Eq,
        N::Lt => Op::Lt,
        N::Ckpt => Op::Ckpt,
        N::Neg => Op::Neg,
        N::Add => Op::Add,
        N::Pop => Op::Pop,
        N::Size => Op::Size,
        N::Type => Op::Type,
        N::Log => Op::Log,
        // The VM defines these and no ledger ADT template emits one, so the
        // interpreter has never needed them. Listing them keeps the match
        // exhaustive: a new instruction forces a decision here rather than
        // falling through.
        N::And | N::New | N::Or | N::Sub | N::Unknown(_) => {
            return Err(InterpreterError::Unsupported(format!(
                "VM instruction {}",
                i.op
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
    count_operand(&i.op, name, operand(i, name)?)
}

/// Resolve an instruction operand that counts stack slots or bytes.
///
/// The ledger DSL sizes a `concat` from the element type it holds, so the
/// operand is not always a literal: a `List` read emits
/// `(n (+ 2 (max-sizeof <element type>)))`.
fn count_operand(op: &ir::OpName, name: &str, o: &ir::Operand) -> Result<u64, InterpreterError> {
    match o {
        ir::Operand::Int(n) => u64::try_from(n)
            .map_err(|_| InterpreterError::TypeError(format!("{op}: {name} out of range"))),
        ir::Operand::MaxSizeof(ty) => max_sizeof(ty),
        ir::Operand::Add(left, right) => count_operand(op, name, left)?
            .checked_add(count_operand(op, name, right)?)
            .ok_or_else(|| InterpreterError::TypeError(format!("{op}: {name} out of range"))),
        other => Err(InterpreterError::TypeError(format!(
            "{op}: {name} is not a count: {other:?}"
        ))),
    }
}

/// `(max-sizeof type)`: the largest FAB encoding a value of this type takes.
///
/// The canonical runtime computes it as `maxAlignedSize(descriptor.alignment())`.
/// A FAB alignment belongs to the type rather than to any one value, so the
/// default value at that type carries the alignment to measure.
fn max_sizeof(ty: &Type) -> Result<u64, InterpreterError> {
    use midnight_transient_crypto::fab::AlignmentExt;
    let encoded = encode_typed(&default_value(ty)?, ty)?;
    u64::try_from(encoded.alignment.max_aligned_size()).map_err(|_| {
        InterpreterError::TypeError(format!("max-sizeof of {ty:?} does not fit a u64"))
    })
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
    // The ledger DSL's container constructors nest state values, so they build
    // a subtree here instead of reducing to one aligned value: a `List`
    // push-front pushes a three-slot array, and every `resetToDefault` pushes
    // the container's empty shape.
    if let ir::Operand::StateValue(state_value) = o {
        match state_value {
            ir::StateValue::Array(elements) => {
                let mut array = StorageArray::new();
                for element in elements {
                    array = array.push(push_value(ctx, element)?);
                }
                return Ok(StateValue::Array(array));
            }
            ir::StateValue::Map(entries) => {
                let mut map = StorageHashMap::new();
                for (key, value) in entries {
                    map = map.insert(operand_aligned(ctx, key)?, push_value(ctx, value)?);
                }
                return Ok(StateValue::Map(map));
            }
            ir::StateValue::MerkleTree { depth, entries } => {
                // Every template builds an empty tree and fills it with later
                // `ins` ops. A seeded one would need each leaf hashed the way
                // `rt-leaf-hash` does, which nothing here implements.
                if !entries.is_empty() {
                    return Err(InterpreterError::Unsupported(
                        "push of a Merkle tree carrying initial entries".to_string(),
                    ));
                }
                let height = u8::try_from(*depth).map_err(|_| {
                    InterpreterError::TypeError(format!("Merkle tree depth {depth} exceeds a u8"))
                })?;
                return Ok(StateValue::BoundedMerkleTree(MerkleTree::blank(height)));
            }
            // Scalar shapes: `reduce_operand` below unwraps them.
            ir::StateValue::Null | ir::StateValue::Cell(_) | ir::StateValue::Adt(..) => {}
        }
    }
    match reduce_operand(o) {
        // A pushed `(state-value-null)` (e.g. inserting into a Set, where the
        // "value" slot is just a marker). The on-chain `StateValue::Null`
        // field_repr is `[0]`, distinct from `StateValue::Cell(unit)` which is
        // `[1, 0]`.
        VmArg::Null | VmArg::Stack => Ok(StateValue::Null),
        _ => operand_aligned(ctx, o).map(StateValue::from),
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
        // Infer the expression's declared type *before* evaluating, so a
        // `Value::Integer` is re-encoded with the right alignment. Without
        // this, a `request_id as Field` cast still pushes a u64-aligned key,
        // which never matches a `Map<Field, V>` entry that was inserted with
        // Field alignment on-chain.
        VmArg::Expr(e) => {
            let inferred = infer_type_of_expr(ctx, e);
            let val = eval_expr(ctx, e)?;
            match (&val, inferred.as_ref()) {
                // A struct or tuple has no type-free encoding, so it needs the
                // declared one; an integer needs it for the right width.
                (Value::Integer(_) | Value::Struct(_) | Value::Tuple(_), Some(ty)) => {
                    encode_typed(&val, ty)
                }
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
        // `(null type)`: the type's default instance. A `List` read builds the
        // empty answer this way, joining the `is_some` flag to a default value
        // of the element type.
        VmArg::Vm(ir::Operand::Null(ty)) => encode_typed(&default_value(ty)?, ty),
        // `(leaf-hash x)`: a Merkle tree stores the leaf digest, not the value,
        // so every tree write pushes one of these.
        VmArg::Vm(ir::Operand::LeafHash(inner)) => {
            Ok(merkle_leaf_hash(operand_aligned(ctx, inner)?))
        }
        // The ledger keys a token balance by its colour joined to the
        // recipient, so every unshielded token operation pushes one of these.
        VmArg::Vm(ir::Operand::AlignedConcat(parts)) => {
            let encoded = parts
                .iter()
                .map(|part| operand_aligned(ctx, part))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AlignedValue::concat(encoded.iter()))
        }
        other => Err(InterpreterError::Unsupported(format!(
            "push of a {} operand",
            other.kind()
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
        // A `Cell` wraps exactly one aligned value, so unwrapping it is the
        // encoding. The other state variants are containers in the state tree
        // with no aligned form at all.
        Value::StateValue(sv) => compact_runtime::cell_aligned_value(sv).ok_or_else(|| {
            InterpreterError::Unsupported("aligned encoding of a non-Cell state value".to_string())
        }),
        // A struct or tuple needs its declared type to encode, which the
        // caller has only for an expression operand. `encode_typed` handles
        // those; reaching here means none was inferred.
        other => Err(InterpreterError::Unsupported(format!(
            "aligned encoding of {other:?} without a declared type"
        ))),
    }
}

/// Resolve an `addi` or `subi` immediate. It is either a literal number, or
/// an expression the ledger DSL computed.
fn resolve_immediate(
    ctx: &mut ExecContext,
    op: &ir::OpName,
    o: &ir::Operand,
) -> Result<u32, InterpreterError> {
    match reduce_operand(o) {
        VmArg::Int(n) => u32::try_from(n).map_err(|_| {
            InterpreterError::TypeError(format!("{op} immediate {n} out of u32 range"))
        }),
        VmArg::Expr(e) => {
            let val = eval_expr(ctx, e)?;
            val.as_u32().ok_or_else(|| {
                InterpreterError::TypeError(format!("{op} immediate is not u32: {val:?}"))
            })
        }
        other => Err(InterpreterError::TypeError(format!(
            "cannot resolve a {} {op} immediate",
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
    let n = u128::try_from(value).map_err(|e| {
        InterpreterError::TypeError(format!("invalid integer path literal {value}: {e}"))
    })?;
    encode_typed(&Value::Integer(n), &Type::Unsigned(max_for_bytes(bytes)))
}

fn max_for_bytes(bytes: u64) -> BigUint {
    (BigUint::from(1u8) << (8 * bytes)) - 1u8
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
            op: op.into(),
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
        execute(circuit, program, make_counter_state(0), args, epoch())
    }

    fn epoch() -> Env<'static> {
        Env::new(Timestamp::from_secs(0))
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
    fn a_sequence_runs_every_item_and_takes_the_last() {
        let state = make_counter_state(0);
        let circ = circuit(
            Vec::new(),
            uint("255"),
            ir::Expr::Seq(vec![increment_round(), increment_round(), int(3)]),
        );
        let program = Program::new(&[], &[], &[]);
        let result = execute(&circ, &program, state, &[], epoch()).expect("execute the sequence");
        assert_eq!(counter_cell(&result.state), 2, "both items ran");
        assert!(values_equal(
            &result.result.expect("a result value"),
            &Value::Integer(3)
        ));
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
        // The runtime symbols of the compiler's natives, which `make
        // compact-natives` writes. A call dispatches on this symbol.
        let natives = include_str!("compact-natives.txt");
        // Natives with no implementation yet: the foreign-curve natives of the
        // ZKIR v3 table, which need foreign field and point values. The
        // witness natives are not here: `WitnessNative` dispatches each of
        // them, so they count as handled. See docs/compact-natives.md.
        const KNOWN_UNIMPLEMENTED: &[&str] = &[
            "curve25519Add",
            "curve25519BaseInv",
            "curve25519BaseNeg",
            "curve25519Mul",
            "curve25519MulGenerator",
            "curve25519PointX",
            "curve25519PointY",
            "curve25519ScalarInv",
            "curve25519ScalarNeg",
            "secp256k1Add",
            "secp256k1BaseInv",
            "secp256k1BaseNeg",
            "secp256k1Mul",
            "secp256k1MulGenerator",
            "secp256k1PointX",
            "secp256k1PointY",
            "secp256k1ScalarInv",
            "secp256k1ScalarNeg",
            "secp256r1Add",
            "secp256r1BaseInv",
            "secp256r1BaseNeg",
            "secp256r1Mul",
            "secp256r1MulGenerator",
            "secp256r1PointX",
            "secp256r1PointY",
            "secp256r1ScalarInv",
            "secp256r1ScalarNeg",
        ];

        for name in natives.lines() {
            let handled =
                WitnessNative::from_name(name).is_some() || try_builtin(name, &[]).is_some();
            let known_unimplemented = KNOWN_UNIMPLEMENTED.contains(&name);
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
    }

    #[test]
    fn natives_that_share_a_source_name_dispatch_on_their_own_symbol() {
        let ec_add = |id: &str, entry: &str, curve: ir::Curve| ir::Native {
            type_arguments: Vec::new(),
            name: ident(id),
            entry: entry.to_string(),
            class: "circuit".to_string(),
            arguments: vec![
                argument("%a.3", Type::Point(curve.clone())),
                argument("%b.4", Type::Point(curve.clone())),
            ],
            result_type: Type::Point(curve),
        };
        let natives = vec![
            ec_add("%ecAdd.1", "__compactRuntime.ecAdd", ir::Curve::Jubjub),
            ec_add(
                "%ecAdd.2",
                "__compactRuntime.secp256k1Add",
                ir::Curve::Secp256k1,
            ),
        ];
        let program = Program::new(&[], &[], &natives);

        assert_eq!(program.runtime_name(&ident("%ecAdd.1")), "ecAdd");
        assert_eq!(program.runtime_name(&ident("%ecAdd.2")), "secp256k1Add");
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

        let got = match try_builtin_typed("transientHash", &[Value::Struct(fields)], &[ty])
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

        let got = match try_builtin_typed("transientHash", &[arg], &[ty])
            .expect("builtin known")
            .expect("a struct inside a vector must hash")
        {
            Value::AlignedValue(av) => Fr::try_from(&*av.value).unwrap(),
            other => panic!("expected AlignedValue, got {other:?}"),
        };

        assert_eq!(got, transient_hash(&[Fr::from(8u64), Fr::from(9u64)]));
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
        // (u64 limb arithmetic), then the ledger's streaming
        // `PersistentHashWriter`.
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
    fn a_typed_integer_ledger_key_takes_the_declared_width() {
        // Uint<0..65535> is a 2-byte atom, not the type-less 8-byte default.
        let sv = encode_ledger_key(&Value::Integer(7), Some(&uint("65535"))).expect("encode key");
        match sv {
            StateValue::Cell(ref sp) => assert_eq!((**sp).clone(), AlignedValue::from(7u16)),
            other => panic!("expected Cell, got {other:?}"),
        }
    }

    #[test]
    fn a_field_integer_above_u64_encodes_exactly() {
        use midnight_transient_crypto::curve::Fr;
        let av = encode_typed(&Value::Integer((1u128 << 64) + 5), &field()).expect("encode");
        assert_eq!(av, AlignedValue::from(fr_two_pow_64() + Fr::from(5u64)));
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
        let coin = AlignedValue::concat(
            [
                AlignedValue::from(nonce),
                AlignedValue::from(color),
                AlignedValue::from(value),
                AlignedValue::from(mt_index),
            ]
            .iter(),
        );

        let result = execute_in(
            &circ,
            &program,
            &[("coin", Value::AlignedValue(coin.clone()))],
        )
        .expect("execute createZswapInput");
        match &result.zswap_inputs[..] {
            [captured] => match &captured.coin {
                Value::AlignedValue(got) => assert_eq!(got, &coin),
                other => panic!("captured {other:?}"),
            },
            other => panic!("createZswapInput must capture exactly one coin, got {other:?}"),
        }
    }

    #[test]
    fn own_public_key_without_a_coin_key_fails() {
        let key_type = Type::Struct {
            name: "ZswapCoinPublicKey".into(),
            fields: vec![("bytes".to_string(), Type::Bytes(32))],
        };
        let natives = vec![ir::Native {
            type_arguments: Vec::new(),
            name: ident("%ownPublicKey.10"),
            entry: "__compactRuntime.ownPublicKey".to_string(),
            class: "witness".to_string(),
            arguments: Vec::new(),
            result_type: key_type.clone(),
        }];
        let program = Program::new(&[], &[], &natives);
        let circ = circuit(
            Vec::new(),
            key_type,
            ir::Expr::Call {
                name: ident("%ownPublicKey.10"),
                args: Vec::new(),
            },
        );

        match execute_in(&circ, &program, &[]) {
            Err(InterpreterError::Witness(msg)) => assert!(
                msg.contains("coin public key"),
                "the error must name the missing key, got: {msg}"
            ),
            Err(other) => panic!("expected a witness error, got {other:?}"),
            Ok(result) => panic!(
                "ownPublicKey ran with no coin key and returned {:?}",
                result.result
            ),
        }
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
    // runtime (`runtime/src/casts.ts` in RomarQ/compact): little-endian
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

        // The shape a real contract emits: the parts are `var-ref`s, not
        // literals, so this drives the expression evaluation and the typed
        // encoding behind it rather than the constant path alone.
        ctx.locals.insert("colour".to_string(), Value::Integer(7));
        ctx.locals
            .insert("recipient".to_string(), Value::Integer(9));
        let from_vars = ir::Operand::AlignedConcat(vec![
            ir::Operand::Expr(Box::new(var("colour"))),
            ir::Operand::Expr(Box::new(var("recipient"))),
        ]);
        assert_eq!(
            push_value(&mut ctx, &from_vars).expect("push a concat of var-refs"),
            StateValue::from(AlignedValue::concat(
                [integer_fallback_aligned(7), integer_fallback_aligned(9),].iter()
            ))
        );

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
            call_context: CallContext::default(),
            coin_public_key: None,
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

    #[test]
    fn byte_ref_rejects_malformed_operands() {
        let reference = circuit(
            vec![argument("x", Type::Bytes(1))],
            uint("255"),
            ir::Expr::BytesRef {
                ty: Type::Bytes(1),
                expr: Box::new(var("x")),
                index: Box::new(int(0)),
            },
        );
        for input in [
            Value::Tuple(vec![Value::Integer(7)]),
            Value::AlignedValue(AlignedValue::from(())),
            Value::AlignedValue(AlignedValue::concat(
                [AlignedValue::from([1u8]), AlignedValue::from([2u8])].iter(),
            )),
            Value::AlignedValue(AlignedValue::from([1u8, 2])),
        ] {
            let err = run(&reference, &[("x", input)]).expect_err("malformed byte value");
            assert!(matches!(err, InterpreterError::TypeError(_)), "got {err}");
        }
    }

    #[test]
    fn byte_ref_reads_the_zero_trimmed_tail() {
        for (input, index, expected) in [
            ("ab000000", 0, 0xab),
            ("ab000000", 3, 0),
            ("00000000", 3, 0),
        ] {
            let got = eval(
                ir::Expr::BytesRef {
                    ty: Type::Bytes(4),
                    expr: Box::new(bytes(input)),
                    index: Box::new(int(index)),
                },
                uint("255"),
            )
            .expect("byte within the declared width");
            assert!(values_equal(&got, &Value::Integer(expected)), "got {got:?}");
        }
    }

    #[test]
    fn byte_slice_preserves_the_zero_trimmed_tail() {
        for (input, start, expected) in [
            ("abcdef0000", 1, [0xcd, 0xef, 0]),
            ("ab00000000", 2, [0, 0, 0]),
            ("0000000000", 0, [0, 0, 0]),
        ] {
            let got = eval(
                ir::Expr::BytesSlice {
                    ty: Type::Bytes(5),
                    expr: Box::new(bytes(input)),
                    index: Box::new(int(start)),
                    len: 3,
                },
                Type::Bytes(3),
            )
            .expect("slice within the declared width");
            let Value::AlignedValue(got) = got else {
                panic!("expected a bytes value, got {got:?}");
            };
            assert_eq!(got, AlignedValue::from(expected));
        }
    }

    #[test]
    fn byte_access_rejects_indices_past_the_declared_width() {
        let oversized = usize::MAX as u128 + 1;
        for expr in [
            ir::Expr::BytesRef {
                ty: Type::Bytes(4),
                expr: Box::new(bytes("ab000000")),
                index: Box::new(int(4)),
            },
            ir::Expr::BytesSlice {
                ty: Type::Bytes(4),
                expr: Box::new(bytes("ab000000")),
                index: Box::new(int(3)),
                len: 2,
            },
            ir::Expr::BytesRef {
                ty: Type::Bytes(4),
                expr: Box::new(bytes("ab000000")),
                index: Box::new(int(oversized)),
            },
            ir::Expr::BytesSlice {
                ty: Type::Bytes(4),
                expr: Box::new(bytes("ab000000")),
                index: Box::new(int(oversized)),
                len: 1,
            },
        ] {
            let err = eval(expr, Type::unit()).expect_err("out-of-bounds access");
            assert!(
                matches!(err, InterpreterError::TypeError(ref msg) if msg.contains("out of bounds")),
                "got {err}"
            );
        }
    }

    #[test]
    fn infer_type_of_map_and_vector_ref_uses_node_annotations() {
        let circuits = vec![ir::Circuit {
            name: ident("%wrap.21"),
            exported: false,
            pure: true,
            proof: false,
            arguments: vec![argument("%x.22", uint("255"))],
            result_type: Type::Bytes(32),
            body: bytes(&"00".repeat(32)),
        }];
        let program = Program::new(&circuits, &[], &[]);
        let mut ps = Vec::new();
        let ctx = test_ctx(&program, &mut ps, HashMap::new());

        let arg = |expr| ir::MapArg {
            expr,
            ty: vector(4, uint("255")),
            element_ty: uint("255"),
        };
        let by_ref = ir::Expr::Map {
            len: 4,
            fun: ir::Fun::Ref(ident("%wrap.21")),
            args: vec![arg(var("xs"))],
        };
        match infer_type_of_expr(&ctx, &by_ref) {
            Some(Type::Vector { len: 4, ty }) => assert!(matches!(*ty, Type::Bytes(32))),
            other => panic!("expected Vector<4, Bytes<32>>, got {other:?}"),
        }

        let inline = ir::Expr::Map {
            len: 4,
            fun: ir::Fun::Circuit {
                arguments: vec![argument("%m.23", uint("255"))],
                result_type: field(),
                body: Box::new(int(0)),
            },
            args: vec![arg(var("xs"))],
        };
        match infer_type_of_expr(&ctx, &inline) {
            Some(Type::Vector { len: 4, ty }) => assert!(matches!(*ty, Type::Field(_))),
            other => panic!("expected Vector<4, Field>, got {other:?}"),
        }

        // `xs` is not in scope, so the operand is opaque to inference; the
        // element type comes from the node's own operand annotation.
        let vref = ir::Expr::VectorRef {
            ty: vector(4, Type::Bytes(32)),
            expr: Box::new(var("xs")),
            index: Box::new(int(0)),
        };
        assert_eq!(infer_type_of_expr(&ctx, &vref), Some(Type::Bytes(32)));
    }

    /// The membership-change shape `maybes[i].is_some`: a `let*`-bound `map`
    /// result must carry its element type so field access can slice the
    /// structs the callee built.
    #[test]
    fn let_bound_map_result_supports_element_field_access() {
        let maybe = Type::Struct {
            name: "Maybe".to_string(),
            fields: vec![
                ("is_some".to_string(), Type::Boolean),
                ("value".to_string(), uint("65535")),
            ],
        };
        // (m) => Maybe { is_some: m > 5, value: m }
        let fun = ir::Fun::Circuit {
            arguments: vec![argument("%m.1", uint("65535"))],
            result_type: maybe.clone(),
            body: Box::new(ir::Expr::New {
                ty: maybe.clone(),
                elements: vec![
                    ir::Expr::Gt {
                        bits: 16,
                        left: Box::new(var("%m.1")),
                        right: Box::new(int(5)),
                    },
                    var("%m.1"),
                ],
            }),
        };
        let element_field = |i: u128, elt: &str, index: u64| ir::Expr::EltRef {
            expr: Box::new(ir::Expr::VectorRef {
                ty: vector(2, maybe.clone()),
                expr: Box::new(var("%maybes.2")),
                index: Box::new(int(i)),
            }),
            elt: elt.to_string(),
            index,
        };
        let body = ir::Expr::LetStar {
            bindings: vec![(
                argument("%maybes.2", vector(2, maybe.clone())),
                ir::Expr::Map {
                    len: 2,
                    fun,
                    args: vec![ir::MapArg {
                        expr: var("xs"),
                        ty: vector(2, uint("65535")),
                        element_ty: uint("65535"),
                    }],
                },
            )],
            body: Box::new(ir::Expr::Tuple(vec![
                single(element_field(0, "is_some", 0)),
                single(element_field(1, "value", 1)),
            ])),
        };
        // 3 is below the `> 5` threshold and 9 is above it, so each component
        // disagrees with the other field of its own struct: reading `value`
        // where `is_some` is meant fails, and so does the reverse.
        let flat =
            AlignedValue::concat([AlignedValue::from(3u16), AlignedValue::from(9u16)].iter());
        let got = run(
            &circuit(
                vec![argument("xs", vector(2, uint("65535")))],
                Type::Tuple(vec![Type::Boolean, uint("65535")]),
                body,
            ),
            &[("xs", Value::AlignedValue(flat))],
        )
        .expect("field access on map elements");
        let expected = Value::Tuple(vec![Value::Bool(false), Value::Integer(9)]);
        assert!(
            values_equal(&got, &expected),
            "expected (false, 9), got {got:?}"
        );
    }

    /// A flattened value that cannot be sliced by its declared layout is
    /// refused, not read anyway. The compiler bounds an index before it emits
    /// one, so both shapes are hand-built: a cell carrying more atoms than the
    /// vector declares, and an alignment that does not give one atom per value
    /// atom, which the wire format can express through `AlignmentSegment::Option`.
    #[test]
    fn indexing_a_flattened_value_refuses_what_it_cannot_slice() {
        use midnight_base_crypto::fab;
        let refusal = |av, ty: &Type, i| match indexed_element(
            &Value::AlignedValue(av),
            Some(ty),
            i,
            "vector",
        ) {
            Err(InterpreterError::TypeError(msg)) => msg,
            other => panic!("expected a TypeError, got {other:?}"),
        };

        let pair = Type::Struct {
            name: "Pair".to_string(),
            fields: vec![
                ("a".to_string(), uint("65535")),
                ("b".to_string(), uint("65535")),
            ],
        };
        let atoms: Vec<AlignedValue> = (1u16..=6).map(AlignedValue::from).collect();
        let msg = refusal(AlignedValue::concat(atoms.iter()), &vector(2, pair), 2);
        assert!(msg.contains("out of bounds (len 2)"), "got: {msg}");

        let variant = fab::Alignment(vec![fab::AlignmentSegment::Atom(
            fab::AlignmentAtom::Bytes { length: 2 },
        )]);
        let maybe = fab::AlignmentSegment::Option(vec![variant.clone(), variant]);
        let unaligned = fab::AlignedValue {
            value: fab::Value(vec![
                fab::ValueAtom(vec![1]),
                fab::ValueAtom(vec![7]),
                fab::ValueAtom(vec![1]),
                fab::ValueAtom(vec![9]),
            ]),
            alignment: fab::Alignment(vec![maybe.clone(), maybe]),
        };
        let element = Type::Tuple(vec![uint("65535"), uint("65535")]);
        let msg = refusal(unaligned, &vector(2, element), 1);
        assert!(msg.contains("alignment_len=2"), "got: {msg}");
    }

    /// The operand annotation the node carries wins over inference of the
    /// operand, which types an arithmetic node as its left operand: a
    /// narrower width than the one the value in flight was encoded at.
    #[test]
    fn vector_ref_inference_prefers_the_node_annotation() {
        let program = Program::new(&[], &[], &[]);
        let mut ps = Vec::new();
        let ctx = test_ctx(
            &program,
            &mut ps,
            HashMap::from([
                ("a".to_string(), uint("255")),
                ("b".to_string(), uint("65535")),
            ]),
        );
        let node = ir::Expr::VectorRef {
            ty: vector(2, uint("65535")),
            expr: Box::new(ir::Expr::VectorLit(vec![
                single(ir::Expr::Add {
                    ty: uint("65535"),
                    left: Box::new(var("a")),
                    right: Box::new(var("b")),
                }),
                single(var("b")),
            ])),
            index: Box::new(int(0)),
        };
        assert_eq!(infer_type_of_expr(&ctx, &node), Some(uint("65535")));
    }

    /// The operand is evaluated before the index. An index expression can call
    /// a witness or read the ledger, and the transcript replays those effects
    /// in the order the circuit produced them.
    #[test]
    fn a_vector_index_runs_after_its_operand() {
        // Returns 0, 1, 2, ... in call order, so a result names its caller.
        struct Ticker(std::sync::atomic::AtomicU64);
        impl WitnessProvider for Ticker {
            fn call_witness(
                &self,
                _ctx: &mut WitnessContext<'_>,
                _name: &str,
                _args: &[Value],
            ) -> Result<WitnessOutcome, InterpreterError> {
                let n = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(WitnessOutcome::Value(Value::Integer(n as u128)))
            }
        }

        let witnesses = vec![ir::Witness {
            name: ident("%tick.9"),
            arguments: Vec::new(),
            result_type: uint("255"),
        }];
        let program = Program::new(&[], &witnesses, &[]);
        let tick = || ir::Expr::Call {
            name: ident("%tick.9"),
            args: Vec::new(),
        };
        // Evaluating the operand first makes it [0, 9] and the index 1, so the
        // element is 9. Evaluating the index first makes it 0 and the operand
        // [1, 9], so the element would be 1.
        let circ = circuit(
            Vec::new(),
            uint("255"),
            ir::Expr::VectorRef {
                ty: vector(2, uint("255")),
                expr: Box::new(ir::Expr::VectorLit(vec![single(tick()), single(int(9))])),
                index: Box::new(tick()),
            },
        );
        let result = execute(
            &circ,
            &program,
            make_counter_state(0),
            &[],
            Env {
                witnesses: &Ticker(std::sync::atomic::AtomicU64::new(0)),
                ..epoch()
            },
        )
        .expect("the witness runs");
        assert!(values_equal(
            &result.result.expect("a result value"),
            &Value::Integer(9)
        ));
    }

    /// An alias is transparent to every value-level operation, so a field read
    /// from an element whose declared type is an alias of a struct slices by
    /// the struct the alias names.
    #[test]
    fn field_access_sees_through_an_alias() {
        let member = Type::Struct {
            name: "Member".to_string(),
            fields: vec![
                ("id".to_string(), uint("65535")),
                ("key".to_string(), Type::Bytes(4)),
            ],
        };
        let flat = AlignedValue::concat(
            [
                AlignedValue::from(1u16),
                bytes_aligned_value(b"AAAA".to_vec(), 4).unwrap(),
                AlignedValue::from(2u16),
                bytes_aligned_value(b"BBBB".to_vec(), 4).unwrap(),
            ]
            .iter(),
        );
        let named = Type::Alias {
            nominal: false,
            name: "Committee".to_string(),
            ty: Box::new(member),
        };
        let body = ir::Expr::EltRef {
            expr: Box::new(ir::Expr::VectorRef {
                ty: vector(2, named.clone()),
                expr: Box::new(var("xs")),
                index: Box::new(int(1)),
            }),
            elt: "key".to_string(),
            index: 1,
        };
        let got = run(
            &circuit(
                vec![argument("xs", vector(2, named.clone()))],
                Type::Bytes(4),
                body,
            ),
            &[("xs", Value::AlignedValue(flat))],
        )
        .expect("read a field through the alias");
        let Value::AlignedValue(got) = got else {
            panic!("expected a flattened value, got {got:?}");
        };
        assert_eq!(got, bytes_aligned_value(b"BBBB".to_vec(), 4).unwrap());

        // The same for the field's own type, which a nested read consults.
        let program = Program::new(&[], &[], &[]);
        let mut ps = Vec::new();
        let ctx = test_ctx(&program, &mut ps, HashMap::from([("m".to_string(), named)]));
        let field_ty = ir::Expr::EltRef {
            expr: Box::new(var("m")),
            elt: "key".to_string(),
            index: 1,
        };
        assert_eq!(infer_type_of_expr(&ctx, &field_ty), Some(Type::Bytes(4)));
    }
}
