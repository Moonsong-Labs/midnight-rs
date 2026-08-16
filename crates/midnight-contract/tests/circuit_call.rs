//! Circuit call integration tests.
//!
//! These tests describe the target API for building and submitting
//! circuit call transactions. Tests marked #[ignore] represent
//! functionality not yet implemented.

use compact_bindgen::{
    AlignedValue, ContractMaintenanceAuthority, ContractState, InMemoryDB, StateValue,
    StorageHashMap,
};
use midnight_coin_structure::contract::ContractAddress;
use midnight_contract::call;
use midnight_contract::interpreter;

use compact_codegen::ir::{
    self, Argument, Expr, FieldType, Ident, Instruction, Literal, Operand, PathElement, Type,
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
        op: op.to_string(),
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

/// `let <local> = <value>;` then `round += <local>`, the shape the counter
/// contract compiles to.
fn increment_by(local: &str, value: Expr, arguments: Vec<Argument>) -> ir::Circuit {
    let body = Expr::Seq(vec![Expr::LetStar {
        bindings: vec![(
            Argument {
                name: id(local),
                ty: uint("65535"),
            },
            value,
        )],
        body: Box::new(Expr::PublicLedger {
            field: id("%round.1"),
            path: vec![PathElement::Index(0)],
            op: "increment".to_string(),
            result_type: Type::unit(),
            instructions: vec![
                instruction(
                    "idx",
                    &[
                        ("cached", Operand::Bool(false)),
                        ("pushPath", Operand::Bool(true)),
                        ("path", field_path(0)),
                    ],
                ),
                instruction(
                    "addi",
                    &[(
                        "immediate",
                        Operand::ValueToInt(Box::new(Operand::Expr(Box::new(var(local))))),
                    )],
                ),
                instruction(
                    "ins",
                    &[
                        ("cached", Operand::Bool(true)),
                        ("n", Operand::Int(1.into())),
                    ],
                ),
            ],
            args: vec![var(local)],
        }),
    }]);
    circuit(arguments, Type::unit(), body)
}

fn counter_increment_ir() -> ir::Circuit {
    increment_by("%tmp.2", Expr::Quote(Literal::Int(1.into())), Vec::new())
}

fn dummy_address() -> ContractAddress {
    ContractAddress(midnight_base_crypto::hash::HashOutput([0xAA; 32]))
}

fn read_counter(state: &ContractState<InMemoryDB>) -> u64 {
    match state.data.get_ref() {
        StateValue::Array(arr) => match arr.get(0).unwrap() {
            StateValue::Cell(sp) => u64::try_from(&*sp.value).unwrap(),
            other => panic!("expected Cell, got {other:?}"),
        },
        other => panic!("expected Array, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Phase 2: Interpreter
// ---------------------------------------------------------------------------

#[test]
fn interpreter_executes_counter_increment() {
    let state = counter_state(0);
    let ir = counter_increment_ir();

    let result = interpreter::execute(&ir, &no_program(), &state).unwrap();
    assert_eq!(read_counter(&result.state), 1);
}

#[test]
fn interpreter_executes_counter_increment_multiple_times() {
    let ir = counter_increment_ir();
    let program = no_program();
    let mut state = counter_state(0);

    for expected in 1..=5 {
        let result = interpreter::execute(&ir, &program, &state).unwrap();
        state = result.state;
        assert_eq!(read_counter(&state), expected);
    }
}

// ---------------------------------------------------------------------------
// Phase 3a: Unproven transaction construction
// ---------------------------------------------------------------------------

#[test]
fn build_unproven_tx_produces_nonempty_bytes() {
    let state = counter_state(0);
    let ir = counter_increment_ir();

    let tx = call::build_unproven_call_tx(
        &ir,
        &no_program(),
        &state,
        "increment",
        dummy_address(),
        "test",
        &[],
        &midnight_contract::runtime::NoWitnesses,
        None,
    )
    .unwrap();

    assert!(!tx.tx_bytes.is_empty());
    assert_eq!(read_counter(&tx.new_state), 1);
}

#[test]
fn build_unproven_tx_includes_correct_state_update() {
    let state = counter_state(42);
    let ir = counter_increment_ir();

    let tx = call::build_unproven_call_tx(
        &ir,
        &no_program(),
        &state,
        "increment",
        dummy_address(),
        "test",
        &[],
        &midnight_contract::runtime::NoWitnesses,
        None,
    )
    .unwrap();

    assert_eq!(read_counter(&tx.new_state), 43);
}

// ---------------------------------------------------------------------------
// Phase 3a: Transaction with proper transcripts
// ---------------------------------------------------------------------------

/// The transaction should contain a proper transcript (not empty).
/// This means the interpreter's Ops are correctly converted to
/// ResultModeVerify Ops and partitioned into guaranteed/fallible.
#[test]
fn unproven_tx_has_transcript() {
    let state = counter_state(0);
    let ir = counter_increment_ir();

    let tx = call::build_unproven_call_tx(
        &ir,
        &no_program(),
        &state,
        "increment",
        dummy_address(),
        "test",
        &[],
        &midnight_contract::runtime::NoWitnesses,
        None,
    )
    .unwrap();

    // Deserialize and check that the transaction has non-empty actions
    // with actual transcript data
    assert!(
        tx.tx_bytes.len() > 100,
        "TX should be larger with transcript data"
    );
}

// ---------------------------------------------------------------------------
// Phase 4: Circuits with arguments
// ---------------------------------------------------------------------------

/// Test circuit arguments by providing initial variable bindings.
#[test]
fn interpreter_handles_circuit_arguments() {
    use midnight_contract::interpreter;
    use midnight_contract::runtime::Value;

    let ir = increment_by(
        "%x.2",
        var("%value.1"),
        vec![Argument {
            name: id("%value.1"),
            ty: uint("65535"),
        }],
    );

    let state = counter_state(10);

    // Pass "value" = 5 as a circuit argument
    let result = interpreter::execute_with(
        &ir,
        &no_program(),
        &state,
        &[("value", Value::Integer(5))],
        &midnight_contract::runtime::NoWitnesses,
    )
    .unwrap();

    // Counter should go from 10 to 15 (added 5)
    assert_eq!(read_counter(&result.state), 15);
}

// ---------------------------------------------------------------------------
// Phase 4: Witness calls
// ---------------------------------------------------------------------------

/// Test witness provider by implementing a mock that returns a fixed value.
#[test]
fn interpreter_handles_witness_calls() {
    use midnight_contract::interpreter;
    use midnight_contract::runtime::{InterpreterError, Value, WitnessOutcome, WitnessProvider};

    struct MockWitness;
    impl WitnessProvider for MockWitness {
        fn call_witness(
            &self,
            _ctx: &mut midnight_contract::runtime::WitnessContext<'_>,
            name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, InterpreterError> {
            match name {
                "private$secret_key" => Ok(WitnessOutcome::Value(Value::Integer(42))),
                _ => Ok(WitnessOutcome::Unknown),
            }
        }
    }

    let witnesses = vec![ir::Witness {
        name: id("%private$secret_key.20"),
        arguments: Vec::new(),
        result_type: Type::Field(FieldType::Native),
    }];
    let program = interpreter::Program::new(&[], &witnesses, &[]);
    let ir = increment_by(
        "%sk.2",
        Expr::Call {
            name: id("%private$secret_key.20"),
            args: Vec::new(),
        },
        Vec::new(),
    );

    let state = counter_state(0);
    let result = interpreter::execute_with(&ir, &program, &state, &[], &MockWitness).unwrap();

    // Witness returned 42, so counter should be 0 + 42 = 42
    assert_eq!(read_counter(&result.state), 42);
}

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
    match interpreter::execute_with(
        &persistent_hash_witness_ir(),
        &program,
        &state,
        &[],
        &FailingHsm,
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
    let result = interpreter::execute_with(
        &persistent_hash_witness_ir(),
        &program,
        &state,
        &[],
        &KnowsNothing,
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
    let mut ctx = WitnessContext::new(&mut private_state);

    // First call: witness sees an empty (= 0) state and returns 0.
    let r1 =
        interpreter::execute_with_context(&ir, &program, &state, &[], &mut ctx, &CounterWitness)
            .unwrap();
    assert!(matches!(r1.result, Some(Value::Integer(0))));
    // The witness's private value must be recorded as a private transcript
    // output, or proving a witness-using circuit fails with "ran out of private
    // transcript outputs". One witness call -> one output.
    assert_eq!(r1.private_transcript_outputs.len(), 1);

    // Second call reuses the same buffer: the witness now sees 1.
    let r2 =
        interpreter::execute_with_context(&ir, &program, &state, &[], &mut ctx, &CounterWitness)
            .unwrap();
    assert!(matches!(r2.result, Some(Value::Integer(1))));
    assert_eq!(r2.private_transcript_outputs.len(), 1);

    // `ctx`'s borrow of `private_state` ends at its last use above, so the
    // post-call buffer is readable here: two increments → 2.
    assert_eq!(decode(&private_state), 2);
}

// ---------------------------------------------------------------------------
// Phase 5: End-to-end
// ---------------------------------------------------------------------------

/// Submit an unproven TX to a real node and verify it's rejected
/// with a proof error (not a deserialization error).
/// This validates the transaction format is correct.
#[tokio::test]
#[ignore = "requires running node: MIDNIGHT_NODE_URL"]
async fn submit_unproven_tx_to_node() {
    let node_url = match std::env::var("MIDNIGHT_NODE_URL").ok() {
        Some(u) => u,
        None => {
            eprintln!("skipping: MIDNIGHT_NODE_URL not set");
            return;
        }
    };

    // Build transaction
    let state = counter_state(0);
    let ir = counter_increment_ir();
    let address = ContractAddress(midnight_base_crypto::hash::HashOutput([0; 32]));
    let tx = call::build_unproven_call_tx(
        &ir,
        &no_program(),
        &state,
        "increment",
        address,
        "undeployed1",
        &[],
        &midnight_contract::runtime::NoWitnesses,
        None,
    )
    .unwrap();

    eprintln!("unproven TX: {} bytes", tx.tx_bytes.len());

    // Submit via the provider's submit function
    let provider = midnight_provider::MidnightProvider::new(&node_url, "http://127.0.0.1:8088")
        .expect("provider construction");
    match provider.submit(&tx.tx_bytes).await {
        Ok(pending) => {
            eprintln!(
                "TX submitted (unexpected for unproven): {}",
                pending.extrinsic_hash_hex()
            );
        }
        Err(e) => {
            let msg = e.to_string();
            eprintln!("TX rejected (expected): {msg}");
            // An unproven TX should be rejected at proof verification,
            // NOT at deserialization. A deserialization error means our
            // TX format is wrong.
            assert!(
                !msg.contains("Deserialization"),
                "TX format is wrong — deserialization error: {msg}"
            );
        }
    }
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

    let result = interpreter::execute_with(
        &ir,
        &program,
        &state,
        &[("coin", coin), ("recipient", recipient)],
        &midnight_contract::runtime::NoWitnesses,
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

    let mut ps = Vec::new();
    let mut wctx = midnight_contract::runtime::WitnessContext::new(&mut ps);
    let result = interpreter::execute_with_owned(
        &mint.def,
        &program,
        state,
        &args,
        &midnight_contract::runtime::NoWitnesses,
        Some(&mut wctx),
        Some(address),
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

    let mut ps = Vec::new();
    let mut wctx = midnight_contract::runtime::WitnessContext::new(&mut ps);
    let result = interpreter::execute_with_owned(
        &ir,
        &no_program(),
        state,
        &[],
        &midnight_contract::runtime::NoWitnesses,
        Some(&mut wctx),
        Some(address),
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
    let new_state = || {
        ContractState::new(
            StateValue::Array(vec![].into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    };
    let args = mint_args([1u8; 32]);

    // With the harvested struct defs the builder slices `recipient.is_left`
    // and builds a transaction.
    let ok = call::build_unproven_call_tx(
        &mint.def,
        &program,
        &new_state(),
        "mint",
        address,
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

    // The circuit's own `arguments` declare `recipient` inline, fields and all,
    // and that declaration is what drives both the slicing and the argument
    // encoding, so the same call succeeds with no side registry at all.
    let bare = call::build_unproven_call_tx(
        &mint.def,
        &program,
        &new_state(),
        "mint",
        address,
        "undeployed1",
        &args,
        &midnight_contract::runtime::NoWitnesses,
        None,
    );
    assert!(
        bare.is_ok(),
        "the circuit's inline argument types must be enough on their own: {:?}",
        bare.err()
    );
}
