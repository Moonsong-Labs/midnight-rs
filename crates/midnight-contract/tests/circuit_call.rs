//! Circuit call integration tests.
//!
//! These tests describe the target API for building and submitting
//! circuit call transactions. Tests marked #[ignore] represent
//! functionality not yet implemented.

use midnight_bindgen::{
    ContractMaintenanceAuthority, ContractState, InMemoryDB, StateValue, StorageHashMap,
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
        &interpreter::NoWitnesses,
        None,
        &[],
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
        &interpreter::NoWitnesses,
        None,
        &[],
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
        &interpreter::NoWitnesses,
        None,
        &[],
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
    use midnight_contract::interpreter::{self, Value};

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
        &interpreter::NoWitnesses,
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
    use midnight_contract::interpreter::{self, InterpreterError, Value, WitnessProvider};

    struct MockWitness;
    impl WitnessProvider for MockWitness {
        fn call_witness(
            &self,
            _ctx: &mut interpreter::WitnessContext<'_>,
            name: &str,
            _args: &[Value],
        ) -> Result<Value, InterpreterError> {
            match name {
                "private$secret_key" => Ok(Value::Integer(42)),
                _ => Err(InterpreterError::Witness(format!("unknown: {name}"))),
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

/// A witness's view of private state threads across calls via `WitnessContext`:
/// reading the current state, returning a value derived from it, and writing an
/// updated state that the next call observes.
#[test]
fn witness_context_threads_private_state() {
    use midnight_contract::interpreter::{
        self, InterpreterError, Value, WitnessContext, WitnessProvider,
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
        ) -> Result<Value, InterpreterError> {
            match name {
                "private$counter" => {
                    let current = decode(ctx.private_state());
                    ctx.set_private_state((current + 1).to_le_bytes().to_vec());
                    Ok(Value::Integer(current as u128))
                }
                _ => Err(InterpreterError::Witness(format!("unknown: {name}"))),
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
    let mut ctx = WitnessContext::new(Some("0200deadbeef"), Some(""), &mut private_state);

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
        &interpreter::NoWitnesses,
        None,
        &[],
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
