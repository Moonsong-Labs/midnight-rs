//! Circuit call transaction builder.
//!
//! Wires the IR interpreter output to midnight-ledger's transaction
//! construction pipeline: interpreter → partition → intent → transaction.
//!
//! State reading, address parsing, and the deploy path live in
//! [`crate::state`], [`crate::address`], and [`crate::deploy`] respectively;
//! this module is purely call-side. A few helpers used by both paths
//! (`build_resolver`, `current_ttl`, `DEFAULT_TTL`) are exposed as
//! `pub(crate)` from here so `deploy` doesn't have to duplicate them.

use std::borrow::Cow;
use std::sync::Arc;

use midnight_base_crypto::hash::HashOutput;
use midnight_base_crypto::time::{Duration, Timestamp};
use midnight_coin_structure::coin::{Info as ZswapCoinInfo, Nonce, ShieldedTokenType};
use midnight_coin_structure::contract::ContractAddress;
use midnight_ledger::construct::ContractCallPrototype;
use midnight_ledger::structure::INITIAL_PARAMETERS;
use midnight_onchain_runtime::state::{ContractOperation, EntryPointBuf};
use midnight_serialize::tagged_serialize;
use midnight_transient_crypto::proofs::KeyLocation;
use midnight_typed_state::{AlignedValue, ContractState, InMemoryDB};

use crate::error::ContractError;
use crate::interpreter;
use crate::runtime;
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

/// Build a `Resolver` that loads proving keys from a [`ZkConfigProvider`].
///
/// Uses the `midnight_helpers` re-exported types so the resolver is compatible
/// with `LedgerContext::update_resolver` (which takes `Arc<Resolver>`).
///
/// The provider is queried per `KeyLocation` the ledger needs during proving;
/// [`ZkConfigError::NotFound`] means "not this contract's circuit" (dust/system
/// circuits resolve elsewhere), so it maps to `Ok(None)`. Provider lookups run
/// inside `spawn_blocking` because the ledger's `ExternalResolver` requires a
/// `Send + Sync` future and a blocking provider must not stall the runtime.
pub(crate) fn build_resolver(
    zk_config: Arc<dyn crate::zk_config::ZkConfigProvider>,
) -> Result<Arc<midnight_helpers::Resolver>, ContractError> {
    use midnight_helpers::{
        DUST_EXPECTED_FILES, DustResolver, FetchMode, MidnightDataProvider, OutputMode,
        PUBLIC_PARAMS, ProvingKeyMaterial, Resolver,
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
        let zk_config = zk_config.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let loc_str = loc.to_string();
                match zk_config.artifacts(&loc_str) {
                    Ok(a) => Ok(Some(ProvingKeyMaterial {
                        prover_key: a.prover_key,
                        verifier_key: a.verifier_key,
                        ir_source: a.zkir,
                    })),
                    Err(crate::zk_config::ZkConfigError::NotFound(_)) => Ok(None),
                    Err(e) => Err(std::io::Error::other(e)),
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

/// Shielded (Zswap) coins to attach to a contract call, funding a circuit's
/// shielded-token deficit (e.g. a `receiveShielded` on the caller's own coin)
/// from the caller's wallet.
///
/// The build pipeline auto-balances only Dust (fees), never shielded tokens, so
/// a circuit that receives a coin needs the coin attached as an external Zswap
/// input or the transaction fails to balance. This carries that input.
///
/// All coins come from the provider's single funding wallet. Spending coins
/// owned by other wallets (multi-party offers) is not supported yet: the call's
/// build context only holds the funding wallet, so any other origin's coin
/// selection would fail.
///
/// Built by the generated `Circuits` builder's `with_shielded_inputs`, and
/// accepted directly by [`Contract::call_with`](crate::Contract::call_with).
/// Empty by default (the common case: a call with no shielded input).
#[derive(Default)]
pub struct ShieldedInputs {
    /// Wallet coins to spend as pinned shielded inputs. Each is selected
    /// exactly by its nullifier (never amount-based, so `receiveShielded`'s
    /// re-committed coin matches) and routed to the segment of the circuit
    /// output it funds. See [`SpendableShieldedCoin`](midnight_wallet::SpendableShieldedCoin).
    pub coins: Vec<midnight_wallet::SpendableShieldedCoin>,
}

/// Reject any caller-provided shielded input whose nullifier is not in the
/// wallet's spendable set. Coin selection pins each input by exact nullifier
/// and panics (leaking wallet state to logs) when no owned coin matches, so we
/// turn a stale, unknown, or foreign coin into a typed error before building.
fn ensure_shielded_inputs_spendable(
    requested: &[midnight_wallet::SpendableShieldedCoin],
    owned: &[midnight_wallet::SpendableShieldedCoin],
) -> Result<(), ContractError> {
    for coin in requested {
        if !owned.iter().any(|c| c.nullifier == coin.nullifier) {
            return Err(ContractError::Construction(format!(
                "shielded input coin (nullifier {:?}) is not spendable by this wallet \
                 (already spent, not yet synced, or not owned by this wallet)",
                coin.nullifier
            )));
        }
    }
    Ok(())
}

/// The compiler-emitted static definition of a circuit: everything the
/// interpreter needs beyond the runtime argument values and the witness
/// provider. Generated bindings build this from the embedded contract-info
/// JSON; hand-written callers use `CircuitDefs::default()` for a circuit with
/// only scalar arguments and no helpers.
///
/// Bundling these four always-co-travelling slices keeps the call builders from
/// taking a row of interchangeable `&[]` parameters, where a caller could
/// silently transpose two of them.
#[derive(Clone, Copy, Default)]
pub struct CircuitDefs<'a> {
    /// Declared types of the circuit's arguments (`name -> type`), needed to
    /// slice a struct argument the circuit destructures with `Expr::Field`.
    pub arg_types: &'a [(&'a str, compact_codegen::ir::TypeRef)],
    /// Helper (pure) function definitions the circuit may call.
    pub helpers: &'a [compact_codegen::ir::HelperDef],
    /// Struct layouts referenced by the circuit's arguments or body.
    pub structs: &'a [compact_codegen::ir::StructDef],
    /// Enum layouts referenced by the circuit's arguments or body.
    pub enums: &'a [compact_codegen::ir::EnumDef],
    /// The circuit's declared result type. Drives the FAB encoding of the
    /// implicit communication output; without it a small `Field` result
    /// falls back to the 8-byte integer encoding and diverges from the
    /// canonical runtime's output binding.
    pub result_type: Option<&'a compact_codegen::ir::TypeRef>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn call_funded_with(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    provider: &midnight_provider::MidnightProvider,
    zk_config: Arc<dyn crate::zk_config::ZkConfigProvider>,
    args: &[(&str, runtime::Value)],
    witnesses: &dyn runtime::WitnessProvider,
    witness_ctx: Option<&mut runtime::WitnessContext<'_>>,
    defs: CircuitDefs<'_>,
    coin_encryption_keys: &[(
        midnight_helpers::CoinPublicKey,
        midnight_helpers::EncryptionPublicKey,
    )],
    shielded: ShieldedInputs,
    // When false, skip Dust funding: the call is built proven but fee-less, for
    // another wallet to sponsor (`MidnightProvider::balance_transaction`).
    pay_fees: bool,
) -> Result<(Vec<u8>, ContractState<InMemoryDB>, Option<runtime::Value>), ContractError> {
    use midnight_helpers::{
        BuildContractAction, BuildInput, BuildOutput, BuildTransient, DefaultDB, FromContext,
        InputInfo, IntentInfo, LedgerContext, OfferInfo, ProofProvider, StandardTrasactionInfo,
    };

    // 1. Execute the circuit IR locally for the updated state. When a
    //    `witness_ctx` is supplied it threads the contract's private state
    //    through any witness calls; after this returns its buffer holds the
    //    post-call private state. `None` means no private-state threading.
    let exec_result = interpreter::execute_with_owned(
        ir,
        state.clone(),
        args,
        defs.arg_types,
        witnesses,
        witness_ctx,
        defs.helpers,
        defs.structs,
        defs.enums,
        Some(contract_address),
        defs.result_type,
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

    // Commitments the ledger partitioned into the fallible section. A
    // circuit-created coin's Zswap output must ride in the same segment as the
    // transcript entry that claims it: `effects_check` keys claimed shielded
    // spends/receives by segment, so a coin the partition pushed into the
    // fallible transcript (the intent's segment) whose Output stayed in the
    // guaranteed offer (segment 0) fails `AllCommitmentsSubsetCheckFailure`
    // (node malformed error 186). Coins sent to a user land in
    // `claimed_shielded_spends`, contract-owned coins in
    // `claimed_shielded_receives`; union both. Collected here while the
    // transcripts are still owned, to route the outputs built below.
    let fallible_commitments: std::collections::HashSet<midnight_coin_structure::coin::Commitment> =
        fallible
            .as_ref()
            .map(|t| {
                t.effects
                    .claimed_shielded_spends
                    .iter()
                    .chain(t.effects.claimed_shielded_receives.iter())
                    .map(|c| **c)
                    .collect()
            })
            .unwrap_or_default();

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

    // Validate caller-provided shielded inputs against the wallet's spendable
    // set (the same coin source the build's `min_match_coin` reads, now that
    // `build_context` has synced). A stale, unknown, or foreign nullifier would
    // otherwise panic deep in coin selection and log wallet state; fail here
    // with a typed error instead.
    if !shielded.coins.is_empty() {
        let owned = provider.spendable_shielded_coins().await?;
        ensure_shielded_inputs_spendable(&shielded.coins, &owned)?;
    }

    // 4. Load proving keys into a Resolver and register with the context
    let resolver = build_resolver(zk_config)?;
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
    //    `midnight_typed_state::AlignedValue` (re-exported from the git-pinned
    //    midnight-base-crypto), while ContractCallPrototype expects the helpers'
    //    AlignedValue (a different crate version). Round-trip via serialization
    //    to cross that boundary, propagating any error here instead of from
    //    inside `build`.
    let input_av_local: AlignedValue = interpreter::encode_circuit_input(args, defs.arg_types)?;
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
    let proof_provider: Arc<dyn ProofProvider<DefaultDB>> = provider.proof_provider();
    let reserved_at = context.latest_block_context().tblock;
    let mut tx_info = StandardTrasactionInfo::new_from_context(context, proof_provider, None);
    tx_info.add_intent(1, Box::new(intent_info));
    // Attach a Zswap output for every coin the circuit created via
    // `createZswapOutput` (shielded mints/sends). Each carries the circuit's
    // exact coin, and a discovery ciphertext when the recipient's encryption
    // key was supplied via `with_coin_encryption_keys`.
    //
    // Route each coin to the offer for the segment its creating op was
    // partitioned into (see `fallible_commitments`): guaranteed coins stay in
    // the guaranteed offer, fallible coins ride at the intent's segment (1, set
    // via `add_intent` above). Segment must match or the tx fails
    // `AllCommitmentsSubsetCheckFailure`.
    // Coins the circuit asked to spend via `createZswapInput` (issue #122 gap 3
    // / the `sendShielded` path). Each is spent against `kernel.self()`, so
    // compute its contract-owned commitment to pair it with the matching
    // self-output below into a transient.
    let self_recipient = midnight_coin_structure::transfer::Recipient::Contract(contract_address);
    let mut pending_input_coins: Vec<(midnight_coin_structure::coin::Commitment, ZswapCoinInfo)> =
        Vec::with_capacity(exec_result.zswap_inputs.len());
    for zi in &exec_result.zswap_inputs {
        let coin = decode_shielded_input(zi)?;
        pending_input_coins.push((coin.commitment(&self_recipient), coin));
    }

    let mut guaranteed_outputs: Vec<Box<dyn BuildOutput<DefaultDB>>> = Vec::new();
    let mut fallible_outputs: Vec<Box<dyn BuildOutput<DefaultDB>>> = Vec::new();
    let mut guaranteed_transients: Vec<Box<dyn BuildTransient<DefaultDB>>> = Vec::new();
    let mut fallible_transients: Vec<Box<dyn BuildTransient<DefaultDB>>> = Vec::new();
    // Routing table for caller-provided shielded inputs: for each circuit
    // output, its token type and the segment it landed in (`true` = fallible /
    // segment 1). A caller coin funds the receive/output of the same token, so
    // its input must ride in that output's segment to balance per-segment.
    let mut output_segments: Vec<(ShieldedTokenType, bool)> = Vec::new();
    for (commitment, decoded, output) in
        build_shielded_offer_outputs(&exec_result.zswap_outputs, coin_encryption_keys)?
    {
        let is_fallible = fallible_commitments.contains(&commitment);
        output_segments.push((decoded.coin.type_, is_fallible));

        // A contract-owned output whose commitment matches a pending
        // `createZswapInput` is a coin created and spent in the same call: emit
        // it as a transient (bundled output + spend) instead of a plain output.
        let paired = if decoded.is_user {
            None
        } else {
            pending_input_coins
                .iter()
                .position(|(c, _)| *c == commitment)
        };
        if let Some(idx) = paired {
            pending_input_coins.remove(idx);
            let transient = Box::new(ContractOwnedTransient {
                coin: decoded.coin,
                contract: ContractAddress(decoded.recipient_key),
                segment: if is_fallible { 1 } else { 0 },
            }) as Box<dyn BuildTransient<DefaultDB>>;
            if is_fallible {
                fallible_transients.push(transient);
            } else {
                guaranteed_transients.push(transient);
            }
        } else if is_fallible {
            fallible_outputs.push(output);
        } else {
            guaranteed_outputs.push(output);
        }
    }

    // Any `createZswapInput` not paired with a same-call self-output spends a
    // coin already in the contract's Zswap state (a persistent contract-owned
    // spend), which needs the coin's Merkle path — not yet wired. Only spends of
    // coins the same call received (e.g. `receiveShielded` then
    // `sendImmediateShielded`) are handled today.
    if !pending_input_coins.is_empty() {
        return Err(ContractError::Construction(format!(
            "createZswapInput on {} coin(s) not created in the same call \
             (persistent contract-owned shielded spend) is not yet supported",
            pending_input_coins.len()
        )));
    }

    // Caller-provided shielded inputs (issue #122 gap 2). Build a pinned
    // `InputInfo` per coin from the funding seed + the coin's nullifier — so
    // coin selection spends that exact coin, matching a `receiveShielded`'s
    // re-committed nonce/color/value — and route each to the segment of the
    // circuit output it funds (default: guaranteed). The build pipeline then
    // balances the shielded offer from these inputs, the same coin-selection
    // path `transfer_shielded` uses.
    let mut guaranteed_inputs: Vec<Box<dyn BuildInput<DefaultDB>>> = Vec::new();
    let mut fallible_inputs: Vec<Box<dyn BuildInput<DefaultDB>>> = Vec::new();
    for coin in shielded.coins {
        let to_fallible = shielded_input_to_fallible(coin.token_type, &output_segments)?;
        let input: InputInfo<midnight_helpers::WalletSeed> = InputInfo {
            origin: wallet_seed.clone(),
            token_type: coin.token_type,
            value: coin.value,
            nullifier: Some(coin.nullifier),
        };
        if to_fallible {
            fallible_inputs.push(Box::new(input));
        } else {
            guaranteed_inputs.push(Box::new(input));
        }
    }

    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: guaranteed_inputs,
        outputs: guaranteed_outputs,
        transients: guaranteed_transients,
    });
    if !fallible_outputs.is_empty()
        || !fallible_inputs.is_empty()
        || !fallible_transients.is_empty()
    {
        tx_info.fallible_offers.insert(
            1,
            OfferInfo {
                inputs: fallible_inputs,
                outputs: fallible_outputs,
                transients: fallible_transients,
            },
        );
    }
    if pay_fees {
        tx_info.set_funding_seeds(vec![wallet_seed]);
    }
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
///
/// `defs` mirrors the funded call path: a circuit that destructures a struct
/// argument (e.g. `recipient.is_left` on an `Either`) needs the argument's
/// declared type plus the struct/enum layouts to slice it, otherwise execution
/// fails with an "unknown receiver type" field access. Pass
/// `CircuitDefs::default()` for circuits with only scalar arguments.
#[allow(clippy::too_many_arguments)]
pub fn build_unproven_call_tx<W: runtime::WitnessProvider>(
    ir: &CircuitIrBody,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: ContractAddress,
    network_id: &str,
    args: &[(&str, runtime::Value)],
    witnesses: &W,
    witness_ctx: Option<&mut runtime::WitnessContext<'_>>,
    defs: CircuitDefs<'_>,
) -> Result<UnprovenCallTx, ContractError> {
    use midnight_ledger::structure::{Intent, Transaction};
    use midnight_storage::storage::HashMap as StorageHashMap;
    use rand::Rng;

    let mut rng = rand::thread_rng();

    let exec_result = interpreter::execute_with_owned(
        ir,
        state.clone(),
        args,
        defs.arg_types,
        witnesses,
        witness_ctx,
        defs.helpers,
        defs.structs,
        defs.enums,
        Some(contract_address),
        defs.result_type,
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

    let input: AlignedValue = interpreter::encode_circuit_input(args, defs.arg_types)?;
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

/// Pick the segment a caller-provided shielded input should ride in: the same
/// segment as the circuit output it funds, matched by token type. Zswap balances
/// per `(token_type, segment)`, so an input that funds a `receiveShielded` in the
/// fallible segment must itself be fallible, or the tx fails to balance.
///
/// `output_segments` pairs each circuit-created output's token type with whether
/// it was partitioned into the fallible segment. Returns `Ok(true)` for fallible
/// (segment 1), `Ok(false)` for guaranteed (segment 0); defaults to guaranteed
/// when no circuit output matches the token (nothing to co-locate with).
///
/// Errors when the circuit creates outputs of this token in *both* segments:
/// the input could fund either, so the segment is ambiguous and silently picking
/// one would risk an opaque per-segment balance failure. Callers that hit this
/// need a way to name the intended segment (not exposed yet).
fn shielded_input_to_fallible(
    token_type: ShieldedTokenType,
    output_segments: &[(ShieldedTokenType, bool)],
) -> Result<bool, ContractError> {
    let mut in_guaranteed = false;
    let mut in_fallible = false;
    for (tt, is_fallible) in output_segments {
        if *tt == token_type {
            if *is_fallible {
                in_fallible = true;
            } else {
                in_guaranteed = true;
            }
        }
    }
    match (in_guaranteed, in_fallible) {
        (true, true) => Err(ContractError::Construction(format!(
            "cannot route shielded input for token {}: the circuit creates outputs of this token \
             in both the guaranteed and fallible segments, so the input's segment is ambiguous",
            hex::encode(token_type.0.0)
        ))),
        (false, true) => Ok(true),
        // Guaranteed match, or no match at all: ride in the guaranteed segment.
        _ => Ok(false),
    }
}

/// A circuit-created shielded coin (`createZswapOutput`) decoded into the
/// fields a Zswap offer `Output` needs.
#[derive(Clone, Copy)]
pub(crate) struct DecodedShieldedOutput {
    /// The coin to mint into the output (nonce, token type/color, value).
    pub coin: ZswapCoinInfo,
    /// `true` => external user recipient (`ZswapCoinPublicKey`); `false` =>
    /// contract recipient (`ContractAddress`).
    pub is_user: bool,
    /// The recipient's 32-byte key: coin public key (user) or address
    /// (contract).
    pub recipient_key: HashOutput,
}

/// Read FAB atom `idx` of `av` as a zero-padded 32-byte value. FAB atoms are
/// zero-trimmed, so a `Bytes<32>`/`HashOutput` atom may be shorter than 32
/// bytes; pad on the right to recover the fixed-width value.
///
/// `what` is the full field context used verbatim in error messages (e.g.
/// `"shielded output: coin.nonce"`), so callers pass whether it is an input or
/// an output; this helper stays agnostic.
fn aligned_atom_bytes32(
    av: &AlignedValue,
    idx: usize,
    what: &str,
) -> Result<[u8; 32], ContractError> {
    let atom = av
        .value
        .0
        .get(idx)
        .ok_or_else(|| ContractError::Construction(format!("{what} missing FAB atom {idx}")))?;
    if atom.0.len() > 32 {
        return Err(ContractError::Construction(format!(
            "{what} atom is {} bytes, wider than 32",
            atom.0.len()
        )));
    }
    let mut out = [0u8; 32];
    out[..atom.0.len()].copy_from_slice(&atom.0);
    Ok(out)
}

/// Decode a captured [`CircuitZswapOutput`] (the `(coin, recipient)` args of a
/// `createZswapOutput` call) into the fields `Output::new` needs.
///
/// `coin` is the interpreter's value of a `ShieldedCoinInfo { nonce: Bytes<32>,
/// color: Bytes<32>, value: Uint<128> }` struct (three FAB atoms); `recipient`
/// is an `Either { is_left: Boolean, left: ZswapCoinPublicKey, right:
/// ContractAddress }` (three atoms). The decoded coin fields are byte-identical
/// to what the circuit hashed, so `Output::new` re-derives the same
/// `coin_com` the proof commits to.
pub(crate) fn decode_shielded_output(
    output: &runtime::CircuitZswapOutput,
) -> Result<DecodedShieldedOutput, ContractError> {
    let coin_av = match &output.coin {
        runtime::Value::AlignedValue(av) => av,
        other => {
            return Err(ContractError::Construction(format!(
                "shielded output coin is not a struct-encoded value: {other:?}"
            )));
        }
    };
    let nonce = aligned_atom_bytes32(coin_av, 0, "shielded output: coin.nonce")?;
    let color = aligned_atom_bytes32(coin_av, 1, "shielded output: coin.color")?;
    let value_atom = coin_av.value.0.get(2).ok_or_else(|| {
        ContractError::Construction("shielded output: coin.value missing FAB atom 2".into())
    })?;
    if value_atom.0.len() > 16 {
        return Err(ContractError::Construction(format!(
            "shielded output: coin.value atom is {} bytes, wider than a Uint<128>",
            value_atom.0.len()
        )));
    }
    let mut value_bytes = [0u8; 16];
    value_bytes[..value_atom.0.len()].copy_from_slice(&value_atom.0);
    let value = u128::from_le_bytes(value_bytes);

    let recipient_av = match &output.recipient {
        runtime::Value::AlignedValue(av) => av,
        other => {
            return Err(ContractError::Construction(format!(
                "shielded output recipient is not a struct-encoded value: {other:?}"
            )));
        }
    };
    // Either.is_left: a Boolean FAB atom — `[1]` for true, empty (trimmed) for
    // false.
    let is_left_atom = recipient_av.value.0.first().ok_or_else(|| {
        ContractError::Construction("shielded output: recipient.is_left missing".into())
    })?;
    let is_user = is_left_atom.0.first().copied() == Some(1);
    let recipient_key = if is_user {
        aligned_atom_bytes32(recipient_av, 1, "shielded output: recipient.left")?
    } else {
        aligned_atom_bytes32(recipient_av, 2, "shielded output: recipient.right")?
    };

    Ok(DecodedShieldedOutput {
        coin: ZswapCoinInfo {
            nonce: Nonce(HashOutput(nonce)),
            type_: ShieldedTokenType(HashOutput(color)),
            value,
        },
        is_user,
        recipient_key: HashOutput(recipient_key),
    })
}

/// Decode a captured [`CircuitZswapInput`](runtime::CircuitZswapInput) (the coin
/// arg of a `createZswapInput` call) into the coin the circuit spends.
///
/// The value is a `QualifiedShieldedCoinInfo { nonce: Bytes<32>, color:
/// Bytes<32>, value: Uint<128>, mt_index: Uint<64> }` (four FAB atoms). Only the
/// `nonce`/`color`/`value` are needed to re-derive the spent coin's commitment;
/// `mt_index` is ignored (it is `0` for a coin upcast from a plain
/// `ShieldedCoinInfo`, i.e. one not in the historical Merkle tree, and the
/// same-call self-output it pairs with sits at index 0 of a fresh transient
/// tree).
fn decode_shielded_input(
    input: &runtime::CircuitZswapInput,
) -> Result<ZswapCoinInfo, ContractError> {
    let coin_av = match &input.coin {
        runtime::Value::AlignedValue(av) => av,
        other => {
            return Err(ContractError::Construction(format!(
                "shielded input coin is not a struct-encoded value: {other:?}"
            )));
        }
    };
    let nonce = aligned_atom_bytes32(coin_av, 0, "shielded input: coin.nonce")?;
    let color = aligned_atom_bytes32(coin_av, 1, "shielded input: coin.color")?;
    let value_atom = coin_av.value.0.get(2).ok_or_else(|| {
        ContractError::Construction("shielded input: coin.value missing FAB atom 2".into())
    })?;
    if value_atom.0.len() > 16 {
        return Err(ContractError::Construction(format!(
            "shielded input: coin.value atom is {} bytes, wider than a Uint<128>",
            value_atom.0.len()
        )));
    }
    let mut value_bytes = [0u8; 16];
    value_bytes[..value_atom.0.len()].copy_from_slice(&value_atom.0);
    let value = u128::from_le_bytes(value_bytes);

    Ok(ZswapCoinInfo {
        nonce: Nonce(HashOutput(nonce)),
        type_: ShieldedTokenType(HashOutput(color)),
        value,
    })
}

/// Where a circuit-minted coin's Zswap output goes.
enum MintRecipient {
    /// External user: their coin public key, plus an optional encryption public
    /// key. When the `epk` is present the output carries a discovery ciphertext
    /// so the recipient's wallet finds the coin through normal sync (no
    /// `watchFor`); without it the output still lands on-chain but the recipient
    /// must already know the coin out of band.
    User {
        cpk: midnight_helpers::CoinPublicKey,
        epk: Option<midnight_helpers::EncryptionPublicKey>,
    },
    /// Contract recipient (e.g. a mint-to-self branch): a contract-owned output.
    Contract(ContractAddress),
}

/// A [`midnight_helpers::BuildOutput`] that emits the exact coin a circuit
/// created via `createZswapOutput` into a Zswap offer. Unlike the wallet's
/// `OutputInfo` (which mints a fresh coin), this carries the circuit's exact
/// `CoinInfo`, so the output's `coin_com` equals the commitment the proof
/// claims (`claimed_shielded_spends`).
struct MintedCoinOutput {
    coin: ZswapCoinInfo,
    token_type: ShieldedTokenType,
    value: u128,
    recipient: MintRecipient,
}

impl midnight_helpers::TokenInfo for MintedCoinOutput {
    fn token_type(&self) -> ShieldedTokenType {
        self.token_type
    }
    fn value(&self) -> u128 {
        self.value
    }
}

impl midnight_helpers::BuildOutput<midnight_helpers::DefaultDB> for MintedCoinOutput {
    fn build(
        &self,
        rng: &mut midnight_helpers::StdRng,
        _context: Arc<midnight_helpers::LedgerContext<midnight_helpers::DefaultDB>>,
    ) -> midnight_helpers::Output<midnight_helpers::ProofPreimage, midnight_helpers::DefaultDB>
    {
        match &self.recipient {
            MintRecipient::User { cpk, epk } => midnight_helpers::Output::new(
                rng,
                &self.coin,
                midnight_helpers::Segment::Guaranteed.into(),
                cpk,
                *epk,
            )
            .expect("circuit-minted user coin output must be constructible"),
            MintRecipient::Contract(addr) => midnight_helpers::Output::new_contract_owned(
                rng,
                &self.coin,
                midnight_helpers::Segment::Guaranteed.into(),
                *addr,
            )
            .expect("circuit-minted contract-owned coin output must be constructible"),
        }
    }
}

/// A contract-owned Zswap transient: a coin the circuit both created
/// (`createZswapOutput` to `kernel.self()`) and spent (`createZswapInput`)
/// within the same call — `receiveShielded` immediately followed by
/// `sendImmediateShielded` is the motivating case. The coin never enters the
/// historical Merkle tree, so it rides as a transient (its output and spending
/// input bundled) rather than a separate output plus a tree-spending input.
struct ContractOwnedTransient {
    /// The exact coin the circuit created and spent (byte-identical to what it
    /// hashed), so the transient's commitment/nullifier match the transcript's
    /// claimed receive/spend effects.
    coin: ZswapCoinInfo,
    /// The owning contract (`kernel.self()`): recipient of the created output
    /// and origin of the spend.
    contract: ContractAddress,
    /// The segment the coin's create/spend ops partitioned into (0 = guaranteed,
    /// 1 = the call's fallible segment). The input and output halves share it.
    segment: u16,
}

impl midnight_helpers::BuildTransient<midnight_helpers::DefaultDB> for ContractOwnedTransient {
    fn build(
        &self,
        rng: &mut midnight_helpers::StdRng,
        _context: Arc<midnight_helpers::LedgerContext<midnight_helpers::DefaultDB>>,
    ) -> midnight_helpers::Transient<midnight_helpers::ProofPreimage, midnight_helpers::DefaultDB>
    {
        // Build the contract-owned output first, then derive the transient from
        // it: `new_from_contract_owned_output` seeds a fresh 1-leaf Merkle tree
        // with this output's commitment and spends it back, so the created coin
        // is consumed within the same tx without ever entering the chain tree.
        let output = midnight_helpers::Output::new_contract_owned(
            rng,
            &self.coin,
            Some(self.segment),
            self.contract,
        )
        .expect("contract-owned transient output must be constructible");
        midnight_helpers::Transient::new_from_contract_owned_output(
            rng,
            &self.coin.qualify(0),
            Some(self.segment),
            output,
        )
        .expect("contract-owned transient must be constructible")
    }
}

/// A circuit-created Zswap offer output, its coin commitment, and the decoded
/// coin/recipient. The commitment lets [`call_funded_with`] route the output to
/// the offer for the segment the ledger partitioned the coin's creating op into,
/// and match it against a `createZswapInput` to form a transient; the decoded
/// fields let it build the transient's coin when they pair.
type ShieldedOfferOutput = (
    midnight_coin_structure::coin::Commitment,
    DecodedShieldedOutput,
    Box<dyn midnight_helpers::BuildOutput<midnight_helpers::DefaultDB>>,
);

/// Turn the coins a circuit created via `createZswapOutput` into Zswap offer
/// outputs, each paired with its coin commitment. For each circuit-created coin
/// sent to an external user whose coin public key is in `enc_keys`, the matching
/// encryption public key is attached so the recipient discovers the coin through
/// normal sync (no `watchFor`).
///
/// The commitment is derived with the same coin-structure `Info::commitment` the
/// on-chain runtime used to record the transcript's claimed effect, so the two
/// match by construction and the caller can route each output by segment.
fn build_shielded_offer_outputs(
    zswap_outputs: &[runtime::CircuitZswapOutput],
    enc_keys: &[(
        midnight_helpers::CoinPublicKey,
        midnight_helpers::EncryptionPublicKey,
    )],
) -> Result<Vec<ShieldedOfferOutput>, ContractError> {
    // Index the mappings once so the per-output lookup is O(1); keyed by the
    // coin public key's raw bytes (`HashOutput` inner array).
    let epk_by_cpk: std::collections::HashMap<[u8; 32], midnight_helpers::EncryptionPublicKey> =
        enc_keys.iter().map(|(cpk, epk)| (cpk.0.0, *epk)).collect();
    let mut outputs: Vec<ShieldedOfferOutput> = Vec::with_capacity(zswap_outputs.len());
    for zo in zswap_outputs {
        let decoded = decode_shielded_output(zo)?;
        let token_type = decoded.coin.type_;
        let value = decoded.coin.value;
        let commitment = {
            let recipient = if decoded.is_user {
                midnight_coin_structure::transfer::Recipient::User(
                    midnight_coin_structure::coin::PublicKey(decoded.recipient_key),
                )
            } else {
                midnight_coin_structure::transfer::Recipient::Contract(ContractAddress(
                    decoded.recipient_key,
                ))
            };
            decoded.coin.commitment(&recipient)
        };
        let recipient = if decoded.is_user {
            let epk = epk_by_cpk.get(&decoded.recipient_key.0).copied();
            MintRecipient::User {
                cpk: midnight_helpers::CoinPublicKey(decoded.recipient_key),
                epk,
            }
        } else {
            MintRecipient::Contract(ContractAddress(decoded.recipient_key))
        };
        outputs.push((
            commitment,
            decoded,
            Box::new(MintedCoinOutput {
                coin: decoded.coin,
                token_type,
                value,
                recipient,
            }) as Box<dyn midnight_helpers::BuildOutput<midnight_helpers::DefaultDB>>,
        ));
    }
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{CircuitZswapOutput, Value};
    use midnight_typed_state::{ContractMaintenanceAuthority, StateValue, StorageHashMap};

    /// A captured `createZswapOutput` coin (a `ShieldedCoinInfo` struct: nonce,
    /// color, value) and an `Either::left(cpk)` recipient must decode into the
    /// fields a Zswap `Output` needs: the coin nonce/type/value and the user
    /// recipient's coin public key. (Commitment-match against a real proof is a
    /// devnet-E2E concern; this pins the FAB decode mechanics.)
    #[test]
    fn decode_shielded_output_extracts_coin_and_user_recipient() {
        let nonce = [2u8; 32];
        let color = [3u8; 32];
        let value: u128 = 1000;
        let cpk = [4u8; 32];

        let coin = Value::AlignedValue(AlignedValue::concat(
            [
                AlignedValue::from(nonce),
                AlignedValue::from(color),
                AlignedValue::from(value),
            ]
            .iter(),
        ));
        let recipient = Value::AlignedValue(AlignedValue::concat(
            [
                AlignedValue::from(true),
                AlignedValue::from(cpk),
                AlignedValue::from([0u8; 32]),
            ]
            .iter(),
        ));

        let decoded = decode_shielded_output(&CircuitZswapOutput { coin, recipient })
            .expect("decode must succeed");

        assert_eq!(decoded.coin.nonce.0.0, nonce);
        assert_eq!(decoded.coin.type_.0.0, color);
        assert_eq!(decoded.coin.value, value);
        assert!(decoded.is_user, "Either::left is a user recipient");
        assert_eq!(decoded.recipient_key.0, cpk);
    }

    /// The commitment `build_shielded_offer_outputs` returns is the routing key
    /// `call_funded_with` matches against the transcript's claimed effects to
    /// pick a coin's offer segment. It must equal the coin's real
    /// `Info::commitment` for the decoded coin + recipient — the same function
    /// the on-chain runtime uses to record the effect — or the coin would be
    /// mis-routed and trip `AllCommitmentsSubsetCheckFailure`.
    #[test]
    fn build_shielded_offer_outputs_returns_coin_commitment() {
        let nonce = [7u8; 32];
        let color = [8u8; 32];
        let value: u128 = 4200;
        let cpk = [9u8; 32];

        let coin = Value::AlignedValue(AlignedValue::concat(
            [
                AlignedValue::from(nonce),
                AlignedValue::from(color),
                AlignedValue::from(value),
            ]
            .iter(),
        ));
        let recipient = Value::AlignedValue(AlignedValue::concat(
            [
                AlignedValue::from(true),
                AlignedValue::from(cpk),
                AlignedValue::from([0u8; 32]),
            ]
            .iter(),
        ));

        let outputs = build_shielded_offer_outputs(&[CircuitZswapOutput { coin, recipient }], &[])
            .expect("build must succeed");
        assert_eq!(outputs.len(), 1);

        let expected = ZswapCoinInfo {
            nonce: Nonce(HashOutput(nonce)),
            type_: ShieldedTokenType(HashOutput(color)),
            value,
        }
        .commitment(&midnight_coin_structure::transfer::Recipient::User(
            midnight_coin_structure::coin::PublicKey(HashOutput(cpk)),
        ));
        assert_eq!(outputs[0].0, expected);
    }

    /// A captured `createZswapInput` coin is a `QualifiedShieldedCoinInfo`
    /// (nonce, color, value, mt_index). Decoding drops `mt_index` and recovers
    /// the coin's nonce/color/value so its contract-owned commitment can be
    /// re-derived to pair with a same-call self-output.
    #[test]
    fn decode_shielded_input_extracts_coin_dropping_mt_index() {
        let nonce = [5u8; 32];
        let color = [6u8; 32];
        let value: u128 = 777;
        let mt_index: u64 = 42;

        let coin = Value::AlignedValue(AlignedValue::concat(
            [
                AlignedValue::from(nonce),
                AlignedValue::from(color),
                AlignedValue::from(value),
                AlignedValue::from(mt_index),
            ]
            .iter(),
        ));

        let decoded = decode_shielded_input(&crate::runtime::CircuitZswapInput { coin })
            .expect("decode must succeed");
        assert_eq!(decoded.nonce.0.0, nonce);
        assert_eq!(decoded.type_.0.0, color);
        assert_eq!(decoded.value, value);
    }

    fn tt(byte: u8) -> ShieldedTokenType {
        ShieldedTokenType(HashOutput([byte; 32]))
    }

    /// A caller's shielded input rides in the same segment as the circuit output
    /// it funds: a guaranteed receive → guaranteed input, a fallible receive →
    /// fallible input. Zswap balances per `(token, segment)`, so a mismatch here
    /// would leave the tx unbalanced.
    #[test]
    fn shielded_input_segment_matches_funded_output() {
        // Guaranteed output of the coin's token → input stays guaranteed.
        assert!(!shielded_input_to_fallible(tt(1), &[(tt(1), false)]).unwrap());
        // Fallible output of the coin's token → input must be fallible too.
        assert!(shielded_input_to_fallible(tt(1), &[(tt(1), true)]).unwrap());
    }

    /// With no circuit output of the coin's token to co-locate with, the input
    /// defaults to the guaranteed segment.
    #[test]
    fn shielded_input_defaults_to_guaranteed_without_match() {
        assert!(!shielded_input_to_fallible(tt(9), &[]).unwrap());
        // A different token's fallible output must not pull this input fallible.
        assert!(!shielded_input_to_fallible(tt(9), &[(tt(1), true)]).unwrap());
    }

    /// When the circuit creates outputs of the coin's token in *both* segments,
    /// the input's segment is ambiguous and routing must error rather than
    /// silently pick one and risk an opaque per-segment balance failure.
    #[test]
    fn shielded_input_ambiguous_segment_errors() {
        let err = shielded_input_to_fallible(tt(1), &[(tt(1), false), (tt(1), true)]).unwrap_err();
        assert!(
            matches!(err, ContractError::Construction(ref m) if m.contains("ambiguous")),
            "got {err:?}"
        );
    }

    fn spendable_coin(nullifier_byte: u8) -> midnight_wallet::SpendableShieldedCoin {
        midnight_wallet::SpendableShieldedCoin {
            token_type: tt(0),
            value: 1,
            nonce: [0u8; 32],
            nullifier: midnight_helpers::Nullifier(HashOutput([nullifier_byte; 32])),
        }
    }

    /// A caller can hand `call_with` any coin (public fields), and a coin can go
    /// stale between enumeration and build. An input whose nullifier isn't in
    /// the wallet's spendable set must fail with a typed error, not panic in
    /// coin selection.
    #[test]
    fn shielded_inputs_reject_unspendable_coin() {
        let owned = vec![spendable_coin(1), spendable_coin(2)];

        // Every requested coin is owned → ok. Empty request → ok.
        ensure_shielded_inputs_spendable(&[spendable_coin(1)], &owned).unwrap();
        ensure_shielded_inputs_spendable(&[], &owned).unwrap();

        // A coin the wallet doesn't hold → typed error, no panic.
        let err = ensure_shielded_inputs_spendable(&[spendable_coin(9)], &owned).unwrap_err();
        assert!(
            matches!(err, ContractError::Construction(ref m) if m.contains("not spendable")),
            "got {err:?}"
        );
    }

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
            &runtime::NoWitnesses,
            None,
            CircuitDefs::default(),
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
