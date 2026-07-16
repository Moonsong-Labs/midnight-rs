//! The outcome of interpreting a circuit: updated state plus the transcript
//! and proving inputs the transaction-construction layer needs.

use midnight_bindgen_runtime::{AlignedValue, ContractState, InMemoryDB};
use midnight_onchain_runtime::ops::Op;
use midnight_onchain_runtime::result_mode::ResultModeGather;

use crate::value::Value;

/// A shielded coin the circuit asked to create on-chain via the
/// `createZswapOutput` kernel native (e.g. through `mintShieldedToken` or
/// `sendShielded`).
///
/// `createZswapOutput(coin, recipient)` records no ledger effect of its own
/// (the mint/spend/receive effects are separate `ledger-query` ops); it marks
/// "attach a Zswap output for this coin here". The interpreter captures the
/// raw arg `Value`s so the call/deploy path can build the corresponding
/// `Output` in the transaction's Zswap offer (optionally with a discovery
/// ciphertext keyed to the recipient's encryption public key).
#[derive(Debug, Clone)]
pub struct CircuitZswapOutput {
    /// The `ShieldedCoinInfo` the circuit constructed (nonce, color/type,
    /// value), as evaluated by the interpreter — a struct-encoded value.
    pub coin: Value,
    /// The `Either<ZswapCoinPublicKey, ContractAddress>` recipient the circuit
    /// passed, as evaluated by the interpreter.
    pub recipient: Value,
}

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
