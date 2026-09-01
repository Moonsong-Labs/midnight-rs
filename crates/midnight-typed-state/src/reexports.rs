//! Re-exports of midnight-ledger types used by generated code.
//!
//! Generated bindings import these explicitly by name, e.g.
//! `use compact_bindgen::{Aligned, AlignedValue, ...};` (or from
//! `midnight_typed_state` for the CLI path), so user items with the
//! same names cannot shadow what the generated code references. They are
//! not intended for direct use by consumers; prefer the typed accessors.

pub use compact_values::{Field, JubjubPoint};
pub use midnight_base_crypto::fab::{
    Aligned, AlignedValue, Alignment, InvalidBuiltinDecode, Value, ValueAtom, ValueSlice,
};
pub use midnight_serialize::{tagged_deserialize, tagged_serialize};
pub use midnight_types::ledger_storage::db::InMemoryDB;
pub use midnight_types::ledger_storage::storage::{
    Array as StorageArray, HashMap as StorageHashMap,
};
pub use midnight_types::onchain_runtime::state::{
    ContractMaintenanceAuthority, ContractState, StateValue,
};
pub use midnight_types::transient_crypto::curve::{EmbeddedGroupAffine, Fr as TransientFr};
pub use midnight_types::transient_crypto::merkle_tree::{MerkleTree, MerkleTreeDigest};

/// Move a field element between the neutral form generated bindings name and
/// the ledger's own.
///
/// Both hold the same little-endian bytes, so this copies rather than
/// re-encodes.
pub fn field_to_ledger(field: &Field) -> Option<TransientFr> {
    TransientFr::from_le_bytes(field.as_le_bytes())
}

/// The neutral form of a ledger field element. See [`field_to_ledger`].
pub fn field_from_ledger(fr: &TransientFr) -> Field {
    Field::from_le_bytes(fr.as_le_bytes())
}
