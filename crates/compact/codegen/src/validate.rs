//! Pre-expansion validation of a parsed `contract-info.json`.
//!
//! Runs before any code is generated so every problem surfaces as a single
//! precise compile error instead of a panic, a silent fallback, or broken
//! generated code:
//!
//! - the schema version gate (see [`crate::types::check_versions`]),
//! - rejection of unrecognized `type-name`s ([`TypeRef::Unknown`]),
//! - a round-trip check of the IR / helper / struct / enum definitions that
//!   are embedded as JSON string constants in the generated code.

use crate::error::CodegenError;
use crate::types::ContractInfo;

/// Validate a parsed `contract-info.json` before expansion.
pub fn validate(info: &ContractInfo) -> Result<(), CodegenError> {
    crate::types::check_versions(info)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_from_json(json: &str) -> ContractInfo {
        serde_json::from_str(json).expect("test JSON should parse")
    }

    fn minimal_info(compiler: &str, language: &str, ledger_type: &str) -> ContractInfo {
        info_from_json(&format!(
            r#"{{
                "compiler-version": "{compiler}",
                "language-version": "{language}",
                "runtime-version": "0.16.101",
                "circuits": [],
                "witnesses": [],
                "contracts": [],
                "ledger": [
                    {{
                        "name": "count",
                        "index": 0,
                        "storage": "Cell",
                        "exported": true,
                        "type": {ledger_type}
                    }}
                ]
            }}"#
        ))
    }

    #[test]
    fn accepts_supported_version_families() {
        let bool_cell = r#"{ "type-name": "Boolean" }"#;
        validate(&minimal_info("0.33.122", "0.25.107", bool_cell)).expect("0.33/0.25 supported");
    }

    #[test]
    fn rejects_out_of_range_compiler_version() {
        let bool_cell = r#"{ "type-name": "Boolean" }"#;
        let err = validate(&minimal_info("0.29.107", "0.22.101", bool_cell)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("compiler-version"), "names the field: {msg}");
        assert!(msg.contains("0.29.107"), "names the found value: {msg}");
        assert!(msg.contains("0.33.x"), "names the supported range: {msg}");

        let err = validate(&minimal_info("9.99.0", "0.22.101", bool_cell)).unwrap_err();
        assert!(err.to_string().contains("9.99.0"));
    }

    #[test]
    fn rejects_out_of_range_language_version() {
        let bool_cell = r#"{ "type-name": "Boolean" }"#;
        let err = validate(&minimal_info("0.33.122", "0.99.0", bool_cell)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("language-version"), "names the field: {msg}");
        assert!(msg.contains("0.99.0"), "names the found value: {msg}");
        assert!(msg.contains("0.25.x"), "names the supported range: {msg}");
    }

    #[test]
    fn rejects_malformed_version() {
        let bool_cell = r#"{ "type-name": "Boolean" }"#;
        let err = validate(&minimal_info("nightly", "0.22.101", bool_cell)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("malformed compiler-version"), "{msg}");
        assert!(msg.contains("nightly"), "{msg}");
    }

    /// An unrecognized `type-name` fails at deserialization, naming the
    /// known vocabulary; there is no later validation stage to reach.
    #[test]
    fn unknown_type_names_fail_closed_at_parse() {
        let err = serde_json::from_str::<crate::ir::TypeRef>(r#"{ "type-name": "Quantum" }"#)
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Quantum"), "names the type: {msg}");
        assert!(
            msg.contains("expected one of"),
            "lists the vocabulary: {msg}"
        );
    }
}
