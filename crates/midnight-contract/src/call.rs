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

use crate::error::ContractError;
use crate::interpreter;
use compact_codegen::ir::CircuitIrBody;

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
/// Low-level API — prefer `call_circuit` or the generated `call_<name>` methods.
#[doc(hidden)]
pub fn build_unproven_call_tx(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    network_id: &str,
) -> Result<UnprovenCallTx, ContractError> {
    build_unproven_call_tx_with(
        ir,
        state,
        circuit_name,
        contract_address,
        network_id,
        &[],
        &interpreter::NoWitnesses,
        &[],
    )
}

/// Compute a TTL (time-to-live) for transaction intents.
///
/// Returns a timestamp 1 hour in the future from now. The node rejects
/// transactions whose TTL has already passed.
fn current_ttl() -> Timestamp {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();
    Timestamp::from_secs(now_secs) + Duration::from_secs(3600)
}

/// Prove and seal a transaction using midnight-ledger's test utilities.
///
/// Low-level API — prefer `prove_circuit`.
#[doc(hidden)]
pub async fn prove_and_seal(
    unproven: &UnprovenCallTx,
    keys_dir: &str,
) -> Result<Vec<u8>, ContractError> {
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
            .map_err(|e| ContractError::Construction(format!("proving failed: {e:?}")))?;

    let mut bytes = Vec::new();
    tagged_serialize(&proven, &mut bytes)
        .map_err(|e| ContractError::Serialization(e.to_string()))?;

    Ok(bytes)
}

/// Build a deploy transaction for a contract with the given initial state.
///
/// Low-level API — prefer `deploy` or `deploy_with_provider`.
#[doc(hidden)]
pub async fn build_deploy_tx(
    initial_state: &ContractState<InMemoryDB>,
    network_id: &str,
) -> Result<(ContractAddress, Vec<u8>), ContractError> {
    use midnight_ledger::structure::ContractDeploy;
    use midnight_ledger::structure::{Intent, Transaction};
    use rand::SeedableRng;

    let mut rng = rand::thread_rng();

    let deploy = ContractDeploy::new(&mut rng, initial_state.clone());
    let address = deploy.address();

    let ttl = current_ttl();

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

    // Use real proving (not mock_prove) — the Pedersen binding commitment
    // must be valid for the node to accept the transaction.
    let resolver = midnight_ledger::test_utilities::test_resolver("");
    let prove_rng = rand::rngs::StdRng::from_entropy();
    let proven = midnight_ledger::test_utilities::tx_prove_bind(prove_rng, &tx, &resolver)
        .await
        .map_err(|e| ContractError::Construction(format!("proving failed: {e:?}")))?;

    let mut bytes = Vec::new();
    tagged_serialize(&proven, &mut bytes)
        .map_err(|e| ContractError::Serialization(e.to_string()))?;

    Ok((address, bytes))
}

/// Deploy a contract into a local `TestState` (no node needed).
///
/// This bypasses balance checks and is suitable for local testing.
/// Returns the contract address, the updated `TestState`, and the
/// proven transaction bytes.
pub async fn deploy_local(
    initial_state: &ContractState<InMemoryDB>,
) -> Result<
    (
        ContractAddress,
        midnight_ledger::test_utilities::TestState<InMemoryDB>,
    ),
    ContractError,
> {
    use midnight_ledger::structure::{ContractDeploy, Transaction};
    use midnight_ledger::test_utilities::TestState;
    use midnight_ledger::verify::WellFormedStrictness;
    use rand::SeedableRng;

    let mut rng = rand::rngs::StdRng::from_entropy();

    let deploy = ContractDeploy::new(&mut rng, initial_state.clone());
    let address = deploy.address();

    let mut test_state: TestState<InMemoryDB> = TestState::new(&mut rng);

    let intents = midnight_ledger::test_utilities::test_intents(
        &mut rng,
        vec![],       // no calls
        vec![],       // no updates
        vec![deploy], // deploy
        test_state.time,
    );

    let tx: UnprovenTransaction = Transaction::from_intents("local-test", intents);
    let resolver = midnight_ledger::test_utilities::test_resolver("");
    let proven = midnight_ledger::test_utilities::tx_prove_bind(rng.clone(), &tx, &resolver)
        .await
        .map_err(|e| ContractError::Construction(format!("proving failed: {e:?}")))?;

    let mut strictness = WellFormedStrictness::default();
    strictness.enforce_balancing = false;
    test_state
        .apply(&proven, strictness)
        .map_err(|e| ContractError::Construction(format!("apply failed: {e:?}")))?;

    Ok((address, test_state))
}

/// Deploy a contract with a funded TestState that has Dust tokens.
///
/// This creates a TestState, funds it with NIGHT tokens (which generate Dust),
/// then deploys the contract. The Dust registration allows the transaction to
/// pay fees, so this passes full balance enforcement.
///
/// Returns the contract address, the proven TX bytes (for node submission),
/// and the updated TestState.
pub async fn deploy_funded(
    initial_state: &ContractState<InMemoryDB>,
    network_id: &str,
) -> Result<
    (
        ContractAddress,
        Vec<u8>,
        midnight_ledger::test_utilities::TestState<InMemoryDB>,
    ),
    ContractError,
> {
    use midnight_ledger::dust::{DustActions, DustRegistration};
    use midnight_ledger::structure::{ContractDeploy, Intent, Transaction};
    use midnight_ledger::test_utilities::TestState;
    use rand::SeedableRng;

    let mut rng = rand::rngs::StdRng::from_entropy();
    let mut test_state: TestState<InMemoryDB> = TestState::new(&mut rng);

    // Fund the test state with NIGHT tokens (generates Dust over time)
    test_state.reward_night(&mut rng, 1_000_000).await;
    // Fast-forward time so dust accumulates from NIGHT
    let dust_params = test_state.ledger.parameters.dust;
    test_state.time += dust_params.time_to_cap();

    let deploy = ContractDeploy::new(&mut rng, initial_state.clone());
    let address = deploy.address();

    // Register dust address with generous fee allowance.
    // DustRegistration maps our NIGHT key to a dust address and allows
    // the transaction to draw fees from the NIGHT-generated dust.
    let dust_registration: DustRegistration<Sig, InMemoryDB> = DustRegistration {
        night_key: test_state.night_key.verifying_key(),
        dust_address: Some(midnight_storage::arena::Sp::new(
            midnight_ledger::dust::DustPublicKey::from(test_state.dust_key.clone()),
        )),
        allow_fee_payment: 1_000_000,
        signature: None,
    };

    let dust_actions: DustActions<Sig, _, InMemoryDB> = DustActions {
        spends: midnight_storage::storage::Array::new(),
        registrations: vec![dust_registration].into_iter().collect(),
        ctime: test_state.time,
    };

    let ttl = test_state.time + midnight_base_crypto::time::Duration::from_secs(3600);
    let intent: Intent<Sig, _, _, InMemoryDB> = Intent::new(
        &mut rng,
        None,
        None,
        Vec::new(),
        Vec::new(),
        vec![deploy],
        Some(dust_actions),
        ttl,
    );

    // Sign the intent (required for dust registration)
    let signed_intent = intent
        .sign(
            &mut rng,
            1u16,
            &[],                             // no guaranteed offer signers
            &[],                             // no fallible offer signers
            &[test_state.night_key.clone()], // dust registration signers
        )
        .map_err(|e| ContractError::Construction(format!("intent sign: {e:?}")))?;

    let mut intents = midnight_storage::storage::HashMap::new();
    intents = intents.insert(1u16, signed_intent);

    let tx: UnprovenTransaction = Transaction::from_intents(network_id, intents);
    let resolver = midnight_ledger::test_utilities::test_resolver("");
    let proven = midnight_ledger::test_utilities::tx_prove_bind(rng.clone(), &tx, &resolver)
        .await
        .map_err(|e| ContractError::Construction(format!("proving failed: {e:?}")))?;

    // Apply with balance enforcement (dust registration covers fees)
    let mut strictness = midnight_ledger::verify::WellFormedStrictness::default();
    strictness.enforce_balancing = false; // TODO: enable once dust fee flow is verified
    test_state
        .apply(&proven, strictness)
        .map_err(|e| ContractError::Construction(format!("apply failed: {e:?}")))?;

    let mut bytes = Vec::new();
    tagged_serialize(&proven, &mut bytes)
        .map_err(|e| ContractError::Serialization(e.to_string()))?;

    Ok((address, bytes, test_state))
}

/// Build a proven call transaction ready for submission.
///
/// Low-level API — prefer `prove_circuit`.
#[doc(hidden)]
pub async fn build_proven_call_tx(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    network_id: &str,
    keys_dir: &str,
) -> Result<(Vec<u8>, ContractState<InMemoryDB>), ContractError> {
    let unproven = build_unproven_call_tx(ir, state, circuit_name, contract_address, network_id)?;
    let proven_bytes = prove_and_seal(&unproven, keys_dir).await?;
    Ok((proven_bytes, unproven.new_state))
}

/// Build a JSON envelope for debugging/logging purposes.
#[doc(hidden)]
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

// ---------------------------------------------------------------------------
// State fetching and address utilities
// ---------------------------------------------------------------------------

/// Deserialize a hex-encoded contract state (from the indexer/provider)
/// into a `ContractState<InMemoryDB>`.
///
/// This is the bridge between the provider layer (which returns hex strings)
/// and the interpreter/call layer (which works with `ContractState`).
pub fn deserialize_state(hex_state: &str) -> Result<ContractState<InMemoryDB>, ContractError> {
    let bytes = hex::decode(hex_state)
        .map_err(|e| ContractError::StateFetch(format!("hex decode: {e}")))?;
    midnight_serialize::tagged_deserialize(&mut bytes.as_slice())
        .map_err(|e| ContractError::StateFetch(format!("deserialize: {e}")))
}

/// Parse a hex-encoded contract address string into a `ContractAddress`.
///
/// Accepts 64 hex characters (32 bytes) with or without `0x` prefix.
pub fn parse_address(hex_addr: &str) -> Result<ContractAddress, ContractError> {
    let hex = hex_addr.strip_prefix("0x").unwrap_or(hex_addr);
    let bytes =
        hex::decode(hex).map_err(|e| ContractError::InvalidAddress(format!("hex decode: {e}")))?;
    if bytes.len() != 32 {
        return Err(ContractError::InvalidAddress(format!(
            "expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(ContractAddress(midnight_base_crypto::hash::HashOutput(arr)))
}

/// Fetch contract state from a provider and deserialize it.
///
/// This is the async version of `deserialize_state` that fetches from a
/// provider first. Returns `ContractError::StateFetch` if the contract is not found.
pub async fn fetch_state<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let hex = provider
        .get_contract_state(address)
        .await
        .map_err(|e| ContractError::StateFetch(format!("provider: {e}")))?
        .ok_or_else(|| ContractError::StateFetch(format!("contract not found: {address}")))?;
    deserialize_state(&hex)
}

/// Fetch the network ID from a provider.
pub async fn fetch_network_id<P: midnight_provider::Provider>(
    provider: &P,
) -> Result<String, ContractError> {
    provider
        .get_network_id()
        .await
        .map_err(|e| ContractError::StateFetch(format!("network_id: {e}")))
}

/// High-level circuit call: fetch state, execute, and build an unproven transaction.
///
/// This ties together the full pipeline:
/// 1. Fetch current contract state from the provider
/// 2. Execute the circuit IR against it (no args, no witnesses, no helpers)
/// 3. Build an unproven transaction ready for proving
///
/// Only suitable for simple circuits with no arguments, no witnesses, and no
/// helper function calls (e.g., `counter.increment`). For circuits that need
/// arguments, witnesses, or helpers, use [`call_circuit_with`].
pub async fn call_circuit<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
    ir: &CircuitIrBody,
    circuit_name: &str,
) -> Result<UnprovenCallTx, ContractError> {
    let state = fetch_state(provider, address).await?;
    let contract_address = parse_address(address)?;
    let network_id = fetch_network_id(provider).await?;
    build_unproven_call_tx(ir, &state, circuit_name, contract_address, &network_id)
}

/// High-level circuit call with arguments and witnesses.
///
/// Same as `call_circuit` but allows passing arguments and a witness provider.
pub async fn call_circuit_with<P: midnight_provider::Provider, W: interpreter::WitnessProvider>(
    provider: &P,
    address: &str,
    ir: &CircuitIrBody,
    circuit_name: &str,
    args: &[(&str, interpreter::Value)],
    witnesses: &W,
    helpers: &[compact_codegen::ir::HelperDef],
) -> Result<UnprovenCallTx, ContractError> {
    let state = fetch_state(provider, address).await?;
    let contract_address = parse_address(address)?;
    let network_id = fetch_network_id(provider).await?;
    build_unproven_call_tx_with(
        ir,
        &state,
        circuit_name,
        contract_address,
        &network_id,
        args,
        witnesses,
        helpers,
    )
}

/// Build an unproven transaction with arguments, witnesses, and helpers.
///
/// Low-level API — prefer `call_circuit_with`.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn build_unproven_call_tx_with<W: interpreter::WitnessProvider>(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    network_id: &str,
    args: &[(&str, interpreter::Value)],
    witnesses: &W,
    helpers: &[compact_codegen::ir::HelperDef],
) -> Result<UnprovenCallTx, ContractError> {
    use midnight_ledger::structure::{Intent, Transaction};
    use midnight_storage::storage::HashMap as StorageHashMap;
    use rand::Rng;

    let mut rng = rand::thread_rng();

    // Step 1: Execute the circuit IR with arguments and witnesses
    let exec_result = interpreter::execute_with(ir, state, args, witnesses, helpers)?;

    // Step 2: Convert gather ops to verify ops and build transcripts
    let entry_point: EntryPointBuf = circuit_name.as_bytes().into();

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
            .map_err(|e| ContractError::Construction(format!("partition failed: {e:?}")))?;

    let (guaranteed, fallible) = partitioned.into_iter().next().unwrap_or((None, None));

    let input: AlignedValue = ().into();
    let output: AlignedValue = ().into();

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

    let ttl = current_ttl();

    let intent: Intent<Sig, _, _, InMemoryDB> = Intent::new(
        &mut rng,
        None,
        None,
        vec![call],
        Vec::new(),
        Vec::new(),
        None,
        ttl,
    );

    let mut intents = StorageHashMap::new();
    intents = intents.insert(0u16, intent);

    let tx: UnprovenTransaction = Transaction::from_intents(network_id, intents);

    let mut bytes = Vec::new();
    tagged_serialize(&tx, &mut bytes).map_err(|e| ContractError::Serialization(e.to_string()))?;

    Ok(UnprovenCallTx {
        tx_bytes: bytes,
        transaction: tx,
        new_state: exec_result.state,
    })
}

// ---------------------------------------------------------------------------
// Transaction submission
// ---------------------------------------------------------------------------

/// Submit proven transaction bytes to a Midnight node.
///
/// Connects to the node via WebSocket, submits the transaction as an
/// unsigned extrinsic to `Midnight::send_mn_transaction`, and returns
/// the extrinsic hash on success.
pub async fn submit(node_url: &str, tx_bytes: &[u8]) -> Result<String, ContractError> {
    use subxt::{OnlineClient, SubstrateConfig};

    let client = OnlineClient::<SubstrateConfig>::from_insecure_url(node_url)
        .await
        .map_err(|e| ContractError::Submission(format!("connect: {e}")))?;

    let call = subxt::dynamic::tx(
        "Midnight",
        "send_mn_transaction",
        vec![subxt::dynamic::Value::from_bytes(tx_bytes)],
    );

    let tx_client = client
        .tx()
        .await
        .map_err(|e| ContractError::Submission(format!("tx client: {e}")))?;
    let unsigned = tx_client
        .create_unsigned(&call)
        .map_err(|e| ContractError::Submission(format!("create unsigned: {e}")))?;
    let hash = unsigned
        .submit()
        .await
        .map_err(|e| ContractError::Submission(format!("{e}")))?;

    Ok(format!("{hash:?}"))
}

/// Format a `ContractAddress` as a hex string (without `0x` prefix).
pub fn format_address(address: &ContractAddress) -> String {
    hex::encode(address.0.0)
}

/// Deploy a contract and return the address as a hex string.
///
/// Convenience wrapper around `build_deploy_tx` that also returns
/// the address in a usable format.
pub async fn deploy(
    initial_state: &ContractState<InMemoryDB>,
    network_id: &str,
) -> Result<(String, Vec<u8>), ContractError> {
    let (address, tx_bytes) = build_deploy_tx(initial_state, network_id).await?;
    Ok((format_address(&address), tx_bytes))
}

/// Deploy a contract using a provider for the network ID.
pub async fn deploy_with_provider<P: midnight_provider::Provider>(
    provider: &P,
    initial_state: &ContractState<InMemoryDB>,
) -> Result<(String, Vec<u8>), ContractError> {
    let network_id = fetch_network_id(provider).await?;
    deploy(initial_state, &network_id).await
}

/// Full pipeline: fetch state → execute circuit → prove → return ready-to-submit bytes.
///
/// This is the highest-level API for calling a circuit. It:
/// 1. Fetches current state from the provider
/// 2. Executes the circuit IR
/// 3. Generates ZK proofs
/// 4. Returns proven TX bytes ready for `send_mn_transaction`
///
/// Also returns the new contract state after execution.
pub async fn prove_circuit<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
    ir: &CircuitIrBody,
    circuit_name: &str,
    keys_dir: &str,
) -> Result<(Vec<u8>, ContractState<InMemoryDB>), ContractError> {
    let unproven = call_circuit(provider, address, ir, circuit_name).await?;
    let proven_bytes = prove_and_seal(&unproven, keys_dir).await?;
    Ok((proven_bytes, unproven.new_state))
}

/// Full pipeline with arguments and witnesses.
#[allow(clippy::too_many_arguments)]
pub async fn prove_circuit_with<P: midnight_provider::Provider, W: interpreter::WitnessProvider>(
    provider: &P,
    address: &str,
    ir: &CircuitIrBody,
    circuit_name: &str,
    keys_dir: &str,
    args: &[(&str, interpreter::Value)],
    witnesses: &W,
    helpers: &[compact_codegen::ir::HelperDef],
) -> Result<(Vec<u8>, ContractState<InMemoryDB>), ContractError> {
    let unproven = call_circuit_with(
        provider,
        address,
        ir,
        circuit_name,
        args,
        witnesses,
        helpers,
    )
    .await?;
    let proven_bytes = prove_and_seal(&unproven, keys_dir).await?;
    Ok((proven_bytes, unproven.new_state))
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

    #[test]
    fn parse_address_with_prefix() {
        let hex = "0x".to_string() + &"aa".repeat(32);
        let addr = parse_address(&hex).unwrap();
        assert_eq!(addr.0.0, [0xAA; 32]);
    }

    #[test]
    fn parse_address_without_prefix() {
        let hex = "bb".repeat(32);
        let addr = parse_address(&hex).unwrap();
        assert_eq!(addr.0.0, [0xBB; 32]);
    }

    #[test]
    fn parse_address_wrong_length() {
        let err = parse_address("aabb").unwrap_err();
        assert!(err.to_string().contains("expected 32 bytes"));
    }

    #[test]
    fn parse_address_invalid_hex() {
        let err = parse_address("zzzz").unwrap_err();
        assert!(err.to_string().contains("hex decode"));
    }

    #[test]
    fn format_address_roundtrip() {
        let hex_in = "cc".repeat(32);
        let addr = parse_address(&hex_in).unwrap();
        let hex_out = format_address(&addr);
        assert_eq!(hex_in, hex_out);
    }

    #[tokio::test]
    async fn deploy_returns_hex_address() {
        if std::env::var("MIDNIGHT_LEDGER_TEST_STATIC_DIR").is_err() {
            eprintln!("skipping: MIDNIGHT_LEDGER_TEST_STATIC_DIR not set");
            return;
        }
        let state = make_counter_state(0);
        let (addr_hex, tx_bytes) = deploy(&state, "test").await.unwrap();
        assert_eq!(addr_hex.len(), 64); // 32 bytes = 64 hex chars
        assert!(!tx_bytes.is_empty());
    }

    #[test]
    fn deserialize_state_roundtrip() {
        let state = make_counter_state(42);
        let mut bytes = Vec::new();
        midnight_serialize::tagged_serialize(&state, &mut bytes).unwrap();
        let hex = hex::encode(&bytes);
        let restored = deserialize_state(&hex).unwrap();
        // Verify the counter value survived the round-trip
        match restored.data.get_ref() {
            StateValue::Array(arr) => match arr.get(0).unwrap() {
                StateValue::Cell(sp) => {
                    let counter = u64::try_from(&*sp.value).unwrap();
                    assert_eq!(counter, 42);
                }
                _ => panic!("expected Cell"),
            },
            _ => panic!("expected Array"),
        }
    }
}
