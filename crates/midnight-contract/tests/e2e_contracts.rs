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
use midnight_contract::interpreter::{self, Value, WitnessOutcome, WitnessProvider};

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

mod bboard {
    midnight_bindgen::contract!("tests/fixtures/bboard/compiler/contract-info.json");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compiled_dir() -> Option<String> {
    std::env::var("MIDNIGHT_COMPILED_DIR").ok()
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

fn load_structs(info: &serde_json::Value) -> Vec<compact_codegen::ir::StructDef> {
    info["structs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|s| serde_json::from_value(s.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn load_fixture_structs(contract_info_json: &str) -> Vec<compact_codegen::ir::StructDef> {
    let info: serde_json::Value = serde_json::from_str(contract_info_json).unwrap();
    load_structs(&info)
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
const BBOARD_INFO: &str = include_str!("fixtures/bboard/compiler/contract-info.json");

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
    let tx = call::build_unproven_call_tx(
        &ir,
        &state,
        "increment",
        address,
        "test",
        &[],
        &midnight_contract::interpreter::NoWitnesses,
        None,
        &[],
    )
    .unwrap();

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
    let structs = load_fixture_structs(TINY_INFO);

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
            _ctx: &mut interpreter::WitnessContext<'_>,
            name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, interpreter::InterpreterError> {
            match name {
                "private$secret_key" => Ok(WitnessOutcome::Value(Value::Integer(1))),
                _ => Ok(WitnessOutcome::Unknown),
            }
        }
    }

    // The state cell is `set` (1), so `get()` must return `some(42)`:
    // the Maybe<Field> struct literal [is_some = true, value = 42].
    let r = interpreter::execute_with(&ir, &state, &[], &TinyWitness, &helpers, &structs)
        .expect("tiny get executes");
    let expected = AlignedValue::concat(
        [
            AlignedValue::from(true),
            AlignedValue::from(midnight_transient_crypto::curve::Fr::from(42u64)),
        ]
        .iter(),
    );
    match r.result {
        Some(Value::AlignedValue(av)) => assert_eq!(av, expected, "get() must return some(42)"),
        other => panic!("expected an AlignedValue result, got {other:?}"),
    }
    eprintln!("tiny get: returned some(42) ✓ ({} reads)", r.reads.len());
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
            _ctx: &mut interpreter::WitnessContext<'_>,
            name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, interpreter::InterpreterError> {
            match name {
                "private$secret_key" => Ok(WitnessOutcome::Value(Value::AlignedValue(
                    AlignedValue::from([0u8; 32]),
                ))),
                _ => Ok(WitnessOutcome::Unknown),
            }
        }
    }

    use midnight_transient_crypto::curve::Fr;
    let enums: Vec<compact_codegen::ir::EnumDef> =
        serde_json::from_str(tiny::Ledger::__ENUMS_JSON).unwrap();
    let result = interpreter::execute_with_enums(
        &ir,
        &state,
        &[(
            "v",
            Value::AlignedValue(AlignedValue::from(Fr::from(42u64))),
        )],
        &TinySetWitness,
        &helpers,
        &[],
        &enums,
    );

    let r = result.expect("tiny set executes");
    eprintln!("tiny set: executed ✓ (ops: {})", r.gather_ops.len());

    // Verify the state change through typed accessors: set(42) writes the
    // value and flips the STATE cell from unset to set.
    let ledger = tiny::Ledger::new(r.state);
    let state_after = ledger.state().expect("read STATE cell");
    assert_eq!(
        state_after,
        tiny::STATE::Set,
        "set() must move the contract to STATE.set"
    );
    let value_after = ledger.value().expect("read value cell");
    assert_eq!(
        AlignedValue::from(value_after),
        AlignedValue::from(Fr::from(42u64)),
        "set(42) must record the value"
    );
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
            _ctx: &mut interpreter::WitnessContext<'_>,
            name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, interpreter::InterpreterError> {
            match name {
                "private$secret_key" => Ok(WitnessOutcome::Value(Value::Integer(1))),
                _ => Ok(WitnessOutcome::Unknown),
            }
        }
    }

    // `advance()` derives the caller's public key from the secret-key
    // witness and asserts it equals the stored authority. The witness here
    // is not the authority ([0xAA; 32]), so the circuit's own Compact
    // `assert` fires — a semantic outcome of the circuit logic, not an
    // interpreter gap. Accept exactly that assertion and nothing else.
    let err = match interpreter::execute_with(&ir, &state, &[], &ElectionWitness, &helpers, &[]) {
        Ok(_) => panic!("advance must fail the authorization assert"),
        Err(e) => e,
    };
    assert!(
        matches!(err, interpreter::InterpreterError::AssertionFailed(_)),
        "expected the circuit's own assert, got {err:?}"
    );
    assert!(
        err.to_string().contains("without authorization"),
        "unexpected assertion message: {err}"
    );
}

// ---------------------------------------------------------------------------
// Bboard: multi-circuit contract with witnesses
// ---------------------------------------------------------------------------
//
// Bboard (`tools/compact-compiler/test-center/test-contracts/bboard.compact`)
// is a small generic bulletin-board contract from the compiler's own test
// corpus. It exercises a useful slice of SDK behaviour: a multi-circuit
// program with witness calls (`local_secret_key()`), a typed ledger
// (`STATE` enum + `Maybe<Opaque<"string">>` + `Counter` + `Bytes<32>`), and
// helper circuits inlined into the bodies. The fixture replaces the older
// gateway/MCS fixtures, which were tied to a project that has not landed yet.

#[test]
fn bboard_all_circuits_parse() {
    let info: serde_json::Value = serde_json::from_str(BBOARD_INFO).unwrap();
    let helpers = load_helpers(&info);
    let circuits = info["circuits"].as_array().unwrap();

    eprintln!(
        "bboard: {} circuits, {} helpers",
        circuits.len(),
        helpers.len()
    );

    for circuit in circuits {
        let name = circuit["name"].as_str().unwrap();
        // The compiler emits `ir: null` for pure circuits (no on-chain body
        // — they get inlined or dispatched via call-pure). Only assert that
        // every non-pure circuit's IR parses.
        if circuit["pure"].as_bool().unwrap_or(false) {
            eprintln!("  {name}: pure (no IR) ✓");
            continue;
        }
        let info_clone: serde_json::Value = serde_json::from_str(BBOARD_INFO).unwrap();
        match try_find_circuit_ir(&info_clone, name) {
            Ok(_ir) => eprintln!("  {name}: IR parsed ✓"),
            Err(e) => panic!("  {name}: IR parse FAILED: {e}"),
        }
    }
}

/// The bboard `local_secret_key()` witness used by the post/take_down tests.
const BBOARD_SK: [u8; 32] = [1u8; 32];

struct BboardWitness;
impl WitnessProvider for BboardWitness {
    fn call_witness(
        &self,
        _ctx: &mut interpreter::WitnessContext<'_>,
        name: &str,
        _args: &[Value],
    ) -> Result<WitnessOutcome, interpreter::InterpreterError> {
        match name {
            "local_secret_key" => Ok(WitnessOutcome::Value(Value::AlignedValue(
                AlignedValue::from(BBOARD_SK),
            ))),
            _ => Ok(WitnessOutcome::Unknown),
        }
    }
}

/// Independently compute bboard's `public_key(sk, instance as Field as
/// Bytes<32>)`: `persistentHash<Vector<3, Bytes<32>>>([pad(32, "bboard:pk:"),
/// instance, sk])`. The instance bytes are the counter value in little-endian
/// order (the `Field → Bytes<32>` cast, per the Compact runtime's
/// `convertFieldToBytes`), so this pins the interpreter's `field-to-bytes`
/// output: a wrong byte order or alignment changes the hash.
fn bboard_public_key(sk: [u8; 32], instance: u64) -> AlignedValue {
    use midnight_base_crypto::hash::PersistentHashWriter;
    use midnight_base_crypto::repr::BinaryHashRepr;
    use midnight_transient_crypto::fab::ValueReprAlignedValue;

    let prefix = b"bboard:pk:";
    let mut pad = [0u8; 32];
    pad[..prefix.len()].copy_from_slice(prefix);
    let mut instance_bytes = [0u8; 32];
    instance_bytes[..8].copy_from_slice(&instance.to_le_bytes());

    let preimage = AlignedValue::concat(
        [
            AlignedValue::from(pad),
            AlignedValue::from(instance_bytes),
            AlignedValue::from(sk),
        ]
        .iter(),
    );
    let mut hasher = PersistentHashWriter::default();
    ValueReprAlignedValue(preimage).binary_repr(&mut hasher);
    AlignedValue::from(hasher.finalize().0)
}

/// Read a cell of the bboard state array as an `AlignedValue`.
fn bboard_cell(state: &ContractState<InMemoryDB>, index: usize) -> AlignedValue {
    match state.data.get_ref() {
        StateValue::Array(arr) => match arr.get(index).expect("cell index") {
            StateValue::Cell(sp) => (**sp).clone(),
            other => panic!("expected Cell at index {index}, got {other:?}"),
        },
        other => panic!("expected Array root, got {other:?}"),
    }
}

#[test]
fn bboard_post_executes() {
    // `post(new_message)` derives the poster's public key from the
    // `local_secret_key()` witness and the instance counter cast through
    // `instance as Field as Bytes<32>` — the compiler-emitted
    // `field-to-bytes` IR form — then occupies the board.
    //
    // The state checks below read raw cells (`bboard_cell`) on purpose: the
    // oracle pins the exact FAB encoding, including alignment, that the
    // interpreter writes; a typed decode would launder encoding bugs.
    let info: serde_json::Value = serde_json::from_str(BBOARD_INFO).unwrap();
    let ir = find_circuit_ir(&info, "post");
    let helpers = load_helpers(&info);
    let structs = load_structs(&info);

    // Post-constructor ledger state: state = vacant (0), message = none,
    // instance Counter at 1, poster = [0; 32].
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(0u64),
                StateValue::Null,
                StateValue::from(1u64),
                StateValue::from(AlignedValue::from([0u8; 32])),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let new_message = [7u8; 32];
    let r = interpreter::execute_with(
        &ir,
        &state,
        &[(
            "new_message",
            Value::AlignedValue(AlignedValue::from(new_message)),
        )],
        &BboardWitness,
        &helpers,
        &structs,
    )
    .expect("bboard post executes");
    eprintln!("bboard post: executed ✓ (ops: {})", r.gather_ops.len());

    // state = STATE.occupied (1).
    let state_cell = bboard_cell(&r.state, 0);
    assert_eq!(
        u64::try_from(&*state_cell.value).expect("state cell"),
        1,
        "post() must move the board to STATE.occupied"
    );

    // message = some(new_message): Maybe [is_some = true, value].
    let message_cell = bboard_cell(&r.state, 1);
    let expected_message =
        AlignedValue::concat([AlignedValue::from(true), AlignedValue::from(new_message)].iter());
    assert_eq!(
        message_cell, expected_message,
        "post() must record some(new_message)"
    );

    // poster = public_key(sk, instance as Field as Bytes<32>) with the
    // instance counter at 1 — the field-to-bytes output feeds this hash.
    let poster_cell = bboard_cell(&r.state, 3);
    assert_eq!(
        poster_cell,
        bboard_public_key(BBOARD_SK, 1),
        "post() must store the poster public key derived through field-to-bytes"
    );
}

#[test]
fn bboard_take_down_executes() {
    // `take_down()` recomputes the poster's public key (the second
    // compiler-emitted `field-to-bytes` site) and asserts it equals the
    // stored poster, then vacates the board and returns the old message.
    //
    // As in `bboard_post_executes`, the state checks read raw cells on
    // purpose so the oracle pins the exact FAB encoding (including
    // alignment) instead of laundering it through a typed decode.
    let info: serde_json::Value = serde_json::from_str(BBOARD_INFO).unwrap();
    let ir = find_circuit_ir(&info, "take_down");
    let helpers = load_helpers(&info);
    let structs = load_structs(&info);

    let message = [7u8; 32];
    let occupied_message =
        AlignedValue::concat([AlignedValue::from(true), AlignedValue::from(message)].iter());
    // Post-`post` ledger state: occupied board whose poster matches the
    // witness key at instance 1.
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(1u64),
                StateValue::from(occupied_message),
                StateValue::from(1u64),
                StateValue::from(bboard_public_key(BBOARD_SK, 1)),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let r = interpreter::execute_with(&ir, &state, &[], &BboardWitness, &helpers, &structs)
        .expect("bboard take_down executes");
    eprintln!("bboard take_down: executed ✓ (ops: {})", r.gather_ops.len());

    // Returns the former message.
    match r.result {
        Some(Value::AlignedValue(av)) => {
            assert_eq!(
                av,
                AlignedValue::from(message),
                "take_down() must return the former message"
            );
        }
        other => panic!("expected an AlignedValue result, got {other:?}"),
    }

    // state = STATE.vacant (0), instance incremented to 2.
    let state_cell = bboard_cell(&r.state, 0);
    assert_eq!(
        u64::try_from(&*state_cell.value).expect("state cell"),
        0,
        "take_down() must vacate the board"
    );
    let instance_cell = bboard_cell(&r.state, 2);
    assert_eq!(
        u64::try_from(&*instance_cell.value).expect("instance cell"),
        2,
        "take_down() must increment the instance counter"
    );

    // message = none<Opaque<"string">>(): Maybe [is_some = false, value = ""]
    // — the default opaque string is one empty Compress-aligned atom.
    use midnight_base_crypto::fab;
    let empty_opaque = fab::AlignedValue::new(
        fab::Value(vec![fab::ValueAtom(Vec::new())]),
        fab::Alignment::singleton(fab::AlignmentAtom::Compress),
    )
    .unwrap();
    let expected_none = AlignedValue::concat([AlignedValue::from(false), empty_opaque].iter());
    assert_eq!(
        bboard_cell(&r.state, 1),
        expected_none,
        "take_down() must clear the message to none"
    );
}

// ---------------------------------------------------------------------------
// Proving (requires ZK keys)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Deploy: funded + with shielded offer
// ---------------------------------------------------------------------------
//
// These tests exercise the SDK's deploy plumbing (NIGHT → Dust → fees, with
// or without a hand-built shielded offer). The contract state they hand in
// is opaque to the deploy path; we use a bboard-shaped state so the test
// stays generic and tracks a contract whose source is committed alongside.

/// Deploy with funded TestState (NIGHT → Dust → fees).
#[tokio::test]
async fn deploy_funded() {
    if std::env::var("MIDNIGHT_LEDGER_TEST_STATIC_DIR").is_err() {
        eprintln!("skipping: MIDNIGHT_LEDGER_TEST_STATIC_DIR not set");
        return;
    }

    // Bboard post-constructor shape: state=vacant, message=none,
    // instance counter at 1, poster = [0; 32]. The deploy path doesn't
    // interpret these — any well-formed initial state would do.
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(0u64),
                StateValue::Null,
                StateValue::from(1u64),
                StateValue::from(AlignedValue::from([0u8; 32])),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let node_url = match std::env::var("MIDNIGHT_NODE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("skipping: MIDNIGHT_NODE_URL not set");
            return;
        }
    };
    let indexer_url = match std::env::var("MIDNIGHT_INDEXER_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("skipping: MIDNIGHT_INDEXER_URL not set");
            return;
        }
    };

    let seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap();
    let provider = midnight_provider::MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider construction")
        .sync_wallet(seed, midnight_provider::Network::Undeployed)
        .await
        .expect("indexer sync should succeed");

    let result = midnight_contract::deploy::deploy_funded(
        &state,
        &provider,
        std::path::Path::new("."),
        &midnight_contract::Prover::default(),
        None,
    )
    .await
    .unwrap();
    let address_hex = result.address_hex();

    eprintln!("deployed (funded): {address_hex}");
    eprintln!("  TX: {} bytes", result.tx_bytes.len());
    assert!(!result.tx_bytes.is_empty());
    eprintln!("deploy_funded: TX built ✓");
}

/// Deploy with a hand-built shielded offer attached.
///
/// Pins the Feature 2 plumbing: a caller-supplied `OfferInfo` reaches
/// `set_guaranteed_offer` instead of the hardcoded empty offer the SDK used
/// before. The offer self-transfers 1 unit of shielded token id `[0; 32]`
/// from the dev wallet back to its own shielded address — structurally valid
/// but unrelated to the contract being deployed (no effects-check
/// interaction). We stop at build (no submit) because the dev devnet's
/// pre-allocated shielded tokens have chain-side transfer restrictions; see
/// the parallel `build_shielded_transfer_arbitrary_token_id` test in
/// midnight-wallet for the same rationale.
#[tokio::test]
async fn deploy_funded_with_shielded_offer() {
    if std::env::var("MIDNIGHT_LEDGER_TEST_STATIC_DIR").is_err() {
        eprintln!("skipping: MIDNIGHT_LEDGER_TEST_STATIC_DIR not set");
        return;
    }
    let node_url = match std::env::var("MIDNIGHT_NODE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("skipping: MIDNIGHT_NODE_URL not set");
            return;
        }
    };
    let indexer_url = match std::env::var("MIDNIGHT_INDEXER_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("skipping: MIDNIGHT_INDEXER_URL not set");
            return;
        }
    };

    // Same bboard-shaped initial state as `deploy_funded` — opaque to the
    // deploy path.
    let state = ContractState::new(
        StateValue::Array(
            vec![
                StateValue::from(0u64),
                StateValue::Null,
                StateValue::from(1u64),
                StateValue::from(AlignedValue::from([0u8; 32])),
            ]
            .into(),
        ),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap();
    let provider = midnight_provider::MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider construction")
        .sync_wallet(seed.clone(), midnight_provider::Network::Undeployed)
        .await
        .expect("indexer sync should succeed");

    // Build a 1-unit self-transfer of the dev devnet's default shielded
    // token id ([0; 32]). The dev wallet holds this at genesis.
    let recipient_addr =
        midnight_wallet::address::derive_shielded(&seed, midnight_provider::Network::Undeployed);
    let recipient = midnight_contract::parse_shielded_recipient(&recipient_addr).unwrap();
    let token_type = midnight_contract::ShieldedTokenType(midnight_helpers::HashOutput([0u8; 32]));
    let input = midnight_contract::InputInfo {
        origin: seed.clone(),
        token_type,
        value: 1,
        nullifier: None,
    };
    let output: midnight_contract::OutputInfo<
        midnight_contract::ShieldedWallet<midnight_contract::DefaultDB>,
    > = midnight_contract::OutputInfo {
        destination: recipient,
        token_type,
        value: 1,
    };
    let offer = midnight_contract::OfferInfo {
        inputs: vec![Box::new(input)],
        outputs: vec![Box::new(output)],
        transients: vec![],
    };

    let result = midnight_contract::deploy::deploy_funded(
        &state,
        &provider,
        std::path::Path::new("."),
        &midnight_contract::Prover::default(),
        Some(offer),
    )
    .await
    .unwrap();

    assert!(!result.tx_bytes.is_empty());
    eprintln!(
        "deploy_funded with shielded offer: addr={} bytes={} ✓",
        result.address_hex(),
        result.tx_bytes.len(),
    );
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
            _ctx: &mut interpreter::WitnessContext<'_>,
            _name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, interpreter::InterpreterError> {
            Ok(WitnessOutcome::Value(Value::AlignedValue(
                AlignedValue::from([0u8; 32]),
            )))
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
                interpreter::execute_with(&ir, state, &dummy_args, &DummyWitness, &helpers, &[]);
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

// ---------------------------------------------------------------------------
// Governance: deploy with a maintenance authority, then rotate it.
// ---------------------------------------------------------------------------

/// Exercises the governance path end to end: deploy with a 1-of-1 committee
/// (`with_maintenance_authority`), then rotate the authority to a fresh key via
/// `replace_authority` — preparing the update, signing it with the current
/// committee key, and submitting. No key is stored by the SDK.
///
/// Requires a devnet + indexer + compiled counter keys
/// (MIDNIGHT_NODE_URL, MIDNIGHT_INDEXER_URL, MIDNIGHT_COMPILED_DIR).
#[tokio::test]
async fn governance_deploy_then_replace_authority() {
    use midnight_contract::{Contract, SigningKey};

    let (node_url, indexer_url, compiled) = match (
        std::env::var("MIDNIGHT_NODE_URL").ok(),
        std::env::var("MIDNIGHT_INDEXER_URL").ok(),
        compiled_dir(),
    ) {
        (Some(n), Some(i), Some(c)) => (n, i, c),
        _ => {
            eprintln!(
                "skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL + MIDNIGHT_COMPILED_DIR"
            );
            return;
        }
    };

    let seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap();
    let provider = midnight_provider::MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider construction")
        .sync_wallet(seed, midnight_provider::Network::Undeployed)
        .await
        .expect("indexer sync should succeed");

    let keys_dir = format!("{compiled}/counter");
    let initial = ContractState::new(
        StateValue::Array(vec![StateValue::from(0u64)].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    // The caller owns the committee signing key; the SDK only learns its public
    // half. Deploy consumes the provider (the builder needs a `'static`
    // provider); access it afterwards via `contract.provider()`.
    let authority = SigningKey::sample(rand::thread_rng());
    let contract = Contract::deploy(provider)
        .with_initial_state(initial)
        .with_zk_keys(&keys_dir)
        .with_maintenance_authority(vec![authority.verifying_key()], 1)
        .await
        .expect("deploy with maintenance authority");
    let address = contract.address().to_string();
    eprintln!("deployed governable contract at {address}");

    // On-chain authority is the 1-of-1 committee we set, at counter 0.
    let on_chain = contract.maintenance_authority().await.unwrap();
    assert_eq!(on_chain.committee, vec![authority.verifying_key()]);
    assert_eq!(on_chain.threshold, 1);
    assert_eq!(on_chain.counter, 0);

    // Rotate the authority to a fresh committee: prepare the update, sign it
    // with the current authority at index 0, and submit.
    let new_authority = SigningKey::sample(rand::thread_rng());
    let new_vk = new_authority.verifying_key();
    contract
        .maintenance()
        .replace_authority(vec![new_vk.clone()], 1)
        .prepare()
        .await
        .expect("prepare replace_authority")
        .sign(0, &authority)
        .await
        .expect("submit replace_authority")
        .wait_best()
        .await
        .expect("replace_authority included in best block");

    // On-chain committee is now the new key, counter incremented.
    let updated = contract.maintenance_authority().await.unwrap();
    assert_eq!(
        updated.committee,
        vec![new_vk],
        "committee should be the new key"
    );
    assert_eq!(
        updated.counter, 1,
        "counter should increment after a maintenance update"
    );
    eprintln!("governance: authority rotated on-chain ✓");
}

/// Batch maintenance: rotate a verifier key by removing then re-inserting it in
/// a single atomic, single-signed transaction.
///
/// Requires a devnet + indexer + compiled counter keys.
#[tokio::test]
async fn governance_batch_rotate_verifier_key() {
    use midnight_contract::{Contract, SigningKey};
    use midnight_onchain_runtime::state::EntryPointBuf;

    let (node_url, indexer_url, compiled) = match (
        std::env::var("MIDNIGHT_NODE_URL").ok(),
        std::env::var("MIDNIGHT_INDEXER_URL").ok(),
        compiled_dir(),
    ) {
        (Some(n), Some(i), Some(c)) => (n, i, c),
        _ => {
            eprintln!(
                "skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL + MIDNIGHT_COMPILED_DIR"
            );
            return;
        }
    };

    let seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap();
    let provider = midnight_provider::MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider construction")
        .sync_wallet(seed, midnight_provider::Network::Undeployed)
        .await
        .expect("indexer sync should succeed");

    let keys_dir = format!("{compiled}/counter");
    let vk_bytes = std::fs::read(format!("{keys_dir}/keys/increment.verifier"))
        .expect("read increment.verifier");
    let initial = ContractState::new(
        StateValue::Array(vec![StateValue::from(0u64)].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let authority = SigningKey::sample(rand::thread_rng());
    let contract = Contract::deploy(provider)
        .with_initial_state(initial)
        .with_zk_keys(&keys_dir)
        .with_maintenance_authority(vec![authority.verifying_key()], 1)
        .await
        .expect("deploy with maintenance authority");
    let address = contract.address().to_string();

    // `with_zk_keys` loaded the `increment` verifier key at deploy, so it is
    // defined. Rotate it: remove + insert in one signed update.
    contract
        .maintenance()
        .remove_verifier_key("increment")
        .insert_verifier_key("increment", vk_bytes)
        .prepare()
        .await
        .expect("prepare batch")
        .sign(0, &authority)
        .await
        .expect("submit batch")
        .wait_best()
        .await
        .expect("batch included in best block");

    let updated =
        midnight_contract::state::fetch_state_from_node(contract.provider(), &address, None)
            .await
            .unwrap();
    let increment: EntryPointBuf = b"increment"[..].into();
    assert!(
        updated.operations.contains_key(&increment),
        "increment should still be defined after remove+insert"
    );
    assert_eq!(
        updated.maintenance_authority.counter, 1,
        "one maintenance update applied → counter 1"
    );
    eprintln!("governance: verifier key rotated in one batched tx ✓");
}
