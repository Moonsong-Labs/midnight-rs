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
//! [`crate::state`], `address`, and [`crate::deploy`] respectively.

use midnight_base_crypto::time::Timestamp;
use midnight_onchain_runtime::ops::Op;
use midnight_onchain_runtime::result_mode::ResultModeVerify;
use midnight_provider::Builds;
use midnight_serialize::tagged_serialize;
use midnight_typed_state::{AlignedValue, ContractState, InMemoryDB};
use midnight_types::ledger_9::convert::IntoLedger;

use crate::error::ContractError;
use crate::interpreter;
use crate::runtime;
use crate::state::StateAtBlock;

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

/// Run a circuit on a contract's state, with no wallet access.
///
/// The circuit's clock checks read `block_time`. `coin_public_key` is what
/// `ownPublicKey()` returns. With `None`, a circuit that calls
/// `ownPublicKey()` fails. The witnesses update `private_state` when it is
/// given.
#[expect(
    clippy::too_many_arguments,
    reason = "one value for each input of the circuit's `Env`, as the callers hold them"
)]
pub(crate) fn run_call(
    circuit: &compact_codegen::ir::Circuit,
    program: &interpreter::Program<'_>,
    state: &ContractState<InMemoryDB>,
    block_time: Timestamp,
    contract_address: midnight_types::ContractAddress,
    args: &[(&str, runtime::Value)],
    witnesses: &dyn runtime::WitnessProvider,
    private_state: Option<&mut Vec<u8>>,
    coin_public_key: Option<midnight_types::CoinPublicKey>,
) -> Result<runtime::ExecutionResult, ContractError> {
    Ok(interpreter::execute(
        circuit,
        program,
        state.clone(),
        args,
        interpreter::Env {
            witnesses,
            private_state,
            address: contract_address.into_ledger(),
            block_time,
            coin_public_key: coin_public_key.map(IntoLedger::into_ledger),
        },
    )?)
}

/// Build the funded, proven transaction of a circuit run on the chain's
/// generation.
///
/// `exec_result` is the run of `circuit` on `state`. The build resyncs the
/// wallet and reserves the inputs that the transaction spends, and the result
/// names them.
#[expect(
    clippy::too_many_arguments,
    reason = "the inputs of the generation's own `call_transaction`, before the dispatch"
)]
pub(crate) async fn call_transaction(
    circuit: &compact_codegen::ir::Circuit,
    args: &[(&str, runtime::Value)],
    exec_result: &runtime::ExecutionResult,
    state: &StateAtBlock,
    circuit_name: &str,
    contract_address: midnight_types::ContractAddress,
    provider: &midnight_provider::MidnightProvider,
    key_location: String,
    coin_encryption_keys: &[midnight_types::ShieldedRecipient],
    shielded: ShieldedInputs,
    // When false, skip Dust funding: the call is built proven but fee-less, for
    // another wallet to sponsor (`MidnightProvider::balance_transaction`).
    pay_fees: bool,
) -> Result<midnight_types::TransferResult, ContractError> {
    // Each arm is boxed so this frame holds one generation's future, not
    // both (see the frame-size note on `MidnightProvider::resync_wallet`).
    match provider.builds().await? {
        Builds::Ledger8(builds) => {
            Box::pin(crate::ledger_8::call::call_transaction(
                &builds,
                circuit,
                args,
                exec_result,
                &state.view,
                &state.bytes,
                state.time,
                circuit_name,
                contract_address,
                key_location,
                coin_encryption_keys,
                shielded,
                pay_fees,
            ))
            .await
        }
        Builds::Ledger9(builds) => {
            Box::pin(crate::ledger_9::call::call_transaction(
                &builds,
                circuit,
                args,
                exec_result,
                &state.view,
                &state.bytes,
                state.time,
                circuit_name,
                contract_address,
                key_location,
                coin_encryption_keys,
                shielded,
                pay_fees,
            ))
            .await
        }
    }
}

/// The circuit's public state program, as the transcript replays it: the
/// [public transcript](runtime::ExecutionResult::public_transcript) without
/// its `idx` ops of an empty path and its `ins` ops of no levels.
pub(crate) fn verify_ops(
    exec_result: &runtime::ExecutionResult,
) -> Vec<Op<ResultModeVerify, InMemoryDB>> {
    exec_result
        .public_transcript()
        .into_iter()
        .filter(|op| match op {
            Op::Idx { path, .. } => !path.is_empty(),
            Op::Ins { n, .. } => *n != 0,
            _ => true,
        })
        .collect()
}

/// The call's input: `args` encoded at the types that `circuit` declares for
/// them.
pub(crate) fn call_input(
    circuit: &compact_codegen::ir::Circuit,
    args: &[(&str, runtime::Value)],
) -> Result<AlignedValue, ContractError> {
    let arg_types: Vec<(&str, compact_codegen::ir::Type)> = circuit
        .arguments
        .iter()
        .map(|a| (a.name.name(), a.ty.clone()))
        .collect();
    Ok(interpreter::encode_circuit_input(args, &arg_types)?)
}

/// `value` in its tagged encoding. `what` names it in the error.
pub(crate) fn tagged_bytes<T>(value: &T, what: &str) -> Result<Vec<u8>, ContractError>
where
    T: midnight_serialize::Serializable + midnight_serialize::Tagged,
{
    let mut bytes = Vec::new();
    tagged_serialize(value, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("serialize {what}: {e}")))?;
    Ok(bytes)
}

/// `value` in another type of the same encoding: another storage backend,
/// or another generation's type that encodes it alike.
pub(crate) fn reencode<A, B>(value: &A, what: &str) -> Result<B, ContractError>
where
    A: midnight_serialize::Serializable + midnight_serialize::Tagged,
    B: midnight_serialize::Deserializable + midnight_serialize::Tagged,
{
    let bytes = tagged_bytes(value, what)?;
    midnight_serialize::tagged_deserialize(&mut bytes.as_slice())
        .map_err(|e| ContractError::Serialization(format!("deserialize {what}: {e}")))
}
