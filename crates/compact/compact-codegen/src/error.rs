//! Errors reported while validating `contract-info.json` before code generation.

use std::fmt;

/// A validation error found in `contract-info.json`.
///
/// All variants abort code generation: the proc macro surfaces them as
/// compile errors, the CLI as a non-zero exit.
#[derive(Debug)]
pub enum CodegenError {
    /// `compiler-version` / `language-version` is outside the supported range.
    UnsupportedVersion {
        /// The JSON field name (`compiler-version` or `language-version`).
        field: &'static str,
        /// The version string found in the file.
        found: String,
        /// The supported `major.minor` families.
        supported: &'static [&'static str],
    },
    /// A version field that does not start with numeric `major.minor` components.
    MalformedVersion {
        /// The JSON field name (`compiler-version` or `language-version`).
        field: &'static str,
        /// The version string found in the file.
        found: String,
    },
    /// A `type-name` this generator does not recognize.
    UnknownTypeName {
        /// The unrecognized `type-name` value.
        type_name: String,
        /// Human-readable path to the offending type node.
        location: String,
    },
    /// Embedded IR / helper / struct / enum definitions failed to round-trip
    /// through JSON (they are embedded as string constants in generated code).
    EmbedJson {
        /// What was being embedded (e.g. ``IR for circuit `increment` ``).
        what: String,
        /// The underlying serde error.
        source: serde_json::Error,
    },
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenError::UnsupportedVersion {
                field,
                found,
                supported,
            } => {
                let families = supported
                    .iter()
                    .map(|fam| format!("{fam}.x"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "unsupported {field} `{found}` (supported: {families}); \
                     recompile the contract with a supported Compact compiler, or widen \
                     the supported range in compact-codegen/src/types.rs"
                )
            }
            CodegenError::MalformedVersion { field, found } => {
                write!(
                    f,
                    "malformed {field} `{found}`: expected a `major.minor[.patch]` version"
                )
            }
            CodegenError::UnknownTypeName {
                type_name,
                location,
            } => {
                let known = crate::types::KNOWN_TYPE_NAMES.join(", ");
                write!(
                    f,
                    "unknown type-name `{type_name}` in {location}; this version of \
                     midnight-bindgen does not support it (known type-names: {known})"
                )
            }
            CodegenError::EmbedJson { what, source } => {
                write!(f, "failed to embed {what} as JSON: {source}")
            }
        }
    }
}

impl std::error::Error for CodegenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CodegenError::EmbedJson { source, .. } => Some(source),
            _ => None,
        }
    }
}
