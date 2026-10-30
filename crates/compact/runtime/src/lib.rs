//! Execution-runtime primitives for the Compact circuit IR interpreter.
//!
//! This crate holds the runtime value domain, witness callbacks, and execution
//! result types that the interpreter in `compact-interpreter` builds on. It is
//! the Rust counterpart of Minokawa's `compact/runtime` TypeScript package: a
//! thin layer of primitives over the on-chain runtime, kept separate from the
//! IR tree-walk so it can be reused by any circuit-body front-end.
//!
//! See `docs/ir/circuit-body-ir.md` for how the analyzed IR is produced and consumed.

mod built_ins;
mod compact_types;
mod conversions;
mod error;
mod foreign;
mod result;
mod standard_library;
mod value;
mod witness;
mod zswap;

pub use built_ins::try_builtin_typed;
pub use compact_types::{
    StructLayout, bytes_aligned_value, default_aligned, default_value, element_atom_range,
    element_count, element_type_at, encode_typed, ir_length, layout_from_fields, merkle_leaf_hash,
    slice_atoms,
};
pub use conversions::{value_to_fr, value_to_hash_output, value_to_u128};
pub use error::InterpreterError;
pub use foreign::{
    ArithOp, Curve25519Base, Curve25519Point, Curve25519Scalar, ForeignField, Secp256k1Base,
    Secp256k1Point, Secp256k1Scalar, Secp256r1Base, Secp256r1Point, Secp256r1Scalar,
};
pub use midnight_onchain_runtime::ops::{LogEventType, VersionedLogItem};
pub use result::ExecutionResult;
pub use standard_library::{Either, Maybe};
pub use value::{Value, integer_fallback_aligned};
pub use witness::{NoWitnesses, WitnessContext, WitnessOutcome, WitnessProvider};
pub use zswap::{CircuitZswapInput, CircuitZswapOutput, WitnessNative};
