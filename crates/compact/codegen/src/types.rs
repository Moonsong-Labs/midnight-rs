use crate::error::CodegenError;

/// The `major.minor` of the compactc at `COMPACT_REV` in the root `Makefile`,
/// the only compiler whose `analyzed-ir.sexp` this generator reads. The
/// generator and the interpreter follow the IR and the runtime of that one
/// compiler. An artifact from another compiler version needs the midnight-rs
/// release that supports it.
///
/// The header's `language-version` and `runtime-version` are constants of the
/// compiler that wrote it, so the compiler version alone decides.
///
/// When `COMPACT_REV` moves to a new `major.minor`, change this value too.
/// Until then, `contract!` refuses the regenerated devnet contracts.
pub const SUPPORTED_COMPILER_FAMILY: &str = "0.35";

/// Check the `compiler-version` of the artifact against
/// [`SUPPORTED_COMPILER_FAMILY`]. Called before expansion; a mismatch aborts
/// code generation with a compile error that names the version found.
pub fn check_compiler_version(info: &ContractInfo) -> Result<(), CodegenError> {
    let found = &info.compiler_version;
    let family = version_family(found).ok_or_else(|| CodegenError::MalformedCompilerVersion {
        found: found.clone(),
    })?;
    if family == SUPPORTED_COMPILER_FAMILY {
        Ok(())
    } else {
        Err(CodegenError::UnsupportedCompilerVersion {
            found: found.clone(),
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
