//! Re-exports of midnight-ledger types used by generated code.
//!
//! Generated bindings import these explicitly by name, e.g.
//! `use compact_bindgen::{Aligned, AlignedValue, ...};` (or from
//! `midnight_typed_state` for the CLI path), so user items with the
//! same names cannot shadow what the generated code references. They are
//! not intended for direct use by consumers; prefer the typed accessors.

pub use midnight_base_crypto::fab::{
    Aligned, AlignedValue, Alignment, InvalidBuiltinDecode, Value, ValueAtom, ValueSlice,
};
pub use midnight_helpers::ledger_storage::db::InMemoryDB;
pub use midnight_helpers::ledger_storage::storage::{
    Array as StorageArray, HashMap as StorageHashMap,
};
pub use midnight_helpers::onchain_runtime::state::{
    ContractMaintenanceAuthority, ContractState, StateValue,
};
pub use midnight_helpers::transient_crypto::curve::{EmbeddedGroupAffine, Fr as TransientFr};
pub use midnight_helpers::transient_crypto::merkle_tree::{MerkleTree, MerkleTreeDigest};
pub use midnight_serialize::{tagged_deserialize, tagged_serialize};
