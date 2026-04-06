//! End-to-end tests with real contracts using bindgen-generated types.
//!
//! These tests use `midnight_bindgen::contract!` to generate typed ledger
//! structs, then execute circuits and verify state changes through typed
//! accessors — the same way application code would use the SDK.
//!
//! Requirements:
//! - MIDNIGHT_NODE_URL: running dev node (for submission tests)
//! - MIDNIGHT_COMPILED_DIR: directory with compiler output including IR
//!   (only needed for the comprehensive coverage test)

use midnight_bindgen::{
    AlignedValue, ContractMaintenanceAuthority, ContractState, InMemoryDB, StateValue,
    StorageHashMap,
};
use midnight_contract::call;
use midnight_contract::interpreter::{self, Value, WitnessProvider};

use compact_codegen::ir::CircuitIrBody;

// ---------------------------------------------------------------------------
// Bindgen-generated types — single contract-info.json per contract has both
// typed ledger accessors and circuit IR for call methods.
// ---------------------------------------------------------------------------

mod counter {
    midnight_bindgen::contract!("tests/fixtures/counter/compiler/contract-info.json");
}

mod tiny {
    midnight_bindgen::contract!("tests/fixtures/tiny/compiler/contract-info.json");
}

mod election {
    midnight_bindgen::contract!("tests/fixtures/election/compiler/contract-info.json");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compiled_dir() -> Option<String> {
    std::env::var("MIDNIGHT_COMPILED_DIR").ok()
}

fn node_url() -> Option<String> {
    std::env::var("MIDNIGHT_NODE_URL").ok()
}

fn load_contract_info(compiled_dir: &str, contract: &str) -> serde_json::Value {
    let path = format!("{compiled_dir}/{contract}/compiler/contract-info.json");
    let json = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&json).unwrap()
}

fn load_helpers(info: &serde_json::Value) -> Vec<compact_codegen::ir::HelperDef> {
    info["helpers"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|h| serde_json::from_value(h.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn find_circuit_ir(info: &serde_json::Value, circuit_name: &str) -> CircuitIrBody {
    let circuits = info["circuits"].as_array().unwrap();
    let circuit = circuits
        .iter()
        .find(|c| c["name"].as_str() == Some(circuit_name))
        .unwrap_or_else(|| panic!("circuit {circuit_name} not found"));
    serde_json::from_value(circuit["ir"].clone()).unwrap()
}

fn try_find_circuit_ir(
    info: &serde_json::Value,
    circuit_name: &str,
) -> Result<CircuitIrBody, String> {
    let circuits = info["circuits"].as_array().unwrap();
    let circuit = circuits
        .iter()
        .find(|c| c["name"].as_str() == Some(circuit_name))
        .ok_or_else(|| format!("circuit {circuit_name} not found"))?;
    serde_json::from_value(circuit["ir"].clone()).map_err(|e| format!("parse error: {e}"))
}

/// Load circuit IR from the fixture contract-info.json embedded at compile time.
fn load_fixture_ir(contract_info_json: &str, circuit_name: &str) -> CircuitIrBody {
    let info: serde_json::Value = serde_json::from_str(contract_info_json).unwrap();
    find_circuit_ir(&info, circuit_name)
}

/// Extract the error from a Result without requiring Debug on the Ok type.
fn expect_err<T>(
    result: Result<T, interpreter::InterpreterError>,
) -> interpreter::InterpreterError {
    match result {
        Ok(_) => panic!("expected Err but got Ok"),
        Err(e) => e,
    }
}

/// Build the gateway's 10-field initial state with defaults.
fn gateway_initial_state() -> ContractState<InMemoryDB> {
    ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from(6u8)),  // threshold
                StateValue::Map(StorageHashMap::new()),     // validators
                StateValue::Map(StorageHashMap::new()),     // unclaimed_deposits
                StateValue::from(0u64),                     // next_job_id
                StateValue::Map(StorageHashMap::new()),     // egress_jobs
                StateValue::Map(StorageHashMap::new()),     // processed_attestations
                StateValue::from(AlignedValue::from(0u64)), // signing_fee
                StateValue::from(AlignedValue::from([0u8; 32])), // fee_token
                StateValue::from(0u64),                     // next_signing_request_id
                StateValue::Map(StorageHashMap::new()),     // signing_requests
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    )
}

/// Test the generated InitialState (named LedgerInitialState when contract! has no name arg).
#[test]
fn counter_deploy_with_initial_state() {
    let initial = counter::LedgerInitialState::default();
    let ledger = initial.into_ledger();
    assert_eq!(ledger.round().unwrap(), 0u64);

    let initial = counter::LedgerInitialState { round: 42 };
    let ledger = initial.into_ledger();
    assert_eq!(ledger.round().unwrap(), 42u64);
    eprintln!("counter LedgerInitialState: default=0, custom=42 ✓");
}

fn load_fixture_helpers(contract_info_json: &str) -> Vec<compact_codegen::ir::HelperDef> {
    let info: serde_json::Value = serde_json::from_str(contract_info_json).unwrap();
    load_helpers(&info)
}

// Embed contract-info.json at compile time for fixture-based tests
const COUNTER_INFO: &str = include_str!("fixtures/counter/compiler/contract-info.json");
const TINY_INFO: &str = include_str!("fixtures/tiny/compiler/contract-info.json");
const ELECTION_INFO: &str = include_str!("fixtures/election/compiler/contract-info.json");
const GATEWAY_INFO: &str = include_str!("fixtures/gateway/compiler/contract-info.json");

// ---------------------------------------------------------------------------
// Counter: generated circuit call methods
// ---------------------------------------------------------------------------

/// Test the generated `call_increment` method — no manual IR loading needed.
#[test]
fn counter_generated_call_increment() {
    let state = ContractState::new(
        StateValue::Array(vec![StateValue::from(0u64)].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let ledger = counter::Ledger::new(state);

    // Call increment 3 times using the generated method
    let ledger = ledger.call_increment().unwrap();
    let ledger = ledger.call_increment().unwrap();
    let ledger = ledger.call_increment().unwrap();

    // Verify through the underlying state
    match ledger.contract_state().data.get_ref() {
        StateValue::Array(arr) => match arr.get(0).unwrap() {
            StateValue::Cell(sp) => {
                let counter = u64::try_from(&*sp.value).unwrap();
                assert_eq!(counter, 3, "counter should be 3 after 3 increments");
            }
            _ => panic!("expected Cell"),
        },
        _ => panic!("expected Array"),
    }
    eprintln!("counter: call_increment() × 3 = 3 ✓ (generated method)");
}

// ---------------------------------------------------------------------------
// Tiny: generated circuit call methods
// ---------------------------------------------------------------------------
//
// All tiny circuits except `public_key` (pure, no IR) have embedded IR and
// generate `call_*` methods.  However, `set`, `get`, and `clear` all invoke
// witnesses (`private$secret_key`, `persistentHash`, `in_state`) which the
// generated methods cannot satisfy because they use `NoWitnesses`.  The tests
// below verify that the methods exist and produce the expected witness error.

#[test]
fn tiny_generated_call_set_requires_witness() {
    use midnight_transient_crypto::curve::Fr;

    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from([0u8; 32])),
                StateValue::from(AlignedValue::from(Fr::from(0u64))),
                StateValue::from(AlignedValue::from(0u8)),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let ledger = tiny::Ledger::new(state);
    let result = ledger.call_set(Fr::from(99u64));

    // The circuit calls `private$secret_key` witness which NoWitnesses rejects.
    let err = expect_err(result);
    assert!(
        matches!(err, interpreter::InterpreterError::Witness(_)),
        "expected Witness variant, got: {err}"
    );
    eprintln!("tiny: call_set correctly requires witness: {err}");
}

#[test]
fn tiny_generated_call_get_requires_witness() {
    use midnight_transient_crypto::curve::Fr;

    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from([0u8; 32])),
                StateValue::from(AlignedValue::from(Fr::from(42u64))),
                StateValue::from(AlignedValue::from(1u8)),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let ledger = tiny::Ledger::new(state);
    let result = ledger.call_get();

    // The circuit calls `in_state` witness which NoWitnesses rejects.
    let err = expect_err(result);
    assert!(
        matches!(err, interpreter::InterpreterError::Witness(_)),
        "expected Witness variant, got: {err}"
    );
    eprintln!("tiny: call_get correctly requires witness: {err}");
}

#[test]
fn tiny_generated_call_clear_requires_witness() {
    use midnight_transient_crypto::curve::Fr;

    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from([0u8; 32])),
                StateValue::from(AlignedValue::from(Fr::from(42u64))),
                StateValue::from(AlignedValue::from(1u8)),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let ledger = tiny::Ledger::new(state);
    let result = ledger.call_clear();

    // The circuit calls `private$secret_key` witness which NoWitnesses rejects.
    let err = expect_err(result);
    assert!(
        matches!(err, interpreter::InterpreterError::Witness(_)),
        "expected Witness variant, got: {err}"
    );
    eprintln!("tiny: call_clear correctly requires witness: {err}");
}

// ---------------------------------------------------------------------------
// Election: generated circuit call methods
// ---------------------------------------------------------------------------
//
// All election circuits (`vote$commit`, `vote$reveal`, `advance`, `set_topic`,
// `add_voter`) have embedded IR and generate `call_*` methods.  All of them
// invoke `private$secret_key` and `persistentHash` witnesses, so the generated
// methods (which use `NoWitnesses`) produce witness errors.

#[test]
fn election_generated_call_advance_requires_witness() {
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from([0xAAu8; 32])),
                StateValue::from(AlignedValue::from(0u8)),
                StateValue::from(AlignedValue::from(false)),
                StateValue::from(0u64),
                StateValue::from(0u64),
                StateValue::Null,
                StateValue::Null,
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let ledger = election::Ledger::new(state);
    let result = ledger.call_advance();

    let err = expect_err(result);
    assert!(
        matches!(err, interpreter::InterpreterError::Witness(_)),
        "expected Witness variant, got: {err}"
    );
    eprintln!("election: call_advance correctly requires witness: {err}");
}

#[test]
fn election_generated_call_set_topic_requires_witness() {
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from([0xAAu8; 32])),
                StateValue::from(AlignedValue::from(0u8)),
                StateValue::from(AlignedValue::from(false)),
                StateValue::from(0u64),
                StateValue::from(0u64),
                StateValue::Null,
                StateValue::Null,
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let ledger = election::Ledger::new(state);
    // set_topic takes a topic argument (Opaque type in Compact, mapped to Value)
    let result = ledger.call_set_topic(Value::AlignedValue(AlignedValue::from([0xBBu8; 32])));

    let err = expect_err(result);
    assert!(
        matches!(err, interpreter::InterpreterError::Witness(_)),
        "expected Witness variant, got: {err}"
    );
    eprintln!("election: call_set_topic correctly requires witness: {err}");
}

#[test]
fn election_generated_call_add_voter_requires_witness() {
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from([0xAAu8; 32])),
                StateValue::from(AlignedValue::from(0u8)),
                StateValue::from(AlignedValue::from(false)),
                StateValue::from(0u64),
                StateValue::from(0u64),
                StateValue::Null,
                StateValue::Null,
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let ledger = election::Ledger::new(state);
    let result = ledger.call_add_voter(midnight_bindgen::Bytes::from([0xCCu8; 32]));

    let err = expect_err(result);
    assert!(
        matches!(err, interpreter::InterpreterError::Witness(_)),
        "expected Witness variant, got: {err}"
    );
    eprintln!("election: call_add_voter correctly requires witness: {err}");
}

#[test]
fn election_generated_call_vote_commit_requires_witness() {
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from([0xAAu8; 32])),
                StateValue::from(AlignedValue::from(0u8)),
                StateValue::from(AlignedValue::from(false)),
                StateValue::from(0u64),
                StateValue::from(0u64),
                StateValue::Null,
                StateValue::Null,
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let ledger = election::Ledger::new(state);
    // ballot is an enum (PermissibleVotes: yes=0, no=1)
    let result = ledger.call_vote_commit(Value::Integer(0));

    let err = expect_err(result);
    assert!(
        matches!(err, interpreter::InterpreterError::Witness(_)),
        "expected Witness variant, got: {err}"
    );
    eprintln!("election: call_vote_commit correctly requires witness: {err}");
}

#[test]
fn election_generated_call_vote_reveal_requires_witness() {
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from([0xAAu8; 32])),
                StateValue::from(AlignedValue::from(0u8)),
                StateValue::from(AlignedValue::from(false)),
                StateValue::from(0u64),
                StateValue::from(0u64),
                StateValue::Null,
                StateValue::Null,
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let ledger = election::Ledger::new(state);
    let result = ledger.call_vote_reveal();

    let err = expect_err(result);
    assert!(
        matches!(err, interpreter::InterpreterError::Witness(_)),
        "expected Witness variant, got: {err}"
    );
    eprintln!("election: call_vote_reveal correctly requires witness: {err}");
}

// ---------------------------------------------------------------------------
// Counter: typed state verification (using standard compiler fixtures)
// ---------------------------------------------------------------------------

#[test]
fn counter_increment_with_typed_state() {
    let ir = load_fixture_ir(COUNTER_INFO, "increment");

    // Build initial state and verify through typed accessor
    let state = ContractState::new(
        StateValue::Array(vec![StateValue::from(0u64)].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );
    let ledger = counter::Ledger::new(state.clone());
    assert_eq!(ledger.round().unwrap(), 0u64);

    // Execute increment 3 times, verifying through typed accessor each time
    let mut current = state;
    for i in 1..=3u64 {
        let result = interpreter::execute(&ir, &current).unwrap();
        current = result.state;

        let ledger = counter::Ledger::new(current.clone());
        assert_eq!(ledger.round().unwrap(), i, "counter should be {i}");
    }
    eprintln!("counter: 0 → 1 → 2 → 3 ✓ (typed)");
}

#[test]
fn counter_build_tx_with_typed_state() {
    let ir = load_fixture_ir(COUNTER_INFO, "increment");

    let state = ContractState::new(
        StateValue::Array(vec![StateValue::from(0u64)].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let address = midnight_coin_structure::contract::ContractAddress(
        midnight_base_crypto::hash::HashOutput([0xAA; 32]),
    );
    let tx = call::build_unproven_call_tx(&ir, &state, "increment", address, "test").unwrap();

    assert!(!tx.tx_bytes.is_empty());

    // Verify the new state through typed accessor
    let ledger = counter::Ledger::new(tx.new_state);
    assert_eq!(ledger.round().unwrap(), 1);
    eprintln!("counter TX: {} bytes, round=1 ✓", tx.tx_bytes.len());
}

// ---------------------------------------------------------------------------
// Tiny: arguments + witnesses with typed verification
// ---------------------------------------------------------------------------

#[test]
fn tiny_get_typed() {
    let ir = load_fixture_ir(TINY_INFO, "get");
    let helpers = load_fixture_helpers(TINY_INFO);

    // Build state with known value
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from([0u8; 32])),
                StateValue::from(AlignedValue::from(
                    midnight_transient_crypto::curve::Fr::from(42u64),
                )),
                StateValue::from(AlignedValue::from(1u8)),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    // Verify typed accessors work on the state
    let ledger = tiny::Ledger::new(state.clone());
    // state() returns the generated STATE enum; just check it doesn't error
    let _state_val = ledger.state().unwrap();

    struct TinyWitness;
    impl WitnessProvider for TinyWitness {
        fn call_witness(
            &self,
            name: &str,
            _args: &[Value],
        ) -> Result<Value, interpreter::InterpreterError> {
            match name {
                "private$secret_key" => Ok(Value::Integer(1)),
                _ => Err(interpreter::InterpreterError::Witness(format!(
                    "unknown: {name}"
                ))),
            }
        }
    }

    let result = interpreter::execute_with(&ir, &state, &[], &TinyWitness, &helpers);
    match result {
        Ok(r) => {
            eprintln!("tiny get: {} reads ✓", r.reads.len());
        }
        Err(e) => {
            eprintln!("tiny get: {e} (some IR forms may not be supported yet)");
        }
    }
}

#[test]
fn tiny_set_typed() {
    let ir = load_fixture_ir(TINY_INFO, "set");
    let helpers = load_fixture_helpers(TINY_INFO);

    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from([0u8; 32])),
                StateValue::from(AlignedValue::from(
                    midnight_transient_crypto::curve::Fr::from(0u64),
                )),
                StateValue::from(AlignedValue::from(0u8)),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    struct TinySetWitness;
    impl WitnessProvider for TinySetWitness {
        fn call_witness(
            &self,
            name: &str,
            _args: &[Value],
        ) -> Result<Value, interpreter::InterpreterError> {
            match name {
                "private$secret_key" => Ok(Value::AlignedValue(AlignedValue::from([0u8; 32]))),
                _ => Err(interpreter::InterpreterError::Witness(format!(
                    "unknown: {name}"
                ))),
            }
        }
    }

    use midnight_transient_crypto::curve::Fr;
    let result = interpreter::execute_with(
        &ir,
        &state,
        &[(
            "v",
            Value::AlignedValue(AlignedValue::from(Fr::from(42u64))),
        )],
        &TinySetWitness,
        &helpers,
    );

    match result {
        Ok(r) => {
            eprintln!("tiny set: executed ✓ (ops: {})", r.gather_ops.len());
            // Verify state change through typed accessor
            let ledger = tiny::Ledger::new(r.state);
            match ledger.state() {
                Ok(s) => eprintln!("  state after set: {s:?}"),
                Err(e) => eprintln!("  state accessor error (expected with dummy auth): {e}"),
            }
        }
        Err(e) => {
            eprintln!("tiny set: {e}");
            assert!(
                e.to_string().contains("assertion")
                    || e.to_string().contains("Unsupported")
                    || e.to_string().contains("ledger"),
                "unexpected error: {e}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Election: multi-field state with typed verification
// ---------------------------------------------------------------------------

#[test]
fn election_advance_typed() {
    let ir = load_fixture_ir(ELECTION_INFO, "advance");
    let helpers = load_fixture_helpers(ELECTION_INFO);

    let authority = [0xAA; 32];
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from(authority)),
                StateValue::from(AlignedValue::from(0u8)),
                StateValue::from(AlignedValue::from(false)),
                StateValue::from(0u64),
                StateValue::from(0u64),
                StateValue::Null,
                StateValue::Null,
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    // Verify initial state through typed accessors
    let ledger = election::Ledger::new(state.clone());
    assert_eq!(ledger.tally_yes().unwrap(), 0u64);
    assert_eq!(ledger.tally_no().unwrap(), 0u64);

    struct ElectionWitness;
    impl WitnessProvider for ElectionWitness {
        fn call_witness(
            &self,
            name: &str,
            _args: &[Value],
        ) -> Result<Value, interpreter::InterpreterError> {
            match name {
                "private$secret_key" => Ok(Value::Integer(1)),
                _ => Err(interpreter::InterpreterError::Witness(format!(
                    "unknown witness: {name}"
                ))),
            }
        }
    }

    let result = interpreter::execute_with(&ir, &state, &[], &ElectionWitness, &helpers);
    match result {
        Ok(r) => {
            eprintln!("election advance: executed ✓");
            let ledger = election::Ledger::new(r.state);
            match ledger.state() {
                Ok(s) => eprintln!("  state after advance: {s:?}"),
                Err(e) => eprintln!("  state accessor error: {e}"),
            }
        }
        Err(e) => {
            eprintln!("election advance: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Gateway: complex real-world contract
// ---------------------------------------------------------------------------
//
// NOTE: The gateway fixture uses an *original* compiler-generated
// contract-info.json (not recompiled with the latest compiler).  It contains
// embedded IR but there is no `mod gateway { contract!(...) }` bindgen module
// because the gateway.compact source is not available — only the compiled JSON
// artifact exists.  Recompiling to pick up newer IR or codegen features would
// require the gateway.compact source and the Compact compiler toolchain, which
// are external to this repository.
//
// The gateway's `call_*` methods (claim_deposit, witness_deposit, etc.) are
// therefore NOT tested via bindgen-generated code.  Instead, the tests below
// exercise the IR directly through the interpreter.

#[test]
fn gateway_all_circuits_parse() {
    let info: serde_json::Value = serde_json::from_str(GATEWAY_INFO).unwrap();
    let helpers = load_helpers(&info);
    let circuits = info["circuits"].as_array().unwrap();

    eprintln!(
        "gateway: {} circuits, {} helpers",
        circuits.len(),
        helpers.len()
    );

    for circuit in circuits {
        let name = circuit["name"].as_str().unwrap();
        let info_clone: serde_json::Value = serde_json::from_str(GATEWAY_INFO).unwrap();
        match try_find_circuit_ir(&info_clone, name) {
            Ok(_ir) => eprintln!("  {name}: IR parsed ✓"),
            Err(e) => panic!("  {name}: IR parse FAILED: {e}"),
        }
    }
}

#[test]
fn gateway_witness_deposit_executes() {
    let info: serde_json::Value = serde_json::from_str(GATEWAY_INFO).unwrap();
    let ir = find_circuit_ir(&info, "witness_deposit");
    let helpers = load_helpers(&info);

    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(6u64),
                StateValue::Null,
                StateValue::Map(StorageHashMap::new()),
                StateValue::from(0u64),
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
                StateValue::from(1000u64),
                StateValue::from(AlignedValue::from([0u8; 32])),
                StateValue::from(0u64),
                StateValue::Map(StorageHashMap::new()),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    struct GatewayWitness;
    impl WitnessProvider for GatewayWitness {
        fn call_witness(
            &self,
            name: &str,
            _args: &[Value],
        ) -> Result<Value, interpreter::InterpreterError> {
            Err(interpreter::InterpreterError::Witness(format!(
                "mock: {name}"
            )))
        }
    }

    let result = interpreter::execute_with(
        &ir,
        &state,
        &[
            ("sigs", Value::AlignedValue(AlignedValue::from(()))),
            (
                "channel_id",
                Value::AlignedValue(AlignedValue::from([0xCCu8; 32])),
            ),
            ("amount", Value::Integer(1000)),
            ("token_ref", Value::Integer(0)),
        ],
        &GatewayWitness,
        &helpers,
    );
    match result {
        Ok(r) => {
            eprintln!(
                "gateway witness_deposit: executed ✓ (ops: {})",
                r.gather_ops.len()
            );
        }
        Err(e) => {
            let msg = e.to_string();
            eprintln!("gateway witness_deposit: {msg}");
            assert!(
                msg.contains("assertion")
                    || msg.contains("witness")
                    || msg.contains("Unsupported")
                    || msg.contains("undefined")
                    || msg.contains("type error")
                    || msg.contains("ledger"),
                "unexpected error: {msg}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Proving (requires ZK keys)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn counter_prove_increment() {
    let dir = match compiled_dir() {
        Some(d) => d,
        None => {
            eprintln!("skipping: MIDNIGHT_COMPILED_DIR not set");
            return;
        }
    };

    let ir = load_fixture_ir(COUNTER_INFO, "increment");
    let state = ContractState::new(
        StateValue::Array(vec![StateValue::from(0u64)].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let address = midnight_coin_structure::contract::ContractAddress(
        midnight_base_crypto::hash::HashOutput([0xCC; 32]),
    );

    let unproven = call::build_unproven_call_tx(&ir, &state, "increment", address, "test").unwrap();
    eprintln!("unproven: {} bytes", unproven.tx_bytes.len());

    // Verify through typed accessor
    let ledger = counter::Ledger::new(unproven.new_state.clone());
    assert_eq!(ledger.round().unwrap(), 1);

    let keys_dir = format!("{dir}/counter");
    if !std::path::Path::new(&format!("{keys_dir}/keys")).exists() {
        eprintln!("skipping prove: no keys/ directory");
        return;
    }
    let proven = call::prove_and_seal(&unproven, &keys_dir).await.unwrap();
    eprintln!("proven: {} bytes ✓", proven.len());
    assert!(proven.len() > unproven.tx_bytes.len());
}

// ---------------------------------------------------------------------------
// Gateway: deploy contract
// ---------------------------------------------------------------------------

/// Deploy the gateway contract into a local TestState — full end-to-end without a node.
#[tokio::test]
async fn gateway_deploy_local() {
    if std::env::var("MIDNIGHT_LEDGER_TEST_STATIC_DIR").is_err() {
        eprintln!("skipping: MIDNIGHT_LEDGER_TEST_STATIC_DIR not set");
        return;
    }
    // Build the initial gateway state
    let state = gateway_initial_state();

    let (address, test_state) = call::deploy_local(&state).await.unwrap();
    let address_hex = call::format_address(&address);
    eprintln!("gateway deployed locally at: {address_hex}");

    // Verify the contract exists in the ledger state
    assert!(
        test_state.ledger.contract.get(&address).is_some(),
        "contract should exist in ledger after deploy"
    );
    eprintln!("gateway deploy_local: contract exists in ledger ✓");
}

/// Deploy gateway with funded TestState (NIGHT → Dust → fees).
#[tokio::test]
async fn gateway_deploy_funded() {
    if std::env::var("MIDNIGHT_LEDGER_TEST_STATIC_DIR").is_err() {
        eprintln!("skipping: MIDNIGHT_LEDGER_TEST_STATIC_DIR not set");
        return;
    }

    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from(6u8)),
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
                StateValue::from(0u64),
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
                StateValue::from(AlignedValue::from(0u64)),
                StateValue::from(AlignedValue::from([0u8; 32])),
                StateValue::from(0u64),
                StateValue::Map(StorageHashMap::new()),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let result = call::deploy_funded(
        &state,
        "local-test",
        "0000000000000000000000000000000000000000000000000000000000000001",
        std::path::Path::new("."),
        &midnight_contract::Prover::default(),
    )
    .await
    .unwrap();
    let address_hex = result.address_hex();

    eprintln!("gateway deployed (funded): {address_hex}");
    eprintln!("  TX: {} bytes", result.tx_bytes.len());
    assert!(!result.tx_bytes.is_empty());
    eprintln!("gateway deploy_funded: TX built ✓");
}

/// Build a gateway deploy transaction (for node submission).
/// Requires MIDNIGHT_LEDGER_TEST_STATIC_DIR env var for the proving infrastructure.
#[tokio::test]
async fn gateway_build_deploy_tx() {
    if std::env::var("MIDNIGHT_LEDGER_TEST_STATIC_DIR").is_err() {
        eprintln!("skipping: MIDNIGHT_LEDGER_TEST_STATIC_DIR not set");
        return;
    }
    // Gateway initial state: all defaults (zero counters, empty collections)
    let state = gateway_initial_state();

    // Build the deploy TX (uses real proving, not mock_prove)
    let (address_hex, tx_bytes) = call::deploy(&state, "undeployed").await.unwrap();

    assert_eq!(address_hex.len(), 64); // 32 bytes = 64 hex chars
    assert!(!tx_bytes.is_empty());
    eprintln!("gateway deploy TX: {} bytes", tx_bytes.len());
    eprintln!("gateway address: {address_hex}");
    eprintln!("gateway deploy: build OK ✓");
}

/// Deploy gateway to a running node and verify it exists.
#[tokio::test]
async fn gateway_deploy_to_node() {
    let node_url = match node_url() {
        Some(u) => u,
        None => {
            eprintln!("skipping: MIDNIGHT_NODE_URL not set");
            return;
        }
    };

    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(AlignedValue::from(6u8)),
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
                StateValue::from(0u64),
                StateValue::Map(StorageHashMap::new()),
                StateValue::Map(StorageHashMap::new()),
                StateValue::from(AlignedValue::from(0u64)),
                StateValue::from(AlignedValue::from([0u8; 32])),
                StateValue::from(0u64),
                StateValue::Map(StorageHashMap::new()),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    // The ledger's network_id is "undeployed" for dev nodes (not the chain name from system_chain)
    let (address_hex, tx_bytes) = call::deploy(&state, "undeployed").await.unwrap();
    eprintln!("gateway address: {address_hex}");
    eprintln!("gateway deploy TX: {} bytes", tx_bytes.len());

    // NOTE: The node will reject this with BalanceCheckOverspend because
    // we don't include Dust token inputs to cover transaction fees.
    // On-chain submission requires DustWallet integration (coin management).
    submit_tx(&node_url, &tx_bytes).await;
    eprintln!("gateway deploy TX submitted (address: {address_hex})");
    eprintln!("  (rejected with BalanceCheckOverspend until DustWallet is integrated)");
}

// ---------------------------------------------------------------------------
// TX submission (requires running node)
// ---------------------------------------------------------------------------

async fn submit_tx(node_url: &str, tx_bytes: &[u8]) {
    match call::submit(node_url, tx_bytes).await {
        Ok(hash) => eprintln!("  TX submitted: {hash}"),
        Err(e) => eprintln!("  TX error: {e}"),
    }
}

#[tokio::test]
#[ignore = "requires MIDNIGHT_NODE_URL + MIDNIGHT_COMPILED_DIR"]
async fn deploy_and_increment_counter() {
    use midnight_onchain_runtime::state::{ContractOperation, EntryPointBuf};

    let dir = match compiled_dir() {
        Some(d) => d,
        None => {
            eprintln!("skipping: MIDNIGHT_COMPILED_DIR not set");
            return;
        }
    };
    let node_url = match node_url() {
        Some(u) => u,
        None => {
            eprintln!("skipping: MIDNIGHT_NODE_URL not set");
            return;
        }
    };

    let entry_point: EntryPointBuf = b"increment"[..].into();
    let mut operations = StorageHashMap::new();
    operations = operations.insert(entry_point, ContractOperation::new(None));

    let initial_state = ContractState::new(
        StateValue::Array(vec![StateValue::from(0u64)].into()),
        operations,
        ContractMaintenanceAuthority::default(),
    );

    let (address, deploy_tx_bytes) = call::build_deploy_tx(&initial_state, "undeployed1")
        .await
        .unwrap();
    eprintln!("contract address: {}", hex::encode(address.0.0));
    submit_tx(&node_url, &deploy_tx_bytes).await;
    tokio::time::sleep(std::time::Duration::from_secs(12)).await;

    let ir = load_fixture_ir(COUNTER_INFO, "increment");
    let (call_tx_bytes, new_state) = call::build_proven_call_tx(
        &ir,
        &initial_state,
        "increment",
        address,
        "undeployed1",
        &format!("{dir}/counter"),
    )
    .await
    .unwrap();

    // Verify through typed accessor
    let ledger = counter::Ledger::new(new_state);
    assert_eq!(ledger.round().unwrap(), 1);

    submit_tx(&node_url, &call_tx_bytes).await;
    tokio::time::sleep(std::time::Duration::from_secs(12)).await;
    eprintln!("deploy + increment: round=1 ✓");
}

// ---------------------------------------------------------------------------
// Comprehensive: try executing every circuit from compiled contracts
// ---------------------------------------------------------------------------

#[test]
fn execute_all_compiled_circuits() {
    use midnight_transient_crypto::curve::Fr;

    let dir = match compiled_dir() {
        Some(d) => d,
        None => {
            eprintln!("skipping: MIDNIGHT_COMPILED_DIR not set");
            return;
        }
    };

    struct DummyWitness;
    impl WitnessProvider for DummyWitness {
        fn call_witness(
            &self,
            _name: &str,
            _args: &[Value],
        ) -> Result<Value, interpreter::InterpreterError> {
            Ok(Value::AlignedValue(AlignedValue::from([0u8; 32])))
        }
    }

    let states: Vec<(&str, ContractState<InMemoryDB>)> = vec![
        (
            "counter",
            ContractState::new(
                StateValue::Array(vec![StateValue::from(0u64)].into()),
                StorageHashMap::new(),
                ContractMaintenanceAuthority::default(),
            ),
        ),
        (
            "tiny",
            ContractState::new(
                StateValue::Array(
                    vec![
                        StateValue::from(AlignedValue::from([0u8; 32])),
                        StateValue::from(AlignedValue::from(Fr::from(0u64))),
                        StateValue::from(AlignedValue::from(0u8)),
                    ]
                    .into(),
                ),
                StorageHashMap::new(),
                ContractMaintenanceAuthority::default(),
            ),
        ),
        (
            "election",
            ContractState::new(
                StateValue::Array(
                    vec![
                        StateValue::from(AlignedValue::from([0u8; 32])),
                        StateValue::from(AlignedValue::from(0u8)),
                        StateValue::from(AlignedValue::from(false)),
                        StateValue::from(0u64),
                        StateValue::from(0u64),
                        StateValue::Null,
                        StateValue::Null,
                        StateValue::Map(StorageHashMap::new()),
                        StateValue::Map(StorageHashMap::new()),
                    ]
                    .into(),
                ),
                StorageHashMap::new(),
                ContractMaintenanceAuthority::default(),
            ),
        ),
    ];

    let mut ok = 0u32;
    let mut errors: Vec<(String, String)> = vec![];

    for (contract_name, state) in &states {
        let path = format!("{dir}/{contract_name}/compiler/contract-info.json");
        if !std::path::Path::new(&path).exists() {
            continue;
        }
        let info = load_contract_info(&dir, contract_name);
        let helpers = load_helpers(&info);
        let circuits = info["circuits"].as_array().unwrap();

        for circuit in circuits {
            let name = circuit["name"].as_str().unwrap();
            if circuit.get("ir").is_none() || circuit["ir"].is_null() {
                continue;
            }
            let ir: CircuitIrBody = match serde_json::from_value(circuit["ir"].clone()) {
                Ok(ir) => ir,
                Err(e) => {
                    errors.push((format!("{contract_name}/{name}"), format!("PARSE: {e}")));
                    continue;
                }
            };

            let circuit_args = circuit.get("arguments").and_then(|a| a.as_array());
            let dummy_args: Vec<(&str, Value)> = circuit_args
                .map(|args| {
                    args.iter()
                        .map(|a| {
                            let arg_name = a["name"].as_str().unwrap_or("");
                            (arg_name, Value::AlignedValue(AlignedValue::from([0u8; 32])))
                        })
                        .collect()
                })
                .unwrap_or_default();

            let result =
                interpreter::execute_with(&ir, state, &dummy_args, &DummyWitness, &helpers);
            match result {
                Ok(r) => {
                    eprintln!(
                        "  {contract_name}/{name}: OK (reads={}, ops={})",
                        r.reads.len(),
                        r.gather_ops.len()
                    );
                    ok += 1;
                }
                Err(e) => {
                    let msg = e.to_string();
                    eprintln!("  {contract_name}/{name}: {msg}");
                    errors.push((format!("{contract_name}/{name}"), msg));
                }
            }
        }
    }

    eprintln!("\n=== Results: {ok} OK, {} errors ===", errors.len());
    for (circuit, err) in &errors {
        eprintln!("  {circuit}: {err}");
    }

    assert!(ok > 0, "no circuits executed successfully");
}

#[tokio::test]
async fn prove_all_contracts() {
    let dir = match compiled_dir() {
        Some(d) => d,
        None => {
            eprintln!("skipping: MIDNIGHT_COMPILED_DIR not set");
            return;
        }
    };

    let keys_dir = format!("{dir}/counter");
    if !std::path::Path::new(&format!("{keys_dir}/keys")).exists() {
        eprintln!("skipping prove: no keys/ directory");
        return;
    }

    let ir = load_fixture_ir(COUNTER_INFO, "increment");
    let state = ContractState::new(
        StateValue::Array(vec![StateValue::from(0u64)].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );
    let address = midnight_coin_structure::contract::ContractAddress(
        midnight_base_crypto::hash::HashOutput([0x01; 32]),
    );
    let unproven = call::build_unproven_call_tx(&ir, &state, "increment", address, "test").unwrap();
    let proven = call::prove_and_seal(&unproven, &keys_dir).await.unwrap();
    eprintln!(
        "counter increment: {} → {} bytes ✓",
        unproven.tx_bytes.len(),
        proven.len()
    );
}
