//! Witness (private-state) callbacks invoked during circuit execution.

use crate::error::InterpreterError;
use crate::value::Value;

/// Mutable context handed to each witness call during circuit execution.
///
/// A witness reads the contract's current private state, computes its value,
/// and may mutate the private state in place. The mutated state is what the SDK
/// persists after a successful call (see `docs/private-state.md`).
///
/// The private state is opaque bytes; the witness owns its encoding. When no
/// `PrivateStateProvider` is attached the buffer starts empty and lives only
/// for the duration of the call.
pub struct WitnessContext<'a> {
    private_state: &'a mut Vec<u8>,
}

impl<'a> WitnessContext<'a> {
    /// Wrap a mutable private-state buffer.
    pub fn new(private_state: &'a mut Vec<u8>) -> Self {
        Self { private_state }
    }

    /// The contract's current private state as opaque bytes (empty if unset).
    pub fn private_state(&self) -> &[u8] {
        self.private_state
    }

    /// Mutable access to the private-state buffer, to update it in place.
    pub fn private_state_mut(&mut self) -> &mut Vec<u8> {
        self.private_state
    }

    /// Replace the private state wholesale.
    pub fn set_private_state(&mut self, bytes: Vec<u8>) {
        *self.private_state = bytes;
    }
}

/// Outcome of dispatching a witness call to a [`WitnessProvider`].
///
/// Distinguishes "the provider doesn't implement this name" (a normal,
/// non-error outcome that lets the interpreter fall through to builtins and
/// IR helpers) from witness-level failures, which are `Err` on the
/// surrounding `Result` and always abort execution.
#[derive(Debug, Clone)]
pub enum WitnessOutcome {
    /// The provider handled the call and produced the witness value.
    Value(Value),
    /// The provider doesn't implement a witness with this name. The
    /// interpreter falls through to builtin and helper dispatch.
    Unknown,
}

/// Trait for providing witness (private state) callbacks during circuit execution.
///
/// Implement this to supply private state for circuits that call witnesses.
/// Each method corresponds to a witness function in the Compact contract.
pub trait WitnessProvider: Send + Sync {
    /// Called when the circuit invokes a witness function.
    ///
    /// `ctx` carries the mutable private state — read it to compute the
    /// witness value, and mutate it to record state changes that should
    /// survive to the next call. `name` is the witness function name
    /// (e.g. `"private$secret_key"`); `args` are the evaluated arguments.
    ///
    /// Return [`WitnessOutcome::Value`] when the call was handled, and
    /// [`WitnessOutcome::Unknown`] when this provider has no witness named
    /// `name` (the interpreter then falls through to builtins and helpers).
    /// Return `Err` only for genuine failures — a signer that is unreachable,
    /// undecodable private state, bad arguments — which abort the circuit;
    /// errors are never treated as "unknown name".
    fn call_witness(
        &self,
        ctx: &mut WitnessContext<'_>,
        name: &str,
        args: &[Value],
    ) -> Result<WitnessOutcome, InterpreterError>;
}

/// A no-op witness provider that reports every name as unknown.
pub struct NoWitnesses;

impl WitnessProvider for NoWitnesses {
    fn call_witness(
        &self,
        _ctx: &mut WitnessContext<'_>,
        _name: &str,
        _args: &[Value],
    ) -> Result<WitnessOutcome, InterpreterError> {
        Ok(WitnessOutcome::Unknown)
    }
}
