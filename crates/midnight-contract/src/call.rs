//! Circuit calls: run the circuit IR, then build the call's transaction.
//!
//! State reading, address parsing, and the deploy path live in
//! [`crate::state`], `address`, and [`crate::deploy`] respectively.

use std::sync::Arc;

use midnight_base_crypto::time::Timestamp;
use midnight_onchain_runtime::ops::Op;
use midnight_onchain_runtime::result_mode::ResultModeVerify;
use midnight_serialize::tagged_serialize;
use midnight_typed_state::{AlignedValue, ContractState, InMemoryDB};
use midnight_types::convert::IntoLedger;

use crate::error::ContractError;
use crate::interpreter;
use crate::runtime;
use crate::state::StateAtBlock;

/// Coins of the caller's wallet that a contract call spends first.
///
/// A circuit that receives a shielded coin (e.g. `receiveShielded`) leaves the
/// call short of that token. The wallet covers each such deficit: it spends
/// these coins first, then selects coins of its own for what they leave, and
/// returns the change. With no coins here, the wallet selects them all.
///
/// All coins come from the provider's single funding wallet. Spending coins
/// owned by other wallets (multi-party offers) is not supported yet.
///
/// Built by the generated `Circuits` builder's `with_shielded_inputs`, and
/// accepted directly by [`Contract::call_with`](crate::Contract::call_with).
/// Empty by default (the common case: a call with no shielded input).
#[derive(Default)]
pub struct ShieldedInputs {
    /// Wallet coins to spend, each named exactly by its nullifier. See
    /// [`SpendableShieldedCoin`](midnight_types::SpendableShieldedCoin).
    ///
    /// The call spends each coin whole, at the segment of the largest deficit
    /// of its token that is left, or at the guaranteed segment when none is
    /// left. Zswap balances each token at each segment rather than each coin,
    /// so several coins may fund one larger `receiveShielded`. Whatever the
    /// spent coins carry beyond the deficit returns to this wallet as a
    /// change output, so a coin whose token has no deficit comes back whole.
    pub coins: Vec<midnight_types::SpendableShieldedCoin>,
}

/// Run a circuit on a contract's state, with no wallet access.
///
/// The circuit's clock checks read `block_time`. `contract_address` is what
/// `kernel.self()` returns, and `coin_public_key` is what `ownPublicKey()`
/// returns. A circuit that reads a value given as `None` fails. The witnesses
/// update `private_state` when it is given.
#[expect(
    clippy::too_many_arguments,
    reason = "one value for each input of the circuit's `Env`, as the callers hold them"
)]
pub(crate) fn run_call(
    circuit: &compact_codegen::ir::Circuit,
    program: &interpreter::Program<'_>,
    state: &ContractState<InMemoryDB>,
    block_time: Timestamp,
    contract_address: Option<midnight_types::ContractAddress>,
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
            address: contract_address.map(IntoLedger::into_ledger),
            block_time,
            coin_public_key: coin_public_key.map(IntoLedger::into_ledger),
        },
    )?)
}

/// Build the funded, proven transaction of a circuit run.
///
/// `exec_result` is the run of `circuit` on `state`. The build resyncs the
/// wallet, which covers the call and reserves the inputs that it spends, and
/// the result names them.
#[expect(
    clippy::too_many_arguments,
    reason = "the inputs of the ledger's own `call_transaction`, one value each"
)]
pub(crate) async fn call_transaction(
    circuit: &compact_codegen::ir::Circuit,
    args: &[(&str, runtime::Value)],
    exec_result: &runtime::ExecutionResult,
    state: &StateAtBlock,
    circuit_name: &str,
    contract_address: midnight_types::ContractAddress,
    provider: &midnight_provider::MidnightProvider,
    zk_config: Arc<dyn crate::zk_config::ZkConfigProvider>,
    verifier_key: &[u8],
    coin_encryption_keys: &[midnight_types::ShieldedRecipient],
    shielded: ShieldedInputs,
    // When false, skip Dust funding: the call is built proven but fee-less, for
    // another wallet to sponsor (`MidnightProvider::balance_transaction`).
    pay_fees: bool,
) -> Result<midnight_types::TransferResult, ContractError> {
    let builds = provider.builds().await?;
    // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
    Box::pin(crate::ledger::call::call_transaction(
        &builds,
        circuit,
        args,
        exec_result,
        &state.contract_state,
        state.time,
        circuit_name,
        contract_address,
        zk_config,
        verifier_key,
        coin_encryption_keys,
        shielded,
        pay_fees,
    ))
    .await
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
    Ok(interpreter::encode_circuit_input(args, &circuit.arguments)?)
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
