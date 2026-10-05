//! Circuit calls: run the circuit IR, then build the call's transaction on
//! the chain's ledger generation.
//!
//! The interpreter runs against the Compact side's view of the contract state
//! and produces the same result whatever the chain runs. The transaction is
//! per generation (see the generation modules' `call`): the interpreter's ops
//! cross into the generation's own by their encoding, which is the same in
//! every generation.
//!
//! State reading, address parsing, and the deploy path live in
//! [`crate::state`], `address`, and [`crate::deploy`] respectively. The TTL
//! helpers (`current_ttl`, `DEFAULT_TTL`) are exposed as `pub(crate)` from
//! here so `deploy` doesn't have to duplicate them.

use std::borrow::Cow;
use std::sync::Arc;

use midnight_base_crypto::time::{Duration, Timestamp};
use midnight_helpers::ledger_9::mn_ledger;
use midnight_onchain_runtime::ops::Op;
use midnight_onchain_runtime::result_mode::ResultModeVerify;
use midnight_onchain_runtime::state::{ContractOperation, EntryPointBuf};
use midnight_provider::{Builds, ProviderError, WalletError};
use midnight_serialize::tagged_serialize;
use midnight_transient_crypto::proofs::KeyLocation;
use midnight_typed_state::{AlignedValue, ContractState, InMemoryDB};
use midnight_types::SpentInputs;

use crate::error::ContractError;
use crate::interpreter;
use crate::runtime;

/// The signature type used in Midnight transactions.
pub type Sig = midnight_helpers::ledger_9::Signature;

/// Type alias for the unproven transaction object [`build_unproven_call_tx`]
/// builds: a ledger 9 transaction over the Compact side's own types.
pub type UnprovenTransaction = mn_ledger::structure::Transaction<
    Sig,
    mn_ledger::structure::ProofPreimageMarker,
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

/// Default transaction TTL: 1 hour.
///
/// Used by the low-level [`build_unproven_call_tx`] path. The high-level path
/// ([`crate::deploy::deploy_funded`], [`call_funded_with`], and the
/// [`crate::DeployBuilder`] / [`crate::Contract::call_with`] APIs that wrap
/// them) reads `global_ttl` from chain parameters via the upstream
/// `StandardTransactionInfo::build`, so this constant doesn't apply there.
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
    /// exactly by its nullifier (never amount-based) and routed to the segment
    /// of the circuit output it funds. See
    /// [`SpendableShieldedCoin`](midnight_types::SpendableShieldedCoin).
    ///
    /// The coins must COVER the shielded value the circuit receives. Zswap
    /// balances per (token, segment) delta rather than per coin identity, so
    /// several coins may fund one larger `receiveShielded`, and whatever they
    /// carry beyond what the circuit's outputs draw returns to this wallet as a
    /// change output. Coins that fall short leave the call unbalanced, which
    /// the fee-paying step refuses before submitting; a Dustless call carries
    /// the shortfall to the node instead.
    pub coins: Vec<midnight_types::SpendableShieldedCoin>,
}

/// Run a circuit and build its funded, proven transaction on the chain's
/// generation. Returns the transaction bytes, the inputs the build reserved,
/// the contract's state after the circuit, and the circuit's result.
///
/// `state` is the Compact side's view of the contract state, and
/// `state_bytes` the encoding the chain served it in.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn call_funded_with(
    circuit: &compact_codegen::ir::Circuit,
    program: &interpreter::Program<'_>,
    state: &ContractState<InMemoryDB>,
    state_bytes: &[u8],
    circuit_name: &str,
    contract_address: midnight_types::ContractAddress,
    provider: &midnight_provider::MidnightProvider,
    zk_config: Arc<dyn crate::zk_config::ZkConfigProvider>,
    args: &[(&str, runtime::Value)],
    witnesses: &dyn runtime::WitnessProvider,
    witness_ctx: Option<&mut runtime::WitnessContext<'_>>,
    coin_encryption_keys: &[(
        midnight_types::CoinPublicKey,
        midnight_types::EncryptionPublicKey,
    )],
    shielded: ShieldedInputs,
    // When false, skip Dust funding: the call is built proven but fee-less, for
    // another wallet to sponsor (`MidnightProvider::balance_transaction`).
    pay_fees: bool,
) -> Result<
    (
        Vec<u8>,
        Vec<SpentInputs>,
        ContractState<InMemoryDB>,
        Option<runtime::Value>,
    ),
    ContractError,
> {
    // Execute the circuit IR locally for the updated state. When a
    // `witness_ctx` is supplied it threads the contract's private state
    // through any witness calls; after this returns its buffer holds the
    // post-call private state. `None` means no private-state threading.
    let exec_result = interpreter::execute_with_owned(
        circuit,
        program,
        state.clone(),
        args,
        witnesses,
        witness_ctx,
        Some(compact_address(contract_address)),
    )?;

    let builds = provider.builds().await?;
    // Refuse only at zero: the wallet prices the fee after the proof, so a
    // positive balance that falls short fails there, with the fee it needs.
    if pay_fees && provider.balance().await?.dust.spendable_speck == 0 {
        return Err(ContractError::Provider(ProviderError::Wallet(
            WalletError::InsufficientDust {
                required: None,
                available: 0,
            },
        )));
    }

    // Each arm is boxed so this frame holds one generation's future, not
    // both (see the frame-size note on `MidnightProvider::resync_wallet`).
    let (tx_bytes, reserved) = match builds {
        Builds::Ledger8(builds) => {
            Box::pin(crate::ledger_8::call::call_transaction(
                &builds,
                circuit,
                args,
                &exec_result,
                state,
                state_bytes,
                circuit_name,
                contract_address,
                zk_config,
                coin_encryption_keys,
                shielded,
                pay_fees,
            ))
            .await?
        }
        Builds::Ledger9(builds) => {
            Box::pin(crate::ledger_9::call::call_transaction(
                &builds,
                circuit,
                args,
                &exec_result,
                state,
                state_bytes,
                circuit_name,
                contract_address,
                zk_config,
                coin_encryption_keys,
                shielded,
                pay_fees,
            ))
            .await?
        }
    };
    Ok((tx_bytes, reserved, exec_result.state, exec_result.result))
}

/// The address as the Compact side names it.
pub(crate) fn compact_address(
    address: midnight_types::ContractAddress,
) -> midnight_coin_structure::contract::ContractAddress {
    midnight_coin_structure::contract::ContractAddress(address.0)
}

/// The circuit's public state program, as the transcript replays it: the
/// ops the interpreter gathered, with the values it read filled in.
pub(crate) fn verify_ops(
    exec_result: &runtime::ExecutionResult,
) -> Vec<Op<ResultModeVerify, InMemoryDB>> {
    let mut read_iter = exec_result.reads.iter();
    exec_result
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
            Op::Idx { path, .. } => !path.is_empty(),
            Op::Ins { n, .. } => *n != 0,
            _ => true,
        })
        .collect()
}

/// `value` in another type of the same encoding: another storage backend,
/// or another generation's type that encodes it alike.
pub(crate) fn reencode<A, B>(value: &A, what: &str) -> Result<B, ContractError>
where
    A: midnight_serialize::Serializable + midnight_serialize::Tagged,
    B: midnight_serialize::Deserializable + midnight_serialize::Tagged,
{
    let mut bytes = Vec::new();
    tagged_serialize(value, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("serialize {what}: {e}")))?;
    midnight_serialize::tagged_deserialize(&mut bytes.as_slice())
        .map_err(|e| ContractError::Serialization(format!("deserialize {what}: {e}")))
}

/// Build an unproven ledger 9 transaction from a circuit IR body and contract
/// state.
///
/// It builds for ledger 9 whatever the chain runs, so a chain on ledger 8
/// cannot take the result. Low-level builder; the high-level path goes through
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
/// The circuit's own `arguments` declare the type of each argument, struct
/// fields included. The interpreter uses that type to slice a struct argument
/// (such as `recipient.is_left` on an `Either`), and the builder uses it to
/// encode the call input.
#[allow(clippy::too_many_arguments)]
pub fn build_unproven_call_tx<W: runtime::WitnessProvider>(
    circuit: &compact_codegen::ir::Circuit,
    program: &interpreter::Program<'_>,
    state: &ContractState<InMemoryDB>,
    circuit_name: &str,
    contract_address: midnight_types::ContractAddress,
    network_id: &str,
    args: &[(&str, runtime::Value)],
    witnesses: &W,
    witness_ctx: Option<&mut runtime::WitnessContext<'_>>,
) -> Result<UnprovenCallTx, ContractError> {
    use midnight_storage::storage::HashMap as StorageHashMap;
    use mn_ledger::structure::{Intent, Transaction};
    use rand::Rng;

    let mut rng = rand::thread_rng();

    let exec_result = interpreter::execute_with_owned(
        circuit,
        program,
        state.clone(),
        args,
        witnesses,
        witness_ctx,
        Some(compact_address(contract_address)),
    )?;

    let entry_point: EntryPointBuf = circuit_name.as_bytes().into();

    let verify_ops = verify_ops(&exec_result);

    let address_for_ctx = compact_address(contract_address);
    let context =
        midnight_onchain_runtime::context::QueryContext::new(state.data.clone(), address_for_ctx);
    let pre_transcript = mn_ledger::construct::PreTranscript {
        context,
        program: verify_ops,
        comm_comm: None,
    };

    let partitioned = mn_ledger::construct::partition_transcripts(
        &[pre_transcript],
        &mn_ledger::structure::INITIAL_PARAMETERS,
    )
    .map_err(|e| ContractError::Construction(format!("partition failed: {e:?}")))?;

    let (guaranteed, fallible) = partitioned.into_iter().next().unwrap_or((None, None));

    let arg_types: Vec<(&str, compact_codegen::ir::Type)> = circuit
        .arguments
        .iter()
        .map(|a| (a.name.name(), a.ty.clone()))
        .collect();
    let input: AlignedValue = interpreter::encode_circuit_input(args, &arg_types)?;
    let output: AlignedValue = if exec_result.communication_outputs.is_empty() {
        ().into()
    } else {
        AlignedValue::concat(&exec_result.communication_outputs)
    };

    let op = state
        .operations
        .get(&entry_point)
        .map(|sp| (*sp).clone())
        .unwrap_or_else(|| ContractOperation::new(None, None));

    let call = mn_ledger::construct::ContractCallPrototype {
        address: address_for_ctx,
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
