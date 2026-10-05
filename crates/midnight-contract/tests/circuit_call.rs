//! Circuit call integration tests.
//!
//! These tests run circuits through the interpreter that `midnight_contract`
//! re-exports, and through the low-level `call::build_unproven_call_tx`
//! builder. They cover witness dispatch, private-state threading, captured
//! Zswap outputs, `kernel.self()`, and struct arguments.

use compact_bindgen::{
    AlignedValue, ContractMaintenanceAuthority, ContractState, InMemoryDB, StateValue,
    StorageHashMap,
};
use midnight_base_crypto::time::Timestamp;
use midnight_coin_structure::contract::ContractAddress;
use midnight_contract::call;
use midnight_contract::interpreter;

use compact_codegen::ir::{
    self, Argument, Expr, FieldType, Ident, Instruction, Literal, OpClass, Operand, Type,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn counter_state(round: u64) -> ContractState<InMemoryDB> {
    let root = StateValue::Array(vec![StateValue::from(round)].into());
    ContractState::new(
        root,
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    )
}

fn id(name: &str) -> Ident {
    Ident(name.to_string())
}

fn var(name: &str) -> Expr {
    Expr::VarRef(id(name))
}

fn uint(maxval: &str) -> Type {
    Type::Unsigned(maxval.parse().expect("a decimal bound"))
}

/// A ledger field's `idx` path: one literal one-byte key, as the compiler
/// prints it (`(path ((align 0 1)))`).
fn field_path(index: u8) -> Operand {
    Operand::List(vec![Operand::Align {
        value: index.into(),
        bytes: 1,
    }])
}

fn instruction(op: &str, args: &[(&str, Operand)]) -> Instruction {
    Instruction {
        op: op.into(),
        args: args
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect(),
    }
}

/// A proof circuit with the given signature and body, named as the compiler
/// names an exported circuit.
fn circuit(arguments: Vec<Argument>, result_type: Type, body: Expr) -> ir::Circuit {
    ir::Circuit {
        name: id("%test.0"),
        exported: true,
        pure: false,
        proof: true,
        arguments,
        result_type,
        body,
    }
}

/// A program that resolves nothing: a body that calls no circuit, witness or
/// native.
fn no_program() -> interpreter::Program<'static> {
    interpreter::Program::new(&[], &[], &[])
}

/// No witnesses, a scratch private state, the zero address, the epoch.
fn epoch() -> interpreter::Env<'static> {
    interpreter::Env::new(Timestamp::from_secs(0))
}

// ---------------------------------------------------------------------------
// Witness calls
// ---------------------------------------------------------------------------

/// A witness declaration whose name collides with the interpreter builtin of
/// the same name, which is exactly the collision the Unknown/Err distinction
/// protects.
fn persistent_hash_witness() -> ir::Witness {
    ir::Witness {
        name: id("%persistentHash.1"),
        arguments: vec![Argument {
            name: id("%value.2"),
            ty: uint("65535"),
        }],
        result_type: Type::Bytes(32),
    }
}

/// IR whose result is a `persistentHash` witness call over a literal.
fn persistent_hash_witness_ir() -> ir::Circuit {
    circuit(
        Vec::new(),
        Type::Bytes(32),
        Expr::Call {
            name: id("%persistentHash.1"),
            args: vec![Expr::Quote(Literal::Int(7.into()))],
        },
    )
}

/// A real provider failure (HSM down, decode error, ...) must propagate even
/// when the witness name collides with a builtin: the builtin must NOT run.
/// Under the old fall-through semantics every `InterpreterError::Witness` was
/// treated as "unknown name", so this IR would silently reroute to the
/// `persistentHash` builtin and return `Ok` — this test pins the fix.
#[test]
fn witness_failure_on_builtin_name_propagates() {
    use midnight_contract::interpreter;
    use midnight_contract::runtime::{InterpreterError, Value, WitnessOutcome, WitnessProvider};

    struct FailingHsm;
    impl WitnessProvider for FailingHsm {
        fn call_witness(
            &self,
            _ctx: &mut midnight_contract::runtime::WitnessContext<'_>,
            name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, InterpreterError> {
            Err(InterpreterError::Witness(format!(
                "hsm unreachable: {name}"
            )))
        }
    }

    let witnesses = vec![persistent_hash_witness()];
    let program = interpreter::Program::new(&[], &witnesses, &[]);
    let state = counter_state(0);
    match interpreter::execute(
        &persistent_hash_witness_ir(),
        &program,
        state,
        &[],
        interpreter::Env {
            witnesses: &FailingHsm,
            ..epoch()
        },
    ) {
        Ok(_) => panic!("a witness-level failure must propagate, not fall through to the builtin"),
        Err(InterpreterError::Witness(msg)) => {
            assert_eq!(msg, "hsm unreachable: persistentHash");
        }
        Err(other) => panic!("expected the provider's witness error, got {other:?}"),
    }
}

/// `WitnessOutcome::Unknown` for a builtin name still falls through and the
/// builtin runs — pins the pre-existing fall-through path for providers that
/// genuinely don't implement the name.
#[test]
fn unknown_witness_falls_through_to_builtin() {
    use compact_bindgen::AlignedValue;
    use midnight_contract::interpreter;
    use midnight_contract::runtime::{InterpreterError, Value, WitnessOutcome, WitnessProvider};

    struct KnowsNothing;
    impl WitnessProvider for KnowsNothing {
        fn call_witness(
            &self,
            _ctx: &mut midnight_contract::runtime::WitnessContext<'_>,
            _name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, InterpreterError> {
            Ok(WitnessOutcome::Unknown)
        }
    }

    let witnesses = vec![persistent_hash_witness()];
    let program = interpreter::Program::new(&[], &witnesses, &[]);
    let state = counter_state(0);
    let result = interpreter::execute(
        &persistent_hash_witness_ir(),
        &program,
        state,
        &[],
        interpreter::Env {
            witnesses: &KnowsNothing,
            ..epoch()
        },
    )
    .expect("Unknown must fall through to the persistentHash builtin");

    // The builtin hashes Integer args as Fr; recompute the expected digest
    // independently so a different code path (or no builtin at all) fails.
    use midnight_base_crypto::hash::PersistentHashWriter;
    use midnight_base_crypto::repr::BinaryHashRepr;
    use midnight_transient_crypto::curve::Fr;
    use midnight_transient_crypto::fab::ValueReprAlignedValue;
    let mut hasher = PersistentHashWriter::default();
    ValueReprAlignedValue(AlignedValue::from(Fr::from(7u64))).binary_repr(&mut hasher);
    let expected = AlignedValue::from(hasher.finalize().0);

    match result.result {
        Some(Value::AlignedValue(av)) => assert_eq!(av, expected, "builtin hash mismatch"),
        other => panic!("expected the builtin's AlignedValue hash, got {other:?}"),
    }
}

/// A witness's view of private state threads across calls via `WitnessContext`:
/// reading the current state, returning a value derived from it, and writing an
/// updated state that the next call observes.
#[test]
fn witness_context_threads_private_state() {
    use midnight_contract::interpreter;
    use midnight_contract::runtime::{
        InterpreterError, Value, WitnessContext, WitnessOutcome, WitnessProvider,
    };

    fn decode(bytes: &[u8]) -> u64 {
        bytes.try_into().map(u64::from_le_bytes).unwrap_or(0)
    }

    // Reads a u64 counter from the private state, returns it, then stores
    // counter + 1 so the next call sees the incremented value.
    struct CounterWitness;
    impl WitnessProvider for CounterWitness {
        fn call_witness(
            &self,
            ctx: &mut WitnessContext<'_>,
            name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, InterpreterError> {
            match name {
                "private$counter" => {
                    let current = decode(ctx.private_state());
                    ctx.set_private_state((current + 1).to_le_bytes().to_vec());
                    Ok(WitnessOutcome::Value(Value::Integer(current as u128)))
                }
                _ => Ok(WitnessOutcome::Unknown),
            }
        }
    }

    // IR whose return value is just the witness call.
    let witnesses = vec![ir::Witness {
        name: id("%private$counter.1"),
        arguments: Vec::new(),
        result_type: Type::Field(FieldType::Native),
    }];
    let program = interpreter::Program::new(&[], &witnesses, &[]);
    let ir = circuit(
        Vec::new(),
        Type::Field(FieldType::Native),
        Expr::Call {
            name: id("%private$counter.1"),
            args: Vec::new(),
        },
    );

    let state = counter_state(0);
    let mut private_state = Vec::new();

    // First call: witness sees an empty (= 0) state and returns 0.
    let r1 = interpreter::execute(
        &ir,
        &program,
        state.clone(),
        &[],
        interpreter::Env {
            witnesses: &CounterWitness,
            private_state: Some(&mut private_state),
            ..epoch()
        },
    )
    .unwrap();
    assert!(matches!(r1.result, Some(Value::Integer(0))));
    // The witness's private value must be recorded as a private transcript
    // output, or proving a witness-using circuit fails with "ran out of private
    // transcript outputs". One witness call -> one output.
    assert_eq!(r1.private_transcript_outputs.len(), 1);

    // Second call reuses the same buffer: the witness now sees 1.
    let r2 = interpreter::execute(
        &ir,
        &program,
        state,
        &[],
        interpreter::Env {
            witnesses: &CounterWitness,
            private_state: Some(&mut private_state),
            ..epoch()
        },
    )
    .unwrap();
    assert!(matches!(r2.result, Some(Value::Integer(1))));
    assert_eq!(r2.private_transcript_outputs.len(), 1);

    // Two increments → 2.
    assert_eq!(decode(&private_state), 2);
}

// ---------------------------------------------------------------------------
// Shielded mint: createZswapOutput capture
// ---------------------------------------------------------------------------

/// `createZswapOutput(coin, recipient)` is a witness-class native with no
/// effect of its own; it marks "attach a Zswap output for this coin here".
/// The interpreter must capture its `(coin, recipient)` args on the
/// `ExecutionResult` (so the call path can build the offer `Output`) and
/// return unit, rather than erroring as an unknown witness.
#[test]
fn interpreter_captures_create_zswap_output() {
    use midnight_contract::runtime::Value;

    let natives = vec![ir::Native {
        type_arguments: Vec::new(),
        name: id("%createZswapOutput.3"),
        entry: "__compactRuntime.createZswapOutput".to_string(),
        class: "witness".to_string(),
        arguments: vec![
            Argument {
                name: id("%coin.4"),
                ty: Type::Bytes(32),
            },
            Argument {
                name: id("%recipient.5"),
                ty: Type::Bytes(32),
            },
        ],
        result_type: Type::unit(),
    }];
    let program = interpreter::Program::new(&[], &[], &natives);
    let ir = circuit(
        vec![
            Argument {
                name: id("%coin.1"),
                ty: Type::Bytes(32),
            },
            Argument {
                name: id("%recipient.2"),
                ty: Type::Bytes(32),
            },
        ],
        Type::unit(),
        Expr::Call {
            name: id("%createZswapOutput.3"),
            args: vec![var("%coin.1"), var("%recipient.2")],
        },
    );

    let state = counter_state(0);
    let coin = Value::AlignedValue(AlignedValue::from([7u8; 32]));
    let recipient = Value::AlignedValue(AlignedValue::from([9u8; 32]));

    let result = interpreter::execute(
        &ir,
        &program,
        state,
        &[("coin", coin), ("recipient", recipient)],
        epoch(),
    )
    .expect("createZswapOutput must be handled, not error");

    assert_eq!(
        result.zswap_outputs.len(),
        1,
        "one circuit-created Zswap output should be captured"
    );
    let out = &result.zswap_outputs[0];
    assert_eq!(
        out.coin.try_to_aligned_value().unwrap(),
        AlignedValue::from([7u8; 32])
    );
    assert_eq!(
        out.recipient.try_to_aligned_value().unwrap(),
        AlignedValue::from([9u8; 32])
    );
}

// ---------------------------------------------------------------------------
// Shielded mint: full `mintShieldedToken` circuit
// ---------------------------------------------------------------------------

/// A probe whose recipient is a runtime `Either`, so the interpreter has to
/// destructure it; the devnet mint folds that branch away.
fn mint_probe_info() -> compact_codegen::types::ContractInfo {
    let text =
        include_str!("../../../tests/fixtures/compiled/mint-probe/compiler/analyzed-ir.sexp");
    compact_codegen::artifact::load_str(text).unwrap()
}

fn mint_circuit(info: &compact_codegen::types::ContractInfo) -> &compact_codegen::types::Circuit {
    info.circuits
        .iter()
        .find(|c| c.name == "mint")
        .expect("mint circuit")
}

/// The `recipient` argument's declared type must carry the nested `Either` /
/// `ZswapCoinPublicKey` / `ContractAddress` layout the interpreter needs to
/// slice it. The type is the only source of that layout, so a compiler that
/// stopped emitting fields inline would break the funded call path here
/// rather than somewhere deep in execution.
#[test]
fn the_recipient_argument_type_carries_its_nested_layout() {
    let info = mint_probe_info();
    let mint = mint_circuit(&info);

    let arg_types = compact_codegen::arg_types::circuit_arg_types(mint.arguments());
    let (_, recipient) = arg_types
        .iter()
        .find(|(n, _)| n == "recipient")
        .expect("mint takes a recipient argument");

    let Type::Struct { name, fields } = recipient else {
        panic!("recipient must be a struct type, got {recipient:?}")
    };
    assert_eq!(name, "Either");
    let field_names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(field_names, ["is_left", "left", "right"]);

    for (branch, expected) in [(1, "ZswapCoinPublicKey"), (2, "ContractAddress")] {
        let Type::Struct { name, fields } = &fields[branch].1 else {
            panic!("branch {branch} must be a struct type")
        };
        assert_eq!(name, expected);
        assert!(
            fields.iter().any(|(n, _)| n == "bytes"),
            "{expected} must carry its `bytes` field"
        );
    }
}

/// Encode an `Either::left(cpk)` recipient as the interpreter sees a
/// struct-typed argument: a flat `AlignedValue` of `[is_left, left.bytes,
/// right.bytes]`.
fn either_left(cpk: [u8; 32]) -> AlignedValue {
    AlignedValue::concat(
        [
            AlignedValue::from(true),
            AlignedValue::from(cpk),
            AlignedValue::from([0u8; 32]),
        ]
        .iter(),
    )
}

/// The mint circuit's arguments, as the funded call path passes them.
fn mint_args(domain_sep: [u8; 32]) -> [(&'static str, midnight_contract::runtime::Value); 4] {
    use midnight_contract::runtime::Value;
    [
        (
            "domain_sep",
            Value::AlignedValue(AlignedValue::from(domain_sep)),
        ),
        ("value", Value::Integer(1000)),
        ("nonce", Value::AlignedValue(AlignedValue::from([2u8; 32]))),
        ("recipient", Value::AlignedValue(either_left([3u8; 32]))),
    ]
}

/// Run the mint circuit against an EMPTY contract state, passing the real
/// contract address. `kernel.self()` (`dup{n:2} idx[0] popeq`) reads the
/// address from the VM **context**, not user state, so the deployed `data` is
/// an empty array. The interpreter must resolve `kernel.self()` to the supplied
/// address; the minted coin's color is `tokenType(domain_sep, address)`, so the
/// captured output's color depends on the address — proving the resolution uses
/// the real address rather than a zero/dummy one.
fn run_mint(
    domain_sep: [u8; 32],
    address: midnight_coin_structure::contract::ContractAddress,
) -> midnight_contract::runtime::CircuitZswapOutput {
    use midnight_contract::interpreter;

    let info = mint_probe_info();
    let mint = mint_circuit(&info);
    let program = interpreter::Program::new(&info.helpers, &info.witnesses, &info.natives);

    // Harvest the inline `Either` / `ZswapCoinPublicKey` / `ContractAddress`
    // defs from the circuit arguments, exactly as the funded call path does.

    // Deployed mint contract has no user ledger fields: data is an empty array.
    let state = ContractState::new(
        StateValue::Array(vec![].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let args = mint_args(domain_sep);

    let result = interpreter::execute(
        &mint.def,
        &program,
        state,
        &args,
        interpreter::Env { address, ..epoch() },
    )
    .expect("mint circuit must execute");

    assert_eq!(
        result.zswap_outputs.len(),
        1,
        "mintShieldedToken creates exactly one Zswap output"
    );
    result.zswap_outputs.into_iter().next().unwrap()
}

/// `kernel.self()` lowers to a context read (`dup{n:2} idx[0] popeq`,
/// result-type `ContractAddress`). The interpreter runs these ops through the
/// VM with the supplied contract address injected into the `QueryContext`, so
/// the read returns that address (and the ops land in the transcript the
/// proving key expects). A minimal circuit that just returns `kernel.self()`
/// must yield the supplied address — covering the resolution independent of the
/// mint effects.
#[test]
fn interpreter_resolves_kernel_self_to_supplied_address() {
    use midnight_contract::interpreter;
    use midnight_contract::runtime::Value;

    let address_type = Type::Struct {
        name: "ContractAddress".to_string(),
        fields: vec![("bytes".to_string(), Type::Bytes(32))],
    };
    let ir = circuit(
        Vec::new(),
        address_type.clone(),
        Expr::PublicLedger {
            op_class: OpClass::Plain("read".into()),
            field: id("%kernel.3"),
            path: Vec::new(),
            op: "self".to_string(),
            result_type: address_type,
            instructions: vec![
                instruction("dup", &[("n", Operand::Int(2.into()))]),
                instruction(
                    "idx",
                    &[
                        ("cached", Operand::Bool(true)),
                        ("pushPath", Operand::Bool(false)),
                        ("path", field_path(0)),
                    ],
                ),
                instruction(
                    "popeq",
                    &[("cached", Operand::Bool(true)), ("result", Operand::Void)],
                ),
            ],
            args: Vec::new(),
        },
    );

    let state = ContractState::new(
        StateValue::Array(vec![].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );
    let address = ContractAddress(midnight_base_crypto::hash::HashOutput([0x5Au8; 32]));

    let result = interpreter::execute(
        &ir,
        &no_program(),
        state,
        &[],
        interpreter::Env { address, ..epoch() },
    )
    .expect("kernel.self() circuit executes");

    match result.result {
        Some(Value::AlignedValue(av)) => {
            let atom = &av.value.0[0];
            let mut b = [0u8; 32];
            b[..atom.0.len()].copy_from_slice(&atom.0);
            assert_eq!(
                b, [0x5Au8; 32],
                "kernel.self() must return the supplied address"
            );
        }
        other => panic!("expected the contract address, got {other:?}"),
    }
}

/// Full mint circuit, end to end against an empty deployed state, using the
/// `dup` arities the patched compiler emits. Exercises `kernel.self()`
/// resolution, the `persistentCommit` token-color derivation, the Either
/// destructuring, and the `mintShieldedToken`/`claimZswapCoinSpend` effect ops
/// (which need `dup{n:1}`/`dup{n:2}`), and proves the captured coin's color
/// depends on the contract address.
#[test]
fn interpreter_runs_mint_shielded_token_circuit() {
    fn addr(b: u8) -> midnight_coin_structure::contract::ContractAddress {
        ContractAddress(midnight_base_crypto::hash::HashOutput([b; 32]))
    }
    fn color_of(out: &midnight_contract::runtime::CircuitZswapOutput) -> [u8; 32] {
        // coin AlignedValue atoms: [nonce(32), color(32), value]. Color is
        // atom 1, FAB-trimmed of trailing zeros.
        let av = out.coin.try_to_aligned_value().unwrap();
        let atom = &av.value.0[1];
        let mut c = [0u8; 32];
        c[..atom.0.len()].copy_from_slice(&atom.0);
        c
    }

    let domain = [1u8; 32];
    let color_a = color_of(&run_mint(domain, addr(0xAA)));
    let color_b = color_of(&run_mint(domain, addr(0xBB)));

    assert_ne!(
        color_a, [0u8; 32],
        "minted coin color must be a real token type, not zero"
    );
    assert_ne!(
        color_a, color_b,
        "coin color = tokenType(domain_sep, address): different addresses must give \
         different colors, proving kernel.self() resolves to the real contract address"
    );
}

/// Regression: the low-level `build_unproven_call_tx` builder must thread the
/// circuit's declared argument types and struct layouts to the interpreter. The
/// mint circuit destructures an `Either` recipient (`recipient.is_left`);
/// without the argument's declared type and struct layout the field access
/// fails with "unknown receiver type".
#[test]
fn build_unproven_call_tx_handles_struct_arguments() {
    let info = mint_probe_info();
    let mint = mint_circuit(&info);
    let program = interpreter::Program::new(&info.helpers, &info.witnesses, &info.natives);
    let address = ContractAddress(midnight_base_crypto::hash::HashOutput([0xCD; 32]));
    let state = ContractState::new(
        StateValue::Array(vec![].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );
    let args = mint_args([1u8; 32]);

    let ok = call::build_unproven_call_tx(
        &mint.def,
        &program,
        &state,
        "mint",
        midnight_contract::ContractAddress(address.0),
        "undeployed1",
        &args,
        &midnight_contract::runtime::NoWitnesses,
        None,
    );
    assert!(
        ok.is_ok(),
        "build_unproven_call_tx must handle struct arguments: {:?}",
        ok.err()
    );
}
