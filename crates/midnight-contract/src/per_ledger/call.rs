//! A circuit call's transaction on this generation: the interpreter's result
//! partitioned into this generation's transcripts, its coins in Zswap offers,
//! proven, and funded.

use std::borrow::Cow;
use std::sync::Arc;

use super::helpers;
use super::provider::Builds;
use super::types::convert::{IntoLedger, IntoSdk};
use helpers::coin_structure::coin::{
    Info as ZswapCoinInfo, PublicAddress, PublicKey, QualifiedInfo, ShieldedTokenType, TokenType,
};
use helpers::coin_structure::transfer::Recipient;
use helpers::{
    AlignedValue, BuildContext, BuildContractAction, BuildUtxoOutput, BuilderContext,
    ContractAddress, ContractCallPrototype, DefaultDB, EntryPointBuf, FromContext, IntentInfo,
    KeyLocation, OfferInfo, OutputInfo, ProofPreimage, ShieldedWallet, SplittableRng,
    StandardTransactionInfo, TokenInfo, Transcript, UnshieldedOfferInfo, UnshieldedWallet,
    UtxoOutputInfo,
};
use midnight_base_crypto::fab::{InvalidBuiltinDecode, ValueSlice};
use midnight_base_crypto::time::Timestamp;
use midnight_provider::ProviderError;
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::TransferResult;

use super::types::PreparedInput;
use crate::address::format_contract_address;
use crate::call::ShieldedInputs;
use crate::change::{change_due, total_by_key};
use crate::error::ContractError;
use crate::runtime;

/// Draw the segment of a call's intent. The call's fallible Zswap offer and
/// fallible transients go in the same segment.
///
/// A merge refuses two intents at one segment, so each call draws its own
/// segment, as midnight-js `Transaction.fromPartsRandomized` does. The range
/// leaves out the guaranteed segment and
/// [`INTENT_SEGMENT`](super::deploy::INTENT_SEGMENT), which a deploy, a
/// maintenance update and an unshielded transfer use. So a call also merges
/// with each of those.
pub(crate) fn call_segment(rng: &mut impl rand::Rng) -> helpers::SegmentId {
    rng.gen_range(2..u16::MAX)
}

/// The outputs that pay what a transcript claims to send.
///
/// Verification refuses a claimed unshielded spend with no real one behind it,
/// so every claim a call makes needs its output here. A contract recipient
/// accounts for its own balance, and Dust never rides an unshielded offer, so
/// neither is collected.
fn payouts_of(transcript: Option<&Transcript<DefaultDB>>) -> Vec<UtxoOutputInfo<UnshieldedWallet>> {
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
            Some(UtxoOutputInfo {
                value: *kv.1,
                owner: UnshieldedWallet::from(owner),
                token_type,
            })
        })
        .collect()
}

/// Partition a call's ops, `program`, into its guaranteed and fallible
/// transcripts under `parameters`.
///
/// The partition replays the ops in a context of the contract state with its
/// balance, at the block time that the interpreter ran at. It checks each
/// read that the ops recorded, so a clock or balance check that reads another
/// value here fails the build.
pub(crate) fn partition(
    state: &helpers::ContractState<DefaultDB>,
    address: ContractAddress,
    block_time: Timestamp,
    program: Vec<helpers::Op<helpers::ResultModeVerify, DefaultDB>>,
    parameters: &helpers::LedgerParameters,
) -> Result<helpers::mn_ledger::construct::TranscriptPair<DefaultDB>, ContractError> {
    let mut context = helpers::QueryContext::new(state.data.clone(), address);
    context.call_context.tblock = block_time;
    context.call_context.balance = state.balance.clone();
    let pre_transcript = helpers::PreTranscript {
        context,
        program,
        comm_comm: None,
    };
    let partitioned = helpers::partition_transcripts(&[pre_transcript], parameters)
        .map_err(|e| ContractError::Construction(format!("partition: {e:?}")))?;
    Ok(partitioned.into_iter().next().unwrap_or((None, None)))
}

/// Build this generation's transaction for a circuit call the interpreter
/// already ran against `state`, and prove it. When `pay_fees` is false the
/// transaction is left fee-less, for another wallet to sponsor, and the result
/// prices the fee-less transaction.
///
/// The result names the inputs that the build reserved: the pinned coins of
/// `shielded`, and the Dust of the fee.
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
    coin_encryption_keys: &[midnight_types::ShieldedRecipient],
    shielded: ShieldedInputs,
    pay_fees: bool,
) -> Result<TransferResult, ContractError> {
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
    let chain_parameters = BuilderContext::ledger_parameters(&*context).await;
    let (guaranteed, fallible) = partition(
        &state_db,
        helper_addr,
        block_time,
        program,
        &chain_parameters,
    )?;

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
    let mut mints = Vec::new();
    for (transcript, is_fallible) in [(guaranteed.as_ref(), false), (fallible.as_ref(), true)] {
        let Some(transcript) = transcript else {
            continue;
        };
        for kv in transcript.effects.shielded_mints.iter() {
            let token = helper_addr.custom_shielded_token_type(*kv.0);
            mints.push(((token, is_fallible), u128::from(*kv.1)));
        }
    }
    let minted_value = total_by_key(mints)?;

    let guaranteed_payouts = payouts_of(guaranteed.as_ref());
    let fallible_payouts = payouts_of(fallible.as_ref());

    // 3. Make the circuit's proving keys resolvable, and prove with the
    //    resolver that finds them.
    let key_location = crate::resolver::register(
        &format_contract_address(&contract_address),
        circuit_name,
        &zk_config,
    )?;
    context.update_resolver(super::resolver::shared()).await;

    let entry_point: EntryPointBuf = circuit_name.as_bytes().into();
    let op = match state_db.operations.get(&entry_point) {
        Some(op) => (*op).clone(),
        None => helpers::contract_operation_new(None, None)
            .map_err(|e| ContractError::Construction(format!("empty operation: {e}")))?,
    };

    // 4. The circuit's input and output. Witness private values become the
    //    prototype's private transcript outputs (the ZKIR's private inputs).
    //    Without these, proving a witness-using circuit fails with "ran out of
    //    private transcript outputs".
    let input = crate::call::call_input(circuit, args)?;
    let output = exec_result.output();
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
            intent: &helpers::UnprovenIntent<DefaultDB>,
        ) -> helpers::UnprovenIntent<DefaultDB> {
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
    let offer = |payouts: Vec<UtxoOutputInfo<UnshieldedWallet>>| {
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
    let segment = call_segment(&mut tx_info.rng);
    tx_info.add_intent(segment, Box::new(intent_info));
    // Attach a Zswap output for every coin the circuit created via
    // `createZswapOutput` (shielded mints/sends). Each carries the circuit's
    // exact coin, and a discovery ciphertext when `build_shielded_offer_outputs`
    // knows the recipient's encryption key.
    //
    // Route each coin to the offer for the segment its creating op was
    // partitioned into (see `fallible_commitments`): guaranteed coins stay in
    // the guaranteed offer, fallible coins ride at the intent's segment.
    // Segment must match or the tx fails `AllCommitmentsSubsetCheckFailure`.
    // Coins the circuit asked to spend via `createZswapInput` (issue #122 gap 3
    // / the `sendShielded` path). Each is spent against `kernel.self()`, so
    // compute its contract-owned commitment to pair it with the matching
    // self-output below into a transient.
    let self_recipient = Recipient::Contract(helper_addr);
    let mut pending_input_coins: Vec<(helpers::coin_structure::coin::Commitment, ZswapCoinInfo)> =
        Vec::with_capacity(exec_result.zswap_inputs.len());
    for zi in &exec_result.zswap_inputs {
        let coin = decode_shielded_input(zi)?;
        pending_input_coins.push((coin.commitment(&self_recipient), coin));
    }

    let mut offers = SegmentOffers::new(segment);
    // Routing table for caller-provided shielded inputs: for each circuit
    // output, its token type and the segment it landed in (`true` = fallible,
    // at the call's segment). A caller coin funds the receive/output of the same
    // token, so its input must ride in that output's segment to balance
    // per-segment.
    let mut output_segments: Vec<(ShieldedTokenType, bool)> = Vec::new();
    // Value the circuit's own outputs draw per (token, segment), against which
    // the attached coins are reconciled below. Transients are excluded: their
    // output and input halves cancel, so they consume none of the attached
    // value.
    let mut drawn = Vec::new();
    for (commitment, output) in build_shielded_offer_outputs(
        &exec_result.zswap_outputs,
        coin_encryption_keys,
        (change_cpk, change_epk),
    )? {
        let is_fallible = fallible_commitments.contains(&commitment);
        output_segments.push((output.coin.type_, is_fallible));

        // A contract-owned output whose commitment matches a pending
        // `createZswapInput` is a coin created and spent in the same call: emit
        // it as a transient (bundled output + spend) instead of a plain output.
        let paired = match output.recipient {
            MintRecipient::Contract(contract) => pending_input_coins
                .iter()
                .position(|(c, _)| *c == commitment)
                .map(|idx| (idx, contract)),
            MintRecipient::User { .. } => None,
        };
        if let Some((idx, contract)) = paired {
            pending_input_coins.remove(idx);
            offers
                .at(is_fallible)
                .transients
                .push(Box::new(ContractOwnedTransient {
                    coin: output.coin,
                    contract,
                    segment: if is_fallible {
                        segment
                    } else {
                        helpers::Segment::Guaranteed.into()
                    },
                }));
        } else {
            drawn.push(((output.coin.type_, is_fallible), output.coin.value));
            offers.at(is_fallible).outputs.push(Box::new(output));
        }
    }
    let circuit_output_value = total_by_key(drawn)?;

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
    let (prepared, held) = builds
        .prepare_shielded_inputs(&build_context, &shielded.coins, &mut tx_info.rng.split())
        .await?;
    let circuit_offers = CircuitOffers {
        offers,
        output_segments,
        circuit_output_value,
        minted_value,
    };
    let pinned = held.spent().clone();
    let mut transaction = held
        .keep_on_ok(attach_and_prove(
            builds,
            tx_info,
            circuit_offers,
            prepared,
            (change_cpk, change_epk),
            pay_fees,
        ))
        .await?;
    transaction.spent.merge(pinned);
    Ok(transaction)
}

/// The circuit's side of a call's Zswap offers, before the caller's coins join
/// them.
///
/// `output_segments` routes those coins, and the two value maps size their
/// change.
struct CircuitOffers {
    offers: SegmentOffers,
    output_segments: Vec<(ShieldedTokenType, bool)>,
    circuit_output_value: std::collections::BTreeMap<(ShieldedTokenType, bool), u128>,
    minted_value: std::collections::BTreeMap<(ShieldedTokenType, bool), u128>,
}

/// A call's two Zswap offers: the guaranteed one, and the fallible one at
/// `fallible_segment`, the segment of the call's intent.
struct SegmentOffers {
    guaranteed: OfferInfo<DefaultDB, BuildContext>,
    fallible: OfferInfo<DefaultDB, BuildContext>,
    fallible_segment: helpers::SegmentId,
}

impl SegmentOffers {
    fn new(fallible_segment: helpers::SegmentId) -> Self {
        let empty = || OfferInfo {
            inputs: Vec::new(),
            outputs: Vec::new(),
            transients: Vec::new(),
        };
        Self {
            guaranteed: empty(),
            fallible: empty(),
            fallible_segment,
        }
    }

    fn at(&mut self, fallible: bool) -> &mut OfferInfo<DefaultDB, BuildContext> {
        if fallible {
            &mut self.fallible
        } else {
            &mut self.guaranteed
        }
    }
}

/// Attach the reserved `prepared` coins to the circuit's offers, then prove the
/// call.
///
/// Any surplus goes back to the wallet as change. When `pay_fees`, the call
/// also pays its own fee.
async fn attach_and_prove(
    builds: &Builds<'_>,
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    circuit_offers: CircuitOffers,
    prepared: Vec<PreparedInput>,
    (change_cpk, change_epk): (helpers::CoinPublicKey, helpers::EncryptionPublicKey),
    pay_fees: bool,
) -> Result<TransferResult, ContractError> {
    let CircuitOffers {
        mut offers,
        output_segments,
        circuit_output_value,
        minted_value,
    } = circuit_offers;
    let mut attached = Vec::new();
    for input in prepared {
        let to_fallible = shielded_input_to_fallible(input.token_type(), &output_segments)?;
        attached.push(((input.token_type(), to_fallible), input.value()));
        offers.at(to_fallible).inputs.push(Box::new(input));
    }
    let attached_value = total_by_key(attached)?;

    // Return whatever the attached coins carry beyond what the circuit's outputs
    // draw. Zswap asserts only `balance >= 0` per (token, segment), so a surplus
    // is accepted and destroyed; a change output back to the caller's own wallet
    // makes the delta exactly zero and lets a caller fund a receive of N from
    // coins totalling more than N. The change rides in the segment of the inputs
    // that produced it, since the ledger balances per (token, segment).
    for ((token_type, is_fallible), change) in
        change_due(&attached_value, &circuit_output_value, &minted_value)
    {
        offers.at(is_fallible).outputs.push(Box::new(OutputInfo {
            destination: ShieldedWallet::<DefaultDB>::from_pub_keys(change_cpk, change_epk),
            token_type,
            value: change,
        }));
    }

    let SegmentOffers {
        guaranteed,
        fallible,
        fallible_segment,
    } = offers;
    tx_info.set_guaranteed_offer(guaranteed);
    if !fallible.outputs.is_empty()
        || !fallible.inputs.is_empty()
        || !fallible.transients.is_empty()
    {
        tx_info.fallible_offers.insert(fallible_segment, fallible);
    }
    // Build fee-less, even when this call pays for itself.
    //
    // A fee balanced in the same transaction prices each round with a proof.
    // `mock_prove` refuses a user circuit, and a real proof would prove the
    // circuit again on every round. Dust is attached below, after proving, in
    // its own intent segment, which leaves the circuit proof untouched. A
    // transaction with no user circuit can take
    // `super::deploy::build_with_mock_fee_proofs` instead.
    let unpaid = prove_without_fee(tx_info).await?;
    if !pay_fees {
        return Ok(unpaid);
    }

    // Fund the already-proven transaction the same way a sponsor funds someone
    // else's: the fee intent rides its own segment, so nothing here re-proves
    // the circuit.
    Ok(builds.balance_transaction(&unpaid.tx_bytes).await?)
}

/// Build and prove `tx_info` with no fee, and serialize and price the result.
///
/// `tx_info` must carry no funding seed. The build then draws no Dust, so it
/// leaves nothing to reserve, and the result names no input.
/// `Builds::balance_transaction` reserves the Dust that it draws for the fee.
pub(super) async fn prove_without_fee(
    tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
) -> Result<TransferResult, ContractError> {
    let prepared = super::types::prepare_no_validate(tx_info)
        .await
        .map_err(ProviderError::Wallet)?;
    Ok(prepared.prove().await.map_err(ProviderError::Wallet)?)
}

/// Pick the segment a caller-provided shielded input should ride in: the same
/// segment as the circuit output it funds, matched by token type. Zswap balances
/// per `(token_type, segment)`, so an input that funds a `receiveShielded` in the
/// fallible segment must itself be fallible, or the tx fails to balance.
///
/// `output_segments` pairs each circuit-created output's token type with whether
/// it was partitioned into the fallible segment. Returns `Ok(true)` for fallible
/// (the call's segment), `Ok(false)` for guaranteed; defaults to guaranteed
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
            token_type.into_sdk()
        ))),
        (false, true) => Ok(true),
        // Guaranteed match, or no match at all: ride in the guaranteed segment.
        _ => Ok(false),
    }
}

/// A circuit-created shielded coin (`createZswapOutput`) decoded into the
/// fields a Zswap offer `Output` needs.
#[derive(Clone)]
pub(crate) struct DecodedShieldedOutput {
    /// The coin to mint into the output (nonce, token type/color, value).
    pub coin: ZswapCoinInfo,
    /// The external user (`ZswapCoinPublicKey`) or the contract
    /// (`ContractAddress`) that receives the coin.
    pub recipient: Recipient,
}

/// Decode `value`, a struct the interpreter captured, as `T`. `what` names the
/// capture in the error.
fn decode<T>(value: &runtime::Value, what: &str) -> Result<T, ContractError>
where
    T: for<'a> TryFrom<&'a ValueSlice, Error = InvalidBuiltinDecode>,
{
    let runtime::Value::AlignedValue(av) = value else {
        return Err(ContractError::Construction(format!(
            "{what} is not a struct-encoded value: {value:?}"
        )));
    };
    T::try_from(&*av.value)
        .map_err(|_| ContractError::Construction(format!("{what} does not decode: {value:?}")))
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
    let coin = decode(&output.coin, "shielded output coin")?;
    // A tuple, not upstream `Recipient`: the commitment hashes only the chosen
    // side, so the unused side need not be empty.
    let (is_left, user, contract): (bool, PublicKey, ContractAddress) =
        decode(&output.recipient, "shielded output recipient")?;
    Ok(DecodedShieldedOutput {
        coin,
        recipient: if is_left {
            Recipient::User(user)
        } else {
            Recipient::Contract(contract)
        },
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
    let coin: QualifiedInfo = decode(&input.coin, "shielded input coin")?;
    Ok(ZswapCoinInfo::from(&coin))
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
    recipient: MintRecipient,
}

impl helpers::TokenInfo for MintedCoinOutput {
    fn token_type(&self) -> ShieldedTokenType {
        self.coin.type_
    }
    fn value(&self) -> u128 {
        self.coin.value
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
    /// The segment the coin's create/spend ops partitioned into: the
    /// guaranteed segment, or the call's segment when fallible. The input and
    /// output halves share it.
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

/// A circuit-created Zswap offer output and its coin commitment. The commitment
/// lets [`call_transaction`] route the output to the offer for the segment the
/// ledger partitioned the coin's creating op into, and match it against a
/// `createZswapInput` to form a transient; the output gives the coin and the
/// contract of that transient when they pair.
type ShieldedOfferOutput = (helpers::coin_structure::coin::Commitment, MintedCoinOutput);

/// Turn the coins a circuit created via `createZswapOutput` into Zswap offer
/// outputs, each paired with its coin commitment.
///
/// An output for a user carries a discovery ciphertext when the recipient's
/// encryption key is known, so the recipient finds the coin through normal
/// sync (no `watchFor`). `coin_encryption_keys` gives the encryption key of
/// each recipient it lists. `wallet_keys` is the calling wallet's pair, and it
/// applies when `coin_encryption_keys` has no entry for that coin public key.
/// Thus a coin that a circuit sends to `ownPublicKey()` reaches the wallet with
/// no entry from the caller, as in midnight-js. An output for another user that
/// `coin_encryption_keys` leaves out has no ciphertext.
///
/// The commitment is derived with the same coin-structure `Info::commitment` the
/// on-chain runtime used to record the transcript's claimed effect, so the two
/// match by construction and the caller can route each output by segment.
fn build_shielded_offer_outputs(
    zswap_outputs: &[runtime::CircuitZswapOutput],
    coin_encryption_keys: &[midnight_types::ShieldedRecipient],
    wallet_keys: (helpers::CoinPublicKey, helpers::EncryptionPublicKey),
) -> Result<Vec<ShieldedOfferOutput>, ContractError> {
    // Index the recipients once so the per-output lookup is O(1); keyed by the
    // coin public key's raw bytes (`HashOutput` inner array).
    let mut epk_by_cpk: std::collections::HashMap<[u8; 32], helpers::EncryptionPublicKey> =
        coin_encryption_keys
            .iter()
            .map(|r| (r.coin_public_key.0.0, r.enc_public_key.into_ledger()))
            .collect();
    let (wallet_cpk, wallet_epk) = wallet_keys;
    epk_by_cpk.entry(wallet_cpk.0.0).or_insert(wallet_epk);
    let mut outputs: Vec<ShieldedOfferOutput> = Vec::with_capacity(zswap_outputs.len());
    for zo in zswap_outputs {
        let decoded = decode_shielded_output(zo)?;
        let commitment = decoded.coin.commitment(&decoded.recipient);
        let recipient = match decoded.recipient {
            Recipient::User(cpk) => MintRecipient::User {
                cpk,
                epk: epk_by_cpk.get(&cpk.0.0).copied(),
            },
            Recipient::Contract(contract) => MintRecipient::Contract(contract),
        };
        outputs.push((
            commitment,
            MintedCoinOutput {
                coin: decoded.coin,
                recipient,
            },
        ));
    }
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{CircuitZswapOutput, Value};
    use helpers::HashOutput;
    use helpers::coin_structure::coin::{Nonce, UnshieldedTokenType};

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
        assert_eq!(payouts[0].owner.user_address, a_user());
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
        let coin = ZswapCoinInfo {
            nonce: Nonce(HashOutput([2u8; 32])),
            type_: ShieldedTokenType(HashOutput([3u8; 32])),
            value: 1000,
        };
        let cpk = [4u8; 32];

        let decoded =
            decode_shielded_output(&coin_to_user(&coin, cpk)).expect("decode must succeed");

        assert_eq!(decoded.coin, coin);
        assert_eq!(
            decoded.recipient,
            Recipient::User(PublicKey(HashOutput(cpk))),
            "Either::left is a user recipient"
        );
    }

    /// The commitment `build_shielded_offer_outputs` returns is the routing key
    /// `call_transaction` matches against the transcript's claimed effects to
    /// pick a coin's offer segment. It must equal the coin's real
    /// `Info::commitment` for the decoded coin + recipient — the same function
    /// the on-chain runtime uses to record the effect — or the coin would be
    /// mis-routed and trip `AllCommitmentsSubsetCheckFailure`.
    #[test]
    fn build_shielded_offer_outputs_returns_coin_commitment() {
        let coin = ZswapCoinInfo {
            nonce: Nonce(HashOutput([7u8; 32])),
            type_: ShieldedTokenType(HashOutput([8u8; 32])),
            value: 4200,
        };
        let cpk = [9u8; 32];
        let wallet = a_wallet();

        let outputs = build_shielded_offer_outputs(
            &[coin_to_user(&coin, cpk)],
            &[],
            (wallet.coin_public_key, wallet.enc_public_key),
        )
        .expect("build must succeed");
        assert_eq!(outputs.len(), 1);

        let expected = coin.commitment(&helpers::coin_structure::transfer::Recipient::User(
            helpers::coin_structure::coin::PublicKey(HashOutput(cpk)),
        ));
        assert_eq!(outputs[0].0, expected);
    }

    /// A coin that a circuit sends to the calling wallet (`ownPublicKey()`)
    /// must be findable with no entry from the caller. A coin for another user
    /// with no entry must not be sealed to the wallet's key, which its
    /// recipient cannot open.
    #[test]
    fn only_the_wallets_own_coin_is_sealed_without_an_entry() {
        let wallet = a_wallet();
        let coin = |nonce| ZswapCoinInfo {
            nonce: Nonce(HashOutput(nonce)),
            type_: ShieldedTokenType(HashOutput([8u8; 32])),
            value: 4200,
        };
        let to_wallet = coin_to_user(&coin([1u8; 32]), wallet.coin_public_key.0.0);
        let to_stranger = coin_to_user(&coin([2u8; 32]), [9u8; 32]);

        let outputs = build_shielded_offer_outputs(
            &[to_wallet, to_stranger],
            &[],
            (wallet.coin_public_key, wallet.enc_public_key),
        )
        .expect("build must succeed");

        let sealed_to: Vec<_> = outputs
            .iter()
            .map(|(_, output)| match output.recipient {
                MintRecipient::User { epk, .. } => epk,
                MintRecipient::Contract(_) => panic!("both coins go to a user"),
            })
            .collect();
        assert_eq!(sealed_to, [Some(wallet.enc_public_key), None]);
    }

    fn a_wallet() -> helpers::ShieldedWallet<DefaultDB> {
        helpers::ShieldedWallet::default(helpers::WalletSeed::from([1u8; 32]))
    }

    /// A captured `createZswapOutput` of `coin` to the user `cpk`, encoded as
    /// the interpreter captures it.
    fn coin_to_user(coin: &ZswapCoinInfo, cpk: [u8; 32]) -> CircuitZswapOutput {
        let coin = Value::AlignedValue(AlignedValue::concat(
            [
                AlignedValue::from(coin.nonce.0.0),
                AlignedValue::from(coin.type_.0.0),
                AlignedValue::from(coin.value),
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
        CircuitZswapOutput { coin, recipient }
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
}
