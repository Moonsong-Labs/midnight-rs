//! Circuit call transaction builder.
//!
//! Wires the IR interpreter output to midnight-ledger's transaction
//! construction pipeline: interpreter → partition → intent → transaction.
//!
//! State reading, address parsing, and the deploy path live in
//! [`crate::state`], [`crate::address`], and [`crate::deploy`] respectively;
//! this module is purely call-side. A few helpers used by both paths
//! (`build_resolver`, `current_ttl`, `make_proof_provider`, `DEFAULT_TTL`) are
//! exposed as `pub(crate)` from here so `deploy` doesn't have to duplicate
//! them.

use std::borrow::Cow;
use std::sync::Arc;

use midnight_base_crypto::time::{Duration, Timestamp};
use midnight_bindgen_runtime::{AlignedValue, ContractState, InMemoryDB};
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

/// Build a `Resolver` that loads proving keys from a compiled contract directory.
///
/// Uses the `midnight_helpers` re-exported types so the resolver
/// is compatible with `LedgerContext::update_resolver` (which takes `Arc<Resolver>`).
///
/// The directory should contain `keys/` and `zkir/` subdirectories.
pub(crate) fn build_resolver(
    zk_keys_dir: &std::path::Path,
) -> Result<Arc<midnight_helpers::Resolver>, ContractError> {
    use midnight_helpers::{
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
    type KeyLoader = Box<dyn Fn(midnight_helpers::KeyLocation) -> KeyLoaderFut + Send + Sync>;

    let external_resolver: KeyLoader = Box::new(move |midnight_helpers::KeyLocation(loc)| {
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

/// Build a dust-only [`midnight_helpers::Resolver`] with no circuit proving keys.
///
/// Maintenance updates and deploys carry no contract calls, so the external key
/// resolver never fires — it always returns `Ok(None)`. Uses the
/// `midnight_helpers` re-exported types so the resolver is compatible with
/// `LedgerContext::update_resolver`.
pub(crate) fn build_dust_only_resolver() -> Result<Arc<midnight_helpers::Resolver>, ContractError> {
    use midnight_helpers::{
        DUST_EXPECTED_FILES, DustResolver, FetchMode, MidnightDataProvider, OutputMode,
        PUBLIC_PARAMS, Resolver,
    };

    let dust_resolver = DustResolver(
        MidnightDataProvider::new(
            FetchMode::OnDemand,
            OutputMode::Log,
            DUST_EXPECTED_FILES.to_owned(),
        )
        .map_err(|e| ContractError::Construction(format!("dust resolver: {e}")))?,
    );

    Ok(Arc::new(Resolver::new(
        PUBLIC_PARAMS.clone(),
        dust_resolver,
        Box::new(|_| Box::pin(std::future::ready(Ok(None)))),
    )))
}

/// Default transaction TTL: 1 hour.
///
/// Used by the low-level [`build_unproven_call_tx`] path. The high-level path
/// ([`crate::deploy::deploy_funded`], [`call_funded_with`], and the
/// [`crate::DeployBuilder`] / [`crate::Contract::call_with`] APIs that wrap
/// them) reads `global_ttl` from chain parameters via the upstream
/// `StandardTrasactionInfo::build`, so this constant doesn't apply there.
pub(crate) const DEFAULT_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Compute a TTL (time-to-live) for transaction intents.
///
/// Returns a timestamp `ttl_duration` in the future from now. The node rejects
/// transactions whose TTL has already passed.
pub(crate) fn current_ttl(ttl_duration: std::time::Duration) -> Timestamp {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();
    Timestamp::from_secs(now_secs) + Duration::from_secs(ttl_duration.as_secs().into())
}

pub(crate) fn make_proof_provider(
    prover: &crate::Prover,
) -> std::sync::Arc<dyn midnight_helpers::ProofProvider<midnight_helpers::DefaultDB>> {
    match prover {
        crate::Prover::Local => std::sync::Arc::new(midnight_helpers::LocalProofServer::new()),
        crate::Prover::Remote(url) => {
            std::sync::Arc::new(crate::remote_prover::RemoteProofServer::new(url.clone()))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn call_funded_with(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    provider: &midnight_provider::MidnightProvider,
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
    args: &[(&str, interpreter::Value)],
    witnesses: &dyn interpreter::WitnessProvider,
    witness_ctx: Option<&mut interpreter::WitnessContext<'_>>,
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
    use midnight_helpers::{
        BuildContractAction, DefaultDB, FromContext, IntentInfo, LedgerContext, OfferInfo,
        ProofProvider, StandardTrasactionInfo,
    };

    // 1. Execute the circuit IR locally for the updated state. When a
    //    `witness_ctx` is supplied it threads the contract's private state
    //    through any witness calls; after this returns its buffer holds the
    //    post-call private state. `None` means no private-state threading.
    let exec_result = interpreter::execute_with_owned(
        ir,
        state.clone(),
        args,
        &[],
        witnesses,
        witness_ctx,
        helpers,
        structs,
        enums,
    )?;

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
        midnight_helpers::deserialize(&mut buf.as_slice())
            .map_err(|e| ContractError::Serialization(format!("deserialize transcript: {e}")))
    };
    let guaranteed_db: Option<midnight_helpers::Transcript<DefaultDB>> =
        guaranteed.map(to_default_db_transcript).transpose()?;
    let fallible_db: Option<midnight_helpers::Transcript<DefaultDB>> =
        fallible.map(to_default_db_transcript).transpose()?;

    // 3. Build context from the provider's synced wallet
    let wallet_seed = provider.seed().await?;

    let context = provider.build_context().await?;

    // 4. Load proving keys into a Resolver and register with the context
    let resolver = build_resolver(keys_dir)?;
    context.update_resolver(resolver).await;

    // 5. Cross the InMemoryDB → DefaultDB boundary for state, then extract the
    //    verifier-key operation up-front so CallAction can hold typed values.
    let mut state_bytes = Vec::new();
    tagged_serialize(state, &mut state_bytes)
        .map_err(|e| ContractError::Serialization(e.to_string()))?;
    let state_db: midnight_helpers::ContractState<DefaultDB> =
        midnight_helpers::deserialize(&mut state_bytes.as_slice())
            .map_err(|e| ContractError::Serialization(format!("deserialize state: {e}")))?;

    use midnight_helpers::{
        ContractAddress as HelperAddr, ContractCallPrototype, ContractOperation, EntryPointBuf,
        KeyLocation, ProofPreimage, Transcript,
    };

    let entry_point: EntryPointBuf = circuit_name.as_bytes().into();
    let op = state_db
        .operations
        .get(&entry_point)
        .map(|sp| (*sp).clone())
        .unwrap_or_else(|| ContractOperation::new(None));
    let helper_addr = HelperAddr(midnight_helpers::HashOutput(contract_address.0.0));

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
        *guard = midnight_helpers::Sp::new(ls);
    }

    // 6. Build circuit input / output AlignedValues. The interpreter side uses
    //    `midnight_bindgen_runtime::AlignedValue` (re-exported from the git-pinned
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
    let input_av: midnight_helpers::AlignedValue =
        midnight_helpers::deserialize(&mut input_buf.as_slice())
            .map_err(|e| ContractError::Serialization(format!("deserialize input: {e}")))?;

    let output_av_local: AlignedValue = if exec_result.communication_outputs.is_empty() {
        ().into()
    } else {
        AlignedValue::concat(&exec_result.communication_outputs)
    };
    let mut output_buf = Vec::new();
    tagged_serialize(&output_av_local, &mut output_buf)
        .map_err(|e| ContractError::Serialization(format!("serialize output: {e}")))?;
    let output_av: midnight_helpers::AlignedValue =
        midnight_helpers::deserialize(&mut output_buf.as_slice())
            .map_err(|e| ContractError::Serialization(format!("deserialize output: {e}")))?;

    // Witness private values become the prototype's private transcript outputs
    // (the ZKIR's private inputs). Without these, proving a witness-using circuit
    // fails with "ran out of private transcript outputs". Cross the InMemoryDB ->
    // DefaultDB boundary the same way as input/output above.
    let private_transcript_outputs: Vec<midnight_helpers::AlignedValue> = exec_result
        .private_transcript_outputs
        .iter()
        .map(|av| {
            let mut buf = Vec::new();
            tagged_serialize(av, &mut buf).map_err(|e| {
                ContractError::Serialization(format!("serialize private output: {e}"))
            })?;
            midnight_helpers::deserialize(&mut buf.as_slice()).map_err(|e| {
                ContractError::Serialization(format!("deserialize private output: {e}"))
            })
        })
        .collect::<Result<Vec<_>, ContractError>>()?;

    // 7. Build the call action holding only typed values; `build` is now infallible.
    struct CallAction<D: midnight_helpers::DB + Clone> {
        address: HelperAddr,
        entry_point: EntryPointBuf,
        op: ContractOperation,
        input: midnight_helpers::AlignedValue,
        output: midnight_helpers::AlignedValue,
        circuit_name: String,
        guaranteed_transcript: Option<Transcript<D>>,
        fallible_transcript: Option<Transcript<D>>,
        private_transcript_outputs: Vec<midnight_helpers::AlignedValue>,
    }

    #[async_trait::async_trait]
    impl<D: midnight_helpers::DB + Clone> BuildContractAction<D> for CallAction<D> {
        async fn build(
            &mut self,
            rng: &mut midnight_helpers::StdRng,
            _context: std::sync::Arc<LedgerContext<D>>,
            intent: &midnight_helpers::Intent<
                midnight_helpers::Signature,
                midnight_helpers::ProofPreimageMarker,
                midnight_helpers::PedersenRandomness,
                D,
            >,
        ) -> midnight_helpers::Intent<
            midnight_helpers::Signature,
            midnight_helpers::ProofPreimageMarker,
            midnight_helpers::PedersenRandomness,
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
                private_transcript_outputs: std::mem::take(&mut self.private_transcript_outputs),
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
        private_transcript_outputs,
    };

    let intent_info: IntentInfo<DefaultDB> = IntentInfo {
        guaranteed_unshielded_offer: None,
        fallible_unshielded_offer: None,
        actions: vec![Box::new(call_action)],
    };

    // 7. Build funded transaction with Dust fees and real ZK proofs
    let proof_provider: Arc<dyn ProofProvider<DefaultDB>> = make_proof_provider(prover);
    let reserved_at = context.latest_block_context().tblock;
    let mut tx_info = StandardTrasactionInfo::new_from_context(context, proof_provider, None);
    tx_info.add_intent(1, Box::new(intent_info));
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    });
    tx_info.set_funding_seeds(vec![wallet_seed]);
    tx_info.use_mock_proofs_for_fees(false);

    let built = midnight_wallet::transfer::build_no_validate(tx_info)
        .await
        .map_err(|e| ContractError::Construction(format!("prove/balance failed: {e}")))?;

    // Reserve the dust spends used by this transaction on the provider's
    // wallet so subsequent builds before the indexer catches up don't
    // re-select the same UTXOs.
    if let Ok(mut wallet) = provider.wallet_mut().await {
        wallet.reserve_pending(built.dust_batches, Vec::new(), reserved_at);
    }

    let mut bytes = Vec::new();
    midnight_helpers::midnight_serialize::tagged_serialize(&built.finalized, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("{e}")))?;

    Ok((bytes, exec_result.state, exec_result.result))
}

/// Build an unproven transaction from a circuit IR body and contract state.
///
/// Low-level builder; the high-level path goes through
/// [`Contract::call_with`](crate::Contract::call_with) (and the generated
/// `call_<name>` methods that wrap it).
#[doc(hidden)]
/// Build an unproven contract-call transaction. The `witness_ctx` parameter
/// threads the contract's loaded private state through any stateful witnesses
/// the circuit invokes — pass `Some(&mut ctx)` for cold-signing / custodian
/// flows where the caller wants to capture the post-call private state but
/// not submit. Passing `None` runs witnesses against a throwaway buffer whose
/// mutations are discarded (matches the behaviour before PSI support landed).
#[allow(clippy::too_many_arguments)]
pub fn build_unproven_call_tx<W: interpreter::WitnessProvider>(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    network_id: &str,
    args: &[(&str, interpreter::Value)],
    witnesses: &W,
    witness_ctx: Option<&mut interpreter::WitnessContext<'_>>,
    helpers: &[compact_codegen::ir::HelperDef],
) -> Result<UnprovenCallTx, ContractError> {
    use midnight_ledger::structure::{Intent, Transaction};
    use midnight_storage::storage::HashMap as StorageHashMap;
    use rand::Rng;

    let mut rng = rand::thread_rng();

    let exec_result = interpreter::execute_with_owned(
        ir,
        state.clone(),
        args,
        &[],
        witnesses,
        witness_ctx,
        helpers,
        &[],
        &[],
    )?;

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

    let input: AlignedValue = if args.is_empty() {
        ().into()
    } else {
        let arg_values: Vec<AlignedValue> =
            args.iter().map(|(_, v)| v.to_aligned_value()).collect();
        AlignedValue::concat(&arg_values)
    };
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
        private_transcript_outputs: exec_result.private_transcript_outputs,
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

/// Default timeout for waiting for transaction inclusion in a block.
pub(crate) const DEFAULT_TX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Default poll interval for checking transaction inclusion.
pub(crate) const DEFAULT_TX_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Wait until the indexer has processed a new block for a contract.
///
/// Polls `get_latest_contract_block_height` until the height exceeds
/// `height_before` (the height recorded before the transaction was submitted).
/// Pass `None` for `height_before` when the contract was just deployed and has
/// no prior block height.
pub(crate) async fn wait_for_contract_update<P: midnight_provider::Provider>(
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
                    None => true,
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

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_bindgen_runtime::{ContractMaintenanceAuthority, StateValue, StorageHashMap};

    fn make_counter_state(round: u64) -> ContractState<InMemoryDB> {
        ContractState::new(
            StateValue::Array(vec![StateValue::from(round)].into()),
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

        let result = build_unproven_call_tx(
            &ir,
            &state,
            "increment",
            address,
            "test-network",
            &[],
            &interpreter::NoWitnesses,
            None,
            &[],
        )
        .expect("build tx");

        assert!(
            !result.tx_bytes.is_empty(),
            "transaction bytes should not be empty"
        );
        eprintln!("unproven TX size: {} bytes", result.tx_bytes.len());

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
