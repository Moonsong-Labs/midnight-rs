//! Circuit call transaction builder.
//!
//! Wires the IR interpreter output to midnight-ledger's transaction
//! construction pipeline: interpreter → partition → intent → transaction.

use std::borrow::Cow;
use std::sync::Arc;

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

/// Raw key file contents loaded from a compiled contract directory.
struct KeyFiles {
    prover_key: Vec<u8>,
    verifier_key: Vec<u8>,
    ir_source: Vec<u8>,
}

/// Read proving key artifacts for a single circuit from a compiled contract directory.
///
/// Looks for `{base_dir}/keys/{circuit_name}.prover`,
/// `{base_dir}/keys/{circuit_name}.verifier`, and
/// `{base_dir}/zkir/{circuit_name}.bzkir`.
///
/// Returns `Ok(None)` if none of the three files exist, `Ok(Some(...))` if all
/// three exist, or an error if the set is incomplete (some present, some missing).
fn read_key_files(
    base_dir: &std::path::Path,
    circuit_name: &str,
) -> std::io::Result<Option<KeyFiles>> {
    let read_file = |dir: &str, ext: &str| -> std::io::Result<Option<Vec<u8>>> {
        let path = base_dir.join(dir).join(format!("{circuit_name}.{ext}"));
        match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
            Ok(v) => Ok(Some(v)),
        }
    };
    let prover_key = read_file("keys", "prover")?;
    let verifier_key = read_file("keys", "verifier")?;
    let ir_source = read_file("zkir", "bzkir")?;
    match (prover_key, verifier_key, ir_source) {
        (None, None, None) => Ok(None),
        (Some(prover_key), Some(verifier_key), Some(ir_source)) => Ok(Some(KeyFiles {
            prover_key,
            verifier_key,
            ir_source,
        })),
        (p, v, i) => {
            let mut missing = Vec::new();
            let mut present = Vec::new();
            for (name, val) in [("prover", &p), ("verifier", &v), ("bzkir", &i)] {
                if val.is_none() {
                    missing.push(name);
                } else {
                    present.push(name);
                }
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "incomplete key artifacts for {circuit_name}: found [{found}] but missing [{missing}]",
                    found = present.join(", "),
                    missing = missing.join(", "),
                ),
            ))
        }
    }
}

/// Result of deploying a contract (before or after submission).
pub struct DeployResult {
    /// The contract's on-chain address.
    pub address: ContractAddress,
    /// The proven transaction bytes, ready for `submit()`.
    pub tx_bytes: Vec<u8>,
}

impl DeployResult {
    /// The contract address as a hex string.
    pub fn address_hex(&self) -> String {
        format_address(&self.address)
    }

    /// Submit this deploy transaction to a node.
    ///
    /// Returns a [`PendingTx`] handle for awaiting inclusion / finalization.
    pub async fn submit(&self, node_url: &str) -> Result<PendingTx, ContractError> {
        submit(node_url, &self.tx_bytes).await
    }
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

/// Load verifier keys from a compiled contract directory and insert them
/// into the contract state's operations map.
///
/// Reads all `*.verifier` files from `{dir}/keys/`, deserializes each into a
/// `VerifierKey`, and inserts it keyed by the file stem (e.g.,
/// `keys/increment.verifier` → entry point `"increment"`).
///
/// Required for on-chain deployment — without verifier keys, the node
/// cannot verify ZK proofs for circuit calls.
pub fn with_zk_keys(
    mut state: ContractState<InMemoryDB>,
    keys_dir: impl AsRef<std::path::Path>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    use midnight_transient_crypto::proofs::VerifierKey;

    let base = keys_dir.as_ref();
    let keys_path = if base.join("keys").is_dir() {
        base.join("keys")
    } else {
        base.to_path_buf()
    };
    let entries = std::fs::read_dir(&keys_path).map_err(|e| {
        ContractError::Construction(format!(
            "cannot read keys directory {}: {e}",
            keys_path.display()
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| ContractError::Construction(format!("read dir: {e}")))?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("verifier") {
            continue;
        }

        let circuit_name = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
            ContractError::Construction(format!("invalid filename: {}", path.display()))
        })?;

        let bytes = std::fs::read(&path)
            .map_err(|e| ContractError::Construction(format!("read {}: {e}", path.display())))?;

        let vk: VerifierKey = midnight_serialize::tagged_deserialize(&mut bytes.as_slice())
            .map_err(|e| {
                ContractError::Construction(format!("deserialize {circuit_name}.verifier: {e}"))
            })?;

        let entry_point: EntryPointBuf = circuit_name.as_bytes().into();
        let op = ContractOperation::new(Some(vk));
        state.operations = state.operations.insert(entry_point, op);
    }

    Ok(state)
}

/// Build a `Resolver` that loads proving keys from a compiled contract directory.
///
/// Uses the `midnight_node_ledger_helpers` re-exported types so the resolver
/// is compatible with `LedgerContext::update_resolver` (which takes `Arc<Resolver>`).
///
/// The directory should contain `keys/` and `zkir/` subdirectories.
fn build_resolver(
    zk_keys_dir: &std::path::Path,
) -> Result<Arc<midnight_node_ledger_helpers::Resolver>, ContractError> {
    use midnight_node_ledger_helpers::{
        DUST_EXPECTED_FILES, DustResolver, FetchMode, MidnightDataProvider, OutputMode,
        PUBLIC_PARAMS, ProvingKeyMaterial, Resolver,
    };

    let base_dir = if zk_keys_dir.join("keys").is_dir() {
        zk_keys_dir.to_path_buf()
    } else {
        zk_keys_dir.parent().unwrap_or(zk_keys_dir).to_path_buf()
    };

    let dust_resolver = DustResolver(
        MidnightDataProvider::new(
            FetchMode::OnDemand,
            OutputMode::Log,
            DUST_EXPECTED_FILES.to_owned(),
        )
        .map_err(|e| ContractError::Construction(format!("dust resolver: {e}")))?,
    );

    type KeyLoaderFut = std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::io::Result<Option<ProvingKeyMaterial>>>
                + Send
                + Sync,
        >,
    >;
    type KeyLoader =
        Box<dyn Fn(midnight_node_ledger_helpers::KeyLocation) -> KeyLoaderFut + Send + Sync>;

    let external_resolver: KeyLoader =
        Box::new(move |midnight_node_ledger_helpers::KeyLocation(loc)| {
            let base = base_dir.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    let loc_str = loc.to_string();
                    match read_key_files(&base, &loc_str)? {
                        None => Ok(None),
                        Some(keys) => Ok(Some(ProvingKeyMaterial {
                            prover_key: keys.prover_key,
                            verifier_key: keys.verifier_key,
                            ir_source: keys.ir_source,
                        })),
                    }
                })
                .await
                .map_err(std::io::Error::other)?
            })
        });

    Ok(Arc::new(Resolver::new(
        PUBLIC_PARAMS.clone(),
        dust_resolver,
        external_resolver,
    )))
}

/// Construct a `Resolver` that loads proving keys from a compiled contract
/// directory, without setting any environment variables.
///
/// Uses the direct `midnight_ledger` types (git version). For the
/// `midnight_node_ledger_helpers`-compatible version, use `build_resolver`.
///
/// The directory should contain `keys/` and `zkir/` subdirectories.
fn make_resolver(
    zk_keys_dir: &std::path::Path,
) -> Result<midnight_ledger::test_utilities::Resolver, ContractError> {
    use midnight_base_crypto::data_provider::{FetchMode, MidnightDataProvider, OutputMode};
    use midnight_ledger::dust::{DUST_EXPECTED_FILES, DustResolver};
    use midnight_ledger::prove::Resolver;
    use midnight_ledger::test_utilities::PUBLIC_PARAMS;
    use midnight_transient_crypto::proofs::ProvingKeyMaterial;

    let base_dir = if zk_keys_dir.join("keys").is_dir() {
        zk_keys_dir.to_path_buf()
    } else {
        zk_keys_dir.parent().unwrap_or(zk_keys_dir).to_path_buf()
    };

    let dust_resolver = DustResolver(
        MidnightDataProvider::new(
            FetchMode::OnDemand,
            OutputMode::Log,
            DUST_EXPECTED_FILES.to_owned(),
        )
        .map_err(|e| ContractError::Construction(format!("dust resolver: {e}")))?,
    );

    type ProveFut = std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::io::Result<Option<ProvingKeyMaterial>>>
                + Send
                + Sync,
        >,
    >;
    type ProveLoader = Box<dyn Fn(KeyLocation) -> ProveFut + Send + Sync>;

    let external_resolver: ProveLoader = Box::new(move |KeyLocation(loc)| {
        let base = base_dir.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let loc_str = loc.to_string();
                match read_key_files(&base, &loc_str)? {
                    None => Ok(None),
                    Some(keys) => Ok(Some(ProvingKeyMaterial {
                        prover_key: keys.prover_key,
                        verifier_key: keys.verifier_key,
                        ir_source: keys.ir_source,
                    })),
                }
            })
            .await
            .map_err(std::io::Error::other)?
        })
    });

    Ok(Resolver::new(
        PUBLIC_PARAMS.clone(),
        dust_resolver,
        external_resolver,
    ))
}

/// Construct a `Resolver` for deploy transactions (no circuit proving keys needed).
///
/// Deploy transactions contain no contract calls, so the external resolver
/// never fires — it always returns `Ok(None)`.
fn make_deploy_resolver() -> Result<midnight_ledger::test_utilities::Resolver, ContractError> {
    use midnight_base_crypto::data_provider::{FetchMode, MidnightDataProvider, OutputMode};
    use midnight_ledger::dust::{DUST_EXPECTED_FILES, DustResolver};
    use midnight_ledger::prove::Resolver;
    use midnight_ledger::test_utilities::PUBLIC_PARAMS;

    let dust_resolver = DustResolver(
        MidnightDataProvider::new(
            FetchMode::OnDemand,
            OutputMode::Log,
            DUST_EXPECTED_FILES.to_owned(),
        )
        .map_err(|e| ContractError::Construction(format!("dust resolver: {e}")))?,
    );

    Ok(Resolver::new(
        PUBLIC_PARAMS.clone(),
        dust_resolver,
        Box::new(|_| Box::pin(std::future::ready(Ok(None)))),
    ))
}

/// Default transaction TTL: 1 hour.
pub const DEFAULT_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Compute a TTL (time-to-live) for transaction intents.
///
/// Returns a timestamp `ttl_duration` in the future from now. The node rejects
/// transactions whose TTL has already passed.
fn current_ttl(ttl_duration: std::time::Duration) -> Timestamp {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();
    Timestamp::from_secs(now_secs) + Duration::from_secs(ttl_duration.as_secs().into())
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

    let resolver = make_resolver(std::path::Path::new(keys_dir))?;
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

    let ttl = current_ttl(DEFAULT_TTL);

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
    let resolver = make_deploy_resolver()?;
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
    let resolver = make_deploy_resolver()?;
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

fn make_proof_provider(
    prover: &crate::Prover,
) -> std::sync::Arc<
    dyn midnight_node_ledger_helpers::ProofProvider<midnight_node_ledger_helpers::DefaultDB>,
> {
    match prover {
        crate::Prover::Local => {
            std::sync::Arc::new(midnight_node_ledger_helpers::LocalProofServer::new())
        }
        crate::Prover::Remote(url) => std::sync::Arc::new(
            midnight_node_toolkit::remote_prover::RemoteProofServer::new(url.clone()),
        ),
    }
}

/// Deploy a contract with Dust fee payment from the provider's funded wallet.
///
/// Builds a funded transaction by asking the provider for a fresh
/// [`LedgerContext`] (resyncs the wallet, then constructs the context from
/// the wallet's local state) and runs the helpers' fee-balancing /
/// proving pipeline.
///
/// Returns a [`DeployResult`] containing the contract address and proven TX bytes.
pub async fn deploy_funded(
    initial_state: &ContractState<InMemoryDB>,
    provider: &midnight_provider::MidnightProvider,
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
) -> Result<DeployResult, ContractError> {
    use midnight_node_ledger_helpers::{
        BuildContractAction, ContractDeploy as LhContractDeploy, DefaultDB, FromContext,
        IntentInfo, LedgerContext, OfferInfo, ProofProvider, StandardTrasactionInfo,
    };
    use std::sync::Arc;

    let wallet_seed = {
        let arc = provider
            .wallet()
            .ok_or_else(|| ContractError::Construction("provider has no wallet".into()))?;
        let w = arc.read().await;
        *w.seed()
    };

    let context = provider
        .build_context()
        .await
        .map_err(|e| ContractError::Construction(format!("build context: {e}")))?;

    // 3. Convert our ContractState<InMemoryDB> → ContractState<DefaultDB> via serialization
    let mut state_bytes = Vec::new();
    tagged_serialize(initial_state, &mut state_bytes)
        .map_err(|e| ContractError::Serialization(e.to_string()))?;
    let state_for_deploy: midnight_node_ledger_helpers::ContractState<DefaultDB> =
        midnight_node_ledger_helpers::deserialize(&mut state_bytes.as_slice())
            .map_err(|e| ContractError::Construction(format!("state conversion: {e}")))?;

    // 4. Create deploy action
    let deploy = LhContractDeploy::new(&mut rand::thread_rng(), state_for_deploy);
    let address_raw = deploy.address();
    let address = ContractAddress(midnight_base_crypto::hash::HashOutput(address_raw.0.0));

    struct DeployAction<D: midnight_node_ledger_helpers::DB + Clone> {
        deploy: LhContractDeploy<D>,
    }

    #[async_trait::async_trait]
    impl<D: midnight_node_ledger_helpers::DB + Clone> BuildContractAction<D> for DeployAction<D> {
        async fn build(
            &mut self,
            _rng: &mut midnight_node_ledger_helpers::StdRng,
            _context: Arc<LedgerContext<D>>,
            intent: &midnight_node_ledger_helpers::Intent<
                midnight_node_ledger_helpers::Signature,
                midnight_node_ledger_helpers::ProofPreimageMarker,
                midnight_node_ledger_helpers::PedersenRandomness,
                D,
            >,
        ) -> midnight_node_ledger_helpers::Intent<
            midnight_node_ledger_helpers::Signature,
            midnight_node_ledger_helpers::ProofPreimageMarker,
            midnight_node_ledger_helpers::PedersenRandomness,
            D,
        > {
            intent.add_deploy(self.deploy.clone())
        }
    }

    let deploy_action = DeployAction { deploy };

    let intent_info: IntentInfo<DefaultDB> = IntentInfo {
        guaranteed_unshielded_offer: None,
        fallible_unshielded_offer: None,
        actions: vec![Box::new(deploy_action)],
    };

    // 5. Load proving keys into a Resolver and register with the context
    let resolver = build_resolver(keys_dir)?;
    context.update_resolver(resolver).await;

    // 6. Build funded transaction with Dust fees
    let proof_provider: Arc<dyn ProofProvider<DefaultDB>> = make_proof_provider(prover);
    let mut tx_info = StandardTrasactionInfo::new_from_context(context, proof_provider, None);
    tx_info.add_intent(1, Box::new(intent_info));
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    });
    tx_info.set_funding_seeds(vec![wallet_seed]);
    tx_info.use_mock_proofs_for_fees(true);

    let finalized = midnight_wallet::transfer::build_no_validate(tx_info)
        .await
        .map_err(|e| ContractError::Construction(format!("prove/balance failed: {e}")))?;

    let mut bytes = Vec::new();
    midnight_node_ledger_helpers::midnight_serialize::tagged_serialize(&finalized, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("{e}")))?;

    Ok(DeployResult {
        address,
        tx_bytes: bytes,
    })
}

/// Execute a circuit call with Dust fee payment from a pre-synced wallet.
///
/// This is the call equivalent of `deploy_funded`. It:
/// 1. Executes the circuit IR locally to produce transcripts
/// 2. Builds a funded transaction context from the wallet's indexed state
/// 3. Builds a funded call transaction with Dust fees
/// 4. Proves the transaction
/// 5. Returns the proven TX bytes and updated state
///
/// Submission is separate (via `submit()`).
#[allow(clippy::too_many_arguments)]
pub async fn call_funded(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    provider: &midnight_provider::MidnightProvider,
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
) -> Result<
    (
        Vec<u8>,
        ContractState<InMemoryDB>,
        Option<interpreter::Value>,
    ),
    ContractError,
> {
    call_funded_with(
        ir,
        state,
        circuit_name,
        contract_address,
        provider,
        keys_dir,
        prover,
        &[],
        &interpreter::NoWitnesses,
        &[],
        &[],
        &[],
    )
    .await
}

/// Execute a circuit call with arguments/witnesses and submit on-chain.
#[allow(clippy::too_many_arguments)]
pub async fn call_funded_with(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    provider: &midnight_provider::MidnightProvider,
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
    args: &[(&str, interpreter::Value)],
    witnesses: &dyn interpreter::WitnessProvider,
    helpers: &[compact_codegen::ir::HelperDef],
    structs: &[compact_codegen::ir::StructDef],
    enums: &[compact_codegen::ir::EnumDef],
) -> Result<
    (
        Vec<u8>,
        ContractState<InMemoryDB>,
        Option<interpreter::Value>,
    ),
    ContractError,
> {
    use midnight_node_ledger_helpers::{
        BuildContractAction, DefaultDB, FromContext, IntentInfo, LedgerContext, OfferInfo,
        ProofProvider, StandardTrasactionInfo,
    };
    use std::sync::Arc;

    // 1. Execute the circuit IR locally for the updated state
    let exec_result =
        interpreter::execute_with_enums(ir, state, args, witnesses, helpers, structs, enums)?;

    // 2. Build transcripts by partitioning the circuit's state ops.
    //    Serialize them so they can cross the InMemoryDB → DefaultDB boundary.
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

    let query_ctx =
        midnight_onchain_runtime::context::QueryContext::new(state.data.clone(), contract_address);
    let pre_transcript = midnight_ledger::construct::PreTranscript {
        context: query_ctx,
        program: verify_ops,
        comm_comm: None,
    };
    let partitioned =
        midnight_ledger::construct::partition_transcripts(&[pre_transcript], &INITIAL_PARAMETERS)
            .map_err(|e| ContractError::Construction(format!("partition: {e:?}")))?;
    let (guaranteed, fallible) = partitioned.into_iter().next().unwrap_or((None, None));

    // Round-trip transcripts across the InMemoryDB → DefaultDB boundary so the
    // CallAction below can hold typed values and never panic inside `build`.
    let to_default_db_transcript = |t| {
        let mut buf = Vec::new();
        tagged_serialize(&t, &mut buf)
            .map_err(|e| ContractError::Serialization(format!("serialize transcript: {e}")))?;
        midnight_node_ledger_helpers::deserialize(&mut buf.as_slice())
            .map_err(|e| ContractError::Serialization(format!("deserialize transcript: {e}")))
    };
    let guaranteed_db: Option<midnight_node_ledger_helpers::Transcript<DefaultDB>> =
        guaranteed.map(to_default_db_transcript).transpose()?;
    let fallible_db: Option<midnight_node_ledger_helpers::Transcript<DefaultDB>> =
        fallible.map(to_default_db_transcript).transpose()?;

    // 3. Build context from the provider's synced wallet
    let wallet_seed = {
        let arc = provider
            .wallet()
            .ok_or_else(|| ContractError::Construction("provider has no wallet".into()))?;
        let w = arc.read().await;
        *w.seed()
    };

    let context = provider
        .build_context()
        .await
        .map_err(|e| ContractError::Construction(format!("build context: {e}")))?;

    // 4. Load proving keys into a Resolver and register with the context
    let resolver = build_resolver(keys_dir)?;
    context.update_resolver(resolver).await;

    // 5. Cross the InMemoryDB → DefaultDB boundary for state, then extract the
    //    verifier-key operation up-front so CallAction can hold typed values.
    let mut state_bytes = Vec::new();
    tagged_serialize(state, &mut state_bytes)
        .map_err(|e| ContractError::Serialization(e.to_string()))?;
    let state_db: midnight_node_ledger_helpers::ContractState<DefaultDB> =
        midnight_node_ledger_helpers::deserialize(&mut state_bytes.as_slice())
            .map_err(|e| ContractError::Serialization(format!("deserialize state: {e}")))?;

    use midnight_node_ledger_helpers::{
        ContractAddress as HelperAddr, ContractCallPrototype, ContractOperation, EntryPointBuf,
        KeyLocation, ProofPreimage, Transcript,
    };

    let entry_point: EntryPointBuf = circuit_name.as_bytes().into();
    let op = state_db
        .operations
        .get(&entry_point)
        .map(|sp| (*sp).clone())
        .unwrap_or_else(|| ContractOperation::new(None));
    let helper_addr = HelperAddr(midnight_node_ledger_helpers::HashOutput(
        contract_address.0.0,
    ));

    // 5b. Insert the contract into the context's ledger state so client-side
    //     well_formed() validation can find it. The indexed wallet state doesn't
    //     include deployed contracts.
    {
        let mut guard = context
            .ledger_state
            .lock()
            .map_err(|_| ContractError::Construction("ledger_state lock poisoned".into()))?;
        let mut ls = (**guard).clone();
        ls.contract = ls.contract.insert(helper_addr, state_db.clone());
        *guard = midnight_node_ledger_helpers::Sp::new(ls);
    }

    // 6. Build circuit input / output AlignedValues. The interpreter side uses
    //    `midnight_bindgen::AlignedValue` (re-exported from the git-pinned
    //    midnight-base-crypto), while ContractCallPrototype expects the helpers'
    //    AlignedValue (a different crate version). Round-trip via serialization
    //    to cross that boundary, propagating any error here instead of from
    //    inside `build`.
    let input_av_local: AlignedValue = if args.is_empty() {
        ().into()
    } else {
        let arg_values: Vec<AlignedValue> =
            args.iter().map(|(_, v)| v.to_aligned_value()).collect();
        AlignedValue::concat(&arg_values)
    };
    let mut input_buf = Vec::new();
    tagged_serialize(&input_av_local, &mut input_buf)
        .map_err(|e| ContractError::Serialization(format!("serialize input: {e}")))?;
    let input_av: midnight_node_ledger_helpers::AlignedValue =
        midnight_node_ledger_helpers::deserialize(&mut input_buf.as_slice())
            .map_err(|e| ContractError::Serialization(format!("deserialize input: {e}")))?;

    // The output AlignedValue comes from the disclosed (communication) outputs
    // of the local interpreter run; each disclose() in the circuit emits a ZKIR
    // Output, and the concatenated AlignedValue must match for the commitment
    // to verify.
    let output_av_local: AlignedValue = if exec_result.communication_outputs.is_empty() {
        ().into()
    } else {
        AlignedValue::concat(&exec_result.communication_outputs)
    };
    let mut output_buf = Vec::new();
    tagged_serialize(&output_av_local, &mut output_buf)
        .map_err(|e| ContractError::Serialization(format!("serialize output: {e}")))?;
    let output_av: midnight_node_ledger_helpers::AlignedValue =
        midnight_node_ledger_helpers::deserialize(&mut output_buf.as_slice())
            .map_err(|e| ContractError::Serialization(format!("deserialize output: {e}")))?;

    // 7. Build the call action holding only typed values; `build` is now infallible.
    struct CallAction<D: midnight_node_ledger_helpers::DB + Clone> {
        address: HelperAddr,
        entry_point: EntryPointBuf,
        op: ContractOperation,
        input: midnight_node_ledger_helpers::AlignedValue,
        output: midnight_node_ledger_helpers::AlignedValue,
        circuit_name: String,
        guaranteed_transcript: Option<Transcript<D>>,
        fallible_transcript: Option<Transcript<D>>,
    }

    #[async_trait::async_trait]
    impl<D: midnight_node_ledger_helpers::DB + Clone> BuildContractAction<D> for CallAction<D> {
        async fn build(
            &mut self,
            rng: &mut midnight_node_ledger_helpers::StdRng,
            _context: std::sync::Arc<LedgerContext<D>>,
            intent: &midnight_node_ledger_helpers::Intent<
                midnight_node_ledger_helpers::Signature,
                midnight_node_ledger_helpers::ProofPreimageMarker,
                midnight_node_ledger_helpers::PedersenRandomness,
                D,
            >,
        ) -> midnight_node_ledger_helpers::Intent<
            midnight_node_ledger_helpers::Signature,
            midnight_node_ledger_helpers::ProofPreimageMarker,
            midnight_node_ledger_helpers::PedersenRandomness,
            D,
        > {
            use rand::Rng;

            let call = ContractCallPrototype {
                address: self.address,
                entry_point: self.entry_point.clone(),
                op: self.op.clone(),
                input: self.input.clone(),
                output: self.output.clone(),
                guaranteed_public_transcript: self.guaranteed_transcript.take(),
                fallible_public_transcript: self.fallible_transcript.take(),
                private_transcript_outputs: vec![],
                communication_commitment_rand: rng.r#gen(),
                key_location: KeyLocation(std::borrow::Cow::Owned(self.circuit_name.clone())),
            };

            intent.add_call::<ProofPreimage>(call)
        }
    }

    let call_action = CallAction {
        address: helper_addr,
        entry_point,
        op,
        input: input_av,
        output: output_av,
        circuit_name: circuit_name.to_string(),
        guaranteed_transcript: guaranteed_db,
        fallible_transcript: fallible_db,
    };

    let intent_info: IntentInfo<DefaultDB> = IntentInfo {
        guaranteed_unshielded_offer: None,
        fallible_unshielded_offer: None,
        actions: vec![Box::new(call_action)],
    };

    // 7. Build funded transaction with Dust fees and real ZK proofs
    let proof_provider: Arc<dyn ProofProvider<DefaultDB>> = make_proof_provider(prover);
    let mut tx_info = StandardTrasactionInfo::new_from_context(context, proof_provider, None);
    tx_info.add_intent(1, Box::new(intent_info));
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    });
    tx_info.set_funding_seeds(vec![wallet_seed]);
    tx_info.use_mock_proofs_for_fees(false);

    let finalized = midnight_wallet::transfer::build_no_validate(tx_info)
        .await
        .map_err(|e| ContractError::Construction(format!("prove/balance failed: {e}")))?;

    let mut bytes = Vec::new();
    midnight_node_ledger_helpers::midnight_serialize::tagged_serialize(&finalized, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("{e}")))?;

    Ok((bytes, exec_result.state, exec_result.result))
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
/// provider first. Returns `ContractError::NotFound` if the contract is not found.
pub async fn fetch_state<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let hex = provider
        .get_contract_state(address, None)
        .await
        .map_err(|e| ContractError::StateFetch(format!("provider: {e}")))?
        .ok_or_else(|| ContractError::NotFound(address.to_string()))?;
    deserialize_state(&hex)
}

/// Fetch contract state from a provider at a specific block offset.
///
/// Pass `None` for the offset to fetch the latest state. Use
/// [`BlockRef::to_contract_action_offset`] to convert a `BlockRef` into the
/// expected offset type.
pub async fn fetch_state_at<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
    offset: Option<midnight_provider::ContractActionOffset>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let hex = provider
        .get_contract_state(address, offset)
        .await
        .map_err(|e| ContractError::StateFetch(format!("provider: {e}")))?
        .ok_or_else(|| ContractError::NotFound(address.to_string()))?;
    deserialize_state(&hex)
}

/// Fetch contract state directly from the node RPC (`midnight_contractState`).
///
/// This uses the standard node RPC available on all devnet nodes, unlike
/// `midnight_queryContractState` which requires a custom node build.
pub async fn fetch_state_from_node(
    provider: &midnight_provider::MidnightProvider,
    address: &str,
    at_block_hash: Option<&str>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let hex = provider
        .get_state_from_node(address, at_block_hash)
        .await
        .map_err(|e| ContractError::StateFetch(format!("node RPC: {e}")))?
        .ok_or_else(|| ContractError::NotFound(address.to_string()))?;
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
    let exec_result = interpreter::execute_with(ir, state, args, witnesses, helpers, &[])?;

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

    // Serialize circuit arguments into the input field for the ZK proof.
    // The prover expects these as public inputs matching the circuit signature.
    let input: AlignedValue = if args.is_empty() {
        ().into()
    } else {
        let arg_values: Vec<AlignedValue> =
            args.iter().map(|(_, v)| v.to_aligned_value()).collect();
        AlignedValue::concat(&arg_values)
    };
    // Build the output AlignedValue from disclosed (communication) outputs.
    let output: AlignedValue = if exec_result.communication_outputs.is_empty() {
        ().into()
    } else {
        AlignedValue::concat(&exec_result.communication_outputs)
    };

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

    let ttl = current_ttl(DEFAULT_TTL);

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

/// Inclusion details for a transaction that landed in a block.
#[derive(Debug, Clone, Copy)]
pub struct TxInBlock {
    pub block_hash: [u8; 32],
    pub extrinsic_hash: [u8; 32],
}

/// Handle to a submitted transaction whose progress can be awaited.
///
/// Returned by [`submit`]. Both [`PendingTx::wait_best`] and
/// [`PendingTx::wait_finalized`] consume `self` and return the handle back
/// alongside the inclusion details, so callers re-bind the same name through
/// each step without needing `let mut`. Either may be called first;
/// `wait_finalized` skips the best-block status if `wait_best` was not used.
/// Calling either method twice (or `wait_best` after `wait_finalized`)
/// returns a "watch stream ended" error because subxt closes the stream once
/// the transaction reaches a terminal state.
///
/// # Timeouts and cancellation
///
/// Neither wait method imposes a deadline. If the node accepts the
/// transaction but the chain stalls (no block production, or no
/// finalization after inclusion), the underlying subxt stream stays open
/// and the wait future blocks indefinitely. Callers that need a deadline
/// should wrap the wait in [`tokio::time::timeout`]:
///
/// ```rust,ignore
/// use std::time::Duration;
///
/// let (best, pending) = tokio::time::timeout(
///     Duration::from_secs(60),
///     pending.wait_best(),
/// ).await??;
/// ```
///
/// Cancelling the wait future (drop, `tokio::select!`, timeout) is safe
/// and asynchronously closes the subxt subscription. It does **not**
/// retract the transaction from the mempool; the node keeps it queued
/// until it lands in a block or is dropped by the node itself.
pub struct PendingTx {
    progress: subxt::tx::TransactionProgress<
        subxt::SubstrateConfig,
        subxt::client::OnlineClientAtBlockImpl<subxt::SubstrateConfig>,
    >,
}

impl PendingTx {
    /// The hash of the submitted extrinsic.
    pub fn extrinsic_hash(&self) -> [u8; 32] {
        self.progress.extrinsic_hash().0
    }

    /// The extrinsic hash formatted as a hex string (no `0x` prefix, to match
    /// the convention used by [`Contract::address`](crate::Contract::address)).
    pub fn extrinsic_hash_hex(&self) -> String {
        hex::encode(self.extrinsic_hash())
    }

    /// Drive the watch stream until the transaction lands in the best block.
    ///
    /// Consumes `self` and returns it back so callers can chain a subsequent
    /// `wait_finalized` or `into_contract` without `let mut`.
    ///
    /// The returned `block_hash` reflects the inclusion observed at the time
    /// of return. If the chain re-orgs the transaction out of that block
    /// before finalization, the hash from this method becomes stale; for an
    /// authoritative inclusion use [`PendingTx::wait_finalized`].
    pub async fn wait_best(mut self) -> Result<(TxInBlock, Self), ContractError> {
        use subxt::tx::TransactionStatus;
        while let Some(status) = self.progress.next().await {
            match status.map_err(|e| ContractError::Submission(format!("watch: {e}")))? {
                TransactionStatus::InBestBlock(in_block) => {
                    let tx = TxInBlock {
                        block_hash: in_block.block_hash().0,
                        extrinsic_hash: in_block.extrinsic_hash().0,
                    };
                    return Ok((tx, self));
                }
                TransactionStatus::Error { message } => {
                    return Err(ContractError::Submission(format!("error: {message}")));
                }
                TransactionStatus::Invalid { message } => {
                    return Err(ContractError::Submission(format!("invalid: {message}")));
                }
                TransactionStatus::Dropped { message } => {
                    return Err(ContractError::Submission(format!("dropped: {message}")));
                }
                _ => continue,
            }
        }
        Err(ContractError::Submission(
            "watch stream ended before reaching best block".into(),
        ))
    }

    /// Drive the watch stream until the transaction is in a finalized block.
    ///
    /// Consumes `self` and returns it back. May be called without a prior
    /// `wait_best`; the best-block status is then skipped.
    pub async fn wait_finalized(mut self) -> Result<(TxInBlock, Self), ContractError> {
        use subxt::tx::TransactionStatus;
        while let Some(status) = self.progress.next().await {
            match status.map_err(|e| ContractError::Submission(format!("watch: {e}")))? {
                TransactionStatus::InFinalizedBlock(in_block) => {
                    let tx = TxInBlock {
                        block_hash: in_block.block_hash().0,
                        extrinsic_hash: in_block.extrinsic_hash().0,
                    };
                    return Ok((tx, self));
                }
                TransactionStatus::Error { message } => {
                    return Err(ContractError::Submission(format!("error: {message}")));
                }
                TransactionStatus::Invalid { message } => {
                    return Err(ContractError::Submission(format!("invalid: {message}")));
                }
                TransactionStatus::Dropped { message } => {
                    return Err(ContractError::Submission(format!("dropped: {message}")));
                }
                _ => continue,
            }
        }
        Err(ContractError::Submission(
            "watch stream ended before finalization".into(),
        ))
    }
}

/// Submit proven transaction bytes to a Midnight node and return a handle for
/// awaiting inclusion / finalization.
///
/// Connects to the node via WebSocket, submits the transaction as an unsigned
/// extrinsic to `Midnight::send_mn_transaction`, and returns a [`PendingTx`]
/// once the watch stream is established.
pub async fn submit(node_url: &str, tx_bytes: &[u8]) -> Result<PendingTx, ContractError> {
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
    let progress = unsigned
        .submit_and_watch()
        .await
        .map_err(|e| ContractError::Submission(format!("submit_and_watch: {e}")))?;

    Ok(PendingTx { progress })
}

/// Format a `ContractAddress` as a hex string (without `0x` prefix).
pub fn format_address(address: &ContractAddress) -> String {
    hex::encode(address.0.0)
}

/// Deploy a contract to a running node and submit the transaction in one step.
///
/// Convenience wrapper that combines `deploy_funded` + `submit`.
/// Returns the contract address hex string and a [`PendingTx`] for awaiting
/// inclusion / finalization.
pub async fn deploy_and_submit(
    initial_state: &ContractState<InMemoryDB>,
    node_url: &str,
    provider: &midnight_provider::MidnightProvider,
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
) -> Result<(String, PendingTx), ContractError> {
    let result = deploy_funded(initial_state, provider, keys_dir, prover).await?;
    let pending = submit(node_url, &result.tx_bytes).await?;
    Ok((result.address_hex(), pending))
}

/// Wait until a contract is deployed and visible via the provider.
///
/// Polls the provider every `poll_interval` until the contract state is found
/// or `timeout` is reached. Returns the contract state on success.
pub async fn wait_for_deployment<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let start = std::time::Instant::now();
    loop {
        match provider.get_contract_state(address, None).await {
            Ok(Some(hex)) => return deserialize_state(&hex),
            Ok(None) => {}
            Err(e) => {
                if start.elapsed() >= timeout {
                    return Err(ContractError::StateFetch(format!(
                        "timeout waiting for contract {address}: {e}"
                    )));
                }
            }
        }
        if start.elapsed() >= timeout {
            return Err(ContractError::StateFetch(format!(
                "timeout after {:.0}s waiting for contract {address}",
                timeout.as_secs_f64()
            )));
        }
        tokio::time::sleep(poll_interval).await;
    }
}

/// Default timeout for waiting for transaction inclusion in a block.
pub const DEFAULT_TX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Default poll interval for checking transaction inclusion.
pub const DEFAULT_TX_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Wait until the indexer has processed a new block for a contract.
///
/// Polls `get_latest_contract_block_height` until the height exceeds
/// `height_before` (the height recorded before the transaction was submitted).
/// Pass `None` for `height_before` when the contract was just deployed and
/// has no prior block height.
pub async fn wait_for_contract_update<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
    height_before: Option<i64>,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<(), ContractError> {
    let start = std::time::Instant::now();
    let mut last_error: Option<String> = None;
    loop {
        match provider.get_latest_contract_block_height(address).await {
            Ok(Some(current_height)) => {
                let changed = match height_before {
                    Some(prev) => current_height > prev,
                    None => true, // no prior height, any height means the tx landed
                };
                if changed {
                    return Ok(());
                }
            }
            Ok(None) => {}
            Err(e) => {
                last_error = Some(e.to_string());
            }
        }
        if start.elapsed() >= timeout {
            let detail = last_error
                .map(|e| format!("; last error: {e}"))
                .unwrap_or_default();
            return Err(ContractError::Submission(format!(
                "timeout after {:.0}s waiting for contract {address} state update{detail}",
                timeout.as_secs_f64()
            )));
        }
        tokio::time::sleep(poll_interval).await;
    }
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
    fn with_zk_keys_loads_increment() {
        let keys_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/counter/compiled");
        if !keys_dir.exists() {
            eprintln!("skipping: keys dir not found at {}", keys_dir.display());
            return;
        }

        let state = make_counter_state(0);
        assert!(state.operations.is_empty());

        let state = with_zk_keys(state, &keys_dir).unwrap();

        // Should now have an "increment" operation with a verifier key
        let entry: midnight_onchain_runtime::state::EntryPointBuf = b"increment"[..].into();
        let op = state.operations.get(&entry).expect("increment operation");
        assert!(op.latest().is_some(), "verifier key should be present");
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
