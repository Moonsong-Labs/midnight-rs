//! End-to-end tests with real contracts using bindgen-generated types.
//!
//! These tests use `compact_bindgen::contract!` to generate typed ledger
//! structs, then execute circuits and verify state changes through typed
//! accessors — the same way application code would use the SDK.
//!
//! The submission tests need MIDNIGHT_NODE_URL and MIDNIGHT_INDEXER_URL.
//! Everything else runs against the interpreter fixtures of this crate and the
//! compiled devnet counter; see [`fixtures_dir`] and [`counter_zk_dir`] for the
//! two layouts and the variables that override them.

mod common;

use common::{epoch, with_witnesses};
use compact_bindgen::{AlignedValue, ContractState, InMemoryDB, StateValue, StorageHashMap};
use midnight_contract::ContractMaintenanceVerifyingKey;
use midnight_contract::interpreter;
use midnight_contract::runtime::{Value, WitnessOutcome, WitnessProvider};

use compact_codegen::types::ContractInfo;

// ---------------------------------------------------------------------------
// Bindgen-generated types — single analyzed-ir.sexp per contract has both
// typed ledger accessors and circuit IR for call methods.
// ---------------------------------------------------------------------------

mod counter {
    compact_bindgen::contract!("tests/fixtures/counter/compiler/analyzed-ir.sexp");
}

mod tiny {
    compact_bindgen::contract!("tests/fixtures/tiny/compiler/analyzed-ir.sexp");
}

mod election {
    compact_bindgen::contract!("tests/fixtures/election/compiler/analyzed-ir.sexp");
}

mod bboard {
    compact_bindgen::contract!("tests/fixtures/bboard/compiler/analyzed-ir.sexp");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Where the interpreter fixtures live: `<name>/compiler/analyzed-ir.sexp`
/// per contract. Defaults to this crate's own fixtures, which `make fixtures`
/// compiles.
fn fixtures_dir() -> String {
    common::env_dir("MIDNIGHT_COMPILED_DIR")
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures").to_string())
}

/// A compiled counter directory holding `keys/` and `zkir/`, which is a
/// different layout from [`fixtures_dir`] and cannot be served by the same
/// path. Defaults to the compiled devnet counter.
fn counter_zk_dir() -> String {
    common::env_dir("COUNTER_KEYED_DIR").unwrap_or_else(|| common::compiled_dir("counter"))
}

/// Parse an analyzed-ir.sexp artifact embedded at compile time.
fn load_info(analyzed_ir_text: &str) -> ContractInfo {
    compact_codegen::artifact::load_str(analyzed_ir_text).unwrap()
}

/// The counter ledger at `round`. The fixture states are built by hand, so a
/// test that reads one through the generated `Ledger` does not check the
/// generator against itself.
fn counter_state(round: u64) -> ContractState<InMemoryDB> {
    common::contract_state(vec![StateValue::from(round)])
}

/// The tiny ledger: a zero authority, `value`, and the STATE cell `state`.
fn tiny_state(value: u64, state: u8) -> ContractState<InMemoryDB> {
    common::contract_state(vec![
        StateValue::from(AlignedValue::from([0u8; 32])),
        StateValue::from(AlignedValue::from(
            midnight_transient_crypto::curve::Fr::from(value),
        )),
        StateValue::from(AlignedValue::from(state)),
    ])
}

/// The election ledger with `authority` and every other field at zero. The
/// two Merkle tree fields hold `Null`, where the generated builder writes
/// blank trees.
fn election_state(authority: [u8; 32]) -> ContractState<InMemoryDB> {
    common::contract_state(vec![
        StateValue::from(AlignedValue::from(authority)),
        StateValue::from(AlignedValue::from(0u8)),
        StateValue::from(AlignedValue::from(false)),
        StateValue::from(0u64),
        StateValue::from(0u64),
        StateValue::Null,
        StateValue::Null,
        StateValue::Map(StorageHashMap::new()),
        StateValue::Map(StorageHashMap::new()),
    ])
}

/// Answers the witness `name` with `key`, and every other witness with
/// `Unknown`.
struct SecretKeyWitness {
    name: &'static str,
    key: [u8; 32],
}

impl WitnessProvider for SecretKeyWitness {
    fn call_witness(
        &self,
        _ctx: &mut midnight_contract::runtime::WitnessContext<'_>,
        name: &str,
        _args: &[Value],
    ) -> Result<WitnessOutcome, midnight_contract::runtime::InterpreterError> {
        Ok(if name == self.name {
            WitnessOutcome::Value(Value::AlignedValue(AlignedValue::from(self.key)))
        } else {
            WitnessOutcome::Unknown
        })
    }
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

// Embed analyzed-ir.sexp at compile time for fixture-based tests
const COUNTER_INFO: &str = include_str!("fixtures/counter/compiler/analyzed-ir.sexp");
const TINY_INFO: &str = include_str!("fixtures/tiny/compiler/analyzed-ir.sexp");
const ELECTION_INFO: &str = include_str!("fixtures/election/compiler/analyzed-ir.sexp");
const BBOARD_INFO: &str = include_str!("fixtures/bboard/compiler/analyzed-ir.sexp");

// ---------------------------------------------------------------------------
// Counter: typed state verification (using standard compiler fixtures)
// ---------------------------------------------------------------------------

#[test]
fn counter_increment_with_typed_state() {
    let info = load_info(COUNTER_INFO);
    let ir = &common::exported_circuit(&info, "increment").def;
    let program = common::program(&info);

    // Build initial state and verify through typed accessor
    let state = counter_state(0);
    let ledger = counter::Ledger::new(state.clone());
    assert_eq!(ledger.round().unwrap(), 0u64);

    // Execute increment 3 times, verifying through typed accessor each time
    let mut current = state;
    for i in 1..=3u64 {
        let result = interpreter::execute(ir, &program, current, &[], epoch()).unwrap();
        current = result.state;

        let ledger = counter::Ledger::new(current.clone());
        assert_eq!(ledger.round().unwrap(), i, "counter should be {i}");
    }
    eprintln!("counter: 0 → 1 → 2 → 3 ✓ (typed)");
}

// ---------------------------------------------------------------------------
// Tiny: arguments + witnesses with typed verification
// ---------------------------------------------------------------------------

#[test]
fn tiny_get_typed() {
    let info = load_info(TINY_INFO);
    let ir = &common::exported_circuit(&info, "get").def;
    let program = common::program(&info);

    // Build state with known value
    let state = tiny_state(42, 1);

    // Verify typed accessors work on the state
    let ledger = tiny::Ledger::new(state.clone());
    // state() returns the generated STATE enum; just check it doesn't error
    let _state_val = ledger.state().unwrap();

    // The state cell is `set` (1), so `get()` must return `some(42)`:
    // the Maybe<Field> struct literal [is_some = true, value = 42].
    let r = interpreter::execute(ir, &program, state, &[], epoch()).expect("tiny get executes");
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
    let info = load_info(TINY_INFO);
    let ir = &common::exported_circuit(&info, "set").def;
    let program = common::program(&info);

    let state = tiny_state(0, 0);

    use midnight_transient_crypto::curve::Fr;
    // Enum variants resolve from the type's own variant list, so the circuit
    // and its program are all the interpreter needs.
    let result = interpreter::execute(
        ir,
        &program,
        state,
        &[(
            "v",
            Value::AlignedValue(AlignedValue::from(Fr::from(42u64))),
        )],
        with_witnesses(&SecretKeyWitness {
            name: "private$secret_key",
            key: [0; 32],
        }),
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
    let info = load_info(ELECTION_INFO);
    let ir = &common::exported_circuit(&info, "advance").def;
    let program = common::program(&info);

    let state = election_state([0xAA; 32]);

    // Verify initial state through typed accessors
    let ledger = election::Ledger::new(state.clone());
    assert_eq!(ledger.tally_yes().unwrap(), 0u64);
    assert_eq!(ledger.tally_no().unwrap(), 0u64);

    // `advance()` derives the caller's public key from the secret-key
    // witness and asserts it equals the stored authority. The witness here
    // is not the authority ([0xAA; 32]), so the circuit's own Compact
    // `assert` fires — a semantic outcome of the circuit logic, not an
    // interpreter gap. Accept exactly that assertion and nothing else.
    let witness = SecretKeyWitness {
        name: "private$secret_key",
        key: [1; 32],
    };
    let err = match interpreter::execute(ir, &program, state, &[], with_witnesses(&witness)) {
        Ok(_) => panic!("advance must fail the authorization assert"),
        Err(e) => e,
    };
    assert!(
        matches!(
            err,
            midnight_contract::runtime::InterpreterError::AssertionFailed(_)
        ),
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
// Bboard (`crates/midnight-contract/tests/fixtures/bboard/bboard.compact`)
// is a small generic bulletin-board contract from the compiler's own test
// corpus. It exercises a useful slice of SDK behaviour: a multi-circuit
// program with witness calls (`local_secret_key()`), a typed ledger
// (`STATE` enum + `Maybe<Opaque<"string">>` + `Counter` + `Bytes<32>`), and
// helper circuits inlined into the bodies. The fixture replaces the older
// gateway/MCS fixtures, which were tied to a project that has not landed yet.

/// The bboard `local_secret_key()` witness used by the post/take_down tests.
const BBOARD_SK: [u8; 32] = [1u8; 32];

const BBOARD_WITNESS: SecretKeyWitness = SecretKeyWitness {
    name: "local_secret_key",
    key: BBOARD_SK,
};

/// Independently compute bboard's `public_key(sk, instance as Field as
/// Bytes<32>)`: `persistentHash<Vector<3, Bytes<32>>>([pad(32, "bboard:pk:"),
/// instance, sk])`. The instance bytes are the counter value in little-endian
/// order (the `Field → Bytes<32>` cast, per the Compact runtime's
/// `convertFieldToBytes`), so this pins the interpreter's `field-to-bytes`
/// output: a wrong byte order or alignment changes the hash.
fn bboard_public_key(sk: [u8; 32], instance: u64) -> AlignedValue {
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
    common::persistent_hash(preimage)
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
    let info = load_info(BBOARD_INFO);
    let ir = &common::exported_circuit(&info, "post").def;
    let program = common::program(&info);

    let mut constructed =
        bboard::Simulator::new(bboard::LedgerInitialState::default(), std::time::UNIX_EPOCH);
    constructed
        .run_constructor()
        .expect("bboard's constructor runs");
    let state = constructed.ledger().into_contract_state();

    let new_message = [7u8; 32];
    let r = interpreter::execute(
        ir,
        &program,
        state,
        &[(
            "new_message",
            Value::AlignedValue(AlignedValue::from(new_message)),
        )],
        with_witnesses(&BBOARD_WITNESS),
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
    let info = load_info(BBOARD_INFO);
    let ir = &common::exported_circuit(&info, "take_down").def;
    let program = common::program(&info);

    let message = [7u8; 32];
    let occupied_message =
        AlignedValue::concat([AlignedValue::from(true), AlignedValue::from(message)].iter());
    // Post-`post` ledger state: occupied board whose poster matches the
    // witness key at instance 1.
    let state = common::contract_state(vec![
        StateValue::from(1u64),
        StateValue::from(occupied_message),
        StateValue::from(1u64),
        StateValue::from(bboard_public_key(BBOARD_SK, 1)),
    ]);

    let r = interpreter::execute(ir, &program, state, &[], with_witnesses(&BBOARD_WITNESS))
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

/// The generated `public_key` runs bboard's pure circuit in process and agrees
/// with the oracle, which hashes without the interpreter. `sk` and `instance`
/// differ, so arguments bound in the wrong order change the hash.
#[test]
fn bboard_pure_public_key_matches_the_oracle() {
    let mut instance = [0u8; 32];
    instance[..8].copy_from_slice(&1u64.to_le_bytes());

    let key = bboard::pure_circuits::public_key(BBOARD_SK.into(), instance.into())
        .expect("public_key runs in process");

    assert_eq!(
        AlignedValue::from(key.into_inner()),
        bboard_public_key(BBOARD_SK, 1)
    );
}

// ---------------------------------------------------------------------------
// Comprehensive: try executing every circuit from compiled contracts
// ---------------------------------------------------------------------------

#[test]
fn execute_all_compiled_circuits() {
    use midnight_contract::runtime::InterpreterError;

    /// Every circuit the three fixtures define. A fixture regenerated with a
    /// contract added or removed changes this.
    const EXPECTED_CIRCUITS: u32 = 10;

    let dir = fixtures_dir();

    struct DummyWitness;
    impl WitnessProvider for DummyWitness {
        fn call_witness(
            &self,
            _ctx: &mut midnight_contract::runtime::WitnessContext<'_>,
            _name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, midnight_contract::runtime::InterpreterError> {
            Ok(WitnessOutcome::Value(Value::AlignedValue(
                AlignedValue::from([0u8; 32]),
            )))
        }
    }

    let states: Vec<(&str, ContractState<InMemoryDB>)> = vec![
        ("counter", counter_state(0)),
        ("tiny", tiny_state(0, 0)),
        ("election", election_state([0; 32])),
    ];

    let mut ok = 0u32;
    let mut rejected = 0u32;
    let mut errors: Vec<(String, String)> = vec![];

    for (contract_name, state) in &states {
        let path = format!("{dir}/{contract_name}/compiler/analyzed-ir.sexp");
        if !std::path::Path::new(&path).exists() {
            continue;
        }
        let info = common::load_compiled(&format!("{dir}/{contract_name}"));
        let program = common::program(&info);

        for circuit in &info.circuits {
            let name = circuit.name.as_str();

            let dummy_args: Vec<(&str, Value)> = circuit
                .arguments()
                .iter()
                .map(|a| {
                    (
                        a.name.name(),
                        Value::AlignedValue(AlignedValue::from([0u8; 32])),
                    )
                })
                .collect();

            let result = interpreter::execute(
                &circuit.def,
                &program,
                state.clone(),
                &dummy_args,
                with_witnesses(&DummyWitness),
            );
            match result {
                Ok(r) => {
                    eprintln!(
                        "  {contract_name}/{name}: OK (reads={}, ops={})",
                        r.reads.len(),
                        r.gather_ops.len()
                    );
                    ok += 1;
                }
                // A circuit rejecting `[0u8; 32]` arguments through its own
                // `assert` is the contract working, so it is not a finding
                // here. Anything else is the interpreter failing to execute.
                Err(InterpreterError::AssertionFailed(msg)) => {
                    eprintln!("  {contract_name}/{name}: rejected dummy args: {msg}");
                    rejected += 1;
                }
                Err(e) => {
                    let msg = e.to_string();
                    eprintln!("  {contract_name}/{name}: {msg}");
                    errors.push((format!("{contract_name}/{name}"), msg));
                }
            }
        }
    }

    eprintln!(
        "\n=== Results: {ok} executed, {rejected} rejected dummy args, {} errors ===",
        errors.len()
    );
    for (circuit, err) in &errors {
        eprintln!("  {circuit}: {err}");
    }

    assert!(
        errors.is_empty(),
        "the interpreter failed to execute {} circuit(s): {errors:?}",
        errors.len()
    );
    assert_eq!(
        ok + rejected,
        EXPECTED_CIRCUITS,
        "expected {EXPECTED_CIRCUITS} circuits across the fixtures, reached {}",
        ok + rejected
    );
}
// ---------------------------------------------------------------------------
// Governance: deploy with a maintenance authority, then apply both kinds of
// maintenance update to it.
// ---------------------------------------------------------------------------

/// Exercises the governance path end to end: deploy with a 1-of-1 committee
/// (`with_maintenance_authority`), rotate a verifier key by removing and
/// re-inserting it in one atomic signed transaction, then rotate the committee
/// itself via `replace_authority`. Each update is prepared, signed with the
/// current committee key, and submitted. No key is stored by the SDK.
///
/// One deploy serves both updates. Two deploys from the dev seed race: a build
/// only avoids inputs its own wallet reserved, awaiting a deploy returns at the
/// best block rather than at finalization, and a second wallet syncing before
/// the indexer surfaces the first spend re-selects it. That is ledger custom
/// error 196, `DustDoubleSpend`. Sharing one provider across two
/// `#[tokio::test]`s does not work either: each test owns its runtime, so the
/// first runtime's shutdown drops the node connection the second would use.
///
/// Requires a devnet + indexer (MIDNIGHT_NODE_URL, MIDNIGHT_INDEXER_URL).
#[tokio::test]
async fn governance_deploy_then_apply_both_updates() {
    use midnight_contract::{Contract, SigningKey};
    use midnight_onchain_runtime::state::EntryPointBuf;

    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = midnight_provider::MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let keys_dir = counter_zk_dir();
    let vk_bytes = std::fs::read(format!("{keys_dir}/keys/increment.verifier"))
        .expect("read increment.verifier");
    let initial = counter_state(0);

    // The caller owns the committee signing key; the SDK only learns its public
    // half.
    let authority = SigningKey::sample(rand::thread_rng());
    let contract = Contract::deploy(&provider)
        .with_initial_state(initial)
        .with_zk_config(&keys_dir)
        .with_maintenance_authority(vec![authority.verifying_key()], 1)
        .await
        .expect("deploy with maintenance authority");
    let address = contract.address().to_string();
    eprintln!("deployed governable contract at {address}");

    // On-chain authority is the 1-of-1 committee we set, at counter 0.
    let on_chain = contract.maintenance_authority().await.unwrap();
    assert_eq!(
        on_chain.committee,
        vec![ContractMaintenanceVerifyingKey::Schnorr(
            authority.verifying_key()
        )]
    );
    assert_eq!(on_chain.threshold, 1);
    assert_eq!(on_chain.counter, 0);

    // Batch maintenance: `with_zk_config` loaded the `increment` verifier key at
    // deploy, so it is defined. Rotate it, remove + insert in one signed update.
    let (batch, _) = contract
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

    let rotated = midnight_contract::state::fetch_state(contract.provider(), &address, None)
        .await
        .unwrap();
    let increment: EntryPointBuf = b"increment"[..].into();
    assert!(
        rotated.operations.contains_key(&increment),
        "increment should still be defined after remove+insert"
    );
    assert_eq!(
        rotated.maintenance_authority.counter, 1,
        "one maintenance update applied → counter 1"
    );
    eprintln!("governance: verifier key rotated in one batched tx ✓");

    // Rotate the committee itself. The original key is still the authority, so
    // it signs this one too.
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

    let updated = contract.maintenance_authority().await.unwrap();
    assert_eq!(
        updated.committee,
        vec![ContractMaintenanceVerifyingKey::Schnorr(new_vk)],
        "committee should be the new key"
    );
    assert_eq!(
        updated.counter, 2,
        "a second maintenance update → counter 2"
    );
    eprintln!("governance: authority rotated on-chain ✓");

    let pinned = Contract::at(&provider, &address)
        .at_block(batch.block_hash)
        .build()
        .maintenance_authority()
        .await
        .expect("read the authority pinned at the batch's block");
    assert_eq!(
        pinned.counter, 1,
        "a read pinned at the batch's block must not see the later replace_authority"
    );
}
