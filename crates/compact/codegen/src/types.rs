use crate::error::CodegenError;

/// `compiler-version` `major.minor` families this generator is known to work
/// with. Checked by [`check_versions`] before any code is generated; a
/// an artifact outside this range fails compilation.
///
/// The range is derived from the committed fixtures: those that the compactc
/// at `COMPACT_REV` in the root `Makefile` emits, and older ones that no
/// `Makefile` target regenerates.
///
/// When `COMPACT_REV` moves to a new compiler version:
/// 1. regenerate the contracts and fixtures (`make compile-contracts
///    regen-test-fixtures`),
/// 2. add the new `major.minor` family here (and the matching language family
///    to [`SUPPORTED_LANGUAGE_VERSION_FAMILIES`]),
/// 3. re-bless the trybuild expectation that embeds the supported list:
///    `TRYBUILD=overwrite cargo test -p compact-bindgen-macro` rewrites
///    `tests/ui/fail/version-mismatch.stderr`; eyeball the diff,
/// 4. run the full test suite; drop an old family only once no fixture or
///    devnet contract uses it anymore.
pub const SUPPORTED_COMPILER_VERSION_FAMILIES: &[&str] = &["0.33", "0.35"];

/// `language-version` `major.minor` families this generator is known to work
/// with. See [`SUPPORTED_COMPILER_VERSION_FAMILIES`] for how to widen.
pub const SUPPORTED_LANGUAGE_VERSION_FAMILIES: &[&str] = &["0.25", "0.27"];

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
    pub witnesses: Vec<crate::ir::Witness>,
    pub contracts: Vec<String>,
    pub ledger: Vec<LedgerField>,
    /// Whether the contract's constructor takes arguments or has a body.
    /// False for `(constructor () (tuple))`, the form the compiler emits for a
    /// contract that declares no constructor, and for a contract with no
    /// ledger.
    pub has_constructor: bool,
    pub helpers: Vec<crate::ir::Circuit>,
    /// Native declarations. A witness-class native also appends to the
    /// private transcript, so the interpreter needs them to route a call.
    pub natives: Vec<crate::ir::Native>,
}

/// One field in a contract's on-chain state, read from a ledger binding of
/// the analyzed IR (`analyzed-ir.sexp`).
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
    pub element_type: Option<crate::ir::Type>,
    /// Key type for `Map` storage. Absent otherwise.
    pub key: Option<crate::ir::Type>,
    /// Value type for `Map` storage. Absent otherwise.
    pub value: Option<crate::ir::Type>,
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
    /// The name the contract exports this circuit under. The definition's
    /// own identifier is the internal one the artifact uses.
    pub name: String,
    /// The circuit as the artifact defines it: arguments, result type, body,
    /// and the exported/pure/proof flags.
    pub def: crate::ir::Circuit,
}

impl Circuit {
    pub fn pure(&self) -> bool {
        self.def.pure
    }

    pub fn proof(&self) -> bool {
        self.def.proof
    }

    pub fn arguments(&self) -> &[crate::ir::Argument] {
        &self.def.arguments
    }

    pub fn result_type(&self) -> &crate::ir::Type {
        &self.def.result_type
    }
}
