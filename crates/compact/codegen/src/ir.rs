//! Layout registries the interpreter is given alongside a circuit.
//!
//! Circuits, expressions and types all come from the normalized IR
//! ([`crate::nir`]) now; what remains here is the by-name struct layout a
//! consumer ships for a type that arrives without its own field list.

use crate::nir::Type;

/// A struct definition shipped by the compiler so the IR consumer can compute
/// atom layouts for `Value::AlignedValue` field slicing.
#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Type)>,
}
