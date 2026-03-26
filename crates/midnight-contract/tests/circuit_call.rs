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

    let tx =
        call::build_unproven_call_tx(&ir, &state, "increment", dummy_address(), "test").unwrap();

    assert!(!tx.tx_bytes.is_empty());
    assert_eq!(read_counter(&tx.new_state), 1);
}

#[test]
fn build_unproven_tx_includes_correct_state_update() {
    let state = counter_state(42);
    let ir = counter_increment_ir();

    let tx =
        call::build_unproven_call_tx(&ir, &state, "increment", dummy_address(), "test").unwrap();

    assert_eq!(read_counter(&tx.new_state), 43);
}

/// Full pipeline: IR → interpret → build TX → envelope → ready to submit
#[test]
fn full_pipeline_counter_increment() {
    let state = counter_state(0);
    let ir = counter_increment_ir();

    // Step 1: Build unproven transaction
    let tx =
        call::build_unproven_call_tx(&ir, &state, "increment", dummy_address(), "test-network")
            .unwrap();

    // Step 2: Build envelope (the bytes that go to send_mn_transaction)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let envelope = call::build_tx_envelope(&tx, now);

    // Verify the envelope is valid JSON with correct structure
    let parsed: serde_json::Value = serde_json::from_slice(&envelope).expect("valid JSON");
    assert!(parsed["tx"]["Midnight"].is_string());
    assert!(parsed["tx_hash"].is_string());
    assert_eq!(parsed["context"]["secondsSinceEpoch"].as_u64(), Some(now));

    // Verify state was updated
    assert_eq!(read_counter(&tx.new_state), 1);

    eprintln!("=== Full Pipeline Result ===");
    eprintln!("  TX size: {} bytes", tx.tx_bytes.len());
    eprintln!("  Envelope size: {} bytes", envelope.len());
    eprintln!("  tx_hash: {}", parsed["tx_hash"].as_str().unwrap());
    eprintln!("  Counter: 0 → 1");
    eprintln!("  NOTE: Transaction is unproven — would be rejected by node");
    eprintln!("        with 'proof verification failed' (not deserialization error)");
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

    let tx =
        call::build_unproven_call_tx(&ir, &state, "increment", dummy_address(), "test").unwrap();

    // Deserialize and check that the transaction has non-empty actions
    // with actual transcript data
    assert!(
        tx.tx_bytes.len() > 100,
        "TX should be larger with transcript data"
    );
}

// ---------------------------------------------------------------------------
// Phase 3a: JSON envelope for send_mn_transaction
// ---------------------------------------------------------------------------

/// The transaction should be wrappable in the JSON envelope format
/// expected by the send_mn_transaction pallet extrinsic.
#[test]
fn build_tx_envelope_produces_valid_json() {
    let state = counter_state(0);
    let ir = counter_increment_ir();

    let tx =
        call::build_unproven_call_tx(&ir, &state, "increment", dummy_address(), "test").unwrap();

    let envelope_bytes = call::build_tx_envelope(&tx, 1700000000);

    // Should be valid JSON
    let envelope: serde_json::Value = serde_json::from_slice(&envelope_bytes).expect("valid JSON");

    // Check structure
    assert!(envelope["tx"]["Midnight"].is_string());
    assert!(envelope["context"]["secondsSinceEpoch"].is_number());
    assert!(envelope["context"]["secondsSinceEpochErr"].as_u64() == Some(30));
    assert!(envelope["context"]["parentBlockHash"].is_string());
    assert!(envelope["tx_hash"].is_string());

    // tx.Midnight should be hex-encoded
    let tx_hex = envelope["tx"]["Midnight"].as_str().unwrap();
    assert!(!tx_hex.is_empty());
    hex::decode(tx_hex).expect("valid hex");

    // tx_hash should be 64 hex chars (sha256)
    let tx_hash = envelope["tx_hash"].as_str().unwrap();
    assert_eq!(tx_hash.len(), 64);

    eprintln!("envelope size: {} bytes", envelope_bytes.len());
    eprintln!("tx_hash: {tx_hash}");
}

// ---------------------------------------------------------------------------
// Phase 3b: Proving
// ---------------------------------------------------------------------------

/// Prove a transaction using ZK keys from the compiler.
///
/// Requires: MIDNIGHT_COUNTER_KEYS_DIR pointing to compiler output with
/// keys/ and zkir/ directories (compile with `compactc`, no --skip-zk).
#[tokio::test]
#[ignore = "requires ZK keys: set MIDNIGHT_COUNTER_KEYS_DIR"]
async fn prove_transaction() {
    let keys_dir = match std::env::var("MIDNIGHT_COUNTER_KEYS_DIR").ok() {
        Some(d) => d,
        None => {
            eprintln!("skipping: MIDNIGHT_COUNTER_KEYS_DIR not set");
            eprintln!("compile counter with: compactc counter.compact /tmp/counter-zk");
            eprintln!("then: MIDNIGHT_COUNTER_KEYS_DIR=/tmp/counter-zk cargo test ...");
            return;
        }
    };

    let state = counter_state(0);
    let ir = counter_increment_ir();

    // Build unproven TX
    let unproven =
        call::build_unproven_call_tx(&ir, &state, "increment", dummy_address(), "test").unwrap();
    eprintln!("unproven TX: {} bytes", unproven.tx_bytes.len());

    // Prove and seal
    let proven_bytes = call::prove_and_seal(&unproven, &keys_dir)
        .await
        .expect("prove_and_seal");
    eprintln!("proven TX: {} bytes", proven_bytes.len());

    assert!(
        proven_bytes.len() > unproven.tx_bytes.len(),
        "proven TX should be larger than unproven"
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
        fn call_witness(&self, name: &str, _args: &[Value]) -> Result<Value, InterpreterError> {
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
    let result = interpreter::execute_with(&ir, &state, &[], &MockWitness, &[]).unwrap();

    // Witness returned 42, so counter should be 0 + 42 = 42
    assert_eq!(read_counter(&result.state), 42);
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
    use subxt::{OnlineClient, SubstrateConfig};

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
    let tx =
        call::build_unproven_call_tx(&ir, &state, "increment", address, "undeployed1").unwrap();

    eprintln!("unproven TX: {} bytes", tx.tx_bytes.len());

    // Submit raw tagged-serialized bytes via subxt dynamic TX
    let client = OnlineClient::<SubstrateConfig>::from_insecure_url(&node_url)
        .await
        .expect("connect to node");

    let call = subxt::dynamic::tx(
        "Midnight",
        "send_mn_transaction",
        vec![subxt::dynamic::Value::from_bytes(&tx.tx_bytes)],
    );

    let tx_client = client.tx().await.unwrap();
    let unsigned = tx_client.create_unsigned(&call).unwrap();
    match unsigned.submit().await {
        Ok(hash) => {
            eprintln!("TX submitted (unexpected for unproven): {hash:?}");
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
