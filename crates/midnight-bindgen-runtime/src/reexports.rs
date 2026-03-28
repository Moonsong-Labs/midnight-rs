//! Re-exports of midnight-ledger types used by generated code.
//!
//! Generated bindings import these via `use midnight_bindgen::*` (or
//! `use midnight_bindgen_runtime::*` for the CLI path). They are not
//! intended for direct use by consumers — prefer the typed accessors.

pub use midnight_base_crypto::fab::{
    Aligned, AlignedValue, Alignment, InvalidBuiltinDecode, Value, ValueAtom, ValueSlice,
};
pub use midnight_onchain_state::state::{ContractMaintenanceAuthority, ContractState, StateValue};
pub use midnight_serialize::{tagged_deserialize, tagged_serialize};
pub use midnight_storage::db::InMemoryDB;
pub use midnight_storage::storage::{Array as StorageArray, HashMap as StorageHashMap};
pub use midnight_transient_crypto::curve::{EmbeddedGroupAffine, Fr as TransientFr};
pub use midnight_transient_crypto::merkle_tree::{MerkleTree, MerkleTreeDigest};
