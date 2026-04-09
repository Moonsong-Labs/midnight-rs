use serde::Deserialize;

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

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type-name")]
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
    /// Catch-all for unrecognized `type-name` values that future Compact
    /// compiler versions may introduce. Falls back to `Vec<u8>` with a warning.
    #[serde(other)]
    Unknown,
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
    #[serde(rename = "result-type", alias = "result type")]
    pub result_type: TypeNode,
}
