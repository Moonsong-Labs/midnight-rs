//! Tests that run real contracts: in the interpreter, through a generated pure
//! circuit, and on a devnet.
//!
//! The governance test needs MIDNIGHT_NODE_URL and MIDNIGHT_INDEXER_URL. It
//! deploys the compiled devnet counter, see [`counter_zk_dir`]. The other tests
//! read the interpreter fixtures of this crate.

mod common;

use common::with_witnesses;
use compact_bindgen::{AlignedValue, ContractState, InMemoryDB, StateValue, StorageHashMap};
use midnight_contract::ContractMaintenanceVerifyingKey;
use midnight_contract::interpreter;
use midnight_contract::runtime::{Value, WitnessOutcome, WitnessProvider};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A compiled counter directory that holds `keys/` and `zkir/`. Defaults to
/// the compiled devnet counter.
fn counter_zk_dir() -> String {
    common::env_dir("COUNTER_KEYED_DIR").unwrap_or_else(|| common::compiled_dir("counter"))
}

/// The counter ledger at `round`.
fn counter_state(round: u64) -> ContractState<InMemoryDB> {
    common::contract_state(vec![StateValue::from(round)])
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

// ---------------------------------------------------------------------------
// Bboard: the pure `public_key` circuit
// ---------------------------------------------------------------------------

mod bboard {
    compact_bindgen::contract!("tests/fixtures/bboard/compiler/analyzed-ir.sexp");
}

/// A bboard poster's secret key.
const BBOARD_SK: [u8; 32] = [1u8; 32];

/// Independently compute bboard's `public_key(sk, instance as Field as
/// Bytes<32>)`: `persistentHash<Vector<3, Bytes<32>>>([pad(32, "bboard:pk:"),
/// instance, sk])`. The instance bytes are the counter value in little-endian
/// order, as the Compact runtime's `convertFieldToBytes` casts a `Field` to
/// `Bytes<32>`.
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
// Election: every circuit runs in the interpreter
// ---------------------------------------------------------------------------

#[test]
fn every_election_circuit_runs_without_an_interpreter_error() {
    use midnight_contract::runtime::InterpreterError;

    /// Every circuit that the election fixture exports. A fixture regenerated
    /// with a circuit added or removed changes this.
    const EXPECTED_CIRCUITS: u32 = 5;

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

    let info = common::load_compiled(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/election"
    ));
    let program = common::program(&info);
    let state = election_state([0; 32]);

    let mut ok = 0u32;
    let mut rejected = 0u32;
    let mut errors: Vec<(String, String)> = vec![];

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
                    "  election/{name}: OK (reads={}, ops={})",
                    r.reads.len(),
                    r.gather_ops.len()
                );
                ok += 1;
            }
            // A circuit rejecting `[0u8; 32]` arguments through its own
            // `assert` is the contract working, so it is not a finding
            // here. Anything else is the interpreter failing to execute.
            Err(InterpreterError::AssertionFailed(msg)) => {
                eprintln!("  election/{name}: rejected dummy args: {msg}");
                rejected += 1;
            }
            Err(e) => {
                let msg = e.to_string();
                eprintln!("  election/{name}: {msg}");
                errors.push((format!("election/{name}"), msg));
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
        "expected {EXPECTED_CIRCUITS} circuits in the election fixture, reached {}",
        ok + rejected
    );
}

// ---------------------------------------------------------------------------
// Governance: deploy with a maintenance authority, then apply both kinds of
// maintenance update to it.
// ---------------------------------------------------------------------------

/// Exercises the governance path end to end: deploy with a 1-of-1 Schnorr
/// committee (`with_maintenance_authority`), rotate a verifier key by removing
/// and re-inserting it in one atomic signed transaction, rotate the committee
/// itself to an ECDSA key via `replace_authority`, then rotate the verifier key
/// again under the ECDSA key. Each update is prepared, signed with the current
/// committee key, and submitted. No key is stored by the SDK.
///
/// One deploy serves every update. Two deploys from the dev seed race: a build
/// only avoids inputs its own wallet reserved, `into_contract` returns at the
/// best block rather than at finalization, and a second wallet syncing before
/// the indexer surfaces the first spend re-selects it. That is ledger custom
/// error 196, `DustDoubleSpend`. Sharing one provider across two
/// `#[tokio::test]`s does not work either: each test owns its runtime, so the
/// first runtime's shutdown drops the node connection the second would use.
///
/// Requires a devnet + indexer (MIDNIGHT_NODE_URL, MIDNIGHT_INDEXER_URL).
#[tokio::test]
async fn governance_applies_updates_under_each_kind_of_committee() {
    use midnight_contract::{Contract, ContractMaintenanceSigningKey, ecdsa, schnorr};
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
    let authority_key = schnorr::SigningKey::sample(rand::thread_rng());
    let member = ContractMaintenanceVerifyingKey::Schnorr(authority_key.verifying_key());
    let authority = ContractMaintenanceSigningKey::Schnorr(authority_key);
    let contract = Contract::deploy(&provider)
        .with_initial_state(initial)
        .with_zk_config(&keys_dir)
        .with_maintenance_authority(vec![member.clone()], 1)
        .send()
        .await
        .expect("submit deploy")
        .into_contract()
        .await
        .expect("deploy with maintenance authority");
    let address = contract.address().to_string();
    eprintln!("deployed governable contract at {address}");

    // On-chain authority is the 1-of-1 committee we set, at counter 0.
    let on_chain = contract.maintenance_authority().await.unwrap();
    assert_eq!(on_chain.committee, vec![member]);
    assert_eq!(on_chain.threshold, 1);
    assert_eq!(on_chain.counter, 0);

    // Batch maintenance: `with_zk_config` loaded the `increment` verifier key at
    // deploy, so it is defined. Rotate it, remove + insert in one signed update.
    let batch = contract
        .maintenance()
        .remove_verifier_key("increment")
        .insert_verifier_key("increment", vk_bytes.clone())
        .prepare()
        .await
        .expect("prepare batch")
        .sign(0, &authority)
        .send()
        .await
        .expect("submit batch")
        .get_receipt()
        .await
        .expect("batch applied");

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

    // Rotate the committee itself, to an ECDSA key. The original key is still
    // the authority, so it signs this one too.
    let new_authority_key = ecdsa::SigningKey::sample(rand::thread_rng());
    let new_member = ContractMaintenanceVerifyingKey::ECDSA(new_authority_key.verifying_key());
    let new_authority = ContractMaintenanceSigningKey::ECDSA(new_authority_key);
    contract
        .maintenance()
        .replace_authority(vec![new_member.clone()], 1)
        .prepare()
        .await
        .expect("prepare replace_authority")
        .sign(0, &authority)
        .send()
        .await
        .expect("submit replace_authority")
        .get_receipt()
        .await
        .expect("replace_authority applied");

    let updated = contract.maintenance_authority().await.unwrap();
    assert_eq!(
        updated.committee,
        vec![new_member],
        "committee should be the new key"
    );
    assert_eq!(
        updated.counter, 2,
        "a second maintenance update → counter 2"
    );
    eprintln!("governance: authority rotated on-chain ✓");

    // The ECDSA committee signs the next update.
    contract
        .maintenance()
        .remove_verifier_key("increment")
        .insert_verifier_key("increment", vk_bytes)
        .prepare()
        .await
        .expect("prepare the ECDSA-signed batch")
        .sign(0, &new_authority)
        .send()
        .await
        .expect("submit the ECDSA-signed batch")
        .get_receipt()
        .await
        .expect("ECDSA-signed batch applied");
    assert_eq!(
        contract.maintenance_authority().await.unwrap().counter,
        3,
        "a third maintenance update → counter 3"
    );
    eprintln!("governance: ECDSA committee signed an update ✓");

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
