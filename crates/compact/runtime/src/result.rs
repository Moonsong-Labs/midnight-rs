//! The outcome of interpreting a circuit: updated state plus the transcript
//! and proving inputs the transaction-construction layer needs.

use midnight_onchain_runtime::ops::Op;
use midnight_onchain_runtime::result_mode::ResultModeGather;
use midnight_typed_state::{AlignedValue, ContractState, InMemoryDB};

use crate::value::Value;
use crate::zswap::CircuitZswapOutput;

/// Result of executing a circuit.
pub struct ExecutionResult {
    /// Updated contract state after execution.
    pub state: ContractState<InMemoryDB>,
    /// Values read from popeq operations (the "gather" results).
    pub reads: Vec<AlignedValue>,
    /// Ops executed in gather mode (for building transcripts).
    pub gather_ops: Vec<Op<ResultModeGather, InMemoryDB>>,
    /// The circuit's return value, if any (non-void circuits).
    pub result: Option<Value>,
    /// Values disclosed via `disclose()` calls (communication outputs).
    /// These must be included in `ContractCallPrototype.output` for the
    /// communication commitment to match the ZKIR's `Output` instructions.
    pub communication_outputs: Vec<AlignedValue>,
    /// Witness return values, in call order — the prover's private transcript
    /// outputs (the ZKIR's private inputs). These must be set on
    /// `ContractCallPrototype.private_transcript_outputs`, or proving a
    /// witness-using circuit fails with "ran out of private transcript outputs".
    /// Empty for witness-free circuits.
    pub private_transcript_outputs: Vec<AlignedValue>,
    /// Coins the circuit asked to create on-chain via `createZswapOutput`
    /// (shielded mints / sends), in call order. The call/deploy path turns
    /// each into an `Output` in the Zswap offer. Empty for circuits that
    /// don't create shielded outputs.
    pub zswap_outputs: Vec<CircuitZswapOutput>,
}
