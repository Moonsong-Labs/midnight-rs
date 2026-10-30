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
/// [`ContractError::Construction`] when the deploy has neither a state nor a
/// constructor, or when it runs a constructor that declares an argument that
/// `args` does not name.
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
        return Err(ContractError::Construction(format!(
            "missing constructor arguments {missing:?}, call .with_constructor_args(...) on the \
             builder, or .with_initial_state(...) to skip the constructor"
        )));
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

    use midnight_transient_crypto::curve::Fr;
    use midnight_typed_state::{AlignedValue, StateValue};

    use super::*;
    use crate::runtime::{NoWitnesses, WitnessContext, WitnessOutcome};

    /// A constructor loaded through `compact_codegen` from the compiled
    /// fixture `analyzed_ir` (relative to the workspace root), with `circuit`
    /// in place of the fixture's own constructor when it is given. Its state
    /// holds each ledger cell at its default.
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
        let cells: Vec<StateValue<InMemoryDB>> = info
            .ledger
            .iter()
            .map(|field| match (field.storage, &field.element_type) {
                (compact_codegen::types::StorageKind::Cell, Some(ty)) => {
                    StateValue::from(crate::runtime::default_aligned(ty))
                }
                _ => panic!("the fixture field `{}` is not a typed cell", field.name),
            })
            .collect();
        Constructor {
            circuit,
            circuits: info.helpers,
            witnesses: info.witnesses,
            natives: info.natives,
            state: ContractState::new(
                StateValue::Array(cells.into()),
                midnight_typed_state::StorageHashMap::new(),
                midnight_typed_state::ContractMaintenanceAuthority::default(),
            ),
        }
    }

    const TINY: &str = "crates/midnight-contract/tests/fixtures/tiny/compiler/analyzed-ir.sexp";
    const KERNEL: &str = "tests/conformance/fixtures/kernel/compiler/analyzed-ir.sexp";
    const PLAIN: &str = "tests/fixtures/compiled/plain-constructor/compiler/analyzed-ir.sexp";

    /// Run `constructor` as a deploy does, with no private state.
    fn deploy_run(
        constructor: &Constructor,
        args: Vec<(&str, Value)>,
        witnesses: &dyn WitnessProvider,
    ) -> Result<Constructed, ContractError> {
        let args: Vec<(String, Value)> = args
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect();
        run(
            constructor,
            &args,
            witnesses,
            Vec::new(),
            CoinPublicKey(midnight_helpers::HashOutput([2; 32])),
            Timestamp::from_secs(1_000),
        )
    }

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
            Err(ContractError::Construction(message)) => {
                assert!(message.contains(r#"["v"]"#), "{message}");
            }
            Err(other) => panic!("expected Construction, got {other:?}"),
            Ok(_) => panic!("a constructor with no arguments must not run"),
        }
    }

    /// Answers tiny's `private$secret_key`.
    struct SecretKey;

    impl WitnessProvider for SecretKey {
        fn call_witness(
            &self,
            _ctx: &mut WitnessContext<'_>,
            name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, InterpreterError> {
            Ok(match name {
                "private$secret_key" => {
                    WitnessOutcome::Value(Value::AlignedValue(AlignedValue::from([7u8; 32])))
                }
                _ => WitnessOutcome::Unknown,
            })
        }
    }

    /// The deploy replays the constructor's public transcript to find its
    /// effects. A replay that refuses the writes of a constructor fails each
    /// deploy that runs one, and only a deploy on a devnet shows it. tiny's
    /// constructor writes each of its cells, from an argument and a witness.
    #[test]
    fn a_constructor_that_writes_the_ledger_deploys_its_state() {
        let forty_two = AlignedValue::from(Fr::from(42u64));
        let constructed = deploy_run(
            &constructor(TINY, None),
            vec![("v", Value::AlignedValue(forty_two.clone()))],
            &SecretKey,
        )
        .unwrap_or_else(|e| panic!("tiny's constructor must deploy: {e:?}"));

        let StateValue::Array(cells) = &*constructed.state.data.get() else {
            panic!("tiny's state is an array of cells");
        };
        assert_eq!(cells.get(1), Some(&StateValue::from(forty_two)));
    }

    /// The interpreter keeps no effect, so a check that reads the run alone
    /// passes a mint. A check that refuses every kernel operation refuses a
    /// read of the block time.
    #[test]
    fn a_mint_is_a_ledger_effect_and_a_clock_read_is_none() {
        let minted = deploy_run(
            &constructor(KERNEL, Some("mint")),
            vec![
                ("domain_sep", Value::AlignedValue([1u8; 32].into())),
                ("amount", Value::Integer(5)),
            ],
            &NoWitnesses,
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

        deploy_run(
            &constructor(KERNEL, Some("block_time_lt")),
            vec![("time", Value::Integer(2_000))],
            &NoWitnesses,
        )
        .unwrap_or_else(|e| panic!("a read of the block time makes no effect: {e:?}"));
    }

    /// A Zswap input or output with no claim makes no ledger effect, so the
    /// replay alone passes it. The deploy then drops the coin with no error.
    #[test]
    fn a_shielded_coin_with_no_ledger_effect_is_refused() {
        let spent = deploy_run(&constructor(PLAIN, Some("take")), vec![], &NoWitnesses);
        assert!(
            matches!(
                spent,
                Err(ContractError::ConstructorEffect(
                    ConstructorEffect::ShieldedInput
                ))
            ),
            "a spent coin must be refused, got {:?}",
            spent.err()
        );

        let created = deploy_run(
            &constructor(PLAIN, Some("give")),
            vec![("amount", Value::Integer(5))],
            &NoWitnesses,
        );
        assert!(
            matches!(
                created,
                Err(ContractError::ConstructorEffect(
                    ConstructorEffect::ShieldedOutput
                ))
            ),
            "a created coin must be refused, got {:?}",
            created.err()
        );
    }
}
