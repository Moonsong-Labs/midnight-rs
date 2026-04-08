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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LedgerField {
    pub name: String,
    pub index: serde_json::Value, // usize or array for >15 fields
    pub storage: String,
    /// Whether this field was declared with `export ledger` in the Compact source.
    /// Non-exported fields are still on-chain but were historically hidden from the SDK.
    #[serde(default)]
    pub exported: bool,
    // Flattened type fields — varies by storage kind. Older compiler
    // versions emit `key-type`/`value-type`/`element-type`; newer ones
    // (0.30.102+) emit the unsuffixed `key`/`value`/`element`. Accept
    // both via serde aliases.
    #[serde(rename = "type")]
    pub cell_type: Option<TypeNode>,
    #[serde(alias = "key")]
    pub key_type: Option<TypeNode>,
    #[serde(alias = "value")]
    pub value_type: Option<TypeNode>,
    #[serde(alias = "element")]
    pub element_type: Option<TypeNode>,
    pub depth: Option<serde_json::Value>,
}

/// A ledger field index — either a single level or a multi-level B-tree path.
pub enum FieldIndex {
    /// Single index (contracts with ≤15 fields).
    Single(usize),
    /// Multi-level B-tree path (contracts with >15 fields).
    Path(Vec<usize>),
}

impl LedgerField {
    /// Storage kind normalized to lowercase kebab-case.
    ///
    /// Accepts both the old compiler format ("cell", "merkle-tree") and the
    /// new PascalCase format ("Cell", "MerkleTree").
    pub fn storage_kind(&self) -> String {
        match self.storage.as_str() {
            "Cell" | "cell" => "cell".to_string(),
            "Counter" | "counter" => "counter".to_string(),
            "Map" | "map" => "map".to_string(),
            "Set" | "set" => "set".to_string(),
            "List" | "list" | "Array" | "array" => "list".to_string(),
            "MerkleTree" | "merkle-tree" => "merkle-tree".to_string(),
            "HistoricMerkleTree" | "historic-merkle-tree" => "historic-merkle-tree".to_string(),
            other => other.to_lowercase(),
        }
    }

    /// Effective element type for sets and lists.
    ///
    /// The old compiler uses `"element-type"`, the new compiler uses `"type"`.
    /// This method checks both, preferring `element_type` over `cell_type`.
    pub fn effective_element_type(&self) -> Option<&TypeNode> {
        self.element_type.as_ref().or(self.cell_type.as_ref())
    }

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
