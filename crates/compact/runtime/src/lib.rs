//! Execution-runtime primitives for the Compact circuit IR interpreter.
//!
//! This crate holds the runtime value domain, witness callbacks, and execution
//! result types that the interpreter in `midnight-contract` builds on. It is
//! the Rust counterpart of Minokawa's `compact/runtime` TypeScript package: a
//! thin layer of primitives over the on-chain runtime, kept separate from the
//! IR tree-walk so it can be reused by any circuit-body front-end.
//!
//! See `docs/ir/` for how the circuit body IR is produced and consumed.

mod built_ins;
mod compact_types;
mod conversions;
mod error;
mod result;
mod value;
mod witness;
mod zswap;

pub use built_ins::try_builtin;
pub use compact_types::{
    StructLayout, build_struct_layouts, bytes_aligned_value, check_uint_range, encode_typed,
};
pub use conversions::{
    aligned_atom_to_u128, value_to_embedded_group, value_to_fr, value_to_hash_output, value_to_u128,
};
pub use error::InterpreterError;
pub use result::ExecutionResult;
pub use value::{Value, integer_fallback_aligned};
pub use witness::{NoWitnesses, WitnessContext, WitnessOutcome, WitnessProvider};
pub use zswap::{CircuitZswapInput, CircuitZswapOutput, WitnessNative};
