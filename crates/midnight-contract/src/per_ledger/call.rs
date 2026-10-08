//! A circuit call's transaction on this generation: the interpreter's result
//! partitioned into this generation's transcripts, its coins in Zswap offers,
//! proven, and funded.

use std::borrow::Cow;
use std::sync::Arc;

use super::helpers;
use super::provider::Builds;
use super::types::convert::IntoLedger;
use helpers::coin_structure::coin::{
    Info as ZswapCoinInfo, Nonce, PublicAddress, ShieldedTokenType, TokenType, UnshieldedTokenType,
    UserAddress,
};
use helpers::{
    AlignedValue, BuildContext, BuildContractAction, BuildInput, BuildOutput, BuildTransient,
    BuildUtxoOutput, BuilderContext, ContractAddress, ContractCallPrototype, DefaultDB,
    EntryPointBuf, FromContext, HashOutput, IntentInfo, KeyLocation, OfferInfo, ProofPreimage,
    SplittableRng, StandardTransactionInfo, TokenInfo, Transcript, UnshieldedOfferInfo, UtxoOutput,
};
use midnight_base_crypto::time::Timestamp;
use midnight_provider::ProviderError;
use midnight_typed_state::{ContractState, InMemoryDB};

use crate::call::ShieldedInputs;
use crate::error::ContractError;
use crate::runtime;

/// An unshielded payout to an address the call's transcript names.
///
/// The upstream `UtxoOutputInfo` builders derive the owner from a seed, which
/// a contract's payout recipient is not: the transcript carries the address
/// itself.
/// The outputs that pay what a transcript claims to send.
///
/// Verification refuses a claimed unshielded spend with no real one behind it,
/// so every claim a call makes needs its output here. A contract recipient
/// accounts for its own balance, and Dust never rides an unshielded offer, so
/// neither is collected.
fn payouts_of(transcript: Option<&Transcript<DefaultDB>>) -> Vec<PayoutOutput> {
    let Some(transcript) = transcript else {
        return Vec::new();
    };
    transcript
        .effects
        .claimed_unshielded_spends
        .iter()
        .filter_map(|kv| {
            let (TokenType::Unshielded(token_type), PublicAddress::User(owner)) = kv.0.into_inner()
            else {
                return None;
            };
            Some(PayoutOutput {
                value: *kv.1,
                owner,
                token_type,
            })
        })
        .collect()
}

struct PayoutOutput {
    value: u128,
    owner: UserAddress,
    token_type: UnshieldedTokenType,
}

impl<D: helpers::DB + Clone, C: helpers::BuilderContext<D>> BuildUtxoOutput<D, C> for PayoutOutput {
    fn build(&self, _context: Arc<C>) -> UtxoOutput {
        UtxoOutput {
            value: self.value,
            owner: self.owner,
            type_: self.token_type,
        }
    }
}

/// The context that the partition replays a call's ops in: the contract
/// state with its balance, at the block time that the interpreter ran at.
///
/// The partition checks each read that the ops recorded, so a clock or
/// balance check that reads another value here fails the build.
fn partition_context(
    state: &helpers::ContractState<DefaultDB>,
    address: ContractAddress,
    block_time: Timestamp,
) -> helpers::QueryContext<DefaultDB> {
    let mut context = helpers::QueryContext::new(state.data.clone(), address);
    context.call_context.tblock = block_time;
    context.call_context.balance = state.balance.clone();
    context
}

/// Build this generation's transaction for a circuit call the interpreter
/// already ran against `state`, and prove it. When `pay_fees` is false the
/// transaction is left fee-less, for another wallet to sponsor.
///
/// `state` is the contract's state as the Compact side reads it, and
/// `state_bytes` the encoding the chain served it in. `block_time` is the
/// time the interpreter ran the circuit at.
#[expect(
    clippy::too_many_arguments,
    reason = "the neutral call path hands over what the interpreter ran with, one value each"
)]
pub(crate) async fn call_transaction(
    builds: &Builds<'_>,
    circuit: &compact_codegen::ir::Circuit,
    args: &[(&str, runtime::Value)],
    exec_result: &runtime::ExecutionResult,
    state: &ContractState<InMemoryDB>,
    state_bytes: &[u8],
    block_time: Timestamp,
    circuit_name: &str,
    contract_address: midnight_types::ContractAddress,
    zk_config: Arc<dyn crate::zk_config::ZkConfigProvider>,
    coin_encryption_keys: &[(
        midnight_types::CoinPublicKey,
        midnight_types::EncryptionPublicKey,
    )],
    shielded: ShieldedInputs,
    pay_fees: bool,
) -> Result<Vec<u8>, ContractError> {
    let helper_addr: ContractAddress = contract_address.into_ledger();
    let state_db = super::state::native_state(state, state_bytes)?;

    // 1. The build context, from the provider's synced wallet. Nothing here
    //    needs the seed: the wallet performs the spends, and addressing a coin
    //    takes public keys.
    let (change_cpk, change_epk) = builds.provider().shielded_public_keys().await?;
    let (change_cpk, change_epk) = (change_cpk.into_ledger(), change_epk.into_ledger());
    let context = builds.execution_context().await?;

    // 2. Build transcripts by partitioning the circuit's state ops, budgeted
    //    with the chain's own parameters: the node checks the gas each
    //    transcript declares against its current cost model.
    let program = crate::call::verify_ops(exec_result)
        .iter()
        .map(|op| crate::call::reencode(op, "circuit op"))
        .collect::<Result<Vec<helpers::Op<helpers::ResultModeVerify, DefaultDB>>, _>>()?;
    let pre_transcript = helpers::PreTranscript {
        context: partition_context(&state_db, helper_addr, block_time),
        program,
        comm_comm: None,
    };
    let chain_parameters = BuilderContext::ledger_parameters(&*context).await;
    let partitioned = helpers::partition_transcripts(&[pre_transcript], &chain_parameters)
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
    // `claimed_shielded_receives`; union both.
    let fallible_commitments: std::collections::HashSet<helpers::coin_structure::coin::Commitment> =
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

    // Value this call mints, per (token, segment). The ledger credits a mint
    // against the offer's balance, so a circuit output covered by one draws
    // nothing from the attached coins and must not be charged to them.
    let mut minted_value: std::collections::BTreeMap<(ShieldedTokenType, bool), u128> =
        std::collections::BTreeMap::new();
    for (transcript, is_fallible) in [(guaranteed.as_ref(), false), (fallible.as_ref(), true)] {
        let Some(transcript) = transcript else {
            continue;
        };
        for kv in transcript.effects.shielded_mints.iter() {
            let token = helper_addr.custom_shielded_token_type(*kv.0);
            let minted = minted_value.entry((token, is_fallible)).or_default();
            *minted = minted.saturating_add(u128::from(*kv.1));
        }
    }

    let guaranteed_payouts = payouts_of(guaranteed.as_ref());
    let fallible_payouts = payouts_of(fallible.as_ref());

    // 3. Make the circuit's proving keys resolvable, and prove with the
    //    resolver that finds them.
    let key_location =
        crate::resolver::register(&hex::encode(contract_address.0.0), circuit_name, &zk_config)?;
    context.update_resolver(super::resolver::shared()).await;

    let entry_point: EntryPointBuf = circuit_name.as_bytes().into();
    let op = match state_db.operations.get(&entry_point) {
        Some(op) => (*op).clone(),
        None => helpers::contract_operation_new(None, None)
            .map_err(|e| ContractError::Construction(format!("empty operation: {e}")))?,
    };

    // 3b. Insert the contract into the context's ledger state so client-side
    //     well_formed() validation can find it. The indexed wallet state doesn't
    //     include deployed contracts.
    {
        let mut guard = context
            .ledger_state
            .lock()
            .map_err(|_| ContractError::Construction("ledger_state lock poisoned".into()))?;
        let mut ls = (**guard).clone();
        ls.contract = ls.contract.insert(helper_addr, state_db.clone());
        *guard = helpers::Sp::new(ls);
    }

    // 4. The circuit's input and output. Witness private values become the
    //    prototype's private transcript outputs (the ZKIR's private inputs).
    //    Without these, proving a witness-using circuit fails with "ran out of
    //    private transcript outputs".
    let arg_types: Vec<(&str, compact_codegen::ir::Type)> = circuit
        .arguments
        .iter()
        .map(|a| (a.name.name(), a.ty.clone()))
        .collect();
    let input: AlignedValue = crate::interpreter::encode_circuit_input(args, &arg_types)?;
    let output: AlignedValue = if exec_result.communication_outputs.is_empty() {
        ().into()
    } else {
        AlignedValue::concat(&exec_result.communication_outputs)
    };
    let private_transcript_outputs = exec_result.private_transcript_outputs.clone();

    // 5. Build the call action holding only typed values; `build` is infallible.
    struct CallAction {
        address: ContractAddress,
        entry_point: EntryPointBuf,
        op: helpers::ContractOperation,
        input: AlignedValue,
        output: AlignedValue,
        key_location: String,
        guaranteed_transcript: Option<Transcript<DefaultDB>>,
        fallible_transcript: Option<Transcript<DefaultDB>>,
        private_transcript_outputs: Vec<AlignedValue>,
    }

    #[async_trait::async_trait]
    impl<C: BuilderContext<DefaultDB>> BuildContractAction<DefaultDB, C> for CallAction {
        async fn build(
            &mut self,
            rng: &mut helpers::StdRng,
            _context: std::sync::Arc<C>,
            intent: &helpers::Intent<
                helpers::Signature,
                helpers::ProofPreimageMarker,
                helpers::PedersenRandomness,
                DefaultDB,
            >,
        ) -> helpers::Intent<
            helpers::Signature,
            helpers::ProofPreimageMarker,
            helpers::PedersenRandomness,
            DefaultDB,
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
                key_location: KeyLocation(Cow::Owned(self.key_location.clone())),
            };

            intent.add_call::<ProofPreimage>(call)
        }
    }

    let call_action = CallAction {
        address: helper_addr,
        entry_point,
        op,
        input,
        output,
        key_location,
        guaranteed_transcript: guaranteed,
        fallible_transcript: fallible,
        private_transcript_outputs,
    };

    // Outputs only: the contract's own balance funds the payout, so this
    // transaction has nothing to spend.
    let offer = |payouts: Vec<PayoutOutput>| {
        (!payouts.is_empty()).then(|| UnshieldedOfferInfo {
            inputs: Vec::new(),
            outputs: payouts
                .into_iter()
                .map(|p| Box::new(p) as Box<dyn BuildUtxoOutput<DefaultDB, BuildContext>>)
                .collect(),
        })
    };

    let intent_info: IntentInfo<DefaultDB, BuildContext> = IntentInfo {
        guaranteed_unshielded_offer: offer(guaranteed_payouts),
        fallible_unshielded_offer: offer(fallible_payouts),
        actions: vec![Box::new(call_action)],
    };

    // 6. Build funded transaction with Dust fees and real ZK proofs.
    //
    // Stages 1 to 5 run against a context that carries no wallet, so the
    // circuit, its proving keys and the contract's own state never depend on
    // who pays. The payer joins here.
    builds.add_funding(&context).await?;
    let build_context = context.clone();
    let mut tx_info =
        StandardTransactionInfo::new_from_context(context, builds.proof_provider(), None);
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
    let self_recipient = helpers::coin_structure::transfer::Recipient::Contract(helper_addr);
    let mut pending_input_coins: Vec<(helpers::coin_structure::coin::Commitment, ZswapCoinInfo)> =
        Vec::with_capacity(exec_result.zswap_inputs.len());
    for zi in &exec_result.zswap_inputs {
        let coin = decode_shielded_input(zi)?;
        pending_input_coins.push((coin.commitment(&self_recipient), coin));
    }

    let mut guaranteed_outputs: Vec<Box<dyn BuildOutput<DefaultDB, BuildContext>>> = Vec::new();
    let mut fallible_outputs: Vec<Box<dyn BuildOutput<DefaultDB, BuildContext>>> = Vec::new();
    let mut guaranteed_transients: Vec<Box<dyn BuildTransient<DefaultDB, BuildContext>>> =
        Vec::new();
    let mut fallible_transients: Vec<Box<dyn BuildTransient<DefaultDB, BuildContext>>> = Vec::new();
    // Routing table for caller-provided shielded inputs: for each circuit
    // output, its token type and the segment it landed in (`true` = fallible /
    // segment 1). A caller coin funds the receive/output of the same token, so
    // its input must ride in that output's segment to balance per-segment.
    let mut output_segments: Vec<(ShieldedTokenType, bool)> = Vec::new();
    // Value the circuit's own outputs draw per (token, segment), against which
    // the attached coins are reconciled below. Transients are excluded: their
    // output and input halves cancel, so they consume none of the attached
    // value.
    let mut circuit_output_value: std::collections::BTreeMap<(ShieldedTokenType, bool), u128> =
        std::collections::BTreeMap::new();
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
            }) as Box<dyn BuildTransient<DefaultDB, BuildContext>>;
            if is_fallible {
                fallible_transients.push(transient);
            } else {
                guaranteed_transients.push(transient);
            }
        } else {
            let drawn = circuit_output_value
                .entry((decoded.coin.type_, is_fallible))
                .or_default();
            *drawn = drawn.checked_add(decoded.coin.value).ok_or_else(|| {
                ContractError::Construction(
                    "circuit shielded output values overflow a u128 total".into(),
                )
            })?;
            if is_fallible {
                fallible_outputs.push(output);
            } else {
                guaranteed_outputs.push(output);
            }
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

    // The wallet spends each named coin into the context and reserves it under
    // one hold, so no other build can pin the same coin, and it reports the
    // coin's own token and value, which route the input and size the change.
    // Runs after the funding view is in the context: reserving before it
    // would hide these coins from it.
    let (prepared, mut pinned) = builds
        .prepare_shielded_inputs(&build_context, &shielded.coins, &mut tx_info.rng.split())
        .await?;
    let mut guaranteed_inputs: Vec<Box<dyn BuildInput<DefaultDB, BuildContext>>> = Vec::new();
    let mut fallible_inputs: Vec<Box<dyn BuildInput<DefaultDB, BuildContext>>> = Vec::new();
    let mut attached_value: std::collections::BTreeMap<(ShieldedTokenType, bool), u128> =
        std::collections::BTreeMap::new();
    for input in prepared {
        let to_fallible = shielded_input_to_fallible(input.token_type(), &output_segments)?;
        let attached = attached_value
            .entry((input.token_type(), to_fallible))
            .or_default();
        *attached = attached.checked_add(input.value()).ok_or_else(|| {
            ContractError::Construction(
                "attached shielded coin values overflow a u128 total".into(),
            )
        })?;
        if to_fallible {
            fallible_inputs.push(Box::new(input));
        } else {
            guaranteed_inputs.push(Box::new(input));
        }
    }

    // Return whatever the attached coins carry beyond what the circuit's outputs
    // draw. Zswap asserts only `balance >= 0` per (token, segment), so a surplus
    // is accepted and destroyed; a change output back to the caller's own wallet
    // makes the delta exactly zero and lets a caller fund a receive of N from
    // coins totalling more than N. The change rides in the segment of the inputs
    // that produced it, since the ledger balances per (token, segment).
    for (token_type, is_fallible, change) in
        caller_change(&attached_value, &circuit_output_value, &minted_value)
    {
        let output = Box::new(CallerChangeOutput {
            coin_public_key: change_cpk,
            enc_public_key: change_epk,
            token_type,
            value: change,
        }) as Box<dyn BuildOutput<DefaultDB, BuildContext>>;
        if is_fallible {
            fallible_outputs.push(output);
        } else {
            guaranteed_outputs.push(output);
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
    // Build fee-less, even when this call pays for itself.
    //
    // Handing the funding seed to the wallet's fee-balancing fixpoint makes it
    // rebuild the candidate from the unpaid transaction and re-prove the whole
    // thing on every iteration, circuit included, and the first iteration always
    // requests zero Dust so it always reports a shortfall and loops. Proving is
    // the most expensive operation in the SDK, so the circuit's proof is paid
    // for once per iteration rather than once per call. Dust is attached below,
    // after proving, in its own intent segment, which leaves the circuit proof
    // untouched. Deploy and maintenance are unaffected: they mock-prove while
    // balancing, which a user circuit cannot do (upstream `MockProver::check`
    // rejects non-builtin circuits).

    let built = super::types::build_no_validate(tx_info)
        .await
        .map_err(ProviderError::Wallet)?;

    let mut bytes = Vec::new();
    helpers::midnight_serialize::tagged_serialize(&built.finalized, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("{e}")))?;
    // Drop the built transaction before the next await. Holding it across one
    // makes it part of this function's async state machine, and it is large
    // enough that the resulting frame overflows the stack on debug builds.
    // There are no Dust batches to carry over: with no funding seeds the build
    // above draws no Dust, and `balance_transaction` reserves whatever it draws.
    drop(built);

    // Fund the already-proven transaction the same way a sponsor funds someone
    // else's: the fee intent rides its own segment, so nothing here re-proves
    // the circuit.
    //
    // Boxed so the balancing future is heap-allocated rather than inlined into
    // this one. Awaiting it directly reserves room for it in this function's
    // async state machine whether or not the branch runs, and this frame is
    // already close enough to the limit that it overflows the stack on debug
    // builds.
    if pay_fees {
        bytes = Box::pin(builds.balance_transaction(&bytes)).await?;
    }

    // The transaction is built, so the pinned coins stay reserved.
    pinned.keep();

    Ok(bytes)
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

/// The attached value each (token, segment) carries beyond what the circuit's
/// own outputs draw from it, as `(token, is_fallible, change)`.
///
/// An output covered by a mint is funded by the ledger's mint credit rather
/// than by the attached coins, so only the uncovered remainder is charged to
/// them. Charging the whole output instead would leave the caller's coin
/// unaccounted, and the offer's balance would still be non-negative, so the
/// chain would accept the transaction and destroy it.
///
/// A circuit that draws more than was attached yields nothing here. That
/// shortfall makes the offer unbalanced, which the fee-paying step refuses
/// before submitting.
fn caller_change(
    attached_value: &std::collections::BTreeMap<(ShieldedTokenType, bool), u128>,
    circuit_output_value: &std::collections::BTreeMap<(ShieldedTokenType, bool), u128>,
    minted_value: &std::collections::BTreeMap<(ShieldedTokenType, bool), u128>,
) -> Vec<(ShieldedTokenType, bool, u128)> {
    let mut change = Vec::new();
    for ((token_type, is_fallible), attached) in attached_value {
        let key = (*token_type, *is_fallible);
        let drawn = circuit_output_value.get(&key).copied().unwrap_or(0);
        let minted = minted_value.get(&key).copied().unwrap_or(0);
        let Some(owed) = attached
            .checked_sub(drawn.saturating_sub(minted))
            .filter(|c| *c > 0)
        else {
            continue;
        };
        change.push((*token_type, *is_fallible, owed));
    }
    change
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
        cpk: helpers::CoinPublicKey,
        epk: Option<helpers::EncryptionPublicKey>,
    },
    /// Contract recipient (e.g. a mint-to-self branch): a contract-owned output.
    Contract(ContractAddress),
}

/// A [`helpers::BuildOutput`] that emits the exact coin a circuit
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

impl helpers::TokenInfo for MintedCoinOutput {
    fn token_type(&self) -> ShieldedTokenType {
        self.token_type
    }
    fn value(&self) -> u128 {
        self.value
    }
}

impl helpers::BuildOutput<helpers::DefaultDB, helpers::BuildContext> for MintedCoinOutput {
    fn build(
        &self,
        rng: &mut helpers::StdRng,
        _context: Arc<helpers::BuildContext>,
    ) -> helpers::Output<helpers::ProofPreimage, helpers::DefaultDB> {
        match &self.recipient {
            MintRecipient::User { cpk, epk } => helpers::Output::new(
                rng,
                &self.coin,
                helpers::Segment::Guaranteed.into(),
                cpk,
                *epk,
            )
            .expect("circuit-minted user coin output must be constructible"),
            MintRecipient::Contract(addr) => helpers::Output::new_contract_owned(
                rng,
                &self.coin,
                helpers::Segment::Guaranteed.into(),
                *addr,
            )
            .expect("circuit-minted contract-owned coin output must be constructible"),
        }
    }
}

/// A change output returning the attached shielded value a circuit's outputs
/// did not draw to the caller's own wallet.
///
/// A fresh coin carrying a discovery ciphertext sealed to the caller's own
/// encryption key, so the wallet finds the change through normal sync.
///
/// Addressing a coin needs only public material, so this holds the two public
/// keys rather than the seed the helpers' `OutputInfo<WalletSeed>` takes. That
/// keeps the type usable by a signer that never releases its seed, and it is
/// why the change is discovered through its ciphertext rather than registered
/// with `watch_for`, which would need the wallet behind the seed.
struct CallerChangeOutput {
    coin_public_key: helpers::CoinPublicKey,
    enc_public_key: helpers::EncryptionPublicKey,
    token_type: ShieldedTokenType,
    value: u128,
}

impl helpers::TokenInfo for CallerChangeOutput {
    fn token_type(&self) -> ShieldedTokenType {
        self.token_type
    }
    fn value(&self) -> u128 {
        self.value
    }
}

impl helpers::BuildOutput<helpers::DefaultDB, helpers::BuildContext> for CallerChangeOutput {
    fn build(
        &self,
        rng: &mut helpers::StdRng,
        _context: Arc<helpers::BuildContext>,
    ) -> helpers::Output<helpers::ProofPreimage, helpers::DefaultDB> {
        let coin = ZswapCoinInfo::new(rng, self.value, self.token_type);
        helpers::Output::new(
            rng,
            &coin,
            helpers::Segment::Guaranteed.into(),
            &self.coin_public_key,
            Some(self.enc_public_key),
        )
        .expect("caller change output must be constructible")
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

impl helpers::BuildTransient<helpers::DefaultDB, helpers::BuildContext> for ContractOwnedTransient {
    fn build(
        &self,
        rng: &mut helpers::StdRng,
        _context: Arc<helpers::BuildContext>,
    ) -> helpers::Transient<helpers::ProofPreimage, helpers::DefaultDB> {
        // Build the contract-owned output first, then derive the transient from
        // it: `new_from_contract_owned_output` seeds a fresh 1-leaf Merkle tree
        // with this output's commitment and spends it back, so the created coin
        // is consumed within the same tx without ever entering the chain tree.
        let output =
            helpers::Output::new_contract_owned(rng, &self.coin, Some(self.segment), self.contract)
                .expect("contract-owned transient output must be constructible");
        helpers::Transient::new_from_contract_owned_output(
            rng,
            &self.coin.qualify(0),
            Some(self.segment),
            output,
        )
        .expect("contract-owned transient must be constructible")
    }
}

/// A circuit-created Zswap offer output, its coin commitment, and the decoded
/// coin/recipient. The commitment lets [`call_transaction`] route the output to
/// the offer for the segment the ledger partitioned the coin's creating op into,
/// and match it against a `createZswapInput` to form a transient; the decoded
/// fields let it build the transient's coin when they pair.
type ShieldedOfferOutput = (
    helpers::coin_structure::coin::Commitment,
    DecodedShieldedOutput,
    Box<dyn helpers::BuildOutput<helpers::DefaultDB, helpers::BuildContext>>,
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
        midnight_types::CoinPublicKey,
        midnight_types::EncryptionPublicKey,
    )],
) -> Result<Vec<ShieldedOfferOutput>, ContractError> {
    // Index the mappings once so the per-output lookup is O(1); keyed by the
    // coin public key's raw bytes (`HashOutput` inner array).
    let epk_by_cpk: std::collections::HashMap<[u8; 32], helpers::EncryptionPublicKey> = enc_keys
        .iter()
        .map(|(cpk, epk)| (cpk.0.0, epk.into_ledger()))
        .collect();
    let mut outputs: Vec<ShieldedOfferOutput> = Vec::with_capacity(zswap_outputs.len());
    for zo in zswap_outputs {
        let decoded = decode_shielded_output(zo)?;
        let token_type = decoded.coin.type_;
        let value = decoded.coin.value;
        let commitment = {
            let recipient = if decoded.is_user {
                helpers::coin_structure::transfer::Recipient::User(
                    helpers::coin_structure::coin::PublicKey(decoded.recipient_key),
                )
            } else {
                helpers::coin_structure::transfer::Recipient::Contract(ContractAddress(
                    decoded.recipient_key,
                ))
            };
            decoded.coin.commitment(&recipient)
        };
        let recipient = if decoded.is_user {
            let epk = epk_by_cpk.get(&decoded.recipient_key.0).copied();
            MintRecipient::User {
                cpk: helpers::CoinPublicKey(decoded.recipient_key),
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
            })
                as Box<dyn helpers::BuildOutput<helpers::DefaultDB, helpers::BuildContext>>,
        ));
    }
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{CircuitZswapOutput, Value};

    /// A transcript claiming one spend of `token` by `recipient`.
    fn transcript_claiming(
        token: TokenType,
        recipient: PublicAddress,
        value: u128,
    ) -> Transcript<DefaultDB> {
        use helpers::onchain_runtime::context::{ClaimedUnshieldedSpendsKey, Effects};
        let mut effects = Effects::<DefaultDB>::default();
        effects.claimed_unshielded_spends = effects.claimed_unshielded_spends.insert(
            ClaimedUnshieldedSpendsKey::from_inner(token, recipient),
            value,
        );
        Transcript {
            gas: Default::default(),
            effects,
            program: Default::default(),
            version: None,
        }
    }

    fn a_user() -> helpers::coin_structure::coin::UserAddress {
        helpers::coin_structure::coin::UserAddress(HashOutput([7; 32]))
    }

    fn a_token() -> UnshieldedTokenType {
        UnshieldedTokenType(HashOutput([9; 32]))
    }

    #[test]
    fn a_claimed_payout_to_a_user_becomes_an_output() {
        let transcript = transcript_claiming(
            TokenType::Unshielded(a_token()),
            PublicAddress::User(a_user()),
            42,
        );
        let payouts = payouts_of(Some(&transcript));

        assert_eq!(payouts.len(), 1, "the claim needs an output to cover it");
        assert_eq!(payouts[0].value, 42);
        assert_eq!(payouts[0].owner, a_user());
        assert_eq!(payouts[0].token_type, a_token());
    }

    #[test]
    fn claims_that_need_no_output_are_left_out() {
        // A contract recipient accounts for its own balance.
        let to_contract = transcript_claiming(
            TokenType::Unshielded(a_token()),
            PublicAddress::Contract(ContractAddress(HashOutput([3; 32]))),
            42,
        );
        assert!(payouts_of(Some(&to_contract)).is_empty());

        // Dust never rides an unshielded offer.
        let dust = transcript_claiming(TokenType::Dust, PublicAddress::User(a_user()), 42);
        assert!(payouts_of(Some(&dust)).is_empty());

        // A shielded token is covered by a Zswap offer, not this one.
        let shielded = transcript_claiming(
            TokenType::Shielded(ShieldedTokenType(HashOutput([5; 32]))),
            PublicAddress::User(a_user()),
            42,
        );
        assert!(payouts_of(Some(&shielded)).is_empty());
    }

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
        .commitment(&helpers::coin_structure::transfer::Recipient::User(
            helpers::coin_structure::coin::PublicKey(HashOutput(cpk)),
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

    /// The change output exists to return a surplus, so it must not appear when
    /// the attached coins fund the circuit exactly. An extra output there would
    /// push the offer's per-segment delta negative and the node would reject the
    /// call.
    #[test]
    fn caller_change_is_empty_without_a_surplus() {
        let attached = std::collections::BTreeMap::from([((tt(1), false), 10u128)]);
        let minted = std::collections::BTreeMap::new();

        let exact = std::collections::BTreeMap::from([((tt(1), false), 10u128)]);
        assert!(caller_change(&attached, &exact, &minted).is_empty());

        // A shortfall is the node's to report, not ours to turn into an output.
        let over = std::collections::BTreeMap::from([((tt(1), false), 11u128)]);
        assert!(caller_change(&attached, &over, &minted).is_empty());
    }

    /// A surplus comes back in the segment its inputs rode in, and only the
    /// surplus. A token the circuit draws nothing of comes back whole.
    #[test]
    fn caller_change_returns_the_surplus_per_token_and_segment() {
        let attached =
            std::collections::BTreeMap::from([((tt(1), false), 10u128), ((tt(2), true), 7u128)]);
        let drawn = std::collections::BTreeMap::from([((tt(1), false), 4u128)]);
        let minted = std::collections::BTreeMap::new();

        let mut change = caller_change(&attached, &drawn, &minted);
        change.sort_by_key(|(t, _, _)| t.0.0);
        assert_eq!(change, vec![(tt(1), false, 6), (tt(2), true, 7)]);
    }

    /// A mint credit funds the circuit output it covers, so charging that
    /// output to the attached coins would leave the caller's value
    /// unaccounted. The offer's balance stays non-negative either way, so the
    /// chain accepts the transaction and the difference is destroyed.
    #[test]
    fn caller_change_does_not_charge_mint_funded_outputs_to_the_caller() {
        let attached = std::collections::BTreeMap::from([((tt(1), false), 50u128)]);

        // Mints 100 and pays it straight out: the caller funds none of it, so
        // the whole attached coin comes back.
        let drawn = std::collections::BTreeMap::from([((tt(1), false), 100u128)]);
        let minted = std::collections::BTreeMap::from([((tt(1), false), 100u128)]);
        assert_eq!(
            caller_change(&attached, &drawn, &minted),
            vec![(tt(1), false, 50)]
        );

        // Mints 100 and pays out 150: the caller funds the uncovered 50, so
        // nothing is owed back.
        let drawn = std::collections::BTreeMap::from([((tt(1), false), 150u128)]);
        assert!(caller_change(&attached, &drawn, &minted).is_empty());

        // Mints 100 and pays out 200: the caller is 50 short, which the
        // balancing step reports rather than this function.
        let drawn = std::collections::BTreeMap::from([((tt(1), false), 200u128)]);
        assert!(caller_change(&attached, &drawn, &minted).is_empty());
    }
}
