//! Portable circuit IR types.
//!
//! These types are the interpreter's internal circuit IR, produced from a
//! `normalized-ir.sexp` artifact by [`crate::normalized`]. The IR describes
//! the execution logic for each circuit as a tree of statements and
//! expressions, with embedded VM Op sequences for ledger queries.
//!
//! The IR is consumed by a Rust interpreter that executes circuits against
//! a contract state, building transcripts for transaction construction.

// ---------------------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------------------

/// Circuit IR body of one circuit entry.
#[derive(Debug, Clone)]
pub struct CircuitIrBody {
    /// The return value is the value of the body's final expression
    /// statement.
    pub body: Stmt,
}

/// A circuit an exported body calls. `body` follows [`CircuitIrBody`].
#[derive(Debug, Clone)]
pub struct HelperDef {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Stmt,
}

/// A struct definition shipped by the compiler so the IR consumer can compute
/// atom layouts for `Value::AlignedValue` field slicing.
#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<StructField>,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub ty: TypeRef,
}

/// An enum definition shipped by the compiler so the IR consumer can map
/// variant names back to their declaration index (the on-chain encoding
/// is a single `u8` whose value is the variant index).
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeRef,
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// A statement — executed for side effects (ledger mutations, assertions,
/// variable bindings). Does not produce a value.
#[derive(Debug, Clone)]
pub enum Stmt {
    /// Sequence of statements.
    Seq { stmts: Vec<Stmt> },

    /// Bind an expression result to a local name.
    Let { name: String, value: Expr },

    /// Evaluate an expression for its side effects.
    ExprStmt { expr: Expr },

    /// Conditional execution (no else branch).
    If { cond: Expr, then: Box<Stmt> },

    /// Conditional execution with else branch.
    IfElse {
        cond: Expr,
        then: Box<Stmt>,
        else_: Box<Stmt>,
    },
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// An expression — produces a value.
#[derive(Debug, Clone)]
pub enum Expr {
    // -- Literals and references --
    /// Variable reference.
    Var {
        name: String,
    },

    /// Typed literal value (as a string to avoid precision loss for large integers).
    Lit {
        ty: TypeRef,
        value: String,
    },

    // -- Boolean --
    Not {
        expr: Box<Expr>,
    },

    And {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    Or {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    // -- Arithmetic --
    Add {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    Sub {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    Mul {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    // -- Comparison --
    Eq {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    Neq {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    Lt {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    Le {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    Gt {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    Ge {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    // -- Data access --
    /// Struct field access by name.
    Field {
        expr: Box<Expr>,
        name: String,
    },

    /// Tuple element access by index.
    Index {
        expr: Box<Expr>,
        index: usize,
    },

    /// Enum member by name. The on-chain value is the member's index in the
    /// variant list the type carries.
    EnumMember {
        ty: TypeRef,
        member: String,
    },

    /// Byte of a `Bytes` value at a runtime-evaluated index.
    BytesIndex {
        expr: Box<Expr>,
        index: Box<Expr>,
    },

    /// Constant-index slice of a tuple or vector. Carries the operand type,
    /// because slicing a heterogeneous tuple yields element types the length
    /// alone does not determine.
    TupleSlice {
        expr: Box<Expr>,
        index: usize,
        length: usize,
        ty: TypeRef,
    },

    /// Slice of a vector at a runtime-evaluated index.
    VectorSlice {
        expr: Box<Expr>,
        index: Box<Expr>,
        length: usize,
        ty: TypeRef,
    },

    /// Slice of a `Bytes` value; the result is always `Bytes<length>`.
    BytesSlice {
        expr: Box<Expr>,
        index: Box<Expr>,
        length: usize,
    },

    /// Bounded loop building a tuple of `length` elements.
    Map {
        length: usize,
        fun: Fun,
        args: Vec<Expr>,
    },

    /// Bounded loop threading an accumulator through `length` elements.
    Fold {
        length: usize,
        fun: Fun,
        init: Box<Expr>,
        args: Vec<Expr>,
    },

    /// Vector element access by an arbitrary runtime-evaluated index.
    /// Distinct from `Index` (which takes a const usize) so the compiler
    /// can lower `v[i]` where `i` is a variable bound by an unrolled
    /// for-loop without first constant-folding the substitution.
    VectorIndex {
        expr: Box<Expr>,
        index: Box<Expr>,
    },

    // -- Control flow --
    /// Ternary conditional expression.
    IfExpr {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },

    // -- Side effects --
    /// Assertion with error message. Evaluates to unit.
    Assert {
        expr: Box<Expr>,
        message: String,
    },

    // -- Ledger interaction --
    /// Execute a sequence of VM Ops against the contract state.
    /// This is the core operation — it maps to `queryLedgerState` in the JS SDK.
    LedgerQuery {
        ops: Vec<LedgerOp>,
        result_type: TypeRef,
    },

    // -- Calls --
    /// Call a witness function (private state callback).
    CallWitness {
        name: String,
        args: Vec<Expr>,
        result_type: TypeRef,
    },

    /// Call a pure helper function (local computation, no state access).
    CallPure {
        name: String,
        args: Vec<Expr>,
        result_type: TypeRef,
    },

    // -- Type conversions --
    /// Let expression — bindings + body, evaluates to the body's value.
    /// This is the expression-level equivalent of let*, emitted when let*
    /// appears inside an expression context.
    LetExpr {
        bindings: Vec<Stmt>,
        body: Box<Expr>,
    },

    /// Struct constructor: `StructName { field0: e0, field1: e1, ... }`.
    /// The interpreter uses `ty` to look up the struct layout and encode
    /// each element with the correct per-field alignment, producing a
    /// flat `Value::AlignedValue` whose `binary_repr` matches what the
    /// on-chain `persistent_hash` circuit produces for the same input.
    New {
        ty: TypeRef,
        elements: Vec<Expr>,
    },

    /// Type cast / conversion.
    Cast {
        expr: Box<Expr>,
        from: TypeRef,
        to: TypeRef,
    },

    /// Default value for a type.
    Default {
        ty: TypeRef,
    },

    /// Tuple/vector constructor with N pre-evaluated element expressions.
    /// Each element is either a regular `Expr` (single value) or a `Spread`
    /// (whose inner expression contributes `length` elements at that position).
    /// Evaluates to a `Value::Tuple` at runtime.
    Tuple {
        elements: Vec<Expr>,
    },

    /// Spread element inside a `tuple`/`vector` constructor. Only valid as a
    /// child of `Tuple::elements`; the interpreter expands the inner
    /// expression into `length` consecutive elements.
    Spread {
        length: u64,
        expr: Box<Expr>,
    },

    /// Reinterpret a `Bytes` value as a `Field` element.
    BytesToField {
        length: u64,
        expr: Box<Expr>,
    },

    /// Reinterpret a `Field` element as a `Bytes` value.
    FieldToBytes {
        length: u64,
        expr: Box<Expr>,
    },

    /// View a `Bytes` value as `Vector<length, Uint<255>>`.
    BytesToVector {
        length: u64,
        expr: Box<Expr>,
    },

    /// View a `Vector<length, Uint<255>>` as `Bytes`.
    VectorToBytes {
        length: u64,
        expr: Box<Expr>,
    },

    /// Cross-contract circuit invocation: call `circuit` on the contract
    /// value `contract` (whose static type is `contract_type`) with `args`.
    ContractCall {
        circuit: String,
        contract: Box<Expr>,
        contract_type: TypeRef,
        args: Vec<Expr>,
    },
}

// ---------------------------------------------------------------------------
// Ledger operations (VM Ops)
// ---------------------------------------------------------------------------

/// A single VM operation inside a `ledger-query`.
/// These map to `onchain-vm::Op` variants.
#[derive(Debug, Clone)]
pub enum LedgerOp {
    /// Duplicate the stack element `n` below the top (`n = 0` duplicates the
    /// top).
    Dup { n: u8 },

    /// Navigate into a `StateValue` by path.
    Idx {
        cached: bool,
        push_path: bool,
        path: Vec<PathEntry>,
    },

    /// Add a value to a counter. The immediate can be a literal integer
    /// or an expression (e.g., a var reference resolved at runtime).
    Addi { immediate: VmOperand },

    /// Insert/write back a value at the path on the stack.
    Ins { cached: bool, n: u8 },

    /// Push a value onto the query stack.
    Push { storage: bool, value: VmOperand },

    /// Pop and assert equality (verifier check). The on-chain VM has two
    /// variants: `popeq` (cached=false, opcode 0x0c) and its cached form
    /// `popeqc` (cached=true, opcode 0x0d). The compiler emits this flag
    /// based on the source ledger op definition (e.g. Map.member uses
    /// the cached form, raw cell reads use the uncached form).
    Popeq { cached: bool },

    /// Check membership in a map/set.
    Member,

    /// Remove from a map/set.
    Rem { cached: bool },

    /// Get merkle tree root.
    Root,

    /// Equality check.
    Eq,

    /// Noop (padding).
    Noop { n: u32 },

    /// Checkpoint boundary (guaranteed/fallible split).
    Ckpt,

    /// Swap the top of stack with the element `n` deep.
    Swap { n: u8 },

    /// Boolean negation of the top-of-stack value.
    Neg,

    /// Conditional skip: if the top of stack is false, skip the next `skip`
    /// instructions.
    Branch { skip: u32 },

    /// Pop the top two stack values and push their sum.
    Add,
}

/// A path entry for `idx` operations.
#[derive(Debug, Clone)]
pub enum PathEntry {
    /// A literal value (e.g., field index).
    Value { value: String, ty: TypeRef },

    /// A variable reference (dynamic key).
    Var { name: String },

    /// An index computed at run time, for a nested ADT access whose key is
    /// not a plain variable.
    Expr { expr: Box<Expr> },

    /// A stack reference.
    Stack,
}

// ---------------------------------------------------------------------------
// Type references
// ---------------------------------------------------------------------------

/// The value of a `push` / `addi` operand.
///
/// The wire form has no single discriminator; each variant keeps its own
/// shape: a bare number, `null`, a path entry tagged `tag`, a state value
/// tagged `state`, a computed value tagged `vm`, or an expression tagged
/// `op`.
#[derive(Debug, Clone)]
pub enum VmOperand {
    Int(u128),
    Bool(bool),
    Str(String),
    /// A pushed `StateValue::Null` (e.g. inserting into a Set, where the
    /// value slot is just a marker), distinct from a unit cell.
    Null,
    /// A literal path key or a stack marker.
    Key(PathEntry),
    /// A structured state value.
    State(StateInit),
    /// A value the VM computes while building the instruction.
    Vm(VmFn),
    /// An expression evaluated at query time.
    Expr(Box<Expr>),
}

/// A structured `StateValue` literal inside a `push`.
#[derive(Debug, Clone)]
pub enum StateInit {
    Array {
        values: Vec<VmOperand>,
    },
    Map {
        entries: Vec<StateEntry>,
    },
    MerkleTree {
        depth: u64,
        entries: Vec<StateEntry>,
    },
}

#[derive(Debug, Clone)]
pub struct StateEntry {
    pub key: VmOperand,
    pub value: VmOperand,
}

/// A value computed by the VM template machinery.
#[derive(Debug, Clone)]
pub enum VmFn {
    Null {
        value: Box<VmOperand>,
    },
    MaxSizeof {
        value: Box<VmOperand>,
    },
    LeafHash {
        value: Box<VmOperand>,
    },
    CoinCommit {
        coin: Box<VmOperand>,
        recipient: Box<VmOperand>,
    },
    AlignedConcat {
        values: Vec<VmOperand>,
    },
}

/// The callee of a `map` or `fold`: either a circuit named in `helpers`, or
/// an inline function with its own parameters.
#[derive(Debug, Clone)]
pub enum Fun {
    Named { call: String },
    Inline { params: Vec<Param>, body: Box<Expr> },
}

/// The type vocabulary, in the internal `type-name`-tagged encoding.
///
/// One model serves every consumer: circuit signatures and ledger fields
/// (the code generator), IR bodies (the interpreter), and the wire embedded
/// in generated bindings. The wire form is a node tagged on `type-name`,
/// with a struct's fields and an enum's variants carried inline.
#[derive(Debug, Clone)]
pub enum TypeRef {
    Boolean,
    Field,
    Uint {
        maxval: String,
    },
    Bytes {
        length: usize,
    },
    Opaque {
        name: String,
    },
    Void,
    Struct {
        name: String,
        /// Field layout, carried inline.
        elements: Vec<StructField>,
    },
    Enum {
        name: String,
        /// Variant names in declaration order; the on-chain value is the
        /// index into this list.
        variants: Vec<String>,
    },
    Tuple {
        types: Vec<TypeRef>,
    },
    Vector {
        length: usize,
        element: Box<TypeRef>,
    },
    /// A nominal alias. The interpreter and the on-chain encoding treat it
    /// as its inner type; the code generator keeps the name for the SDK
    /// surface.
    Alias {
        name: String,
        inner: Box<TypeRef>,
    },
    /// A contract handle (a `ContractAddress` on the wire).
    Contract {
        name: Option<String>,
    },
}

impl TypeRef {
    /// The type with any alias wrappers removed.
    pub fn resolved(&self) -> &TypeRef {
        let mut t = self;
        while let TypeRef::Alias { inner, .. } = t {
            t = inner;
        }
        t
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_alias_resolves_to_its_inner_type() {
        let aliased = TypeRef::Alias {
            name: "JobId".to_string(),
            inner: Box::new(TypeRef::Alias {
                name: "Inner".to_string(),
                inner: Box::new(TypeRef::Uint {
                    maxval: "255".to_string(),
                }),
            }),
        };
        assert!(matches!(aliased.resolved(), TypeRef::Uint { maxval } if maxval == "255"));

        // A type that is not an alias resolves to itself.
        assert!(matches!(TypeRef::Boolean.resolved(), TypeRef::Boolean));
    }
}
