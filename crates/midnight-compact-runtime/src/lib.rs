//! Execution-runtime primitives for the Compact circuit IR interpreter.
//!
//! This crate holds the runtime value domain, witness callbacks, and execution
//! result types that the interpreter in `midnight-contract` builds on. It is
//! the Rust counterpart of Minokawa's `compact/runtime` TypeScript package: a
//! thin layer of primitives over the on-chain runtime, kept separate from the
//! IR tree-walk so it can be reused by any circuit-body front-end.
//!
//! See `docs/ir/` for how the circuit body IR is produced and consumed.

mod error;
mod result;
mod value;
mod witness;

pub use error::InterpreterError;
pub use result::{CircuitZswapOutput, ExecutionResult};
pub use value::{Value, integer_fallback_aligned};
pub use witness::{NoWitnesses, WitnessContext, WitnessOutcome, WitnessProvider};
