//! Pre-expansion validation of a parsed artifact.
//!
//! Runs before any code is generated so a problem surfaces as a single
//! precise compile error instead of a panic or broken generated code. The
//! only check left is the schema version gate (see
//! [`crate::types::check_compiler_version`]): the artifact reader rejects an
//! unrepresentable construct while parsing, and the embedded circuit
//! metadata is emitted as typed constructors the compiler checks.

use crate::error::CodegenError;
use crate::types::ContractInfo;

/// Validate a parsed artifact before expansion.
pub fn validate(info: &ContractInfo) -> Result<(), CodegenError> {
    crate::types::check_compiler_version(info)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_info(compiler: &str) -> ContractInfo {
        ContractInfo {
            compiler_version: compiler.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn rejects_a_compiler_outside_the_supported_family() {
        let msg = validate(&minimal_info("0.0.1")).unwrap_err().to_string();
        let supported = format!("{}.x", crate::types::SUPPORTED_COMPILER_FAMILY);
        for part in ["compiler-version", "0.0.1", supported.as_str()] {
            assert!(msg.contains(part), "the error should name `{part}`: {msg}");
        }
    }

    #[test]
    fn rejects_malformed_version() {
        let err = validate(&minimal_info("nightly")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("malformed compiler-version"), "{msg}");
        assert!(msg.contains("nightly"), "{msg}");
    }
}
