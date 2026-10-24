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
//! - `{"<curve>Base": "<decimal>"}`, `{"<curve>Scalar": "<decimal>"}`: an
//!   element of that field, for `<curve>` one of `secp256k1`, `secp256r1` and
//!   `curve25519`
//! - `{"<curve>Point": {"x": "<decimal>", "y": "<decimal>"}}`: the point of the
//!   curve at those affine coordinates. `{"secp256k1Point": "identity"}` and
//!   `{"secp256r1Point": "identity"}` are the identity. The Curve25519
//!   identity is the point at (0, 1).
//!
//! The TS driver has the mirror converter in `ts-driver/driver.mjs`.

use midnight_contract::runtime::{
    Curve25519Base, Curve25519Point, Curve25519Scalar, Secp256k1Base, Secp256k1Point,
    Secp256k1Scalar, Secp256r1Base, Secp256r1Point, Secp256r1Scalar, Value, bytes_aligned_value,
};
use midnight_typed_state::AlignedValue;
use serde_json::Value as Json;

use crate::state_json::hex_from_json;

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
            let bytes = hex_from_json(body).map_err(|e| format!("bytes body: {e}"))?;
            let len = bytes.len();
            Ok(Value::AlignedValue(
                bytes_aligned_value(bytes, len).map_err(|e| e.to_string())?,
            ))
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
                // A repeated name would collapse into one map entry, and both
                // executors would agree on the last one, so the case would pass
                // while testing something other than what it spells out.
                if named.contains_key(name) {
                    return Err(format!("duplicate struct field {name:?}"));
                }
                named.insert(name.clone(), to_interpreter_value(v)?);
            }
            Ok(Value::Struct(named))
        }
        "secp256k1Base" => Ok(aligned(field(
            body,
            Secp256k1Base::from_be_bytes,
            "Secp256k1Base",
        )?)),
        "secp256k1Scalar" => Ok(aligned(field(
            body,
            Secp256k1Scalar::from_be_bytes,
            "Secp256k1Scalar",
        )?)),
        "secp256k1Point" => Ok(aligned(match body.as_str() {
            Some("identity") => Secp256k1Point::default(),
            _ => Secp256k1Point::from_coordinates(
                field(&body["x"], Secp256k1Base::from_be_bytes, "Secp256k1Base")?,
                field(&body["y"], Secp256k1Base::from_be_bytes, "Secp256k1Base")?,
            )
            .ok_or_else(|| format!("not a point of secp256k1: {body}"))?,
        })),
        "secp256r1Base" => Ok(aligned(field(
            body,
            Secp256r1Base::from_be_bytes,
            "Secp256r1Base",
        )?)),
        "secp256r1Scalar" => Ok(aligned(field(
            body,
            Secp256r1Scalar::from_be_bytes,
            "Secp256r1Scalar",
        )?)),
        "secp256r1Point" => Ok(aligned(match body.as_str() {
            Some("identity") => Secp256r1Point::default(),
            _ => Secp256r1Point::from_coordinates(
                field(&body["x"], Secp256r1Base::from_be_bytes, "Secp256r1Base")?,
                field(&body["y"], Secp256r1Base::from_be_bytes, "Secp256r1Base")?,
            )
            .ok_or_else(|| format!("not a point of secp256r1: {body}"))?,
        })),
        "curve25519Base" => Ok(aligned(field(
            body,
            Curve25519Base::from_be_bytes,
            "Curve25519Base",
        )?)),
        "curve25519Scalar" => Ok(aligned(field(
            body,
            Curve25519Scalar::from_be_bytes,
            "Curve25519Scalar",
        )?)),
        "curve25519Point" => Ok(aligned(
            Curve25519Point::from_coordinates(
                field(&body["x"], Curve25519Base::from_be_bytes, "Curve25519Base")?,
                field(&body["y"], Curve25519Base::from_be_bytes, "Curve25519Base")?,
            )
            .ok_or_else(|| {
                format!("not a point of the prime-order subgroup of Curve25519: {body}")
            })?,
        )),
        other => Err(format!("unknown value tag {other:?}")),
    }
}

fn aligned(value: impl Into<AlignedValue>) -> Value {
    Value::AlignedValue(value.into())
}

/// The element of a foreign field that the decimal `body` spells. `name` is
/// the Compact type of the field.
fn field<T>(
    body: &Json,
    from_be_bytes: fn([u8; 32]) -> Option<T>,
    name: &str,
) -> Result<T, String> {
    from_be_bytes(parse_decimal_be_32(body)?).ok_or_else(|| format!("{name} out of range: {body}"))
}

/// Parse a decimal string below 2^256 into 32 big-endian bytes.
fn parse_decimal_be_32(body: &Json) -> Result<[u8; 32], String> {
    let le = parse_decimal_le_bytes(body)?;
    let mut bytes = [0u8; 32];
    bytes
        .get_mut(..le.len())
        .ok_or_else(|| format!("decimal wider than 32 bytes: {body}"))?
        .copy_from_slice(&le);
    bytes.reverse();
    Ok(bytes)
}

pub(crate) fn parse_decimal(body: &Json) -> Result<u128, String> {
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
