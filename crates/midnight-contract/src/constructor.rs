//! A contract's constructor, which a deploy runs to make the state that it
//! deploys.

use midnight_base_crypto::time::Timestamp;
use midnight_onchain_runtime::context::{Effects, QueryContext};
use midnight_onchain_runtime::cost_model::INITIAL_COST_MODEL;
use midnight_onchain_runtime::result_mode::ResultModeVerify;
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::ledger_9::convert::IntoLedger;
use midnight_types::{CoinPublicKey, ContractAddress};

use crate::error::{ConstructorEffect, ContractError};
use crate::interpreter::Program;
use crate::runtime::{ExecutionResult, InterpreterError, Value, WitnessProvider};

/// A contract's constructor, which a deploy runs to make the state that it
/// deploys.
///
/// The generated `Contract::deploy` builds one from the analyzed IR and gives
/// it to [`DeployBuilder::with_constructor`](crate::DeployBuilder::with_constructor).
#[derive(Debug, Clone)]
pub struct Constructor {
    /// The constructor as a circuit with a unit result
    /// (`compact_codegen::types::ContractInfo::constructor`).
    pub circuit: compact_codegen::ir::Circuit,
    /// Every circuit of the contract, which the constructor can call.
    pub circuits: Vec<compact_codegen::ir::Circuit>,
    /// The witness declarations of the contract, which the constructor can
    /// call.
    pub witnesses: Vec<compact_codegen::ir::Witness>,
    /// The native declarations of the contract, which the constructor can
    /// call.
    pub natives: Vec<compact_codegen::ir::Native>,
    /// The state that the constructor starts from: each ledger field at the
    /// compiler's initial value for its type.
    pub state: ContractState<InMemoryDB>,
}

/// Where the state of a deploy comes from.
#[expect(
    clippy::large_enum_variant,
    reason = "one value per deploy, which the deploy matches once and moves out of"
)]
pub(crate) enum DeploySource {
    /// A state that the deploy carries as it is.
    State(ContractState<InMemoryDB>),
    /// A constructor that the deploy runs on `args`.
    Constructor {
        constructor: Constructor,
        args: Vec<(String, Value)>,
    },
}

/// Choose the source of a deploy's state. A given `initial_state` wins over
/// `constructor`.
///
/// # Errors
///
/// - [`ContractError::MissingConstructorArgs`] when the deploy runs a
///   constructor that declares an argument that `args` does not name.
/// - [`ContractError::Construction`] when the deploy has neither a state nor
///   a constructor.
pub(crate) fn deploy_source(
    initial_state: Option<ContractState<InMemoryDB>>,
    constructor: Option<Constructor>,
    args: Vec<(String, Value)>,
) -> Result<DeploySource, ContractError> {
    if let Some(state) = initial_state {
        return Ok(DeploySource::State(state));
    }
    let Some(constructor) = constructor else {
        return Err(ContractError::Construction(
            "missing initial_state, call .with_initial_state(...) on the builder".into(),
        ));
    };
    let missing: Vec<String> = constructor
        .circuit
        .arguments
        .iter()
        .map(|argument| argument.name.name())
        .filter(|name| !args.iter().any(|(given, _)| given == name))
        .map(str::to_owned)
        .collect();
    if !missing.is_empty() {
        return Err(ContractError::MissingConstructorArgs { missing });
    }
    Ok(DeploySource::Constructor { constructor, args })
}

/// The state and the private state that a constructor run left.
pub(crate) struct Constructed {
    pub(crate) state: ContractState<InMemoryDB>,
    pub(crate) private_state: Vec<u8>,
}

/// Run `constructor` on `args`, and refuse a run whose effects the deploy does
/// not carry.
///
/// The witnesses start from `private_state`. `ownPublicKey()` returns
/// `coin_public_key`, and the clock checks read `block_time`. The constructor
/// runs at the zero address, the `dummyContractAddress()` of the canonical
/// runtime: the deploy gives the contract its address after the run.
///
/// # Errors
///
/// - [`ContractError::Interpreter`] when the constructor or a witness fails,
///   for example on a failed `assert`.
/// - [`ContractError::ConstructorEffect`] when the run has an effect that the
///   deploy does not carry.
pub(crate) fn run(
    constructor: &Constructor,
    args: &[(String, Value)],
    witnesses: &dyn WitnessProvider,
    mut private_state: Vec<u8>,
    coin_public_key: CoinPublicKey,
    block_time: Timestamp,
) -> Result<Constructed, ContractError> {
    let program = Program::new(
        &constructor.circuits,
        &constructor.witnesses,
        &constructor.natives,
    );
    let args: Vec<(&str, Value)> = args
        .iter()
        .map(|(name, value)| (name.as_str(), value.clone()))
        .collect();
    let ran = crate::call::run_call(
        &constructor.circuit,
        &program,
        &constructor.state,
        block_time,
        zero_address(),
        &args,
        witnesses,
        Some(&mut private_state),
        Some(coin_public_key),
    )?;
    refuse_undeployable(&constructor.state, &ran, block_time)?;
    Ok(Constructed {
        state: ran.state,
        private_state,
    })
}

fn zero_address() -> ContractAddress {
    ContractAddress(midnight_helpers::HashOutput([0; 32]))
}

/// Refuse a constructor run whose effects the deploy does not carry.
///
/// The interpreter drops the effects of each ledger query, so this replays
/// the public transcript of the run on `state_before` to collect them. A
/// ledger effect comes first, because a mint also creates a shielded coin.
fn refuse_undeployable(
    state_before: &ContractState<InMemoryDB>,
    ran: &ExecutionResult,
    block_time: Timestamp,
) -> Result<(), ContractError> {
    let mut context = QueryContext::new(state_before.data.clone(), zero_address().into_ledger());
    context.call_context.tblock = block_time;
    context.call_context.balance = state_before.balance.clone();
    let replayed = context
        .query::<ResultModeVerify>(&crate::call::verify_ops(ran), None, &INITIAL_COST_MODEL)
        .map_err(|e| InterpreterError::LedgerQueryFailed(format!("{e:?}")))?;
    let effect = if replayed.context.effects != Effects::default() {
        ConstructorEffect::LedgerEffect
    } else if !ran.zswap_inputs.is_empty() {
        ConstructorEffect::ShieldedInput
    } else if !ran.zswap_outputs.is_empty() {
        ConstructorEffect::ShieldedOutput
    } else {
        return Ok(());
    };
    Err(ContractError::ConstructorEffect(effect))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::runtime::NoWitnesses;

    /// A constructor loaded through `compact_codegen` from the compiled
    /// fixture `analyzed_ir` (relative to the workspace root), with `circuit`
    /// in place of the fixture's own constructor when it is given.
    fn constructor(analyzed_ir: &str, circuit: Option<&str>) -> Constructor {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(analyzed_ir);
        let info = compact_codegen::artifact::load(&path)
            .unwrap_or_else(|e| panic!("load {}: {e}", path.display()));
        let circuit = match circuit {
            Some(name) => info
                .circuits
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("no circuit `{name}`"))
                .def
                .clone(),
            None => match info.constructor {
                Some(Ok(c)) => c,
                other => panic!("expected a runnable constructor, got {other:?}"),
            },
        };
        Constructor {
            circuit,
            circuits: info.helpers,
            witnesses: info.witnesses,
            natives: info.natives,
            state: ContractState::new(
                midnight_typed_state::StateValue::Array(vec![].into()),
                midnight_typed_state::StorageHashMap::new(),
                midnight_typed_state::ContractMaintenanceAuthority::default(),
            ),
        }
    }

    const TINY: &str = "crates/midnight-contract/tests/fixtures/tiny/compiler/analyzed-ir.sexp";
    const KERNEL: &str = "tests/conformance/fixtures/kernel/compiler/analyzed-ir.sexp";

    /// A deploy that runs the constructor over a given state deploys the wrong
    /// state. A deploy with no arguments either deploys the default state, or
    /// fails late, after the query for the wallet's key, with an unbound
    /// variable that does not name the argument.
    #[test]
    fn deploy_source_prefers_the_given_state_and_names_missing_arguments() {
        let given = ContractState::default();
        let source = deploy_source(Some(given), Some(constructor(TINY, None)), vec![])
            .expect("a given state needs no arguments");
        assert!(matches!(source, DeploySource::State(_)));

        let refused = deploy_source(None, Some(constructor(TINY, None)), vec![]);
        match refused {
            Err(ContractError::MissingConstructorArgs { missing }) => {
                assert_eq!(missing, ["v"]);
            }
            Err(other) => panic!("expected MissingConstructorArgs, got {other:?}"),
            Ok(_) => panic!("a constructor with no arguments must not run"),
        }
    }

    /// The interpreter keeps no effect, so a check that reads the run alone
    /// passes a mint. A check that refuses every kernel operation refuses a
    /// read of the block time.
    #[test]
    fn only_a_ledger_effect_is_refused() {
        let run_kernel = |circuit: &str, args: Vec<(String, Value)>| {
            run(
                &constructor(KERNEL, Some(circuit)),
                &args,
                &NoWitnesses,
                Vec::new(),
                CoinPublicKey(midnight_helpers::HashOutput([2; 32])),
                Timestamp::from_secs(1_000),
            )
        };

        let minted = run_kernel(
            "mint",
            vec![
                (
                    "domain_sep".to_owned(),
                    Value::AlignedValue([1u8; 32].into()),
                ),
                ("amount".to_owned(), Value::Integer(5)),
            ],
        );
        assert!(
            matches!(
                minted,
                Err(ContractError::ConstructorEffect(
                    ConstructorEffect::LedgerEffect
                ))
            ),
            "a mint must be refused as a ledger effect"
        );

        run_kernel(
            "block_time_lt",
            vec![("time".to_owned(), Value::Integer(2_000))],
        )
        .unwrap_or_else(|e| panic!("a read of the block time makes no effect: {e:?}"));
    }
}
