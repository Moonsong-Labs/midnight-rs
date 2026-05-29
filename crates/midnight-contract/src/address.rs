//! Contract-address parsing and formatting helpers.

use midnight_coin_structure::contract::ContractAddress;

use crate::error::ContractError;

/// Parse a hex-encoded contract address string into a [`ContractAddress`].
///
/// Accepts 64 hex characters (32 bytes) with or without `0x` prefix.
pub(crate) fn parse_address(hex_addr: &str) -> Result<ContractAddress, ContractError> {
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

/// Format a [`ContractAddress`] as a hex string (no `0x` prefix).
pub(crate) fn format_address(address: &ContractAddress) -> String {
    hex::encode(address.0.0)
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
}
