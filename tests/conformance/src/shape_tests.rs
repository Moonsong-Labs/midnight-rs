//! Pin the canonical JSON shapes to the TS wire format.
//!
//! These tests encode the contract between `report.rs`/`state_json.rs` and
//! `ts-driver/driver.mjs`: if a serde derive upstream changes shape, they
//! fail here rather than as an opaque golden diff.

use midnight_bindgen_runtime::{AlignedValue, InMemoryDB, StateValue};
use midnight_onchain_runtime::ops::{Key, Op};
use midnight_onchain_runtime::result_mode::ResultModeVerify;
use serde_json::json;

use crate::report::op_to_json;
use crate::state_json::{aligned_value_to_json, state_value_from_json, state_value_to_json};
use serde_json::Value as Json;

#[test]
fn aligned_value_json_matches_ts_shape() {
    let av = AlignedValue::from(42u64);
    assert_eq!(
        aligned_value_to_json(&av),
        json!({
            "value": ["2a"],
            "alignment": [{ "tag": "atom", "value": { "tag": "bytes", "length": 8 } }],
        })
    );

    let b = AlignedValue::from(true);
    assert_eq!(
        aligned_value_to_json(&b),
        json!({
            "value": ["01"],
            "alignment": [{ "tag": "atom", "value": { "tag": "bytes", "length": 1 } }],
        })
    );
}

#[test]
fn field_aligned_value_uses_field_atom() {
    let av = AlignedValue::from(midnight_transient_crypto::curve::Fr::from(7u64));
    assert_eq!(
        aligned_value_to_json(&av),
        json!({
            "value": ["07"],
            "alignment": [{ "tag": "atom", "value": { "tag": "field" } }],
        })
    );
}

#[test]
fn op_json_matches_ts_shape() {
    let ops: Vec<Op<ResultModeVerify, InMemoryDB>> = vec![
        Op::Dup { n: 0 },
        Op::Idx {
            cached: false,
            push_path: false,
            path: vec![Key::Value(AlignedValue::from(1u8))]
                .into_iter()
                .collect(),
        },
        Op::Popeq {
            cached: false,
            result: AlignedValue::from(3u64),
        },
        Op::Add,
        Op::Ins {
            cached: false,
            n: 1,
        },
    ];
    assert_eq!(
        Json::Array(ops.iter().map(op_to_json).collect()),
        json!([
            { "dup": { "n": 0 } },
            { "idx": {
                "cached": false,
                "pushPath": false,
                "path": [{ "tag": "value", "value": {
                    "value": ["01"],
                    "alignment": [{ "tag": "atom", "value": { "tag": "bytes", "length": 1 } }],
                }}],
            }},
            { "popeq": { "cached": false, "result": {
                "value": ["03"],
                "alignment": [{ "tag": "atom", "value": { "tag": "bytes", "length": 8 } }],
            }}},
            "add",
            { "ins": { "cached": false, "n": 1 } },
        ])
    );
}

#[test]
fn push_op_embeds_encoded_state_value() {
    let op: Op<ResultModeVerify, InMemoryDB> = Op::Push {
        storage: false,
        value: StateValue::from(AlignedValue::from(5u64)),
    };
    assert_eq!(
        op_to_json(&op),
        json!({ "push": { "storage": false, "value": {
            "tag": "cell",
            "content": {
                "value": ["05"],
                "alignment": [{ "tag": "atom", "value": { "tag": "bytes", "length": 8 } }],
            },
        }}})
    );
}

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
