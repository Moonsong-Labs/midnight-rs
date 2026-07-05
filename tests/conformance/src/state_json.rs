//! Canonical JSON form of `StateValue` trees, and the inverse decoder.
//!
//! The shape mirrors the TS runtime's `EncodedStateValue` (`{tag, content}`),
//! with two normalizations both sides apply so the JSON is deterministic and
//! serde-friendly:
//!
//! - map content is an array of `[key, value]` entries sorted by the JSON
//!   text of the key (JS `Map`s have insertion order, Rust storage maps have
//!   hash order; neither is canonical),
//! - bounded Merkle trees are `{height, root}` where `root` is the tree's
//!   root hash field element in hex (the full leaf map is an implementation
//!   detail on both sides; the root pins the content).

use midnight_bindgen_runtime::{AlignedValue, InMemoryDB, StateValue};
use serde_json::{Value as Json, json};

/// Canonical JSON for an `AlignedValue`: `{value: [hex...], alignment}`.
///
/// The alignment side reuses the serde derive (it matches the TS wire shape),
/// but the value atoms are hex strings: the Rust serde form of `ValueAtom` is
/// a raw byte array, while the TS side holds `Uint8Array`s, so hex is the
/// shared canonical text for both.
pub fn aligned_value_to_json(av: &AlignedValue) -> Json {
    json!({
        "value": av.value.0.iter().map(|atom| hex::encode(&atom.0)).collect::<Vec<_>>(),
        "alignment": serde_json::to_value(&av.alignment).expect("Alignment serde is infallible"),
    })
}

/// Parse the canonical JSON back into an `AlignedValue`.
pub fn aligned_value_from_json(json: &Json) -> Result<AlignedValue, String> {
    use midnight_base_crypto::fab;
    let atoms = json
        .get("value")
        .and_then(Json::as_array)
        .ok_or_else(|| format!("aligned value without value array: {json}"))?
        .iter()
        .map(|atom| {
            atom.as_str()
                .ok_or_else(|| format!("value atom is not a hex string: {atom}"))
                .and_then(|s| hex::decode(s).map_err(|e| format!("value atom hex: {e}")))
                .map(fab::ValueAtom)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let alignment: fab::Alignment = serde_json::from_value(
        json.get("alignment")
            .ok_or_else(|| format!("aligned value without alignment: {json}"))?
            .clone(),
    )
    .map_err(|e| format!("alignment: {e}"))?;
    fab::AlignedValue::new(fab::Value(atoms), alignment)
        .ok_or_else(|| format!("value does not fit its alignment: {json}"))
}

/// Canonical JSON for a `StateValue` tree.
pub fn state_value_to_json(sv: &StateValue<InMemoryDB>) -> Json {
    match sv {
        StateValue::Null => json!({ "tag": "null" }),
        StateValue::Cell(av) => json!({ "tag": "cell", "content": aligned_value_to_json(av) }),
        StateValue::Map(map) => {
            let mut entries: Vec<(String, Json)> = map
                .iter()
                .map(|entry| {
                    let (k, v) = &*entry;
                    let kj = aligned_value_to_json(k);
                    (kj.to_string(), json!([kj, state_value_to_json(v)]))
                })
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            json!({
                "tag": "map",
                "content": entries.into_iter().map(|(_, e)| e).collect::<Vec<_>>(),
            })
        }
        StateValue::Array(arr) => json!({
            "tag": "array",
            "content": arr.iter().map(|sv| state_value_to_json(&sv)).collect::<Vec<_>>(),
        }),
        StateValue::BoundedMerkleTree(tree) => json!({
            "tag": "boundedMerkleTree",
            "content": {
                "height": tree.height(),
                "root": format!("{:?}", tree.root()),
            },
        }),
        other => panic!("unhandled StateValue variant: {other:?}"),
    }
}

/// Decode the canonical JSON back into a `StateValue`.
///
/// Bounded Merkle trees are decodable only when blank (`root` matching a
/// blank tree of the same height); the harness seeds circuits from
/// constructor output, and constructors only ever build blank trees.
pub fn state_value_from_json(json: &Json) -> Result<StateValue<InMemoryDB>, String> {
    let tag = json
        .get("tag")
        .and_then(Json::as_str)
        .ok_or_else(|| format!("state value without tag: {json}"))?;
    match tag {
        "null" => Ok(StateValue::Null),
        "cell" => {
            let content = json
                .get("content")
                .ok_or_else(|| "cell without content".to_string())?;
            Ok(StateValue::from(aligned_value_from_json(content)?))
        }
        "map" => {
            let entries = json
                .get("content")
                .and_then(Json::as_array)
                .ok_or_else(|| "map without entry array".to_string())?;
            let mut map = midnight_storage::storage::HashMap::new();
            for entry in entries {
                let pair = entry
                    .as_array()
                    .filter(|p| p.len() == 2)
                    .ok_or_else(|| format!("map entry is not a [key, value] pair: {entry}"))?;
                map = map.insert(
                    aligned_value_from_json(&pair[0])?,
                    state_value_from_json(&pair[1])?,
                );
            }
            Ok(StateValue::Map(map))
        }
        "array" => {
            let entries = json
                .get("content")
                .and_then(Json::as_array)
                .ok_or_else(|| "array without content".to_string())?;
            let mut arr = midnight_storage::storage::Array::new();
            for entry in entries {
                arr = arr.push(state_value_from_json(entry)?);
            }
            Ok(StateValue::Array(arr))
        }
        other => Err(format!("cannot decode state value tag {other:?}")),
    }
}
