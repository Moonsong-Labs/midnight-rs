//! Portable circuit IR types.
//!
//! These types represent the `circuit-ir.json` format emitted by the Compact
//! compiler. The IR describes the execution logic for each impure circuit as
//! a tree of statements and expressions, with embedded VM Op sequences for
//! ledger queries.
//!
//! The IR is consumed by a Rust interpreter that executes circuits against
//! a contract state, building transcripts for transaction construction.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------------------

/// Root of the standalone `circuit-ir.json` file (legacy format).
#[derive(Debug, Deserialize, Serialize)]
pub struct CircuitIr {
    pub version: Version,
    pub circuits: HashMap<String, CircuitDef>,
    #[serde(default)]
    pub helpers: HashMap<String, HelperDef>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

/// Circuit IR body embedded in a circuit entry within `contract-info.json`.
#[derive(Debug, Deserialize, Serialize)]
pub struct CircuitIrBody {
    pub body: Stmt,
    /// The circuit's return expression, or `None` for void circuits.
    pub result: Option<Expr>,
}

/// An impure circuit definition (standalone format).
#[derive(Debug, Deserialize, Serialize)]
pub struct CircuitDef {
    pub name: String,
    pub body: Stmt,
    /// The circuit's return expression, or `None` for void circuits.
    pub result: Option<Expr>,
}

/// A pure helper function called during circuit execution.
#[derive(Debug, Deserialize, Serialize)]
pub struct HelperDef {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Stmt,
    pub result: Option<Expr>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Param {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: TypeRef,
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// A statement — executed for side effects (ledger mutations, assertions,
/// variable bindings). Does not produce a value.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op")]
pub enum Stmt {
    /// Sequence of statements.
    #[serde(rename = "seq")]
    Seq { stmts: Vec<Stmt> },

    /// Bind an expression result to a local name.
    #[serde(rename = "let")]
    Let { name: String, value: Expr },

    /// Evaluate an expression for its side effects.
    #[serde(rename = "expr-stmt")]
    ExprStmt { expr: Expr },

    /// Conditional execution (no else branch).
    #[serde(rename = "if")]
    If { cond: Expr, then: Box<Stmt> },

    /// Conditional execution with else branch.
    #[serde(rename = "if-else")]
    IfElse {
        cond: Expr,
        then: Box<Stmt>,
        #[serde(rename = "else")]
        else_: Box<Stmt>,
    },
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// An expression — produces a value.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op")]
pub enum Expr {
    // -- Literals and references --
    /// Variable reference.
    #[serde(rename = "var")]
    Var { name: String },

    /// Typed literal value (as a string to avoid precision loss for large integers).
    #[serde(rename = "lit")]
    Lit {
        #[serde(rename = "type")]
        ty: TypeRef,
        value: String,
    },

    // -- Boolean --
    #[serde(rename = "not")]
    Not { expr: Box<Expr> },

    #[serde(rename = "and")]
    And { left: Box<Expr>, right: Box<Expr> },

    #[serde(rename = "or")]
    Or { left: Box<Expr>, right: Box<Expr> },

    // -- Arithmetic --
    #[serde(rename = "add")]
    Add { left: Box<Expr>, right: Box<Expr> },

    #[serde(rename = "sub")]
    Sub { left: Box<Expr>, right: Box<Expr> },

    #[serde(rename = "mul")]
    Mul { left: Box<Expr>, right: Box<Expr> },

    // -- Comparison --
    #[serde(rename = "eq")]
    Eq { left: Box<Expr>, right: Box<Expr> },

    #[serde(rename = "neq")]
    Neq { left: Box<Expr>, right: Box<Expr> },

    #[serde(rename = "lt")]
    Lt { left: Box<Expr>, right: Box<Expr> },

    #[serde(rename = "le")]
    Le { left: Box<Expr>, right: Box<Expr> },

    #[serde(rename = "gt")]
    Gt { left: Box<Expr>, right: Box<Expr> },

    #[serde(rename = "ge")]
    Ge { left: Box<Expr>, right: Box<Expr> },

    // -- Data access --
    /// Struct field access by name.
    #[serde(rename = "field")]
    Field { expr: Box<Expr>, name: String },

    /// Tuple element access by index.
    #[serde(rename = "index")]
    Index { expr: Box<Expr>, index: usize },

    // -- Control flow --
    /// Ternary conditional expression.
    #[serde(rename = "if-expr")]
    IfExpr {
        cond: Box<Expr>,
        then: Box<Expr>,
        #[serde(rename = "else")]
        else_: Box<Expr>,
    },

    // -- Side effects --
    /// Assertion with error message. Evaluates to unit.
    #[serde(rename = "assert")]
    Assert { expr: Box<Expr>, message: String },

    // -- Ledger interaction --
    /// Execute a sequence of VM Ops against the contract state.
    /// This is the core operation — it maps to `queryLedgerState` in the JS SDK.
    #[serde(rename = "ledger-query")]
    LedgerQuery {
        ops: Vec<LedgerOp>,
        #[serde(rename = "result-type")]
        result_type: TypeRef,
    },

    // -- Calls --
    /// Call a witness function (private state callback).
    #[serde(rename = "call-witness")]
    CallWitness {
        name: String,
        args: Vec<Expr>,
        #[serde(rename = "result-type")]
        result_type: TypeRef,
    },

    /// Call a pure helper function (local computation, no state access).
    #[serde(rename = "call-pure")]
    CallPure {
        name: String,
        args: Vec<Expr>,
        #[serde(rename = "result-type")]
        result_type: TypeRef,
    },

    // -- Type conversions --
    /// Let expression — bindings + body, evaluates to the body's value.
    /// This is the expression-level equivalent of let*, emitted when let*
    /// appears inside an expression context.
    #[serde(rename = "let-expr")]
    LetExpr {
        bindings: Vec<Stmt>,
        body: Box<Expr>,
    },

    /// Struct constructor.
    #[serde(rename = "new")]
    New {
        #[serde(rename = "type")]
        ty: TypeRef,
    },

    /// Type cast / conversion.
    #[serde(rename = "cast")]
    Cast {
        expr: Box<Expr>,
        from: TypeRef,
        to: TypeRef,
    },

    /// Default value for a type.
    #[serde(rename = "default")]
    Default {
        #[serde(rename = "type")]
        ty: TypeRef,
    },
}

// ---------------------------------------------------------------------------
// Ledger operations (VM Ops)
// ---------------------------------------------------------------------------

/// A single VM operation inside a `ledger-query`.
/// These map to `onchain-vm::Op` variants.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op")]
pub enum LedgerOp {
    /// Duplicate the top of stack.
    #[serde(rename = "dup")]
    Dup,

    /// Navigate into a `StateValue` by path.
    #[serde(rename = "idx")]
    Idx {
        cached: bool,
        #[serde(rename = "push-path")]
        push_path: bool,
        path: Vec<PathEntry>,
    },

    /// Add a value to a counter. The immediate can be a literal integer
    /// or an expression (e.g., a var reference resolved at runtime).
    #[serde(rename = "addi")]
    Addi { immediate: serde_json::Value },

    /// Insert/write back a value at the path on the stack.
    #[serde(rename = "ins")]
    Ins { cached: bool, n: u8 },

    /// Push a literal `StateValue` (encoded).
    #[serde(rename = "push")]
    Push {
        storage: bool,
        value: serde_json::Value,
    },

    /// Push a cell wrapping an expression value.
    #[serde(rename = "push-cell")]
    PushCell { value: Box<Expr> },

    /// Pop and assert equality (verifier check).
    #[serde(rename = "popeq")]
    Popeq,

    /// Check membership in a map/set.
    #[serde(rename = "member")]
    Member,

    /// Remove from a map/set.
    #[serde(rename = "rem")]
    Rem { cached: bool, n: u8 },

    /// Get merkle tree root.
    #[serde(rename = "root")]
    Root,

    /// Equality check.
    #[serde(rename = "eq")]
    Eq,

    /// Noop (padding).
    #[serde(rename = "noop")]
    Noop { n: u32 },

    /// Checkpoint boundary (guaranteed/fallible split).
    #[serde(rename = "ckpt")]
    Ckpt,
}

/// A path entry for `idx` operations.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "tag")]
pub enum PathEntry {
    /// A literal value (e.g., field index).
    #[serde(rename = "value")]
    Value {
        value: String,
        #[serde(rename = "type")]
        ty: TypeRef,
    },

    /// A variable reference (dynamic key).
    #[serde(rename = "var")]
    Var { name: String },

    /// A stack reference.
    #[serde(rename = "stack")]
    Stack,
}

// ---------------------------------------------------------------------------
// Type references
// ---------------------------------------------------------------------------

/// A type reference — uses the same vocabulary as `contract-info.json`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum TypeRef {
    Boolean,
    Field,
    Uint {
        maxval: String,
    },
    Bytes {
        length: usize,
    },
    #[serde(rename = "Opaque")]
    Opaque {
        name: String,
    },
    Void,
    Struct {
        name: String,
    },
    Enum {
        name: String,
    },
    Tuple {
        types: Vec<TypeRef>,
    },
    Vector {
        length: usize,
        element: Box<TypeRef>,
    },
    Maybe {
        inner: Box<TypeRef>,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_counter_increment_ir() {
        let json = r#"{
            "version": { "major": 1, "minor": 0 },
            "circuits": {
                "increment": {
                    "name": "increment",
                    "body": {
                        "op": "seq",
                        "stmts": [
                            {
                                "op": "let",
                                "name": "tmp_0",
                                "value": {
                                    "op": "lit",
                                    "type": { "type": "Uint", "maxval": "255" },
                                    "value": "1"
                                }
                            },
                            {
                                "op": "expr-stmt",
                                "expr": {
                                    "op": "ledger-query",
                                    "ops": [
                                        {
                                            "op": "idx",
                                            "cached": false,
                                            "push-path": true,
                                            "path": [
                                                { "tag": "value", "value": "0", "type": { "type": "Uint", "maxval": "255" } }
                                            ]
                                        },
                                        { "op": "addi", "immediate": { "op": "var", "name": "tmp" } },
                                        { "op": "ins", "cached": true, "n": 1 }
                                    ],
                                    "result-type": { "type": "Void" }
                                }
                            }
                        ]
                    },
                    "result": null
                }
            },
            "helpers": {}
        }"#;

        let ir: CircuitIr = serde_json::from_str(json).expect("parse circuit IR");
        assert_eq!(ir.version.major, 1);
        assert_eq!(ir.circuits.len(), 1);

        let increment = &ir.circuits["increment"];
        assert_eq!(increment.name, "increment");
        assert!(increment.result.is_none());

        // Body is a seq of 2 statements
        match &increment.body {
            Stmt::Seq { stmts } => {
                assert_eq!(stmts.len(), 2);
                // First: let tmp_0 = 1
                match &stmts[0] {
                    Stmt::Let { name, value } => {
                        assert_eq!(name, "tmp_0");
                        match value {
                            Expr::Lit { value, .. } => assert_eq!(value, "1"),
                            _ => panic!("expected Lit"),
                        }
                    }
                    _ => panic!("expected Let"),
                }
                // Second: ledger-query with 3 ops
                match &stmts[1] {
                    Stmt::ExprStmt { expr } => match expr {
                        Expr::LedgerQuery { ops, .. } => {
                            assert_eq!(ops.len(), 3);
                            assert!(matches!(&ops[0], LedgerOp::Idx { .. }));
                            assert!(matches!(&ops[1], LedgerOp::Addi { .. }));
                            assert!(matches!(&ops[2], LedgerOp::Ins { cached: true, n: 1 }));
                        }
                        _ => panic!("expected LedgerQuery"),
                    },
                    _ => panic!("expected ExprStmt"),
                }
            }
            _ => panic!("expected Seq"),
        }
    }

    #[test]
    fn parse_advance_circuit_with_witness_calls() {
        let json = r#"{
            "version": { "major": 1, "minor": 0 },
            "circuits": {
                "advance": {
                    "name": "advance",
                    "body": {
                        "op": "seq",
                        "stmts": [
                            {
                                "op": "let",
                                "name": "sk_0",
                                "value": {
                                    "op": "call-witness",
                                    "name": "private$secret_key",
                                    "args": [],
                                    "result-type": { "type": "Field" }
                                }
                            },
                            {
                                "op": "let",
                                "name": "apk_0",
                                "value": {
                                    "op": "call-pure",
                                    "name": "public_key",
                                    "args": [{ "op": "var", "name": "sk_0" }],
                                    "result-type": { "type": "Bytes", "length": 32 }
                                }
                            },
                            {
                                "op": "expr-stmt",
                                "expr": {
                                    "op": "assert",
                                    "expr": {
                                        "op": "eq",
                                        "left": { "op": "var", "name": "apk_0" },
                                        "right": {
                                            "op": "ledger-query",
                                            "ops": [
                                                { "op": "dup" },
                                                { "op": "idx", "cached": false, "push-path": false,
                                                  "path": [{ "tag": "value", "value": "0", "type": { "type": "Uint", "maxval": "255" } }] },
                                                { "op": "popeq" }
                                            ],
                                            "result-type": { "type": "Bytes", "length": 32 }
                                        }
                                    },
                                    "message": "Attempted to advance state without authorization"
                                }
                            }
                        ]
                    },
                    "result": null
                }
            },
            "helpers": {
                "public_key": {
                    "name": "public_key",
                    "params": [{ "name": "sk", "type": { "type": "Field" } }],
                    "body": { "op": "seq", "stmts": [] },
                    "result": {
                        "op": "call-pure",
                        "name": "__builtin_ec_mul_generator",
                        "args": [{ "op": "var", "name": "sk" }],
                        "result-type": { "type": "Bytes", "length": 32 }
                    }
                }
            }
        }"#;

        let ir: CircuitIr = serde_json::from_str(json).expect("parse advance IR");
        assert_eq!(ir.circuits.len(), 1);
        assert_eq!(ir.helpers.len(), 1);

        let advance = &ir.circuits["advance"];
        assert_eq!(advance.name, "advance");

        // Check witness call
        match &advance.body {
            Stmt::Seq { stmts } => {
                assert!(stmts.len() >= 3);
                match &stmts[0] {
                    Stmt::Let { value, .. } => {
                        assert!(matches!(value, Expr::CallWitness { .. }));
                    }
                    _ => panic!("expected Let with witness call"),
                }
            }
            _ => panic!("expected Seq"),
        }

        // Check helper
        let pk_helper = &ir.helpers["public_key"];
        assert_eq!(pk_helper.params.len(), 1);
        assert_eq!(pk_helper.params[0].name, "sk");
    }

    #[test]
    fn parse_empty_helpers() {
        let json = r#"{
            "version": { "major": 1, "minor": 0 },
            "circuits": {},
            "helpers": {}
        }"#;
        let ir: CircuitIr = serde_json::from_str(json).expect("parse empty IR");
        assert!(ir.circuits.is_empty());
        assert!(ir.helpers.is_empty());
    }

    #[test]
    fn parse_embedded_circuit_ir() {
        // Test parsing the "ir" field as embedded in contract-info.json
        let json = r#"{
            "body": {
                "op": "seq",
                "stmts": [
                    {
                        "op": "expr-stmt",
                        "expr": {
                            "op": "ledger-query",
                            "ops": [
                                { "op": "idx", "cached": false, "push-path": true,
                                  "path": [{ "tag": "value", "value": "0", "type": { "type": "Uint", "maxval": "255" } }] },
                                { "op": "addi", "immediate": { "op": "var", "name": "tmp" } },
                                { "op": "ins", "cached": true, "n": 1 }
                            ],
                            "result-type": { "type": "Void" }
                        }
                    }
                ]
            },
            "result": null
        }"#;

        let ir_body: CircuitIrBody = serde_json::from_str(json).expect("parse embedded IR");
        assert!(ir_body.result.is_none());
        match &ir_body.body {
            Stmt::Seq { stmts } => {
                assert_eq!(stmts.len(), 1);
                assert!(matches!(
                    &stmts[0],
                    Stmt::ExprStmt {
                        expr: Expr::LedgerQuery { .. }
                    }
                ));
            }
            _ => panic!("expected Seq"),
        }
    }
}
