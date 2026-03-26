//! Circuit call transaction builder.
//!
//! Wires the IR interpreter output to midnight-ledger's transaction
//! construction pipeline: interpreter → partition → intent → transaction.

use std::borrow::Cow;

use midnight_base_crypto::time::{Duration, Timestamp};
use midnight_bindgen::{AlignedValue, ContractState, InMemoryDB};
use midnight_coin_structure::contract::ContractAddress;
use midnight_ledger::construct::ContractCallPrototype;
use midnight_ledger::structure::INITIAL_PARAMETERS;
use midnight_onchain_runtime::state::{ContractOperation, EntryPointBuf};
use midnight_serialize::tagged_serialize;
use midnight_transient_crypto::proofs::KeyLocation;

use crate::interpreter;
use compact_codegen::ir::CircuitIrBody;

/// Error during circuit call transaction construction.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("interpreter error: {0}")]
    Interpreter(#[from] interpreter::InterpreterError),

    #[error("transaction construction failed: {0}")]
    Construction(String),

    #[error("serialization failed: {0}")]
    Serialization(String),
}

/// The signature type used in Midnight transactions.
pub type Sig = midnight_base_crypto::signatures::Signature;

/// Type alias for the unproven transaction object.
pub type UnprovenTransaction = midnight_ledger::structure::Transaction<
    Sig,
    midnight_ledger::structure::ProofPreimageMarker,
    midnight_transient_crypto::commitment::PedersenRandomness,
    InMemoryDB,
>;

/// Result of building an unproven circuit call transaction.
pub struct UnprovenCallTx {
    /// Serialized transaction bytes (tagged-serialized).
    pub tx_bytes: Vec<u8>,
    /// The transaction object (for proving).
    pub transaction: UnprovenTransaction,
    /// The updated contract state after circuit execution.
    pub new_state: ContractState<InMemoryDB>,
}

/// Build an unproven transaction from a circuit IR body and contract state.
///
/// This is the main entry point for circuit calls. It:
/// 1. Executes the circuit IR against the contract state
/// 2. Converts gather-mode Ops to verify-mode with popeq results
/// 3. Partitions into guaranteed/fallible transcripts
/// 4. Builds ContractCallPrototype, Intent, and Transaction
/// 5. Serializes via `tagged_serialize`
///
/// The returned transaction has `ProofPreimageMarker` — call
/// [`prove_and_seal`] to generate ZK proofs before submission.
pub fn build_unproven_call_tx(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    network_id: &str,
) -> Result<UnprovenCallTx, CallError> {
    use midnight_ledger::structure::{Intent, Transaction};
    use midnight_storage::storage::HashMap as StorageHashMap;
    use rand::Rng;

    let mut rng = rand::thread_rng();

    // Step 1: Execute the circuit IR
    let exec_result = interpreter::execute(ir, state)?;

    // Step 2: Convert gather ops to verify ops and build transcripts
    let entry_point: EntryPointBuf = circuit_name.as_bytes().into();

    // Convert gather-mode ops to verify-mode by filling in popeq results.
    // This mirrors midnight-ledger's `program_with_results` pattern:
    // translate gather → verify, then filter out empty Idx/Ins ops.
    let mut read_iter = exec_result.reads.iter();
    let verify_ops: Vec<
        midnight_onchain_runtime::ops::Op<
            midnight_onchain_runtime::result_mode::ResultModeVerify,
            InMemoryDB,
        >,
    > = exec_result
        .gather_ops
        .iter()
        .map(|op| {
            op.clone().translate(|()| {
                read_iter
                    .next()
                    .cloned()
                    .unwrap_or_else(|| AlignedValue::from(()))
            })
        })
        .filter(|op| match op {
            midnight_onchain_runtime::ops::Op::Idx { path, .. } => !path.is_empty(),
            midnight_onchain_runtime::ops::Op::Ins { n, .. } => *n != 0,
            _ => true,
        })
        .collect();

    // Build PreTranscript and partition into guaranteed/fallible
    let address_for_ctx = contract_address;
    let context =
        midnight_onchain_runtime::context::QueryContext::new(state.data.clone(), address_for_ctx);
    let pre_transcript = midnight_ledger::construct::PreTranscript {
        context,
        program: verify_ops,
        comm_comm: None,
    };

    let partitioned =
        midnight_ledger::construct::partition_transcripts(&[pre_transcript], &INITIAL_PARAMETERS)
            .map_err(|e| CallError::Construction(format!("partition failed: {e:?}")))?;

    let (guaranteed, fallible) = partitioned.into_iter().next().unwrap_or((None, None));

    // For void circuits (no input/output), use empty AlignedValues
    let input: AlignedValue = ().into();
    let output: AlignedValue = ().into();

    // Get the ContractOperation from the deployed state if available
    let op = state
        .operations
        .get(&entry_point)
        .map(|sp| (*sp).clone())
        .unwrap_or_else(|| ContractOperation::new(None));

    let call = ContractCallPrototype {
        address: contract_address,
        entry_point,
        op,
        input,
        output,
        guaranteed_public_transcript: guaranteed,
        fallible_public_transcript: fallible,
        private_transcript_outputs: vec![],
        communication_commitment_rand: rng.r#gen(),
        key_location: KeyLocation(Cow::Owned(circuit_name.to_string())),
    };

    // Step 3: Build Intent
    let ttl = Timestamp::from_secs(0) + Duration::from_secs(3600);

    let intent: Intent<Sig, _, _, InMemoryDB> = Intent::new(
        &mut rng,
        None, // no guaranteed unshielded offer
        None, // no fallible unshielded offer
        vec![call],
        Vec::new(), // no maintenance updates
        Vec::new(), // no deploys
        None,       // no dust actions
        ttl,
    );

    // Step 4: Build Transaction (no coins)
    let mut intents = StorageHashMap::new();
    intents = intents.insert(0u16, intent);

    let tx: UnprovenTransaction = Transaction::from_intents(network_id, intents);

    // Step 5: Serialize
    let mut bytes = Vec::new();
    tagged_serialize(&tx, &mut bytes).map_err(|e| CallError::Serialization(e.to_string()))?;

    Ok(UnprovenCallTx {
        tx_bytes: bytes,
        transaction: tx,
        new_state: exec_result.state,
    })
}

/// Prove and seal a transaction using midnight-ledger's test utilities.
///
/// `keys_dir` is the compiler output directory containing `keys/` and `zkir/`.
/// Uses `test_resolver` from midnight-ledger which downloads ZK params on demand.
///
/// Returns serialized proven + sealed transaction bytes.
pub async fn prove_and_seal(
    unproven: &UnprovenCallTx,
    keys_dir: &str,
) -> Result<Vec<u8>, CallError> {
    use rand::SeedableRng;

    // Set the env var that test_resolver reads.
    // SAFETY: This is unsound in multi-threaded contexts. This function
    // should only be called from single-threaded test contexts. Production
    // code should use a custom resolver that doesn't rely on env vars.
    unsafe { std::env::set_var("MIDNIGHT_LEDGER_TEST_STATIC_DIR", keys_dir) };

    let resolver = midnight_ledger::test_utilities::test_resolver("");
    let rng = rand::rngs::StdRng::from_entropy();

    let proven =
        midnight_ledger::test_utilities::tx_prove_bind(rng, &unproven.transaction, &resolver)
            .await
            .map_err(|e| CallError::Construction(format!("proving failed: {e:?}")))?;

    let mut bytes = Vec::new();
    tagged_serialize(&proven, &mut bytes).map_err(|e| CallError::Serialization(e.to_string()))?;

    Ok(bytes)
}

/// Build a deploy transaction for a contract with the given initial state.
///
/// Returns the contract address and the tagged-serialized transaction bytes
/// ready for `send_mn_transaction`.
pub fn build_deploy_tx(
    initial_state: &ContractState<InMemoryDB>,
    network_id: &str,
) -> Result<(ContractAddress, Vec<u8>), CallError> {
    use midnight_ledger::structure::ContractDeploy;
    use midnight_ledger::structure::{Intent, Transaction};

    let mut rng = rand::thread_rng();

    let deploy = ContractDeploy::new(&mut rng, initial_state.clone());
    let address = deploy.address();

    let ttl = Timestamp::from_secs(0) + Duration::from_secs(3600);

    let intent: Intent<Sig, _, _, InMemoryDB> = Intent::new(
        &mut rng,
        None,
        None,
        Vec::new(),   // no calls
        Vec::new(),   // no updates
        vec![deploy], // deploy
        None,
        ttl,
    );

    let mut intents = midnight_storage::storage::HashMap::new();
    intents = intents.insert(0u16, intent);

    let tx: UnprovenTransaction = Transaction::from_intents(network_id, intents);

    // Deploy TXs have no contract calls — mock_prove works for these.
    let proven = tx
        .mock_prove()
        .map_err(|e| CallError::Construction(format!("mock prove: {e:?}")))?;

    let mut bytes = Vec::new();
    tagged_serialize(&proven, &mut bytes).map_err(|e| CallError::Serialization(e.to_string()))?;

    Ok((address, bytes))
}

/// Build a proven call transaction ready for submission.
///
/// Returns the tagged-serialized proven transaction bytes (for `send_mn_transaction`)
/// and the updated contract state.
pub async fn build_proven_call_tx(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    network_id: &str,
    keys_dir: &str,
) -> Result<(Vec<u8>, ContractState<InMemoryDB>), CallError> {
    let unproven = build_unproven_call_tx(ir, state, circuit_name, contract_address, network_id)?;
    let proven_bytes = prove_and_seal(&unproven, keys_dir).await?;
    Ok((proven_bytes, unproven.new_state))
}

/// Build a JSON envelope for debugging/logging purposes.
///
/// Note: This is NOT used for node submission. The `send_mn_transaction`
/// pallet extrinsic accepts raw tagged-serialized bytes directly.
pub fn build_tx_envelope(tx: &UnprovenCallTx, seconds_since_epoch: u64) -> Vec<u8> {
    use sha2::{Digest, Sha256};

    let tx_hex = hex::encode(&tx.tx_bytes);
    let tx_hash = hex::encode(Sha256::digest(&tx.tx_bytes));

    let envelope = serde_json::json!({
        "tx": { "Midnight": tx_hex },
        "context": {
            "secondsSinceEpoch": seconds_since_epoch,
            "secondsSinceEpochErr": 30,
            "parentBlockHash": "0000000000000000000000000000000000000000000000000000000000000000",
            "lastBlockTime": seconds_since_epoch
        },
        "tx_hash": tx_hash
    });

    serde_json::to_vec(&envelope).expect("JSON serialization should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_bindgen::{ContractMaintenanceAuthority, StateValue, StorageHashMap};

    fn make_counter_state(round: u64) -> ContractState<InMemoryDB> {
        let root = StateValue::Array(vec![StateValue::from(round)].into());
        ContractState::new(
            root,
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }

    #[test]
    fn build_counter_increment_tx() {
        let state = make_counter_state(0);

        let ir_json = r#"{
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
        }"#;

        let ir: CircuitIrBody = serde_json::from_str(ir_json).expect("parse IR");
        let address = ContractAddress(midnight_base_crypto::hash::HashOutput([0xAA; 32]));

        let result = build_unproven_call_tx(&ir, &state, "increment", address, "test-network")
            .expect("build tx");

        // Transaction bytes should be non-empty
        assert!(
            !result.tx_bytes.is_empty(),
            "transaction bytes should not be empty"
        );
        eprintln!("unproven TX size: {} bytes", result.tx_bytes.len());

        // The new state should have counter = 1
        let root = result.new_state.data.get_ref();
        match root {
            StateValue::Array(arr) => {
                let cell = arr.get(0).expect("field 0");
                match cell {
                    StateValue::Cell(sp) => {
                        let counter = u64::try_from(&*sp.value).expect("u64");
                        assert_eq!(counter, 1);
                    }
                    _ => panic!("expected Cell"),
                }
            }
            _ => panic!("expected Array"),
        }
    }
}
