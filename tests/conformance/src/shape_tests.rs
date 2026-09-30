//! Round trips of the state JSON through its decoder, for the map and Merkle
//! tree shapes and the sort order of their entries.

use midnight_base_crypto::hash::HashOutput;
use midnight_transient_crypto::merkle_tree::MerkleTree;
use midnight_typed_state::{AlignedValue, InMemoryDB, StateValue};
use serde_json::json;

use crate::state_json::{state_value_from_json, state_value_to_json};

#[test]
fn state_value_json_roundtrips() {
    let mut map = midnight_storage::storage::HashMap::new();
    map = map.insert(AlignedValue::from(2u64), StateValue::from(20u64));
    map = map.insert(AlignedValue::from(1u64), StateValue::from(10u64));

    let arr: StateValue<InMemoryDB> = StateValue::Array(
        vec![
            StateValue::Null,
            StateValue::from(7u64),
            StateValue::Map(map),
        ]
        .into(),
    );

    let encoded = state_value_to_json(&arr);
    let decoded = state_value_from_json(&encoded).unwrap();
    assert_eq!(decoded, arr);

    // Map entries are sorted by key JSON, independent of insertion order.
    let entries = encoded["content"][2]["content"].as_array().unwrap();
    assert_eq!(entries[0][0]["value"][0], "01");
    assert_eq!(entries[1][0]["value"][0], "02");
}

#[test]
fn bounded_merkle_tree_json_roundtrips() {
    let tree: MerkleTree<(), InMemoryDB> = MerkleTree::blank(4)
        .try_update_hash(2, HashOutput([2u8; 32]), ())
        .and_then(|t| t.try_update_hash(1, HashOutput([1u8; 32]), ()))
        .expect("both indices fit a height-4 tree")
        .rehash();
    let sv = StateValue::BoundedMerkleTree(tree);
    let encoded = state_value_to_json(&sv);

    assert_eq!(encoded["content"]["height"], json!(4));
    // Leaves are sorted by index, independent of insertion order.
    assert_eq!(encoded["content"]["leaves"][0][0], json!(1));
    assert_eq!(encoded["content"]["leaves"][1][0], json!(2));

    assert_eq!(state_value_from_json(&encoded).unwrap(), sv);
}
