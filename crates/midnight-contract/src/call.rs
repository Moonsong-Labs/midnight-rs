//! Circuit call transaction builder.
//!
//! Wires the IR interpreter output to midnight-ledger's transaction
//! construction pipeline: interpreter → partition → intent → transaction.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

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
    /// Returns the transaction hash on success.
    pub async fn submit(&self, node_url: &str) -> Result<String, ContractError> {
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
/// is compatible with `LedgerContext::update_resolver` (which expects the
/// helpers' `Resolver` type, not the git-version `midnight_ledger::prove::Resolver`).
///
/// The directory should contain `keys/` and `zkir/` subdirectories.
///
/// Returns a `&'static Resolver` because the upstream
/// `LedgerContext::update_resolver` requires a `&'static` reference.
/// `Box::leak` is used to satisfy this constraint — each resolver lives
/// for the process lifetime, which is acceptable since proving keys are
/// typically loaded once per contract.
fn build_resolver(
    zk_keys_dir: &std::path::Path,
) -> Result<&'static midnight_node_ledger_helpers::Resolver, ContractError> {
    use midnight_node_ledger_helpers::{
        DUST_EXPECTED_FILES, DustResolver, FetchMode, MidnightDataProvider, OutputMode,
        PUBLIC_PARAMS, ProvingKeyMaterial, Resolver,
    };

    static CACHE: LazyLock<
        Mutex<HashMap<PathBuf, &'static midnight_node_ledger_helpers::Resolver>>,
    > = LazyLock::new(|| Mutex::new(HashMap::new()));

    let base_dir = if zk_keys_dir.join("keys").is_dir() {
        zk_keys_dir.to_path_buf()
    } else {
        zk_keys_dir.parent().unwrap_or(zk_keys_dir).to_path_buf()
    };

    // Canonicalize to avoid caching the same directory under different relative paths.
    let cache_key = base_dir.canonicalize().unwrap_or_else(|_| base_dir.clone());

    // Return cached resolver if one exists for this path.
    {
        let cache = CACHE.lock().unwrap();
        if let Some(&resolver) = cache.get(&cache_key) {
            return Ok(resolver);
        }
    }

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

    let resolver: &'static Resolver = Box::leak(Box::new(Resolver::new(
        PUBLIC_PARAMS.clone(),
        dust_resolver,
        external_resolver,
    )));

    CACHE.lock().unwrap().insert(cache_key, resolver);
    Ok(resolver)
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

/// Deploy a contract with Dust fee payment from a funded wallet.
///
/// Connects to a running Midnight node, syncs wallet state by replaying
/// chain blocks, then builds a funded deploy transaction with Dust fees.
///
/// Uses the same infrastructure as the node's own toolkit:
/// `midnight-node-toolkit` for block fetching and context sync,
/// `midnight-node-ledger-helpers` for wallet derivation, fee balancing, and proving.
///
/// `node_url` is the WebSocket URL of the Midnight node (e.g., `"ws://127.0.0.1:9944"`).
///
/// `wallet_seed_hex` is the hex-encoded 32-byte wallet seed. For the dev node,
/// use `"0000000000000000000000000000000000000000000000000000000000000001"`.
///
/// Returns a [`DeployResult`] containing the contract address and proven TX bytes.
/// Create a `ProofProvider` from a `Prover` configuration.
///
/// For `Prover::Local`, uses the CPU-based `LocalProofServer`.
/// For `Prover::Remote`, delegates to the HTTP-based `RemoteProofServer`.
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

pub async fn deploy_funded(
    initial_state: &ContractState<InMemoryDB>,
    node_url: &str,
    wallet_seed_hex: &str,
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
) -> Result<DeployResult, ContractError> {
    use midnight_node_ledger_helpers::{
        BuildContractAction, ContractDeploy as LhContractDeploy, DefaultDB, FromContext,
        IntentInfo, LedgerContext, OfferInfo, ProofProvider, StandardTrasactionInfo, WalletSeed,
    };
    use midnight_node_toolkit::tx_generator::builder::build_fork_aware_context_raw;
    use midnight_node_toolkit::tx_generator::source::{FetchCacheConfig, GetTxs, GetTxsFromUrl};
    use std::sync::Arc;

    let wallet_seed = WalletSeed::try_from_hex_str(wallet_seed_hex)
        .map_err(|e| ContractError::Construction(format!("invalid wallet seed: {e:?}")))?;

    // 1. Fetch blocks from the running node (with dust_warp to make accumulated dust spendable)
    let fetcher = GetTxsFromUrl::new(
        node_url,
        4,    // fetch workers
        4,    // compute workers
        true, // dust_warp
        false,
        FetchCacheConfig::InMemory,
    );
    let source_txs = GetTxs::get_txs(&fetcher)
        .await
        .map_err(|e| ContractError::Construction(format!("fetch blocks from node: {e}")))?;

    // 2. Replay blocks into a synced LedgerContext (wallet now knows its dust balance)
    let fork_ctx = build_fork_aware_context_raw(&source_txs, &[wallet_seed]);
    let context: Arc<LedgerContext<DefaultDB>> = Arc::new(
        fork_ctx
            .into_ledger8()
            .ok_or_else(|| ContractError::Construction("expected ledger v8 context".into()))?,
    );

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
        deploy: Option<LhContractDeploy<D>>,
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
            intent.add_deploy(self.deploy.take().expect("deploy already consumed"))
        }
    }

    let deploy_action = DeployAction {
        deploy: Some(deploy),
    };

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

    let finalized = tx_info
        .prove()
        .await
        .map_err(|e| ContractError::Construction(format!("prove/balance failed: {e:?}")))?;

    let mut bytes = Vec::new();
    midnight_node_ledger_helpers::midnight_serialize::tagged_serialize(&finalized, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("{e}")))?;

    Ok(DeployResult {
        address,
        tx_bytes: bytes,
    })
}

/// Execute a circuit call and submit it on-chain with Dust fee payment.
///
/// This is the call equivalent of `deploy_funded`. It:
/// 1. Executes the circuit IR locally to produce transcripts
/// 2. Syncs wallet state from the chain
/// 3. Builds a funded call transaction with Dust fees
/// 4. Proves the transaction
/// 5. Returns the proven TX bytes and updated state
///
/// After submission (via `submit()`), the on-chain state will reflect the call.
#[allow(clippy::too_many_arguments)]
pub async fn call_funded(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    node_url: &str,
    wallet_seed_hex: &str,
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
) -> Result<(Vec<u8>, ContractState<InMemoryDB>), ContractError> {
    call_funded_with(
        ir,
        state,
        circuit_name,
        contract_address,
        node_url,
        wallet_seed_hex,
        keys_dir,
        prover,
        &[],
        &interpreter::NoWitnesses,
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
    node_url: &str,
    wallet_seed_hex: &str,
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
    args: &[(&str, interpreter::Value)],
    witnesses: &dyn interpreter::WitnessProvider,
    helpers: &[compact_codegen::ir::HelperDef],
) -> Result<(Vec<u8>, ContractState<InMemoryDB>), ContractError> {
    use midnight_node_ledger_helpers::{
        BuildContractAction, DefaultDB, FromContext, IntentInfo, LedgerContext, OfferInfo,
        ProofProvider, StandardTrasactionInfo, WalletSeed,
    };
    use midnight_node_toolkit::tx_generator::builder::build_fork_aware_context_raw;
    use midnight_node_toolkit::tx_generator::source::{FetchCacheConfig, GetTxs, GetTxsFromUrl};
    use std::sync::Arc;

    // 1. Execute the circuit IR locally for the updated state
    let exec_result = interpreter::execute_with(ir, state, args, witnesses, helpers, &[])?;

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

    let guaranteed_bytes = guaranteed.map(|t| {
        let mut buf = Vec::new();
        tagged_serialize(&t, &mut buf).expect("serialize transcript");
        buf
    });
    let fallible_bytes = fallible.map(|t| {
        let mut buf = Vec::new();
        tagged_serialize(&t, &mut buf).expect("serialize transcript");
        buf
    });

    // 3. Sync wallet state from the chain
    let wallet_seed = WalletSeed::try_from_hex_str(wallet_seed_hex)
        .map_err(|e| ContractError::Construction(format!("invalid wallet seed: {e:?}")))?;

    let fetcher = GetTxsFromUrl::new(node_url, 4, 4, true, false, FetchCacheConfig::InMemory);
    let source_txs = GetTxs::get_txs(&fetcher)
        .await
        .map_err(|e| ContractError::Construction(format!("fetch blocks from node: {e}")))?;

    let fork_ctx = build_fork_aware_context_raw(&source_txs, &[wallet_seed]);
    let context: Arc<LedgerContext<DefaultDB>> = Arc::new(
        fork_ctx
            .into_ledger8()
            .ok_or_else(|| ContractError::Construction("expected ledger v8 context".into()))?,
    );

    // 4. Load proving keys into a Resolver and register with the context
    let resolver = build_resolver(keys_dir)?;
    context.update_resolver(resolver).await;

    // 5. Serialize the contract state for the cross-DB-boundary CallAction
    let mut state_bytes = Vec::new();
    tagged_serialize(state, &mut state_bytes)
        .map_err(|e| ContractError::Serialization(e.to_string()))?;

    // 6. Serialize circuit arguments for the ZK proof input
    let input_bytes = if args.is_empty() {
        let av: AlignedValue = ().into();
        let mut buf = Vec::new();
        tagged_serialize(&av, &mut buf).map_err(|e| ContractError::Serialization(e.to_string()))?;
        buf
    } else {
        let arg_values: Vec<AlignedValue> =
            args.iter().map(|(_, v)| v.to_aligned_value()).collect();
        let av = AlignedValue::concat(&arg_values);
        let mut buf = Vec::new();
        tagged_serialize(&av, &mut buf).map_err(|e| ContractError::Serialization(e.to_string()))?;
        buf
    };

    // 7. Build the call action
    struct CallAction {
        state_bytes: Vec<u8>,
        input_bytes: Vec<u8>,
        circuit_name: String,
        address: ContractAddress,
        guaranteed_bytes: Option<Vec<u8>>,
        fallible_bytes: Option<Vec<u8>>,
    }

    #[async_trait::async_trait]
    impl<D: midnight_node_ledger_helpers::DB + Clone> BuildContractAction<D> for CallAction {
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
            // Deserialize state as D-typed to get the operations (verifier keys)
            let state: midnight_node_ledger_helpers::ContractState<D> =
                midnight_node_ledger_helpers::deserialize(&mut self.state_bytes.as_slice())
                    .expect("deserialize state");

            use midnight_node_ledger_helpers::{
                ContractAddress as HelperAddr, ContractCallPrototype, ContractOperation,
                EntryPointBuf, KeyLocation, ProofPreimage,
            };
            use rand::Rng;

            // Convert ContractAddress across crate versions via raw bytes
            let addr = HelperAddr(midnight_node_ledger_helpers::HashOutput(self.address.0.0));

            let entry_point: EntryPointBuf = self.circuit_name.as_bytes().into();
            let op = state
                .operations
                .get(&entry_point)
                .map(|sp| (*sp).clone())
                .unwrap_or_else(|| ContractOperation::new(None));

            // Deserialize transcripts across DB boundary
            let guaranteed = self.guaranteed_bytes.take().map(|b| {
                midnight_node_ledger_helpers::deserialize(&mut b.as_slice())
                    .expect("deserialize guaranteed transcript")
            });
            let fallible = self.fallible_bytes.take().map(|b| {
                midnight_node_ledger_helpers::deserialize(&mut b.as_slice())
                    .expect("deserialize fallible transcript")
            });

            let call = ContractCallPrototype {
                address: addr,
                entry_point,
                op,
                input: midnight_node_ledger_helpers::deserialize(&mut self.input_bytes.as_slice())
                    .expect("deserialize input (just serialized by same process)"),
                output: ().into(),
                guaranteed_public_transcript: guaranteed,
                fallible_public_transcript: fallible,
                private_transcript_outputs: vec![],
                communication_commitment_rand: rng.r#gen(),
                key_location: KeyLocation(std::borrow::Cow::Owned(self.circuit_name.clone())),
            };

            intent.add_call::<ProofPreimage>(call)
        }
    }

    let call_action = CallAction {
        state_bytes,
        input_bytes,
        circuit_name: circuit_name.to_string(),
        address: contract_address,
        guaranteed_bytes,
        fallible_bytes,
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

    let finalized = tx_info
        .prove()
        .await
        .map_err(|e| ContractError::Construction(format!("prove/balance failed: {e:?}")))?;

    let mut bytes = Vec::new();
    midnight_node_ledger_helpers::midnight_serialize::tagged_serialize(&finalized, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("{e}")))?;

    Ok((bytes, exec_result.state))
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
        .get_contract_state(address, None)
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

/// Deploy a contract to a running node and submit the transaction in one step.
///
/// Convenience wrapper that combines `deploy_funded` + `submit`.
/// Returns the contract address hex string and the transaction hash.
pub async fn deploy_and_submit(
    initial_state: &ContractState<InMemoryDB>,
    node_url: &str,
    wallet_seed_hex: &str,
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
) -> Result<(String, String), ContractError> {
    let result = deploy_funded(initial_state, node_url, wallet_seed_hex, keys_dir, prover).await?;
    let tx_hash = submit(node_url, &result.tx_bytes).await?;
    Ok((result.address_hex(), tx_hash))
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
