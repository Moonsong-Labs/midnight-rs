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

/// An enum definition shipped by the compiler so the IR consumer can map
/// variant names back to their declaration index (the on-chain encoding
/// is a single `u8` whose value is the variant index).
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<String>,
}
