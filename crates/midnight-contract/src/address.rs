//! Contract-address parsing and formatting helpers.

use midnight_types::ContractAddress;

use crate::error::ContractError;

/// Parse a hex-encoded contract address string into a [`ContractAddress`].
///
/// Accepts 64 hex characters (32 bytes) with or without `0x` prefix.
pub fn parse_contract_address(hex_addr: &str) -> Result<ContractAddress, ContractError> {
    let hex = hex_addr.strip_prefix("0x").unwrap_or(hex_addr);
    let bytes = hex::decode(hex)
        .map_err(|e| ContractError::InvalidContractAddress(format!("hex decode: {e}")))?;
    if bytes.len() != 32 {
        return Err(ContractError::InvalidContractAddress(format!(
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
pub fn format_contract_address(address: &ContractAddress) -> String {
    hex::encode(address.0.0)
}

/// A value usable as a contract address at the connect boundary
/// ([`Contract::at`](crate::Contract::at)).
///
/// A typed [`ContractAddress`], and a string that [`parse_contract_address`]
/// accepts, become the hex form of [`format_contract_address`]: lower case,
/// without `0x`. A deploy gives the contract address in the same form. A
/// string that does not parse as a contract address stays as given. The first
/// query or call with that string fails.
pub trait IntoContractAddress {
    fn into_contract_address_string(self) -> String;
}

impl IntoContractAddress for &str {
    fn into_contract_address_string(self) -> String {
        parse_contract_address(self).map_or_else(
            |_| self.to_owned(),
            |address| format_contract_address(&address),
        )
    }
}

impl IntoContractAddress for String {
    fn into_contract_address_string(self) -> String {
        self.as_str().into_contract_address_string()
    }
}

impl IntoContractAddress for &String {
    fn into_contract_address_string(self) -> String {
        self.as_str().into_contract_address_string()
    }
}

impl IntoContractAddress for ContractAddress {
    fn into_contract_address_string(self) -> String {
        format_contract_address(&self)
    }
}

impl IntoContractAddress for &ContractAddress {
    fn into_contract_address_string(self) -> String {
        format_contract_address(self)
    }
}

/// serde adapter (de)serializing a [`ContractAddress`] as its hex string form.
/// The type's native serde impl uses raw bytes, which text formats (TOML/JSON
/// config files) cannot carry. Use with
/// `#[serde(with = "midnight_contract::contract_address_serde")]`.
pub mod contract_address_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::{ContractAddress, format_contract_address, parse_contract_address};

    pub fn serialize<S: Serializer>(
        address: &ContractAddress,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format_contract_address(address))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ContractAddress, D::Error> {
        use serde::de::Error as _;
        parse_contract_address(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_contract_address_with_prefix() {
        let hex = "0x".to_string() + &"aa".repeat(32);
        let addr = parse_contract_address(&hex).unwrap();
        assert_eq!(addr.0.0, [0xAA; 32]);
    }

    #[test]
    fn parse_contract_address_without_prefix() {
        let hex = "bb".repeat(32);
        let addr = parse_contract_address(&hex).unwrap();
        assert_eq!(addr.0.0, [0xBB; 32]);
    }

    #[test]
    fn parse_contract_address_wrong_length() {
        let err = parse_contract_address("aabb").unwrap_err();
        assert!(err.to_string().contains("expected 32 bytes"));
    }

    #[test]
    fn parse_contract_address_invalid_hex() {
        let err = parse_contract_address("zzzz").unwrap_err();
        assert!(err.to_string().contains("hex decode"));
    }

    #[test]
    fn into_contract_address_keeps_unparseable_strings() {
        assert_eq!("addr1".into_contract_address_string(), "addr1");
        assert_eq!(
            String::from("addr1").into_contract_address_string(),
            "addr1"
        );
        assert_eq!(
            (&String::from("addr1")).into_contract_address_string(),
            "addr1"
        );
    }

    #[test]
    fn into_contract_address_canonicalizes_parseable_strings() {
        let input = "0x".to_string() + &"AB".repeat(32);
        let canonical = "ab".repeat(32);
        assert_eq!(input.as_str().into_contract_address_string(), canonical);
        assert_eq!((&input).into_contract_address_string(), canonical);
        assert_eq!(input.into_contract_address_string(), canonical);
    }

    #[test]
    fn into_contract_address_formats_typed_addresses() {
        let hex = "dd".repeat(32);
        let addr = parse_contract_address(&hex).unwrap();
        assert_eq!((&addr).into_contract_address_string(), hex);
        assert_eq!(addr.into_contract_address_string(), hex);
    }

    #[test]
    fn contract_address_serde_round_trips_hex() {
        let hex = "ee".repeat(32);
        let json = format!(r#""0x{hex}""#);
        let mut de = serde_json::Deserializer::from_str(&json);
        let addr = contract_address_serde::deserialize(&mut de).unwrap();
        assert_eq!(format_contract_address(&addr), hex);

        let value =
            contract_address_serde::serialize(&addr, serde_json::value::Serializer).unwrap();
        assert_eq!(value, serde_json::Value::String(hex));
    }

    #[test]
    fn contract_address_serde_rejects_invalid_hex() {
        let mut de = serde_json::Deserializer::from_str(r#""zz""#);
        let err = contract_address_serde::deserialize(&mut de).unwrap_err();
        assert!(err.to_string().contains("invalid contract address"));
    }
}
