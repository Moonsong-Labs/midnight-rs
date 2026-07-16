use serde::Deserialize;

use crate::error::CodegenError;

/// `compiler-version` `major.minor` families this generator is known to work
/// with. Checked by [`check_versions`] before any code is generated; a
/// `contract-info.json` outside this range fails compilation.
///
/// The range is derived from the committed fixtures: `tests/fixtures/*`
/// (compactc 0.30.102) and `crates/midnight-contract/tests/fixtures/*` /
/// `devnet/contracts/*` (compactc fork 0.31.104).
///
/// When the Compact compiler fork bumps its version:
/// 1. regenerate the contracts and fixtures (`make build-compactc
///    compile-contracts regen-test-fixtures`),
/// 2. add the new `major.minor` family here (and the matching language family
///    to [`SUPPORTED_LANGUAGE_VERSION_FAMILIES`]),
/// 3. re-bless the trybuild expectation that embeds the supported list:
///    `TRYBUILD=overwrite cargo test -p compact-bindgen-macro` rewrites
///    `tests/ui/fail/version-mismatch.stderr`; eyeball the diff,
/// 4. run the full test suite; drop an old family only once no fixture or
///    devnet contract uses it anymore.
pub const SUPPORTED_COMPILER_VERSION_FAMILIES: &[&str] = &["0.30", "0.31"];

/// `language-version` `major.minor` families this generator is known to work
/// with. See [`SUPPORTED_COMPILER_VERSION_FAMILIES`] for how to widen.
pub const SUPPORTED_LANGUAGE_VERSION_FAMILIES: &[&str] = &["0.22", "0.23"];

/// Check `compiler-version` and `language-version` against the supported
/// `major.minor` families. Called before expansion; failing the gate aborts
/// code generation with a compile error naming the field and the range.
pub fn check_versions(info: &ContractInfo) -> Result<(), CodegenError> {
    check_version_field(
        "compiler-version",
        &info.compiler_version,
        SUPPORTED_COMPILER_VERSION_FAMILIES,
    )?;
    check_version_field(
        "language-version",
        &info.language_version,
        SUPPORTED_LANGUAGE_VERSION_FAMILIES,
    )?;
    Ok(())
}

fn check_version_field(
    field: &'static str,
    found: &str,
    supported: &'static [&'static str],
) -> Result<(), CodegenError> {
    let family = version_family(found).ok_or_else(|| CodegenError::MalformedVersion {
        field,
        found: found.to_string(),
    })?;
    if supported.contains(&family.as_str()) {
        Ok(())
    } else {
        Err(CodegenError::UnsupportedVersion {
            field,
            found: found.to_string(),
            supported,
        })
    }
}

/// Extract the numeric `major.minor` family from a version string.
fn version_family(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if numeric(major) && numeric(minor) {
        Some(format!("{major}.{minor}"))
    } else {
        None
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ContractInfo {
    pub compiler_version: String,
    pub language_version: String,
    pub runtime_version: String,
    #[serde(default)]
    pub circuits: Vec<Circuit>,
    #[serde(default)]
    pub witnesses: Vec<Witness>,
    #[serde(default)]
    pub contracts: Vec<String>,
    #[serde(default)]
    pub ledger: Vec<LedgerField>,
    #[serde(default)]
    pub helpers: Vec<crate::ir::HelperDef>,
    #[serde(default)]
    pub structs: Vec<crate::ir::StructDef>,
}

/// One field in a contract's on-chain state, as emitted in the
/// `ledger` array of `contract-info.json`.
///
/// Field shape per storage kind (compactc 0.30.102+):
///
/// | Storage              | Type fields                   |
/// |----------------------|-------------------------------|
/// | `Cell`               | `type`                        |
/// | `Counter`            | (none)                        |
/// | `Set`                | `type` (element type)         |
/// | `List`               | `type` (element type)         |
/// | `Map`                | `key`, `value`                |
/// | `MerkleTree`         | `type`, `depth`               |
/// | `HistoricMerkleTree` | `type`, `depth`               |
#[derive(Debug, Deserialize)]
pub struct LedgerField {
    pub name: String,
    pub index: serde_json::Value, // usize or array for >15 fields
    pub storage: StorageKind,
    /// Whether this field was declared with `export ledger` in the Compact
    /// source. Non-exported fields are still on-chain but are hidden from
    /// the generated SDK surface.
    #[serde(default)]
    pub exported: bool,
    /// Element type for `Cell`, `Set`, `List`, `MerkleTree` and
    /// `HistoricMerkleTree` storage. Absent for `Counter` and `Map`.
    #[serde(rename = "type", default)]
    pub element_type: Option<TypeNode>,
    /// Key type for `Map` storage. Absent otherwise.
    #[serde(default)]
    pub key: Option<TypeNode>,
    /// Value type for `Map` storage. Absent otherwise.
    #[serde(default)]
    pub value: Option<TypeNode>,
    /// Depth of a `MerkleTree` / `HistoricMerkleTree`. Absent otherwise.
    pub depth: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum StorageKind {
    Cell,
    Counter,
    Map,
    Set,
    List,
    MerkleTree,
    HistoricMerkleTree,
}

impl std::fmt::Display for StorageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StorageKind::Cell => "Cell",
            StorageKind::Counter => "Counter",
            StorageKind::Map => "Map",
            StorageKind::Set => "Set",
            StorageKind::List => "List",
            StorageKind::MerkleTree => "MerkleTree",
            StorageKind::HistoricMerkleTree => "HistoricMerkleTree",
        })
    }
}

/// A ledger field index — either a single level or a multi-level B-tree path.
pub enum FieldIndex {
    /// Single index (contracts with ≤15 fields).
    Single(usize),
    /// Multi-level B-tree path (contracts with >15 fields).
    Path(Vec<usize>),
}

impl LedgerField {
    pub fn index_usize(&self) -> Option<usize> {
        self.index.as_u64().and_then(|n| usize::try_from(n).ok())
    }

    /// Parse the index as either a single usize or a path of usizes.
    pub fn field_index(&self) -> Option<FieldIndex> {
        if let Some(idx) = self.index_usize() {
            Some(FieldIndex::Single(idx))
        } else if let Some(arr) = self.index.as_array() {
            let path: Option<Vec<usize>> = arr
                .iter()
                .map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()))
                .collect();
            path.map(FieldIndex::Path)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub enum TypeNode {
    Boolean,
    Field,
    Uint {
        maxval: serde_json::Value,
    },
    Bytes {
        length: usize,
    },
    Vector {
        length: usize,
        inner: Box<TypeNode>,
    },
    Tuple {
        types: Vec<TypeNode>,
    },
    Struct {
        name: String,
        elements: Vec<StructElement>,
    },
    Enum {
        name: String,
        elements: Vec<String>,
    },
    Alias {
        name: String,
        inner: Box<TypeNode>,
    },
    Opaque {
        ts_type: Option<String>,
    },
    Contract {
        name: Option<String>,
    },
    /// Catch-all for unrecognized `type-name` values that future Compact
    /// compiler versions may introduce. Carries the offending name so
    /// validation (`validate::check_unknown_types`) can fail compilation with
    /// a precise message; it never reaches expansion.
    Unknown {
        type_name: String,
    },
}

/// `type-name` values [`TypeNode`] recognizes. Anything else deserializes to
/// [`TypeNode::Unknown`] and is rejected during validation; the
/// [`crate::error::CodegenError::UnknownTypeName`] message lists these names.
pub(crate) const KNOWN_TYPE_NAMES: &[&str] = &[
    "Boolean", "Field", "Uint", "Bytes", "Vector", "Tuple", "Struct", "Enum", "Alias", "Opaque",
    "Contract",
];

impl<'de> Deserialize<'de> for TypeNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        /// Mirror of [`TypeNode`] without the `Unknown` catch-all, so the
        /// derived internally-tagged deserializer can be reused for the known
        /// vocabulary while `TypeNode`'s manual impl captures unknown tags
        /// (a derived `#[serde(other)]` unit variant cannot carry the name).
        #[derive(Deserialize)]
        #[serde(tag = "type-name")]
        enum Known {
            Boolean,
            Field,
            Uint {
                maxval: serde_json::Value,
            },
            Bytes {
                length: usize,
            },
            Vector {
                length: usize,
                #[serde(rename = "type")]
                inner: Box<TypeNode>,
            },
            Tuple {
                types: Vec<TypeNode>,
            },
            Struct {
                name: String,
                elements: Vec<StructElement>,
            },
            Enum {
                name: String,
                elements: Vec<String>,
            },
            Alias {
                name: String,
                #[serde(rename = "type")]
                inner: Box<TypeNode>,
            },
            Opaque {
                #[serde(rename = "tsType")]
                ts_type: Option<String>,
            },
            Contract {
                name: Option<String>,
            },
        }

        impl From<Known> for TypeNode {
            fn from(known: Known) -> Self {
                match known {
                    Known::Boolean => TypeNode::Boolean,
                    Known::Field => TypeNode::Field,
                    Known::Uint { maxval } => TypeNode::Uint { maxval },
                    Known::Bytes { length } => TypeNode::Bytes { length },
                    Known::Vector { length, inner } => TypeNode::Vector { length, inner },
                    Known::Tuple { types } => TypeNode::Tuple { types },
                    Known::Struct { name, elements } => TypeNode::Struct { name, elements },
                    Known::Enum { name, elements } => TypeNode::Enum { name, elements },
                    Known::Alias { name, inner } => TypeNode::Alias { name, inner },
                    Known::Opaque { ts_type } => TypeNode::Opaque { ts_type },
                    Known::Contract { name } => TypeNode::Contract { name },
                }
            }
        }

        let value = serde_json::Value::deserialize(deserializer)?;
        match value.get("type-name").and_then(serde_json::Value::as_str) {
            Some(tag) if KNOWN_TYPE_NAMES.contains(&tag) => {
                // Re-parse via a string instead of `from_value`: with serde_json's
                // `arbitrary_precision` feature, `from_value` feeds >u64 numbers
                // (e.g. a u128 Uint maxval) to serde's internal buffer as
                // `visit_u128`, which it cannot represent. The string path uses
                // serde_json's number-marker encoding and round-trips exactly.
                serde_json::from_str::<Known>(&value.to_string())
                    .map(TypeNode::from)
                    .map_err(D::Error::custom)
            }
            Some(tag) => Ok(TypeNode::Unknown {
                type_name: tag.to_string(),
            }),
            None => Err(D::Error::custom(
                "type node object has a missing or non-string `type-name` field",
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StructElement {
    pub name: String,
    #[serde(rename = "type")]
    pub type_node: TypeNode,
}

#[derive(Debug, Deserialize)]
pub struct Circuit {
    pub name: String,
    pub pure: bool,
    pub proof: bool,
    pub arguments: Vec<CircuitArgument>,
    #[serde(rename = "result-type")]
    pub result_type: TypeNode,
    /// Portable circuit execution IR (for impure circuits).
    /// Present when the compiler emits the `"ir"` field.
    #[serde(default)]
    pub ir: Option<crate::ir::CircuitIrBody>,
}

#[derive(Debug, Deserialize)]
pub struct CircuitArgument {
    pub name: String,
    #[serde(rename = "type")]
    pub type_node: TypeNode,
}

#[derive(Debug, Deserialize)]
pub struct Witness {
    pub name: String,
    pub arguments: Vec<CircuitArgument>,
    #[serde(rename = "result-type")]
    pub result_type: TypeNode,
}
