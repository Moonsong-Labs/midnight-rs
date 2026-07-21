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

use midnight_contract::runtime::Value;
use midnight_typed_state::AlignedValue;
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
            let le = parse_decimal_le_bytes(body)?;
            let fr = midnight_transient_crypto::curve::Fr::from_le_bytes(&le)
                .ok_or_else(|| format!("field value out of range: {body}"))?;
            Ok(Value::AlignedValue(AlignedValue::from(fr)))
        }
        "uint" => Ok(Value::Integer(parse_decimal(body)?)),
        "enum" => {
            let n = body
                .as_u64()
                .ok_or_else(|| format!("enum ordinal must be a number: {body}"))?;
            Ok(Value::Integer(u128::from(n)))
        }
        "bool" => {
            Ok(Value::Bool(body.as_bool().ok_or_else(|| {
                format!("bool body must be a boolean: {body}")
            })?))
        }
        "string" => {
            let s = body
                .as_str()
                .ok_or_else(|| format!("string body must be a string: {body}"))?;
            // Opaque<"string">: one UTF-8 atom under a compress alignment
            // (`CompactTypeOpaqueString` in the TS runtime).
            use midnight_base_crypto::fab;
            fab::AlignedValue::new(
                fab::Value(vec![fab::ValueAtom(s.as_bytes().to_vec())]),
                fab::Alignment::singleton(fab::AlignmentAtom::Compress),
            )
            .map(Value::AlignedValue)
            .ok_or_else(|| format!("string does not fit a compress alignment: {body}"))
        }
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
        // `[{name: tagged}, ...]` in declaration order. An array (not an
        // object) so the order is explicit on both sides: serde_json sorts
        // object keys, JS preserves insertion order.
        //
        // Builds a `Value::Struct`, the shape whose encoding this fixture
        // exists to pin. Spelling it as a `Value::Tuple` would still encode
        // correctly but would exercise the positional path and leave the
        // named-field path uncovered.
        "struct" => {
            let fields = body
                .as_array()
                .ok_or_else(|| format!("struct body must be an array: {body}"))?;
            let mut named = std::collections::HashMap::with_capacity(fields.len());
            for entry in fields {
                let obj = entry
                    .as_object()
                    .filter(|o| o.len() == 1)
                    .ok_or_else(|| format!("struct field must be a single-key object: {entry}"))?;
                let (name, v) = obj.iter().next().expect("len checked above");
                named.insert(name.clone(), to_interpreter_value(v)?);
            }
            Ok(Value::Struct(named))
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

/// Parse an arbitrary-width decimal string into little-endian bytes (field
/// elements exceed `u128`, so `parse::<u128>` is not enough).
fn parse_decimal_le_bytes(body: &Json) -> Result<Vec<u8>, String> {
    let s = body
        .as_str()
        .ok_or_else(|| format!("numeric body must be a decimal string: {body}"))?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("not a decimal string: {s:?}"));
    }
    let mut le: Vec<u8> = vec![0];
    for digit in s.bytes().map(|b| u16::from(b - b'0')) {
        let mut carry = digit;
        for byte in le.iter_mut() {
            let v = u16::from(*byte) * 10 + carry;
            *byte = (v & 0xff) as u8;
            carry = v >> 8;
        }
        while carry > 0 {
            le.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    while le.len() > 1 && le.last() == Some(&0) {
        le.pop();
    }
    Ok(le)
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
