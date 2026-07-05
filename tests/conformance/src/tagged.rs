//! The tagged value language used by case files.
//!
//! Case JSONs describe circuit arguments and scripted witness returns in a
//! self-describing form both executors convert from independently:
//!
//! - `{"field": "<decimal>"}`: a field element
//! - `{"uint": "<decimal>"}`: an unsigned integer (width comes from the
//!   declared circuit argument type on each side)
//! - `{"bytes": "<hex>"}`: fixed-width bytes; the width is the hex length
//! - `{"bool": true}`
//! - `{"enum": <ordinal>}`
//! - `{"vector": [ ... ]}`: element-wise tagged values
//!
//! The TS driver has the mirror converter in `ts-driver/driver.mjs`.

use midnight_bindgen_runtime::AlignedValue;
use midnight_contract::interpreter::Value;
use serde_json::Value as Json;

/// Convert a tagged case value into the interpreter's argument [`Value`],
/// mirroring what bindgen-generated code passes: integers stay
/// `Value::Integer` (the interpreter applies the declared width), everything
/// else arrives pre-encoded as `Value::AlignedValue`.
pub fn to_interpreter_value(tagged: &Json) -> Result<Value, String> {
    let obj = tagged
        .as_object()
        .filter(|o| o.len() == 1)
        .ok_or_else(|| format!("tagged value must be a single-key object: {tagged}"))?;
    let (tag, body) = obj.iter().next().expect("len checked above");
    match tag.as_str() {
        "field" => {
            let n = parse_decimal(body)?;
            Ok(Value::AlignedValue(AlignedValue::from(
                midnight_transient_crypto::curve::Fr::from(n),
            )))
        }
        "uint" => Ok(Value::Integer(parse_decimal(body)?)),
        "enum" => {
            let n = body
                .as_u64()
                .ok_or_else(|| format!("enum ordinal must be a number: {body}"))?;
            Ok(Value::Integer(u128::from(n)))
        }
        "bool" => Ok(Value::Bool(
            body.as_bool()
                .ok_or_else(|| format!("bool body must be a boolean: {body}"))?,
        )),
        "bytes" => {
            let hex_str = body
                .as_str()
                .ok_or_else(|| format!("bytes body must be a hex string: {body}"))?;
            let bytes = hex::decode(hex_str).map_err(|e| format!("bytes hex: {e}"))?;
            Ok(Value::AlignedValue(bytes_aligned(bytes)?))
        }
        "vector" => {
            let items = body
                .as_array()
                .ok_or_else(|| format!("vector body must be an array: {body}"))?;
            Ok(Value::Tuple(
                items
                    .iter()
                    .map(to_interpreter_value)
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        other => Err(format!("unknown value tag {other:?}")),
    }
}

fn parse_decimal(body: &Json) -> Result<u128, String> {
    body.as_str()
        .ok_or_else(|| format!("numeric body must be a decimal string: {body}"))?
        .parse::<u128>()
        .map_err(|e| format!("decimal parse: {e}"))
}

/// Single-atom `Bytes<N>` aligned value from raw bytes, with the FAB
/// normal-form trailing-zero trim.
fn bytes_aligned(bytes: Vec<u8>) -> Result<AlignedValue, String> {
    use midnight_base_crypto::fab;
    let length = bytes.len();
    let mut atom = bytes;
    while matches!(atom.last(), Some(0)) {
        atom.pop();
    }
    fab::AlignedValue::new(
        fab::Value(vec![fab::ValueAtom(atom)]),
        fab::Alignment::singleton(fab::AlignmentAtom::Bytes {
            length: length as u32,
        }),
    )
    .ok_or_else(|| format!("bytes do not fit a Bytes<{length}> alignment"))
}
