//! Just enough SuperJSON v2 to round-trip a `Uint8Array` value at the root.
//!
//! midnight-js encodes every per-contract private state via
//! [`superjson.stringify(value)`](https://github.com/blitz-js/superjson) before
//! placing it into the export's `states[psi]` slot. For our opaque `Vec<u8>`
//! the natural TS counterpart is `Uint8Array`, so we emit and accept the
//! `Uint8Array` envelope shape — that way a midnight-js consumer reading our
//! export back gets the bytes typed, and we can read a midnight-js export that
//! was written from a `Uint8Array` source on the TS side.
//!
//! Wire shape (verified against `superjson@2.2.6`):
//!
//! ```text
//! { "json": [<u8>, <u8>, ...], "meta": { "values": [["typed-array", "Uint8Array"]], "v": 1 } }
//! ```

use serde::{Deserialize, Serialize};

use crate::PrivateStateError;

/// SuperJSON envelope wrapping a `Uint8Array` at the document root. The bytes
/// serialize as a JSON array of numbers (`Vec<u8>` default serde repr).
#[derive(Serialize, Deserialize)]
struct Uint8ArrayEnvelope {
    json: Vec<u8>,
    meta: Meta,
}

#[derive(Serialize, Deserialize)]
struct Meta {
    /// Type tags. For a root-level `Uint8Array` this is exactly
    /// `[["typed-array", "Uint8Array"]]`.
    values: Vec<Vec<String>>,
    v: u32,
}

/// Wrap `bytes` as a JSON-encoded SuperJSON `Uint8Array` envelope. The result
/// is what a midnight-js writer would have produced from
/// `superjson.stringify(new Uint8Array([...]))`.
pub(crate) fn encode_uint8_array(bytes: &[u8]) -> String {
    let env = Uint8ArrayEnvelope {
        json: bytes.to_vec(),
        meta: Meta {
            values: vec![vec!["typed-array".into(), "Uint8Array".into()]],
            v: 1,
        },
    };
    // `serde_json::to_string` on a `Vec<u8>` produces a JSON array of numbers
    // (the default `Vec<T>` serializer); not the base64 path. We rely on that.
    serde_json::to_string(&env)
        .expect("SuperJSON envelope serialization is infallible for owned data")
}

/// Inverse of [`encode_uint8_array`]. Accepts the envelope shape emitted by
/// midnight-js for either `Uint8Array` or `Buffer` typed values — both
/// deserialize to the same byte buffer for our purposes.
pub(crate) fn decode_uint8_array(s: &str) -> Result<Vec<u8>, PrivateStateError> {
    let env: Uint8ArrayEnvelope = serde_json::from_str(s).map_err(|e| {
        PrivateStateError::InvalidFormat(format!(
            "value is not a SuperJSON Uint8Array envelope: {e}",
        ))
    })?;
    let tag = env.meta.values.first().and_then(|v| {
        let kind = v.first().map(String::as_str);
        let sub = v.get(1).map(String::as_str);
        kind.zip(sub)
    });
    let acceptable = matches!(
        tag,
        Some(("typed-array", "Uint8Array" | "Buffer")) | Some(("Buffer", _))
    );
    if !acceptable {
        return Err(PrivateStateError::InvalidFormat(
            "SuperJSON value type is not Uint8Array / Buffer".into(),
        ));
    }
    Ok(env.json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_envelope() {
        let bytes = vec![0, 1, 2, 0xFF, 0x80];
        let env = encode_uint8_array(&bytes);
        let back = decode_uint8_array(&env).unwrap();
        assert_eq!(back, bytes);
    }

    #[test]
    fn envelope_shape_matches_superjson_v2() {
        // Exact wire shape produced by `superjson@2.2.6` for
        // `superjson.stringify(new Uint8Array([1, 2, 255]))`.
        let env = encode_uint8_array(&[1, 2, 255]);
        let parsed: serde_json::Value = serde_json::from_str(&env).unwrap();
        let expected: serde_json::Value = serde_json::from_str(
            r#"{"json":[1,2,255],"meta":{"values":[["typed-array","Uint8Array"]],"v":1}}"#,
        )
        .unwrap();
        assert_eq!(parsed, expected);
    }

    #[test]
    fn decodes_buffer_typed_array_envelopes_too() {
        // A midnight-js writer that picked `Buffer` instead of `Uint8Array`
        // (it's just a Node-specific subclass) round-trips into the same bytes.
        let buffer_env = r#"{"json":[1,2,3],"meta":{"values":[["typed-array","Buffer"]],"v":1}}"#;
        assert_eq!(decode_uint8_array(buffer_env).unwrap(), vec![1u8, 2, 3]);
    }

    #[test]
    fn rejects_non_envelope_string() {
        // Raw base64 (what our previous pass put on the wire) doesn't parse as
        // an envelope; surfaces as InvalidFormat rather than a panic.
        let err = decode_uint8_array("aGVsbG8=").unwrap_err();
        assert!(matches!(err, PrivateStateError::InvalidFormat(_)));
    }

    #[test]
    fn rejects_wrong_type_tag() {
        let env = r#"{"json":"hello","meta":{"values":[["typed-array","Int8Array"]],"v":1}}"#;
        let err = decode_uint8_array(env).unwrap_err();
        assert!(matches!(err, PrivateStateError::InvalidFormat(_)));
    }
}
