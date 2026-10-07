//! Errors reported while validating `analyzed-ir.sexp` before code generation.

use std::fmt;

/// A validation error found in `analyzed-ir.sexp`.
///
/// All variants abort code generation: the proc macro surfaces them as
/// compile errors.
#[derive(Debug)]
pub enum CodegenError {
    /// The `compiler-version` is outside
    /// [`SUPPORTED_COMPILER_FAMILY`](crate::types::SUPPORTED_COMPILER_FAMILY).
    UnsupportedCompilerVersion {
        /// The version string found in the file.
        found: String,
    },
    /// A `compiler-version` that does not start with numeric `major.minor`
    /// components.
    MalformedCompilerVersion {
        /// The version string found in the file.
        found: String,
    },
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenError::UnsupportedCompilerVersion { found } => {
                let family = crate::types::SUPPORTED_COMPILER_FAMILY;
                write!(
                    f,
                    "unsupported compiler-version `{found}` (supported: {family}.x); \
                     recompile the contract with compactc {family}.x (see \"Compile a contract\" \
                     in the midnight-rs README), or use a midnight-rs release that supports \
                     compactc {found}"
                )
            }
            CodegenError::MalformedCompilerVersion { found } => {
                write!(
                    f,
                    "malformed compiler-version `{found}`: expected a `major.minor[.patch]` version"
                )
            }
        }
    }
}

impl std::error::Error for CodegenError {}
