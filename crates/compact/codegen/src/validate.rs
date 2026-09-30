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

    fn minimal_info(compiler: &str, language: &str) -> ContractInfo {
        ContractInfo {
            compiler_version: compiler.to_string(),
            language_version: language.to_string(),
            runtime_version: "0.16.101".to_string(),
            circuits: Vec::new(),
            witnesses: Vec::new(),
            contracts: Vec::new(),
            ledger: Vec::new(),
            helpers: Vec::new(),
            natives: Vec::new(),
        }
    }

    #[test]
    fn rejects_a_version_outside_the_supported_families() {
        // Each row names the field, the found value, and the supported range.
        for (compiler, language, named) in [
            (
                "0.29.107",
                "0.25.107",
                ["compiler-version", "0.29.107", "0.33.x"],
            ),
            (
                "0.33.122",
                "0.99.0",
                ["language-version", "0.99.0", "0.25.x"],
            ),
        ] {
            let msg = validate(&minimal_info(compiler, language))
                .unwrap_err()
                .to_string();
            for part in named {
                assert!(msg.contains(part), "the error should name `{part}`: {msg}");
            }
        }
    }

    #[test]
    fn rejects_malformed_version() {
        let err = validate(&minimal_info("nightly", "0.22.101")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("malformed compiler-version"), "{msg}");
        assert!(msg.contains("nightly"), "{msg}");
    }
}
