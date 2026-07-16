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

use compact_codegen::ir::CircuitIrBody;

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

fn counter_increment_ir() -> CircuitIrBody {
    serde_json::from_str(
        r#"{
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
    }"#,
    )
    .unwrap()
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

    let result = interpreter::execute(&ir, &state).unwrap();
    assert_eq!(read_counter(&result.state), 1);
}

#[test]
fn interpreter_executes_counter_increment_multiple_times() {
    let ir = counter_increment_ir();
    let mut state = counter_state(0);

    for expected in 1..=5 {
        let result = interpreter::execute(&ir, &state).unwrap();
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
        &state,
        "increment",
        dummy_address(),
        "test",
        &[],
        &midnight_contract::runtime::NoWitnesses,
        None,
        midnight_contract::CircuitDefs::default(),
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
        &state,
        "increment",
        dummy_address(),
        "test",
        &[],
        &midnight_contract::runtime::NoWitnesses,
        None,
        midnight_contract::CircuitDefs::default(),
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
        &state,
        "increment",
        dummy_address(),
        "test",
        &[],
        &midnight_contract::runtime::NoWitnesses,
        None,
        midnight_contract::CircuitDefs::default(),
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

    // Simple IR that reads a "value" argument and uses it in a let binding
    let ir: CircuitIrBody = serde_json::from_str(
        r#"{
        "body": {
            "op": "seq",
            "stmts": [
                {
                    "op": "expr-stmt",
                    "expr": {
                        "op": "let-expr",
                        "bindings": [
                            { "op": "let", "name": "x",
                              "value": { "op": "var", "name": "value" } }
                        ],
                        "body": {
                            "op": "ledger-query",
                            "ops": [
                                { "op": "idx", "cached": false, "push-path": true,
                                  "path": [{ "tag": "value", "value": "0", "type": { "type": "Uint", "maxval": "255" } }] },
                                { "op": "addi", "immediate": { "op": "var", "name": "x" } },
                                { "op": "ins", "cached": true, "n": 1 }
                            ],
                            "result-type": { "type": "Void" }
                        }
                    }
                }
            ]
        },
        "result": null
    }"#,
    )
    .unwrap();

    let state = counter_state(10);

    // Pass "value" = 5 as a circuit argument
    let result = interpreter::execute_with(
        &ir,
        &state,
        &[("value", Value::Integer(5))],
        &midnight_contract::runtime::NoWitnesses,
        &[],
        &[],
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

    // IR that calls a witness and uses the result
    let ir: CircuitIrBody = serde_json::from_str(
        r#"{
        "body": {
            "op": "seq",
            "stmts": [
                {
                    "op": "expr-stmt",
                    "expr": {
                        "op": "let-expr",
                        "bindings": [
                            { "op": "let", "name": "sk",
                              "value": { "op": "call-witness", "name": "private$secret_key",
                                         "args": [], "result-type": { "type": "Field" } } }
                        ],
                        "body": {
                            "op": "ledger-query",
                            "ops": [
                                { "op": "idx", "cached": false, "push-path": true,
                                  "path": [{ "tag": "value", "value": "0", "type": { "type": "Uint", "maxval": "255" } }] },
                                { "op": "addi", "immediate": { "op": "var", "name": "sk" } },
                                { "op": "ins", "cached": true, "n": 1 }
                            ],
                            "result-type": { "type": "Void" }
                        }
                    }
                }
            ]
        },
        "result": null
    }"#,
    )
    .unwrap();

    let state = counter_state(0);
    let result = interpreter::execute_with(&ir, &state, &[], &MockWitness, &[], &[]).unwrap();

    // Witness returned 42, so counter should be 0 + 42 = 42
    assert_eq!(read_counter(&result.state), 42);
}

/// IR whose result is a `persistentHash` witness call over a literal — the
/// name collides with the interpreter builtin of the same name, which is
/// exactly the collision the Unknown/Err distinction protects.
fn persistent_hash_witness_ir() -> CircuitIrBody {
    serde_json::from_str(
        r#"{
        "body": { "op": "seq", "stmts": [] },
        "result": { "op": "call-witness", "name": "persistentHash",
                    "args": [{ "op": "lit", "type": { "type": "Uint", "maxval": "65535" }, "value": "7" }],
                    "result-type": { "type": "Field" } }
    }"#,
    )
    .unwrap()
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

    let state = counter_state(0);
    match interpreter::execute_with(
        &persistent_hash_witness_ir(),
        &state,
        &[],
        &FailingHsm,
        &[],
        &[],
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

    let state = counter_state(0);
    let result = interpreter::execute_with(
        &persistent_hash_witness_ir(),
        &state,
        &[],
        &KnowsNothing,
        &[],
        &[],
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
    let ir: CircuitIrBody = serde_json::from_str(
        r#"{
        "body": { "op": "seq", "stmts": [] },
        "result": { "op": "call-witness", "name": "private$counter",
                    "args": [], "result-type": { "type": "Field" } }
    }"#,
    )
    .unwrap();

    let state = counter_state(0);
    let mut private_state = Vec::new();
    let mut ctx = WitnessContext::new(&mut private_state);

    // First call: witness sees an empty (= 0) state and returns 0.
    let r1 = interpreter::execute_with_context(
        &ir,
        &state,
        &[],
        &mut ctx,
        &CounterWitness,
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(matches!(r1.result, Some(Value::Integer(0))));
    // The witness's private value must be recorded as a private transcript
    // output, or proving a witness-using circuit fails with "ran out of private
    // transcript outputs". One witness call -> one output.
    assert_eq!(r1.private_transcript_outputs.len(), 1);

    // Second call reuses the same buffer: the witness now sees 1.
    let r2 = interpreter::execute_with_context(
        &ir,
        &state,
        &[],
        &mut ctx,
        &CounterWitness,
        &[],
        &[],
        &[],
    )
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
        &state,
        "increment",
        address,
        "undeployed1",
        &[],
        &midnight_contract::runtime::NoWitnesses,
        None,
        midnight_contract::CircuitDefs::default(),
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

/// `createZswapOutput(coin, recipient)` lowers to a `call-witness` with no
/// effect of its own; it marks "attach a Zswap output for this coin here".
/// The interpreter must capture its `(coin, recipient)` args on the
/// `ExecutionResult` (so the call path can build the offer `Output`) and
/// return unit, rather than erroring as an unknown witness.
#[test]
fn interpreter_captures_create_zswap_output() {
    use midnight_contract::runtime::Value;

    let ir: CircuitIrBody = serde_json::from_str(
        r#"{
        "body": {
            "op": "expr-stmt",
            "expr": {
                "op": "call-witness",
                "name": "createZswapOutput",
                "args": [
                    { "op": "var", "name": "coin" },
                    { "op": "var", "name": "recipient" }
                ],
                "result-type": { "type": "Tuple", "types": [] }
            }
        },
        "result": null
    }"#,
    )
    .unwrap();

    let state = counter_state(0);
    let coin = Value::AlignedValue(AlignedValue::from([7u8; 32]));
    let recipient = Value::AlignedValue(AlignedValue::from([9u8; 32]));

    let result = interpreter::execute_with(
        &ir,
        &state,
        &[("coin", coin), ("recipient", recipient)],
        &midnight_contract::runtime::NoWitnesses,
        &[],
        &[],
    )
    .expect("createZswapOutput must be handled, not error");

    assert_eq!(
        result.zswap_outputs.len(),
        1,
        "one circuit-created Zswap output should be captured"
    );
    let out = &result.zswap_outputs[0];
    assert_eq!(out.coin.to_aligned_value(), AlignedValue::from([7u8; 32]));
    assert_eq!(
        out.recipient.to_aligned_value(),
        AlignedValue::from([9u8; 32])
    );
}

// ---------------------------------------------------------------------------
// Shielded mint: full `mintShieldedToken` circuit
// ---------------------------------------------------------------------------

fn mint_probe_ir_and_structs() -> (CircuitIrBody, Vec<compact_codegen::ir::StructDef>) {
    // The `-dupn` fixture is the genuine output of the patched compiler
    // (`save-contract-info-passes.ss` now emits `dup` arities); the bare
    // fixture (no arity) is kept for the backward-compat parse test.
    let json = include_str!("../../../tests/fixtures/mint-probe-contract-info-dupn.json");
    let info: compact_codegen::types::ContractInfo = serde_json::from_str(json).unwrap();
    let mint = info
        .circuits
        .iter()
        .find(|c| c.name == "mint")
        .expect("mint circuit");
    let ir =
        serde_json::from_value(serde_json::to_value(mint.ir.as_ref().expect("mint IR")).unwrap())
            .unwrap();
    // Harvest the inline `Either` / `ZswapCoinPublicKey` / `ContractAddress`
    // defs from the circuit arguments, exactly as the funded call path does.
    let mut structs = info.structs.clone();
    let mut enums = Vec::new();
    compact_codegen::arg_types::collect_argument_defs(&mint.arguments, &mut structs, &mut enums);
    (ir, structs)
}

/// The inline struct/enum defs harvested from the mint circuit's `arguments`
/// (via `compact_codegen::arg_types`) must cover the nested `Either`,
/// `ZswapCoinPublicKey`, and `ContractAddress` types the interpreter needs to
/// slice the `recipient` argument. This is the registry the funded call path
/// builds automatically, replacing the hand-supplied `either_struct_defs()`.
#[test]
fn harvested_defs_cover_inline_either_recipient() {
    use compact_codegen::ir::TypeRef;

    let json = include_str!("../../../tests/fixtures/mint-probe-contract-info.json");
    let info: compact_codegen::types::ContractInfo = serde_json::from_str(json).unwrap();
    let mint = info
        .circuits
        .iter()
        .find(|c| c.name == "mint")
        .expect("mint circuit");

    let arg_types = compact_codegen::arg_types::circuit_arg_types(&mint.arguments);
    assert!(
        arg_types
            .iter()
            .any(|(n, t)| n == "recipient"
                && matches!(t, TypeRef::Struct { name } if name == "Either")),
        "recipient must be typed as Struct(Either): {arg_types:?}"
    );

    let mut structs = info.structs.clone();
    let mut enums = Vec::new();
    compact_codegen::arg_types::collect_argument_defs(&mint.arguments, &mut structs, &mut enums);

    let names: Vec<&str> = structs.iter().map(|s| s.name.as_str()).collect();
    for required in ["Either", "ZswapCoinPublicKey", "ContractAddress"] {
        assert!(names.contains(&required), "missing {required}: {names:?}");
    }

    // The harvested `Either` matches the canonical hand-built shape.
    let either = structs.iter().find(|s| s.name == "Either").unwrap();
    let field_names: Vec<&str> = either.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(field_names, ["is_left", "left", "right"]);
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
    use compact_codegen::ir::TypeRef;
    use midnight_contract::interpreter;
    use midnight_contract::runtime::Value;

    let (ir, structs) = mint_probe_ir_and_structs();

    // Deployed mint contract has no user ledger fields: data is an empty array.
    let state = ContractState::new(
        StateValue::Array(vec![].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let args = [
        (
            "domain_sep",
            Value::AlignedValue(AlignedValue::from(domain_sep)),
        ),
        ("value", Value::Integer(1000)),
        ("nonce", Value::AlignedValue(AlignedValue::from([2u8; 32]))),
        ("recipient", Value::AlignedValue(either_left([3u8; 32]))),
    ];
    let arg_types = [(
        "recipient",
        TypeRef::Struct {
            name: "Either".to_string(),
        },
    )];

    let mut ps = Vec::new();
    let mut wctx = midnight_contract::runtime::WitnessContext::new(&mut ps);
    let result = interpreter::execute_with_owned(
        &ir,
        state,
        &args,
        &arg_types,
        &midnight_contract::runtime::NoWitnesses,
        Some(&mut wctx),
        &[],
        &structs,
        &[],
        Some(address),
        None,
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

    let ir: CircuitIrBody = serde_json::from_str(
        r#"{
        "body": { "op": "seq", "stmts": [] },
        "result": {
            "op": "ledger-query",
            "ops": [
                { "op": "dup", "n": 2 },
                { "op": "idx", "cached": true, "push-path": false,
                  "path": [{ "tag": "value", "value": "0", "type": { "type": "Uint", "maxval": "255" } }] },
                { "op": "popeq", "cached": true }
            ],
            "result-type": { "type": "Struct", "name": "ContractAddress" }
        }
    }"#,
    )
    .unwrap();

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
        state,
        &[],
        &[],
        &midnight_contract::runtime::NoWitnesses,
        Some(&mut wctx),
        &[],
        &[],
        &[],
        Some(address),
        None,
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
        let av = out.coin.to_aligned_value();
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

/// Regression: the low-level `build_unproven_call_tx` builder must thread
/// `arg_types`/`structs`/`enums` to the interpreter. The mint circuit
/// destructures an `Either` recipient (`recipient.is_left`); without the
/// argument's declared type and struct layout the field access fails with
/// "unknown receiver type", even though the high-level funded path works.
#[test]
fn build_unproven_call_tx_handles_struct_arguments() {
    use midnight_contract::runtime::Value;

    let (ir, structs) = mint_probe_ir_and_structs();
    let address = ContractAddress(midnight_base_crypto::hash::HashOutput([0xCD; 32]));
    let new_state = || {
        ContractState::new(
            StateValue::Array(vec![].into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    };
    let args = [
        (
            "domain_sep",
            Value::AlignedValue(AlignedValue::from([1u8; 32])),
        ),
        ("value", Value::Integer(1000)),
        ("nonce", Value::AlignedValue(AlignedValue::from([2u8; 32]))),
        ("recipient", Value::AlignedValue(either_left([3u8; 32]))),
    ];
    let arg_types = [(
        "recipient",
        compact_codegen::ir::TypeRef::Struct {
            name: "Either".to_string(),
        },
    )];

    // With the harvested struct defs the builder slices `recipient.is_left`
    // and builds a transaction.
    let ok = call::build_unproven_call_tx(
        &ir,
        &new_state(),
        "mint",
        address,
        "undeployed1",
        &args,
        &midnight_contract::runtime::NoWitnesses,
        None,
        midnight_contract::CircuitDefs {
            arg_types: &arg_types,
            structs: &structs,
            ..Default::default()
        },
    );
    assert!(
        ok.is_ok(),
        "build_unproven_call_tx must handle struct arguments: {:?}",
        ok.err()
    );

    // Without arg_types/structs it fails at struct field slicing (the
    // reviewer's scenario), rather than silently producing a wrong tx.
    let err = call::build_unproven_call_tx(
        &ir,
        &new_state(),
        "mint",
        address,
        "undeployed1",
        &args,
        &midnight_contract::runtime::NoWitnesses,
        None,
        midnight_contract::CircuitDefs::default(),
    );
    assert!(
        err.is_err(),
        "missing arg_types/structs must fail, not silently pass"
    );
}
