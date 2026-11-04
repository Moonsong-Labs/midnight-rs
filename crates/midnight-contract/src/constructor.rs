//! A contract's constructor, which a deploy runs to make the state that it
//! deploys.

use midnight_base_crypto::time::Timestamp;
use midnight_onchain_runtime::context::{Effects, QueryContext};
use midnight_onchain_runtime::cost_model::INITIAL_COST_MODEL;
use midnight_onchain_runtime::result_mode::ResultModeVerify;
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::CoinPublicKey;

use crate::error::ContractError;
use crate::interpreter::Program;
use crate::runtime::{ExecutionResult, InterpreterError, Value, WitnessProvider};

/// A contract's constructor, which a deploy runs to make the state that it
/// deploys.
///
/// The generated `Contract::deploy` builds one from the analyzed IR and gives
/// it to [`DeployBuilder::with_constructor`](crate::DeployBuilder::with_constructor).
#[derive(Debug, Clone)]
pub struct Constructor {
    /// The constructor as a circuit with a unit result.
    pub circuit: compact_codegen::ir::Circuit,
    /// The circuits of the contract, which the constructor can call.
    pub circuits: Vec<compact_codegen::ir::Circuit>,
    /// The witnesses of the contract, which the constructor can call.
    pub witnesses: Vec<compact_codegen::ir::Witness>,
    /// The natives of the contract, which the constructor can call.
    pub natives: Vec<compact_codegen::ir::Native>,
    /// The state that the constructor starts from.
    pub state: ContractState<InMemoryDB>,
}

impl Constructor {
    /// Refuse `args` when they leave out an argument of the constructor.
    pub(crate) fn check_args(&self, args: &[(&str, Value)]) -> Result<(), ContractError> {
        let missing: Vec<&str> = self
            .circuit
            .arguments
            .iter()
            .map(|argument| argument.name.name())
            .filter(|name| !args.iter().any(|(given, _)| given == name))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        Err(ContractError::Construction(format!(
            "missing constructor arguments {missing:?}, call .with_constructor_args(...) on the \
             builder, or .with_initial_state(...) to skip the constructor"
        )))
    }

    /// Run the constructor, and return the state and the private state that it
    /// leaves.
    ///
    /// The constructor runs with no contract address, because the deploy
    /// computes the address from the state that the constructor leaves. The
    /// canonical runtime runs it at a placeholder, so `kernel.self()` there
    /// reads zeros (LFDT-Minokawa/compact#117).
    ///
    /// # Errors
    ///
    /// - [`ContractError::Interpreter`] when the constructor or a witness
    ///   fails, or when the constructor reads `kernel.self()`.
    /// - [`ContractError::Construction`] when the run has an effect that the
    ///   deploy does not carry.
    pub(crate) fn run(
        &self,
        args: &[(&str, Value)],
        witnesses: &dyn WitnessProvider,
        mut private_state: Vec<u8>,
        coin_public_key: CoinPublicKey,
        block_time: Timestamp,
    ) -> Result<(ContractState<InMemoryDB>, Vec<u8>), ContractError> {
        let program = Program::new(&self.circuits, &self.witnesses, &self.natives);
        let ran = crate::call::run_call(
            &self.circuit,
            &program,
            &self.state,
            block_time,
            None,
            args,
            witnesses,
            Some(&mut private_state),
            Some(coin_public_key),
        )?;
        refuse_effects(&self.state, &ran, block_time)?;
        Ok((ran.state, private_state))
    }
}

/// Refuse a constructor run with an effect that the deploy does not carry.
///
/// A deploy carries a state and no transcript, so the ledger takes no ledger
/// effect and no contract spend from it. The SDK also puts no constructor
/// output in the deploy's offer. The interpreter drops the effects of each
/// ledger query, so this replays the run's public transcript to collect them.
fn refuse_effects(
    state_before: &ContractState<InMemoryDB>,
    ran: &ExecutionResult,
    block_time: Timestamp,
) -> Result<(), ContractError> {
    // No op reads the address: the run refuses `kernel.self()`.
    let mut context = QueryContext::new(state_before.data.clone(), Default::default());
    context.call_context.tblock = block_time;
    context.call_context.balance = state_before.balance.clone();
    let replayed = context
        .query::<ResultModeVerify>(&crate::call::verify_ops(ran), None, &INITIAL_COST_MODEL)
        .map_err(|e| InterpreterError::LedgerQueryFailed(format!("{e:?}")))?;
    // A mint also creates a shielded coin, so the ledger effect names it.
    let effect = if replayed.context.effects != Effects::default() {
        "makes a ledger effect, such as a mint or a claim"
    } else if !ran.zswap_inputs.is_empty() {
        "spends a shielded coin"
    } else if !ran.zswap_outputs.is_empty() {
        "creates a shielded coin"
    } else {
        return Ok(());
    };
    Err(ContractError::Construction(format!(
        "the constructor {effect}, which the deploy does not carry: deploy a state with \
         .with_initial_state(...), and make the effect in a circuit call"
    )))
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
    ) -> Result<ContractState<InMemoryDB>, ContractError> {
        let (state, _) = constructor.run(
            &args,
            witnesses,
            Vec::new(),
            CoinPublicKey(midnight_helpers::HashOutput([2; 32])),
            Timestamp::from_secs(1_000),
        )?;
        Ok(state)
    }

    /// `Err` with a `Construction` message that contains `expected`.
    #[track_caller]
    fn assert_refused<T: std::fmt::Debug>(result: Result<T, ContractError>, expected: &str) {
        match result {
            Err(ContractError::Construction(message)) => {
                assert!(message.contains(expected), "{message}");
            }
            other => panic!("expected a Construction error, got {other:?}"),
        }
    }

    /// The interpreter fails on a missing argument with an unbound variable
    /// that does not name it.
    #[test]
    fn a_missing_constructor_argument_is_named() {
        assert_refused(constructor(TINY, None).check_args(&[]), r#"["v"]"#);
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

    /// A replay that refuses the constructor's own writes fails every deploy
    /// that runs a constructor. tiny's constructor writes each of its cells.
    #[test]
    fn a_constructor_that_writes_the_ledger_deploys_its_state() {
        let forty_two = AlignedValue::from(Fr::from(42u64));
        let state = deploy_run(
            &constructor(TINY, None),
            vec![("v", Value::AlignedValue(forty_two.clone()))],
            &SecretKey,
        )
        .unwrap_or_else(|e| panic!("tiny's constructor must deploy: {e:?}"));

        let StateValue::Array(cells) = &*state.data.get() else {
            panic!("tiny's state is an array of cells");
        };
        assert_eq!(cells.get(1), Some(&StateValue::from(forty_two)));
    }

    /// A check that reads the run alone passes a mint, because the interpreter
    /// keeps no effect. A check that refuses every kernel operation refuses a
    /// read of the block time.
    #[test]
    fn a_mint_is_refused_and_a_clock_read_is_not() {
        let minted = deploy_run(
            &constructor(KERNEL, Some("mint")),
            vec![
                ("domain_sep", Value::AlignedValue([1u8; 32].into())),
                ("amount", Value::Integer(5)),
            ],
            &NoWitnesses,
        );
        assert_refused(minted, "makes a ledger effect");

        deploy_run(
            &constructor(KERNEL, Some("block_time_lt")),
            vec![("time", Value::Integer(2_000))],
            &NoWitnesses,
        )
        .unwrap_or_else(|e| panic!("a read of the block time makes no effect: {e:?}"));
    }

    /// A run at a placeholder address deploys what the constructor derives
    /// from `kernel.self()`: zeros in place of the contract's address.
    #[test]
    fn a_constructor_that_reads_kernel_self_is_refused() {
        let family = compact_codegen::types::SUPPORTED_COMPILER_FAMILY;
        let info = compact_codegen::artifact::load_str(&format!(
            r#"(analyzed-ir (compiler-version "{family}.0") (language-version "0.25.107")
              (runtime-version "0.18.107") (exports) (contract-types)
              (kernel-declaration (%kernel.1 () (exported #f) (Kernel)))
              (public-ledger-declaration
                (public-ledger-array
                  (%owner.0 (0) (exported #t) (__compact_Cell (tbytes 32))))
                (constructor ()
                  (seq (public-ledger %kernel.1 read () self
                         (tstruct ContractAddress (bytes (tbytes 32)))
                         (instructions (dup (n 2))
                           (idx (cached #t) (pushPath #f) (path ((align 0 1))))
                           (popeq (cached #t) (result (void)))))
                       (return (tuple))))))"#
        ))
        .expect("the constructor loads");
        let Some(Ok(circuit)) = info.constructor else {
            panic!(
                "expected a runnable constructor, got {:?}",
                info.constructor
            );
        };
        let constructor = Constructor {
            circuit,
            circuits: Vec::new(),
            witnesses: Vec::new(),
            natives: Vec::new(),
            state: ContractState::default(),
        };

        let ran = deploy_run(&constructor, vec![], &NoWitnesses);
        assert!(
            matches!(
                ran,
                Err(ContractError::Interpreter(
                    InterpreterError::NoContractAddress
                ))
            ),
            "got {:?}",
            ran.err()
        );
    }

    /// A Zswap input or output with no claim makes no ledger effect, so the
    /// replay alone passes it, and the deploy drops the coin.
    #[test]
    fn a_shielded_coin_is_refused() {
        let spent = deploy_run(&constructor(PLAIN, Some("take")), vec![], &NoWitnesses);
        assert_refused(spent, "spends a shielded coin");

        let created = deploy_run(
            &constructor(PLAIN, Some("give")),
            vec![("amount", Value::Integer(5))],
            &NoWitnesses,
        );
        assert_refused(created, "creates a shielded coin");
    }
}
