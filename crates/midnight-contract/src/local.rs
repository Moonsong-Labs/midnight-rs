//! Run circuits in process, with no chain, proof or wallet.

use midnight_base_crypto::time::Timestamp;
use midnight_typed_state::ContractState;

use crate::error::ContractError;
use crate::interpreter::{self, Env, Program};
use crate::runtime::Value;

/// Run a pure circuit in process and return its value.
///
/// `circuit` must have its `pure` flag set. `program` holds the circuits,
/// witnesses and natives that its body calls. `args` are the circuit's
/// arguments as (name, value) pairs, keyed by the argument's source name.
///
/// # Errors
///
/// [`ContractError::Interpreter`] when the circuit fails, for example on a
/// failed `assert` or on a call that the interpreter cannot run.
///
/// # Panics
///
/// When the `pure` flag of `circuit` is not set.
pub fn run_pure(
    circuit: &compact_codegen::ir::Circuit,
    program: &Program<'_>,
    args: &[(&str, Value)],
) -> Result<Option<Value>, ContractError> {
    assert!(
        circuit.pure,
        "run_pure needs a pure circuit, got `{}`",
        circuit.name.name()
    );
    // The compiler calls a circuit pure only when it touches no public state
    // and calls no witness, so the state and the time cannot change its value.
    let result = interpreter::execute(
        circuit,
        program,
        ContractState::default(),
        args,
        Env::new(Timestamp::from_secs(0)),
    )?;
    Ok(result.result)
}
