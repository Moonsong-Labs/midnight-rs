//! sexp -> model. Fails closed: an unrecognized expression, type or operand
//! form is an error naming the form, never a guess. The instruction set is
//! the one open surface, mirroring the format: instructions parse
//! generically and the consumer refuses unknown ops at execution time.

use crate::error::Error;
use crate::model::*;
use crate::sexp::Sexp;
use num_bigint::BigUint;

type R<T> = Result<T, Error>;

fn err<T>(context: &str, s: &Sexp) -> R<T> {
    Err(Error::new(format!("{context}: unrecognized form {s}")))
}

fn sym(s: &Sexp, context: &str) -> R<String> {
    s.as_sym()
        .map(str::to_string)
        .ok_or_else(|| Error::new(format!("{context}: expected a symbol, got {s}")))
}

fn ident(s: &Sexp, context: &str) -> R<Ident> {
    Ok(Ident(sym(s, context)?))
}

fn string(s: &Sexp, context: &str) -> R<String> {
    match s {
        Sexp::Str(v) => Ok(v.clone()),
        _ => Err(Error::new(format!("{context}: expected a string, got {s}"))),
    }
}

fn boolean(s: &Sexp, context: &str) -> R<bool> {
    match s {
        Sexp::Bool(b) => Ok(*b),
        _ => Err(Error::new(format!(
            "{context}: expected a boolean, got {s}"
        ))),
    }
}

fn big_nat(s: &Sexp, context: &str) -> R<BigUint> {
    match s {
        Sexp::Int(i) => i
            .to_biguint()
            .ok_or_else(|| Error::new(format!("{context}: expected a natural, got {s}"))),
        _ => Err(Error::new(format!(
            "{context}: expected an integer, got {s}"
        ))),
    }
}

fn nat(s: &Sexp, context: &str) -> R<u64> {
    let b = big_nat(s, context)?;
    u64::try_from(&b).map_err(|_| Error::new(format!("{context}: natural out of u64 range")))
}

fn list<'a>(s: &'a Sexp, context: &str) -> R<&'a [Sexp]> {
    s.as_list()
        .ok_or_else(|| Error::new(format!("{context}: expected a list, got {s}")))
}

/// `(key value)` with a known head.
fn keyed<'a>(s: &'a Sexp, key: &str, context: &str) -> R<&'a Sexp> {
    let l = list(s, context)?;
    if l.len() == 2 && l[0].as_sym() == Some(key) {
        Ok(&l[1])
    } else {
        Err(Error::new(format!(
            "{context}: expected ({key} _), got {s}"
        )))
    }
}

// ---------------------------------------------------------------------
// Types.
// ---------------------------------------------------------------------

fn curve(s: &Sexp) -> R<Curve> {
    match s.head() {
        Some("curve-jubjub") => Ok(Curve::Jubjub),
        Some("curve-secp256k1") => Ok(Curve::Secp256k1),
        _ => err("curve", s),
    }
}

fn field_type(s: &Sexp) -> R<FieldType> {
    let l = list(s, "field-type")?;
    match l.first().and_then(Sexp::as_sym) {
        Some("field-native") => Ok(FieldType::Native),
        Some("field-base") => Ok(FieldType::Base(curve(&l[1])?)),
        Some("field-scalar") => Ok(FieldType::Scalar(curve(&l[1])?)),
        _ => err("field-type", s),
    }
}

fn adt_arg(s: &Sexp) -> R<AdtArg> {
    match s {
        Sexp::Int(_) => Ok(AdtArg::Nat(nat(s, "adt-arg")?)),
        _ => Ok(AdtArg::Type(ty(s)?)),
    }
}

pub fn ty(s: &Sexp) -> R<Type> {
    if let Some(v) = s.as_sym() {
        return Ok(Type::TypeVar(v.to_string()));
    }
    let l = list(s, "type")?;
    let head = l
        .first()
        .and_then(Sexp::as_sym)
        .ok_or_else(|| Error::new(format!("type: headless {s}")))?;
    match head {
        "tboolean" => Ok(Type::Boolean),
        "tfield" => Ok(Type::Field(field_type(&l[1])?)),
        "tunsigned" => Ok(Type::Unsigned(big_nat(&l[1], "tunsigned")?)),
        "tpoint" => Ok(Type::Point(curve(&l[1])?)),
        "tbytes" => Ok(Type::Bytes(nat(&l[1], "tbytes")?)),
        "topaque" => Ok(Type::Opaque(string(&l[1], "topaque")?)),
        "tvector" => Ok(Type::Vector {
            len: nat(&l[1], "tvector")?,
            ty: Box::new(ty(&l[2])?),
        }),
        "ttuple" => Ok(Type::Tuple(l[1..].iter().map(ty).collect::<R<_>>()?)),
        "tstruct" => {
            let name = sym(&l[1], "tstruct name")?;
            let fields = l[2..]
                .iter()
                .map(|f| {
                    let fl = list(f, "tstruct field")?;
                    Ok((sym(&fl[0], "tstruct field name")?, ty(&fl[1])?))
                })
                .collect::<R<_>>()?;
            Ok(Type::Struct { name, fields })
        }
        "tenum" => Ok(Type::Enum {
            name: sym(&l[1], "tenum name")?,
            variants: l[2..]
                .iter()
                .map(|v| sym(v, "tenum variant"))
                .collect::<R<_>>()?,
        }),
        "talias" => Ok(Type::Alias {
            nominal: boolean(&l[1], "talias")?,
            name: sym(&l[2], "talias name")?,
            ty: Box::new(ty(&l[3])?),
        }),
        "tcontract" => Ok(Type::Contract {
            name: sym(&l[1], "tcontract name")?,
            circuits: l[2..].iter().map(contract_circuit).collect::<R<_>>()?,
        }),
        "tunknown" => Ok(Type::Unknown),
        _ => Ok(Type::Adt {
            name: head.to_string(),
            args: l[1..].iter().map(adt_arg).collect::<R<_>>()?,
        }),
    }
}

fn contract_circuit(s: &Sexp) -> R<ContractCircuit> {
    let l = list(s, "contract circuit")?;
    if l.len() != 4 {
        return err("contract circuit", s);
    }
    Ok(ContractCircuit {
        name: sym(&l[0], "contract circuit name")?,
        pure: boolean(&l[1], "contract circuit pure")?,
        argument_types: list(&l[2], "contract circuit args")?
            .iter()
            .map(ty)
            .collect::<R<_>>()?,
        result_type: ty(&l[3])?,
    })
}

// ---------------------------------------------------------------------
// Instructions and operands.
// ---------------------------------------------------------------------

const EXPR_HEADS: &[&str] = &[
    "quote",
    "var-ref",
    "default",
    "if",
    "elt-ref",
    "enum-ref",
    "tuple",
    "vector",
    "tuple-ref",
    "tuple-slice",
    "vector-ref",
    "vector-slice",
    "bytes-ref",
    "bytes-slice",
    "+",
    "-",
    "*",
    "<",
    "<=",
    ">",
    ">=",
    "==",
    "!=",
    "map",
    "fold",
    "call",
    "new",
    "seq",
    "let*",
    "assert",
    "field->bytes",
    "cast-from-bytes",
    "vector->bytes",
    "bytes->vector",
    "cast-from-enum",
    "cast-to-enum",
    "cast-to-field",
    "cast-from-field",
    "safe-cast",
    "downcast-unsigned",
    "contract-call",
    "emit",
    "public-ledger",
    "return",
];

fn operand(s: &Sexp) -> R<Operand> {
    match s {
        Sexp::Int(i) => return Ok(Operand::Int(i.clone())),
        Sexp::Bool(b) => return Ok(Operand::Bool(*b)),
        Sexp::Str(v) => return Ok(Operand::Str(v.clone())),
        _ => {}
    }
    let l = list(s, "operand")?;
    match l.first().and_then(Sexp::as_sym) {
        Some("align") => Ok(Operand::Align {
            value: big_nat(&l[1], "align value")?,
            bytes: nat(&l[2], "align bytes")?,
        }),
        Some("stack") => Ok(Operand::Stack),
        Some("void") => Ok(Operand::Void),
        Some("value->int") => Ok(Operand::ValueToInt(Box::new(operand(&l[1])?))),
        Some("null") => Ok(Operand::Null(ty(&l[1])?)),
        Some("max-sizeof") => Ok(Operand::MaxSizeof(ty(&l[1])?)),
        // A source-level addition shares the head and carries a type first,
        // so match the deferred VM one on its arity.
        Some("+") if l.len() == 3 => Ok(Operand::Add(
            Box::new(operand(&l[1])?),
            Box::new(operand(&l[2])?),
        )),
        Some("leaf-hash") => Ok(Operand::LeafHash(Box::new(operand(&l[1])?))),
        Some("coin-commit") => Ok(Operand::CoinCommit(
            Box::new(operand(&l[1])?),
            Box::new(operand(&l[2])?),
        )),
        Some("aligned-concat") => Ok(Operand::AlignedConcat(
            l[1..].iter().map(operand).collect::<R<_>>()?,
        )),
        Some("state-value") => Ok(Operand::StateValue(state_value(&l[1..])?)),
        Some(h) if EXPR_HEADS.contains(&h) => Ok(Operand::Expr(Box::new(expr(s)?))),
        Some(_) => err("operand", s),
        None => Ok(Operand::List(l.iter().map(operand).collect::<R<_>>()?)),
    }
}

fn state_value(rest: &[Sexp]) -> R<StateValue> {
    match rest.first().and_then(Sexp::as_sym) {
        Some("null") => Ok(StateValue::Null),
        Some("cell") => Ok(StateValue::Cell(Box::new(operand(&rest[1])?))),
        Some("ADT") => Ok(StateValue::Adt(Box::new(operand(&rest[1])?), ty(&rest[2])?)),
        Some("array") => Ok(StateValue::Array(
            rest[1..].iter().map(operand).collect::<R<_>>()?,
        )),
        Some("map") => Ok(StateValue::Map(
            rest[1..]
                .iter()
                .map(|e| {
                    let el = list(e, "state-value map entry")?;
                    Ok((operand(&el[0])?, operand(&el[1])?))
                })
                .collect::<R<_>>()?,
        )),
        Some("merkle-tree") => Ok(StateValue::MerkleTree {
            depth: nat(&rest[1], "merkle depth")?,
            entries: rest[2..]
                .iter()
                .map(|e| {
                    let el = list(e, "state-value tree entry")?;
                    Ok((operand(&el[0])?, operand(&el[1])?))
                })
                .collect::<R<_>>()?,
        }),
        _ => Err(Error::new(format!(
            "state-value: unrecognized kind {:?}",
            rest.first()
        ))),
    }
}

fn instruction(s: &Sexp) -> R<Instruction> {
    let l = list(s, "instruction")?;
    let op = sym(&l[0], "instruction op")?;
    let args = l[1..]
        .iter()
        .map(|a| {
            let al = list(a, "instruction arg")?;
            if al.len() != 2 {
                return err("instruction arg", a);
            }
            Ok((sym(&al[0], "instruction arg name")?, operand(&al[1])?))
        })
        .collect::<R<_>>()?;
    Ok(Instruction { op, args })
}

fn instructions(s: &Sexp) -> R<Vec<Instruction>> {
    let l = list(s, "instructions")?;
    if l.first().and_then(Sexp::as_sym) != Some("instructions") {
        return err("instructions", s);
    }
    l[1..].iter().map(instruction).collect()
}

// ---------------------------------------------------------------------
// Expressions.
// ---------------------------------------------------------------------

fn argument(s: &Sexp) -> R<Argument> {
    let l = list(s, "argument")?;
    if l.len() != 2 {
        return err("argument", s);
    }
    Ok(Argument {
        name: ident(&l[0], "argument name")?,
        ty: ty(&l[1])?,
    })
}

fn arguments(s: &Sexp) -> R<Vec<Argument>> {
    list(s, "arguments")?.iter().map(argument).collect()
}

fn tuple_arg(s: &Sexp) -> R<TupleArg> {
    let l = list(s, "tuple-arg")?;
    match l.first().and_then(Sexp::as_sym) {
        Some("single") => Ok(TupleArg::Single(expr(&l[1])?)),
        Some("spread") => Ok(TupleArg::Spread {
            len: nat(&l[1], "spread")?,
            expr: expr(&l[2])?,
        }),
        _ => err("tuple-arg", s),
    }
}

fn map_arg(s: &Sexp) -> R<MapArg> {
    let l = list(s, "map-arg")?;
    if l.len() != 3 {
        return err("map-arg", s);
    }
    Ok(MapArg {
        expr: expr(&l[0])?,
        ty: ty(&l[1])?,
        element_ty: ty(&l[2])?,
    })
}

fn fun(s: &Sexp) -> R<Fun> {
    let l = list(s, "fun")?;
    match l.first().and_then(Sexp::as_sym) {
        Some("fref") => Ok(Fun::Ref(ident(&l[1], "fref")?)),
        Some("circuit") => Ok(Fun::Circuit {
            arguments: arguments(&l[1])?,
            result_type: ty(&l[2])?,
            body: Box::new(expr(&l[3])?),
        }),
        _ => err("fun", s),
    }
}

fn path_element(s: &Sexp) -> R<PathElement> {
    match s {
        Sexp::Int(_) => Ok(PathElement::Index(nat(s, "path index")?)),
        _ => {
            let l = list(s, "path element")?;
            if l.len() != 2 {
                return err("path element", s);
            }
            Ok(PathElement::Computed {
                ty: Box::new(ty(&l[0])?),
                expr: Box::new(expr(&l[1])?),
            })
        }
    }
}

fn literal(s: &Sexp) -> R<Literal> {
    match s {
        Sexp::Int(i) => Ok(Literal::Int(i.clone())),
        Sexp::Bool(b) => Ok(Literal::Bool(*b)),
        Sexp::Bytes(b) => Ok(Literal::Bytes(b.clone())),
        _ => err("literal", s),
    }
}

fn bx(s: &Sexp) -> R<Box<Expr>> {
    Ok(Box::new(expr(s)?))
}

pub fn expr(s: &Sexp) -> R<Expr> {
    let l = list(s, "expression")?;
    let head = l
        .first()
        .and_then(Sexp::as_sym)
        .ok_or_else(|| Error::new(format!("expression: headless {s}")))?;
    match head {
        "quote" => Ok(Expr::Quote(literal(&l[1])?)),
        "var-ref" => Ok(Expr::VarRef(ident(&l[1], "var-ref")?)),
        "default" => Ok(Expr::Default(ty(&l[1])?)),
        "if" => Ok(Expr::If {
            cond: bx(&l[1])?,
            then: bx(&l[2])?,
            els: bx(&l[3])?,
        }),
        "elt-ref" => Ok(Expr::EltRef {
            expr: bx(&l[1])?,
            elt: sym(&l[2], "elt-ref")?,
            index: nat(&l[3], "elt-ref index")?,
        }),
        "enum-ref" => Ok(Expr::EnumRef {
            ty: ty(&l[1])?,
            elt: sym(&l[2], "enum-ref")?,
        }),
        "tuple" => Ok(Expr::Tuple(l[1..].iter().map(tuple_arg).collect::<R<_>>()?)),
        "vector" => Ok(Expr::VectorLit(
            l[1..].iter().map(tuple_arg).collect::<R<_>>()?,
        )),
        "tuple-ref" => Ok(Expr::TupleRef {
            expr: bx(&l[1])?,
            index: nat(&l[2], "tuple-ref")?,
        }),
        "tuple-slice" => Ok(Expr::TupleSlice {
            ty: ty(&l[1])?,
            expr: bx(&l[2])?,
            index: nat(&l[3], "tuple-slice index")?,
            len: nat(&l[4], "tuple-slice len")?,
        }),
        "vector-ref" => Ok(Expr::VectorRef {
            ty: ty(&l[1])?,
            expr: bx(&l[2])?,
            index: bx(&l[3])?,
        }),
        "vector-slice" => Ok(Expr::VectorSlice {
            ty: ty(&l[1])?,
            expr: bx(&l[2])?,
            index: bx(&l[3])?,
            len: nat(&l[4], "vector-slice len")?,
        }),
        "bytes-ref" => Ok(Expr::BytesRef {
            ty: ty(&l[1])?,
            expr: bx(&l[2])?,
            index: bx(&l[3])?,
        }),
        "bytes-slice" => Ok(Expr::BytesSlice {
            ty: ty(&l[1])?,
            expr: bx(&l[2])?,
            index: bx(&l[3])?,
            len: nat(&l[4], "bytes-slice len")?,
        }),
        "+" => Ok(Expr::Add {
            ty: ty(&l[1])?,
            left: bx(&l[2])?,
            right: bx(&l[3])?,
        }),
        "-" => Ok(Expr::Sub {
            ty: ty(&l[1])?,
            left: bx(&l[2])?,
            right: bx(&l[3])?,
        }),
        "*" => Ok(Expr::Mul {
            ty: ty(&l[1])?,
            left: bx(&l[2])?,
            right: bx(&l[3])?,
        }),
        "<" => Ok(Expr::Lt {
            bits: nat(&l[1], "<")?,
            left: bx(&l[2])?,
            right: bx(&l[3])?,
        }),
        "<=" => Ok(Expr::Le {
            bits: nat(&l[1], "<=")?,
            left: bx(&l[2])?,
            right: bx(&l[3])?,
        }),
        ">" => Ok(Expr::Gt {
            bits: nat(&l[1], ">")?,
            left: bx(&l[2])?,
            right: bx(&l[3])?,
        }),
        ">=" => Ok(Expr::Ge {
            bits: nat(&l[1], ">=")?,
            left: bx(&l[2])?,
            right: bx(&l[3])?,
        }),
        "==" => Ok(Expr::Eq {
            ty: ty(&l[1])?,
            left: bx(&l[2])?,
            right: bx(&l[3])?,
        }),
        "!=" => Ok(Expr::Neq {
            ty: ty(&l[1])?,
            left: bx(&l[2])?,
            right: bx(&l[3])?,
        }),
        "map" => Ok(Expr::Map {
            len: nat(&l[1], "map len")?,
            fun: fun(&l[2])?,
            args: l[3..].iter().map(map_arg).collect::<R<_>>()?,
        }),
        "fold" => {
            let init = list(&l[3], "fold init")?;
            if init.len() != 2 {
                return err("fold init", &l[3]);
            }
            Ok(Expr::Fold {
                len: nat(&l[1], "fold len")?,
                fun: fun(&l[2])?,
                init: bx(&init[0])?,
                init_ty: ty(&init[1])?,
                args: l[4..].iter().map(map_arg).collect::<R<_>>()?,
            })
        }
        "call" => Ok(Expr::Call {
            name: ident(&l[1], "call")?,
            args: l[2..].iter().map(expr).collect::<R<_>>()?,
        }),
        "new" => Ok(Expr::New {
            ty: ty(&l[1])?,
            elements: l[2..].iter().map(expr).collect::<R<_>>()?,
        }),
        "seq" => Ok(Expr::Seq(l[1..].iter().map(expr).collect::<R<_>>()?)),
        "let*" => {
            let bindings = list(&l[1], "let* bindings")?
                .iter()
                .map(|b| {
                    let bl = list(b, "let* binding")?;
                    if bl.len() != 2 {
                        return err("let* binding", b);
                    }
                    Ok((argument(&bl[0])?, expr(&bl[1])?))
                })
                .collect::<R<_>>()?;
            Ok(Expr::LetStar {
                bindings,
                body: bx(&l[2])?,
            })
        }
        "assert" => Ok(Expr::Assert {
            expr: bx(&l[1])?,
            message: string(&l[2], "assert")?,
        }),
        "field->bytes" => Ok(Expr::FieldToBytes {
            len: nat(&l[1], "field->bytes")?,
            field_type: field_type(&l[2])?,
            expr: bx(&l[3])?,
        }),
        "cast-from-bytes" => Ok(Expr::CastFromBytes {
            ty: ty(&l[1])?,
            len: nat(&l[2], "cast-from-bytes")?,
            expr: bx(&l[3])?,
        }),
        "vector->bytes" => Ok(Expr::VectorToBytes {
            len: nat(&l[1], "vector->bytes")?,
            expr: bx(&l[2])?,
        }),
        "bytes->vector" => Ok(Expr::BytesToVector {
            len: nat(&l[1], "bytes->vector")?,
            expr: bx(&l[2])?,
        }),
        "cast-from-enum" => Ok(Expr::CastFromEnum {
            ty: ty(&l[1])?,
            from: ty(&l[2])?,
            expr: bx(&l[3])?,
        }),
        "cast-to-enum" => Ok(Expr::CastToEnum {
            ty: ty(&l[1])?,
            from: ty(&l[2])?,
            expr: bx(&l[3])?,
        }),
        "cast-to-field" => Ok(Expr::CastToField {
            field_type: field_type(&l[1])?,
            from: ty(&l[2])?,
            expr: bx(&l[3])?,
        }),
        "cast-from-field" => Ok(Expr::CastFromField {
            maxval: big_nat(&l[1], "cast-from-field")?,
            field_type: field_type(&l[2])?,
            expr: bx(&l[3])?,
        }),
        "safe-cast" => Ok(Expr::SafeCast {
            ty: ty(&l[1])?,
            from: ty(&l[2])?,
            expr: bx(&l[3])?,
        }),
        "downcast-unsigned" => Ok(Expr::DowncastUnsigned {
            from_maxval: big_nat(&l[1], "downcast-unsigned")?,
            to_maxval: big_nat(&l[2], "downcast-unsigned")?,
            expr: bx(&l[3])?,
        }),
        "contract-call" => {
            let recv = list(&l[2], "contract-call receiver")?;
            if recv.len() != 2 {
                return err("contract-call receiver", &l[2]);
            }
            Ok(Expr::ContractCall {
                circuit: sym(&l[1], "contract-call circuit")?,
                receiver: bx(&recv[0])?,
                contract_type: ty(&recv[1])?,
                args: l[3..].iter().map(expr).collect::<R<_>>()?,
            })
        }
        "emit" => Ok(Expr::Emit {
            event_version: nat(&l[1], "emit version")?,
            event_tag: nat(&l[2], "emit tag")?,
            len: nat(&l[3], "emit len")?,
            payload: bx(&l[4])?,
            instructions: instructions(&l[5])?,
        }),
        "public-ledger" => Ok(Expr::PublicLedger {
            field: ident(&l[1], "public-ledger field")?,
            path: list(&l[2], "public-ledger path")?
                .iter()
                .map(path_element)
                .collect::<R<_>>()?,
            op: sym(&l[3], "public-ledger op")?,
            result_type: ty(&l[4])?,
            instructions: instructions(&l[5])?,
            args: l[6..].iter().map(expr).collect::<R<_>>()?,
        }),
        "return" => Ok(Expr::Return(bx(&l[1])?)),
        _ => err("expression", s),
    }
}

// ---------------------------------------------------------------------
// Program elements and the whole artifact.
// ---------------------------------------------------------------------

fn ledger_binding(s: &Sexp) -> R<LedgerBinding> {
    let l = list(s, "ledger binding")?;
    if l.len() != 4 {
        return err("ledger binding", s);
    }
    Ok(LedgerBinding {
        name: ident(&l[0], "ledger binding name")?,
        path: list(&l[1], "ledger binding path")?
            .iter()
            .map(|i| nat(i, "ledger binding index"))
            .collect::<R<_>>()?,
        exported: boolean(
            keyed(&l[2], "exported", "ledger binding")?,
            "ledger binding exported",
        )?,
        ty: ty(&l[3])?,
    })
}

fn flatten_pl_array(s: &Sexp, out: &mut Vec<LedgerBinding>) -> R<()> {
    let l = list(s, "public-ledger-array")?;
    if l.first().and_then(Sexp::as_sym) != Some("public-ledger-array") {
        return err("public-ledger-array", s);
    }
    for e in &l[1..] {
        if e.head() == Some("public-ledger-array") {
            flatten_pl_array(e, out)?;
        } else {
            out.push(ledger_binding(e)?);
        }
    }
    Ok(())
}

fn program_element(s: &Sexp) -> R<ProgramElement> {
    let l = list(s, "program element")?;
    match l.first().and_then(Sexp::as_sym) {
        Some("circuit") => Ok(ProgramElement::Circuit(Circuit {
            name: ident(&l[1], "circuit name")?,
            exported: boolean(keyed(&l[2], "exported", "circuit")?, "circuit exported")?,
            pure: boolean(keyed(&l[3], "pure", "circuit")?, "circuit pure")?,
            proof: boolean(keyed(&l[4], "proof", "circuit")?, "circuit proof")?,
            arguments: arguments(&l[5])?,
            result_type: ty(&l[6])?,
            body: expr(&l[7])?,
        })),
        Some("native") => {
            let entry = list(&l[2], "native entry")?;
            if entry.len() != 3 || entry[0].as_sym() != Some("entry") {
                return err("native entry", &l[2]);
            }
            Ok(ProgramElement::Native(Native {
                name: ident(&l[1], "native name")?,
                entry: string(&entry[1], "native entry fn")?,
                class: sym(&entry[2], "native entry class")?,
                arguments: arguments(&l[3])?,
                result_type: ty(&l[4])?,
            }))
        }
        Some("witness") => Ok(ProgramElement::Witness(Witness {
            name: ident(&l[1], "witness name")?,
            arguments: arguments(&l[2])?,
            result_type: ty(&l[3])?,
        })),
        Some("kernel-declaration") => Ok(ProgramElement::KernelDeclaration(ledger_binding(&l[1])?)),
        Some("public-ledger-declaration") => {
            let mut fields = Vec::new();
            flatten_pl_array(&l[1], &mut fields)?;
            let c = list(&l[2], "constructor")?;
            if c.first().and_then(Sexp::as_sym) != Some("constructor") {
                return err("constructor", &l[2]);
            }
            Ok(ProgramElement::PublicLedgerDeclaration(LedgerDeclaration {
                fields,
                constructor: Constructor {
                    arguments: arguments(&c[1])?,
                    body: expr(&c[2])?,
                },
            }))
        }
        Some("export-typedef") => Ok(ProgramElement::ExportTypedef {
            name: sym(&l[1], "export-typedef")?,
            type_vars: list(&l[2], "export-typedef vars")?
                .iter()
                .map(|v| sym(v, "export-typedef var"))
                .collect::<R<_>>()?,
            ty: ty(&l[3])?,
        }),
        _ => err("program element", s),
    }
}

pub fn parse(s: &Sexp) -> R<AnalyzedIr> {
    let l = list(s, "analyzed-ir")?;
    if l.first().and_then(Sexp::as_sym) != Some("analyzed-ir") {
        return err("analyzed-ir", s);
    }
    let mut compiler_version = None;
    let mut language_version = None;
    let mut runtime_version = None;
    let mut exports = Vec::new();
    let mut contract_types = Vec::new();
    let mut elements = Vec::new();
    for e in &l[1..] {
        match e.head() {
            Some("compiler-version") => {
                compiler_version = Some(string(&list(e, "hdr")?[1], "compiler-version")?)
            }
            Some("language-version") => {
                language_version = Some(string(&list(e, "hdr")?[1], "language-version")?)
            }
            Some("runtime-version") => {
                runtime_version = Some(string(&list(e, "hdr")?[1], "runtime-version")?)
            }
            Some("exports") => {
                for x in &list(e, "exports")?[1..] {
                    match x {
                        Sexp::Pair(a, b) => {
                            exports.push((sym(a, "export name")?, ident(b, "export target")?))
                        }
                        _ => return err("export entry", x),
                    }
                }
            }
            Some("contract-types") => {
                for t in &list(e, "contract-types")?[1..] {
                    contract_types.push(ty(t)?);
                }
            }
            _ => elements.push(program_element(e)?),
        }
    }
    Ok(AnalyzedIr {
        compiler_version: compiler_version.ok_or_else(|| Error::new("missing compiler-version"))?,
        language_version: language_version.ok_or_else(|| Error::new("missing language-version"))?,
        runtime_version: runtime_version.ok_or_else(|| Error::new("missing runtime-version"))?,
        exports,
        contract_types,
        elements,
    })
}
