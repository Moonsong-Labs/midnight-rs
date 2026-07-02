//! Contract-address parsing and formatting helpers.

use midnight_coin_structure::contract::ContractAddress;

use crate::error::ContractError;

/// Parse a hex-encoded contract address string into a [`ContractAddress`].
///
/// Accepts 64 hex characters (32 bytes) with or without `0x` prefix.
pub fn parse_address(hex_addr: &str) -> Result<ContractAddress, ContractError> {
    let hex = hex_addr.strip_prefix("0x").unwrap_or(hex_addr);
    let bytes =
        hex::decode(hex).map_err(|e| ContractError::InvalidAddress(format!("hex decode: {e}")))?;
    if bytes.len() != 32 {
        return Err(ContractError::InvalidAddress(format!(
            "expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(ContractAddress(midnight_base_crypto::hash::HashOutput(arr)))
}

/// Format a [`ContractAddress`] as a hex string (no `0x` prefix), the form
/// accepted by [`Contract::at`](crate::Contract::at).
pub fn format_address(address: &ContractAddress) -> String {
    hex::encode(address.0.0)
}

/// A value usable as a contract address at the connect boundary
/// ([`Contract::at`](crate::Contract::at)). Strings pass through unchanged and
/// keep their lazy, parse-on-use semantics; a typed [`ContractAddress`] is
/// formatted to the same canonical hex, so callers holding one never convert
/// by hand.
pub trait IntoAddress {
    fn into_address_string(self) -> String;
}

impl IntoAddress for String {
    fn into_address_string(self) -> String {
        self
    }
}

impl IntoAddress for &str {
    fn into_address_string(self) -> String {
        self.to_string()
    }
}

impl IntoAddress for &String {
    fn into_address_string(self) -> String {
        self.clone()
    }
}

impl IntoAddress for ContractAddress {
    fn into_address_string(self) -> String {
        format_address(&self)
    }
}

impl IntoAddress for &ContractAddress {
    fn into_address_string(self) -> String {
        format_address(self)
    }
}

/// serde adapter (de)serializing a [`ContractAddress`] as its hex string form.
/// The type's native serde impl uses raw bytes, which text formats (TOML/JSON
/// config files) cannot carry. Use with
/// `#[serde(with = "midnight_contract::address_serde")]`.
pub mod address_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::{ContractAddress, format_address, parse_address};

    pub fn serialize<S: Serializer>(
        address: &ContractAddress,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format_address(address))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ContractAddress, D::Error> {
        use serde::de::Error as _;
        parse_address(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_address_with_prefix() {
        let hex = "0x".to_string() + &"aa".repeat(32);
        let addr = parse_address(&hex).unwrap();
        assert_eq!(addr.0.0, [0xAA; 32]);
    }

    #[test]
    fn parse_address_without_prefix() {
        let hex = "bb".repeat(32);
        let addr = parse_address(&hex).unwrap();
        assert_eq!(addr.0.0, [0xBB; 32]);
    }

    #[test]
    fn parse_address_wrong_length() {
        let err = parse_address("aabb").unwrap_err();
        assert!(err.to_string().contains("expected 32 bytes"));
    }

    #[test]
    fn parse_address_invalid_hex() {
        let err = parse_address("zzzz").unwrap_err();
        assert!(err.to_string().contains("hex decode"));
    }

    #[test]
    fn format_address_roundtrip() {
        let hex_in = "cc".repeat(32);
        let addr = parse_address(&hex_in).unwrap();
        let hex_out = format_address(&addr);
        assert_eq!(hex_in, hex_out);
    }

    #[test]
    fn into_address_strings_pass_through_unvalidated() {
        assert_eq!("addr1".into_address_string(), "addr1");
        assert_eq!(String::from("addr1").into_address_string(), "addr1");
        assert_eq!((&String::from("addr1")).into_address_string(), "addr1");
    }

    #[test]
    fn into_address_formats_typed_addresses() {
        let hex = "dd".repeat(32);
        let addr = parse_address(&hex).unwrap();
        assert_eq!((&addr).into_address_string(), hex);
        assert_eq!(addr.into_address_string(), hex);
    }

    #[test]
    fn address_serde_round_trips_hex() {
        let hex = "ee".repeat(32);
        let json = format!(r#""0x{hex}""#);
        let mut de = serde_json::Deserializer::from_str(&json);
        let addr = address_serde::deserialize(&mut de).unwrap();
        assert_eq!(format_address(&addr), hex);

        let value = address_serde::serialize(&addr, serde_json::value::Serializer).unwrap();
        assert_eq!(value, serde_json::Value::String(hex));
    }

    #[test]
    fn address_serde_rejects_invalid_hex() {
        let mut de = serde_json::Deserializer::from_str(r#""zz""#);
        let err = address_serde::deserialize(&mut de).unwrap_err();
        assert!(err.to_string().contains("invalid address"));
    }
}
