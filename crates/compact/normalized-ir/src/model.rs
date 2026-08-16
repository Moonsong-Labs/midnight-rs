//! The typed model of the normalized IR. Names and field order mirror the
//! compiler's own language (`compiler/langs.ss` is the grammar) and the
//! ledger DSL notation (`compiler/midnight-ledger.ss`); nothing is renamed.

use num_bigint::{BigInt, BigUint};

/// A compiler identifier, printed as the compiler prints it: `%name.uniq`
/// (temporaries and locals) or a plain symbol.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Ident(pub String);

impl Ident {
    /// The source-level name: `%round.1` -> `round`.
    pub fn name(&self) -> &str {
        let s = self.0.strip_prefix('%').unwrap_or(&self.0);
        match s.rfind('.') {
            Some(i) if s[i + 1..].bytes().all(|b| b.is_ascii_digit()) => &s[..i],
            _ => s,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedIr {
    pub compiler_version: String,
    pub language_version: String,
    pub runtime_version: String,
    /// Exported name -> internal identifier: exported circuits and exported
    /// ledger fields alike.
    pub exports: Vec<(String, Ident)>,
    pub contract_types: Vec<Type>,
    pub elements: Vec<ProgramElement>,
}

impl NormalizedIr {
    pub fn circuits(&self) -> impl Iterator<Item = &Circuit> {
        self.elements.iter().filter_map(|e| match e {
            ProgramElement::Circuit(c) => Some(c),
            _ => None,
        })
    }
    pub fn circuit(&self, exported_name: &str) -> Option<&Circuit> {
        let id = &self.exports.iter().find(|(n, _)| n == exported_name)?.1;
        self.circuits().find(|c| &c.name == id)
    }
    pub fn ledger(&self) -> Option<&LedgerDeclaration> {
        self.elements.iter().find_map(|e| match e {
            ProgramElement::PublicLedgerDeclaration(l) => Some(l),
            _ => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramElement {
    Circuit(Circuit),
    Native(Native),
    Witness(Witness),
    KernelDeclaration(LedgerBinding),
    PublicLedgerDeclaration(LedgerDeclaration),
    ExportTypedef {
        name: String,
        type_vars: Vec<String>,
        ty: Type,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Circuit {
    pub name: Ident,
    pub exported: bool,
    pub pure: bool,
    pub proof: bool,
    pub arguments: Vec<Argument>,
    pub result_type: Type,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Native {
    pub name: Ident,
    /// The TypeScript runtime entry point, provenance for what the
    /// backend calls; an interpreter maps the name to its own builtin.
    pub entry: String,
    /// `circuit` or `witness`: witness-class natives also append to the
    /// private transcript.
    pub class: String,
    pub arguments: Vec<Argument>,
    pub result_type: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Witness {
    pub name: Ident,
    pub arguments: Vec<Argument>,
    pub result_type: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Argument {
    pub name: Ident,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerDeclaration {
    pub fields: Vec<LedgerBinding>,
    pub constructor: Constructor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerBinding {
    pub name: Ident,
    /// The state path from the ledger root; more than one index when more
    /// than fifteen fields force a nested layout.
    pub path: Vec<u64>,
    pub exported: bool,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Constructor {
    pub arguments: Vec<Argument>,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldType {
    Native,
    Base(Curve),
    Scalar(Curve),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Curve {
    Jubjub,
    Secp256k1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Boolean,
    Field(FieldType),
    /// `maxval`, the inclusive upper bound; reaches 2^248-1.
    Unsigned(BigUint),
    Point(Curve),
    Bytes(u64),
    Opaque(String),
    Vector {
        len: u64,
        ty: Box<Type>,
    },
    Tuple(Vec<Type>),
    Struct {
        name: String,
        fields: Vec<(String, Type)>,
    },
    Enum {
        name: String,
        variants: Vec<String>,
    },
    Alias {
        nominal: bool,
        name: String,
        ty: Box<Type>,
    },
    Contract {
        name: String,
        circuits: Vec<ContractCircuit>,
    },
    /// A ledger ADT such as `Counter`, `__compact_Cell`, `Map`; the head is
    /// kept as printed and the set is open.
    Adt {
        name: String,
        args: Vec<AdtArg>,
    },
    TypeVar(String),
    Unknown,
}

impl Type {
    /// The unit type, which the language spells as the empty tuple.
    pub fn unit() -> Type {
        Type::Tuple(Vec::new())
    }

    /// The type with any alias wrappers removed. An alias is transparent to
    /// every value-level operation; only the source-level name differs.
    pub fn resolved(&self) -> &Type {
        let mut t = self;
        while let Type::Alias { ty, .. } = t {
            t = ty;
        }
        t
    }

    /// Whether this is the unit type (`(ttuple)`), the result type of a
    /// circuit that returns nothing.
    pub fn is_unit(&self) -> bool {
        matches!(self.resolved(), Type::Tuple(types) if types.is_empty())
    }

    /// Whether values of this type are computed in the native scalar field.
    /// The compiler distinguishes the Jubjub scalar field from the native
    /// one, but they share a modulus, so both compute identically.
    pub fn is_native_field(&self) -> bool {
        matches!(
            self.resolved(),
            Type::Field(FieldType::Native) | Type::Field(FieldType::Scalar(Curve::Jubjub))
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdtArg {
    Nat(u64),
    Type(Type),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractCircuit {
    pub name: String,
    pub pure: bool,
    pub argument_types: Vec<Type>,
    pub result_type: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Literal {
    Int(BigInt),
    Bool(bool),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TupleArg {
    Single(Expr),
    Spread { len: u64, expr: Expr },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapArg {
    pub expr: Expr,
    pub ty: Type,
    pub element_ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fun {
    Ref(Ident),
    Circuit {
        arguments: Vec<Argument>,
        result_type: Type,
        body: Box<Expr>,
    },
}

/// A symbolic element of a ledger field's access path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathElement {
    Index(u64),
    Computed { ty: Box<Type>, expr: Box<Expr> },
}

/// One expanded Impact VM instruction, `(op (arg value) ...)`. The
/// instruction set is open; a consumer must refuse an instruction it does
/// not implement rather than skip it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub op: String,
    pub args: Vec<(String, Operand)>,
}

impl Instruction {
    pub fn arg(&self, name: &str) -> Option<&Operand> {
        self.args.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// An instruction operand, in the ledger DSL's notation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operand {
    Int(BigInt),
    Bool(bool),
    Str(String),
    /// `(align value bytes)`: an aligned constant.
    Align {
        value: BigUint,
        bytes: u64,
    },
    /// `(stack)`: the value already on the VM stack.
    Stack,
    /// `(void)`.
    Void,
    /// `(value->int x)`.
    ValueToInt(Box<Operand>),
    /// `(null type-operand)`: the default instance of a type.
    Null(Box<Operand>),
    /// `(max-sizeof x)`.
    MaxSizeof(Box<Operand>),
    /// `(leaf-hash x)`.
    LeafHash(Box<Operand>),
    /// `(coin-commit coin recipient)`.
    CoinCommit(Box<Operand>, Box<Operand>),
    /// `(aligned-concat x ...)`.
    AlignedConcat(Vec<Operand>),
    StateValue(StateValue),
    /// An expression the consumer evaluates at run time.
    Expr(Box<Expr>),
    /// A plain list of operands, for example an `idx` path.
    List(Vec<Operand>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateValue {
    Null,
    Cell(Box<Operand>),
    Adt(Box<Operand>),
    Array(Vec<Operand>),
    Map(Vec<(Operand, Operand)>),
    MerkleTree {
        depth: u64,
        entries: Vec<(Operand, Operand)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Quote(Literal),
    VarRef(Ident),
    Default(Type),
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    EltRef {
        expr: Box<Expr>,
        elt: String,
        index: u64,
    },
    EnumRef {
        ty: Type,
        elt: String,
    },
    Tuple(Vec<TupleArg>),
    VectorLit(Vec<TupleArg>),
    TupleRef {
        expr: Box<Expr>,
        index: u64,
    },
    TupleSlice {
        ty: Type,
        expr: Box<Expr>,
        index: u64,
        len: u64,
    },
    VectorRef {
        ty: Type,
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    VectorSlice {
        ty: Type,
        expr: Box<Expr>,
        index: Box<Expr>,
        len: u64,
    },
    BytesRef {
        ty: Type,
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    BytesSlice {
        ty: Type,
        expr: Box<Expr>,
        index: Box<Expr>,
        len: u64,
    },
    Add {
        ty: Type,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Sub {
        ty: Type,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Mul {
        ty: Type,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Lt {
        bits: u64,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Le {
        bits: u64,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Gt {
        bits: u64,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Ge {
        bits: u64,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Eq {
        ty: Type,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Neq {
        ty: Type,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Map {
        len: u64,
        fun: Fun,
        args: Vec<MapArg>,
    },
    Fold {
        len: u64,
        fun: Fun,
        init: Box<Expr>,
        init_ty: Type,
        args: Vec<MapArg>,
    },
    Call {
        name: Ident,
        args: Vec<Expr>,
    },
    New {
        ty: Type,
        elements: Vec<Expr>,
    },
    Seq(Vec<Expr>),
    LetStar {
        bindings: Vec<(Argument, Expr)>,
        body: Box<Expr>,
    },
    Assert {
        expr: Box<Expr>,
        message: String,
    },
    FieldToBytes {
        len: u64,
        field_type: FieldType,
        expr: Box<Expr>,
    },
    CastFromBytes {
        ty: Type,
        len: u64,
        expr: Box<Expr>,
    },
    VectorToBytes {
        len: u64,
        expr: Box<Expr>,
    },
    BytesToVector {
        len: u64,
        expr: Box<Expr>,
    },
    CastFromEnum {
        ty: Type,
        from: Type,
        expr: Box<Expr>,
    },
    CastToEnum {
        ty: Type,
        from: Type,
        expr: Box<Expr>,
    },
    CastToField {
        field_type: FieldType,
        from: Type,
        expr: Box<Expr>,
    },
    CastFromField {
        maxval: BigUint,
        field_type: FieldType,
        expr: Box<Expr>,
    },
    SafeCast {
        ty: Type,
        from: Type,
        expr: Box<Expr>,
    },
    DowncastUnsigned {
        from_maxval: BigUint,
        to_maxval: BigUint,
        expr: Box<Expr>,
    },
    ContractCall {
        circuit: String,
        receiver: Box<Expr>,
        contract_type: Type,
        args: Vec<Expr>,
    },
    Emit {
        event_version: u64,
        event_tag: u64,
        len: u64,
        payload: Box<Expr>,
        instructions: Vec<Instruction>,
    },
    PublicLedger {
        field: Ident,
        path: Vec<PathElement>,
        op: String,
        result_type: Type,
        instructions: Vec<Instruction>,
        args: Vec<Expr>,
    },
    Return(Box<Expr>),
}

#[cfg(test)]
mod type_tests {
    use super::*;

    #[test]
    fn resolved_strips_every_alias_layer() {
        let aliased = Type::Alias {
            nominal: true,
            name: "JobId".to_string(),
            ty: Box::new(Type::Alias {
                nominal: false,
                name: "Inner".to_string(),
                ty: Box::new(Type::Bytes(32)),
            }),
        };
        assert!(matches!(aliased.resolved(), Type::Bytes(32)));
        assert!(matches!(Type::Boolean.resolved(), Type::Boolean));
    }

    #[test]
    fn unit_is_the_empty_tuple_through_aliases() {
        assert!(Type::unit().is_unit());
        assert!(
            Type::Alias {
                nominal: true,
                name: "Nothing".to_string(),
                ty: Box::new(Type::unit()),
            }
            .is_unit()
        );
        assert!(!Type::Tuple(vec![Type::Boolean]).is_unit());
    }

    #[test]
    fn the_jubjub_scalar_field_computes_as_the_native_field() {
        assert!(Type::Field(FieldType::Native).is_native_field());
        assert!(Type::Field(FieldType::Scalar(Curve::Jubjub)).is_native_field());
        assert!(!Type::Field(FieldType::Scalar(Curve::Secp256k1)).is_native_field());
    }
}
