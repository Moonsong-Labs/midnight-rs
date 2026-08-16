//! Pre-expansion validation of a parsed artifact.
//!
//! Runs before any code is generated so a problem surfaces as a single
//! precise compile error instead of a panic or broken generated code. The
//! only check left is the schema version gate (see
//! [`crate::types::check_versions`]): the artifact reader rejects an
//! unrepresentable construct while parsing, and the embedded circuit
//! metadata is emitted as typed constructors the compiler checks.

use crate::error::CodegenError;
use crate::types::ContractInfo;

/// Validate a parsed artifact before expansion.
pub fn validate(info: &ContractInfo) -> Result<(), CodegenError> {
    crate::types::check_versions(info)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nir::Type;
    use crate::types::{FieldIndex, LedgerField, StorageKind};

    fn minimal_info(compiler: &str, language: &str, ledger_type: Type) -> ContractInfo {
        ContractInfo {
            compiler_version: compiler.to_string(),
            language_version: language.to_string(),
            runtime_version: "0.16.101".to_string(),
            circuits: Vec::new(),
            witnesses: Vec::new(),
            contracts: Vec::new(),
            ledger: vec![LedgerField {
                name: "count".to_string(),
                index: FieldIndex::Single(0),
                storage: StorageKind::Cell,
                exported: true,
                element_type: Some(ledger_type),
                key: None,
                value: None,
                depth: None,
            }],
            helpers: Vec::new(),
            natives: Vec::new(),
        }
    }

    #[test]
    fn accepts_supported_version_families() {
        validate(&minimal_info("0.33.122", "0.25.107", Type::Boolean))
            .expect("0.33/0.25 supported");
    }

    #[test]
    fn rejects_out_of_range_compiler_version() {
        let err = validate(&minimal_info("0.29.107", "0.22.101", Type::Boolean)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("compiler-version"), "names the field: {msg}");
        assert!(msg.contains("0.29.107"), "names the found value: {msg}");
        assert!(msg.contains("0.33.x"), "names the supported range: {msg}");

        let err = validate(&minimal_info("9.99.0", "0.22.101", Type::Boolean)).unwrap_err();
        assert!(err.to_string().contains("9.99.0"));
    }

    #[test]
    fn rejects_out_of_range_language_version() {
        let err = validate(&minimal_info("0.33.122", "0.99.0", Type::Boolean)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("language-version"), "names the field: {msg}");
        assert!(msg.contains("0.99.0"), "names the found value: {msg}");
        assert!(msg.contains("0.25.x"), "names the supported range: {msg}");
    }

    #[test]
    fn rejects_malformed_version() {
        let err = validate(&minimal_info("nightly", "0.22.101", Type::Boolean)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("malformed compiler-version"), "{msg}");
        assert!(msg.contains("nightly"), "{msg}");
    }
}
