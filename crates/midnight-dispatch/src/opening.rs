//! A contract's opening ledger state, in a shape no generation owns.
//!
//! Each generation's contract crate has a type of this shape already. They are
//! distinct types, so a caller that named one would pin itself to a
//! generation, but the cell payload is `AlignedValue`, which every generation
//! shares, so it crosses unchanged.

use midnight_base_crypto::fab::AlignedValue;

/// One ledger field's opening value, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpeningField {
    /// A cell holding an encoded value.
    Cell(AlignedValue),
    /// A counter, which opens at this value.
    Counter(u64),
    /// An empty map or set.
    Map,
    /// An empty list.
    List,
    /// An empty merkle tree, historic or not.
    MerkleTree,
}

/// A contract's opening ledger state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Opening {
    pub(crate) fields: Vec<OpeningField>,
}

impl Opening {
    /// The opening state for a contract whose ledger fields are `fields`, in
    /// declaration order.
    pub fn new(fields: Vec<OpeningField>) -> Self {
        Self { fields }
    }
}
