use crate::error::CodegenError;

/// `compiler-version` `major.minor` families this generator is known to work
/// with. Checked by [`check_versions`] before any code is generated; a
/// an artifact outside this range fails compilation.
///
/// The range is derived from the committed fixtures, all emitted by the
/// pinned compactc (0.33.122) through the normalized-ir hook.
///
/// When the pinned compactc bumps its version:
/// 1. regenerate the contracts and fixtures (`make build-compactc
///    compile-contracts regen-test-fixtures`),
/// 2. add the new `major.minor` family here (and the matching language family
///    to [`SUPPORTED_LANGUAGE_VERSION_FAMILIES`]),
/// 3. re-bless the trybuild expectation that embeds the supported list:
///    `TRYBUILD=overwrite cargo test -p compact-bindgen-macro` rewrites
///    `tests/ui/fail/version-mismatch.stderr`; eyeball the diff,
/// 4. run the full test suite; drop an old family only once no fixture or
///    devnet contract uses it anymore.
pub const SUPPORTED_COMPILER_VERSION_FAMILIES: &[&str] = &["0.33"];

/// `language-version` `major.minor` families this generator is known to work
/// with. See [`SUPPORTED_COMPILER_VERSION_FAMILIES`] for how to widen.
pub const SUPPORTED_LANGUAGE_VERSION_FAMILIES: &[&str] = &["0.25"];

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

#[derive(Debug)]
pub struct ContractInfo {
    pub compiler_version: String,
    pub language_version: String,
    pub runtime_version: String,
    pub circuits: Vec<Circuit>,
    pub witnesses: Vec<Witness>,
    pub contracts: Vec<String>,
    pub ledger: Vec<LedgerField>,
    pub helpers: Vec<crate::ir::HelperDef>,
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
#[derive(Debug)]
pub struct LedgerField {
    pub name: String,
    pub index: FieldIndex,
    pub storage: StorageKind,
    /// Whether this field was declared with `export ledger` in the Compact
    /// source. Non-exported fields are still on-chain but are hidden from
    /// the generated SDK surface.
    pub exported: bool,
    /// Element type for `Cell`, `Set`, `List`, `MerkleTree` and
    /// `HistoricMerkleTree` storage. Absent for `Counter` and `Map`.
    pub element_type: Option<crate::ir::TypeRef>,
    /// Key type for `Map` storage. Absent otherwise.
    pub key: Option<crate::ir::TypeRef>,
    /// Value type for `Map` storage. Absent otherwise.
    pub value: Option<crate::ir::TypeRef>,
    /// Depth of a `MerkleTree` / `HistoricMerkleTree`. Absent otherwise.
    pub depth: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// A ledger field index — either a single level or a multi-level B-tree path
/// (contracts with more than 15 fields batch into a B-tree).
#[derive(Debug, Clone)]
pub enum FieldIndex {
    Single(usize),
    Path(Vec<usize>),
}

impl LedgerField {
    pub fn index_usize(&self) -> Option<usize> {
        match &self.index {
            FieldIndex::Single(i) => Some(*i),
            FieldIndex::Path(_) => None,
        }
    }

    pub fn field_index(&self) -> Option<FieldIndex> {
        Some(self.index.clone())
    }
}

#[derive(Debug)]
pub struct Circuit {
    pub name: String,
    pub pure: bool,
    pub proof: bool,
    pub arguments: Vec<CircuitArgument>,
    pub result_type: crate::ir::TypeRef,
    /// Portable circuit execution IR (for impure circuits).
    /// Present when the compiler emits the `"ir"` field.
    pub ir: Option<crate::ir::CircuitIrBody>,
}

#[derive(Debug, Clone)]
pub struct CircuitArgument {
    pub name: String,
    pub ty: crate::ir::TypeRef,
}

#[derive(Debug)]
pub struct Witness {
    pub name: String,
    pub arguments: Vec<CircuitArgument>,
    pub result_type: crate::ir::TypeRef,
}
