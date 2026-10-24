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

/// A list of exactly `N` elements.
fn fixed<'a, const N: usize>(s: &'a Sexp, context: &str) -> R<&'a [Sexp; N]> {
    <&[Sexp; N]>::try_from(list(s, context)?).or_else(|_| err(context, s))
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

/// `(head ...)` with a known head, as the elements after the head.
fn tagged<'a>(s: &'a Sexp, head: &str, context: &str) -> R<&'a [Sexp]> {
    match list(s, context)? {
        [h, rest @ ..] if h.as_sym() == Some(head) => Ok(rest),
        _ => err(context, s),
    }
}

// ---------------------------------------------------------------------
// Types.
// ---------------------------------------------------------------------

fn curve(s: &Sexp) -> R<Curve> {
    match s.head() {
        Some("curve-jubjub") => Ok(Curve::Jubjub),
        Some("curve-secp256k1") => Ok(Curve::Secp256k1),
        Some("curve-secp256r1") => Ok(Curve::Secp256r1),
        Some("curve-curve25519") => Ok(Curve::Curve25519),
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
                    let [name, t] = fixed(f, "tstruct field")?;
                    Ok((sym(name, "tstruct field name")?, ty(t)?))
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
    let [name, pure, argument_types, result_type] = fixed(s, "contract circuit")?;
    Ok(ContractCircuit {
        name: sym(name, "contract circuit name")?,
        pure: boolean(pure, "contract circuit pure")?,
        argument_types: list(argument_types, "contract circuit args")?
            .iter()
            .map(ty)
            .collect::<R<_>>()?,
        result_type: ty(result_type)?,
    })
}

// ---------------------------------------------------------------------
// Instructions and operands.
// ---------------------------------------------------------------------

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
        Some(_) => Ok(Operand::Expr(bx(s)?)),
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
                .map(|e| operand_pair(e, "state-value map entry"))
                .collect::<R<_>>()?,
        )),
        Some("merkle-tree") => Ok(StateValue::MerkleTree {
            depth: nat(&rest[1], "merkle depth")?,
            entries: rest[2..]
                .iter()
                .map(|e| operand_pair(e, "state-value tree entry"))
                .collect::<R<_>>()?,
        }),
        _ => Err(Error::new(format!(
            "state-value: unrecognized kind {:?}",
            rest.first()
        ))),
    }
}

fn operand_pair(s: &Sexp, context: &str) -> R<(Operand, Operand)> {
    let [k, v] = fixed(s, context)?;
    Ok((operand(k)?, operand(v)?))
}

fn instruction(s: &Sexp) -> R<Instruction> {
    let l = list(s, "instruction")?;
    let op = OpName::from(sym(&l[0], "instruction op")?.as_str());
    let args = l[1..]
        .iter()
        .map(|a| {
            let [name, value] = fixed(a, "instruction arg")?;
            Ok((sym(name, "instruction arg name")?, operand(value)?))
        })
        .collect::<R<_>>()?;
    Ok(Instruction { op, args })
}

fn instructions(s: &Sexp) -> R<Vec<Instruction>> {
    tagged(s, "instructions", "instructions")?
        .iter()
        .map(instruction)
        .collect()
}

// ---------------------------------------------------------------------
// Expressions.
// ---------------------------------------------------------------------

fn argument(s: &Sexp) -> R<Argument> {
    let [name, t] = fixed(s, "argument")?;
    Ok(Argument {
        name: ident(name, "argument name")?,
        ty: ty(t)?,
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
    let [e, t, element_t] = fixed(s, "map-arg")?;
    Ok(MapArg {
        expr: expr(e)?,
        ty: ty(t)?,
        element_ty: ty(element_t)?,
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
            let [t, e] = fixed(s, "path element")?;
            Ok(PathElement::Computed {
                ty: Box::new(ty(t)?),
                expr: Box::new(expr(e)?),
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

fn ty_expr_expr(l: &[Sexp], make: fn(Type, Box<Expr>, Box<Expr>) -> Expr) -> R<Expr> {
    Ok(make(ty(&l[1])?, bx(&l[2])?, bx(&l[3])?))
}

fn nat_expr_expr(l: &[Sexp], head: &str, make: fn(u64, Box<Expr>, Box<Expr>) -> Expr) -> R<Expr> {
    Ok(make(nat(&l[1], head)?, bx(&l[2])?, bx(&l[3])?))
}

fn ty_ty_expr(l: &[Sexp], make: fn(Type, Type, Box<Expr>) -> Expr) -> R<Expr> {
    Ok(make(ty(&l[1])?, ty(&l[2])?, bx(&l[3])?))
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
        "vector-ref" => ty_expr_expr(l, |ty, expr, index| Expr::VectorRef { ty, expr, index }),
        "vector-slice" => Ok(Expr::VectorSlice {
            ty: ty(&l[1])?,
            expr: bx(&l[2])?,
            index: bx(&l[3])?,
            len: nat(&l[4], "vector-slice len")?,
        }),
        "bytes-ref" => ty_expr_expr(l, |ty, expr, index| Expr::BytesRef { ty, expr, index }),
        "bytes-slice" => Ok(Expr::BytesSlice {
            ty: ty(&l[1])?,
            expr: bx(&l[2])?,
            index: bx(&l[3])?,
            len: nat(&l[4], "bytes-slice len")?,
        }),
        "+" => ty_expr_expr(l, |ty, left, right| Expr::Add { ty, left, right }),
        "-" => ty_expr_expr(l, |ty, left, right| Expr::Sub { ty, left, right }),
        "*" => ty_expr_expr(l, |ty, left, right| Expr::Mul { ty, left, right }),
        "<" => nat_expr_expr(l, head, |bits, left, right| Expr::Lt { bits, left, right }),
        "<=" => nat_expr_expr(l, head, |bits, left, right| Expr::Le { bits, left, right }),
        ">" => nat_expr_expr(l, head, |bits, left, right| Expr::Gt { bits, left, right }),
        ">=" => nat_expr_expr(l, head, |bits, left, right| Expr::Ge { bits, left, right }),
        "==" => ty_expr_expr(l, |ty, left, right| Expr::Eq { ty, left, right }),
        "!=" => ty_expr_expr(l, |ty, left, right| Expr::Neq { ty, left, right }),
        "map" => Ok(Expr::Map {
            len: nat(&l[1], "map len")?,
            fun: fun(&l[2])?,
            args: l[3..].iter().map(map_arg).collect::<R<_>>()?,
        }),
        "fold" => {
            let [init, init_ty] = fixed(&l[3], "fold init")?;
            Ok(Expr::Fold {
                len: nat(&l[1], "fold len")?,
                fun: fun(&l[2])?,
                init: bx(init)?,
                init_ty: ty(init_ty)?,
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
                    let [binding, value] = fixed(b, "let* binding")?;
                    Ok((argument(binding)?, expr(value)?))
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
        "cast-from-enum" => ty_ty_expr(l, |ty, from, expr| Expr::CastFromEnum { ty, from, expr }),
        "cast-to-enum" => ty_ty_expr(l, |ty, from, expr| Expr::CastToEnum { ty, from, expr }),
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
        "safe-cast" => ty_ty_expr(l, |ty, from, expr| Expr::SafeCast { ty, from, expr }),
        "downcast-unsigned" => Ok(Expr::DowncastUnsigned {
            from_maxval: big_nat(&l[1], "downcast-unsigned")?,
            to_maxval: big_nat(&l[2], "downcast-unsigned")?,
            expr: bx(&l[3])?,
        }),
        "contract-call" => {
            let [receiver, contract_type] = fixed(&l[2], "contract-call receiver")?;
            Ok(Expr::ContractCall {
                circuit: sym(&l[1], "contract-call circuit")?,
                receiver: bx(receiver)?,
                contract_type: ty(contract_type)?,
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
            op_class: op_class(&l[2])?,
            path: list(&l[3], "public-ledger path")?
                .iter()
                .map(path_element)
                .collect::<R<_>>()?,
            op: sym(&l[4], "public-ledger op")?,
            result_type: ty(&l[5])?,
            instructions: instructions(&l[6])?,
            args: l[7..].iter().map(expr).collect::<R<_>>()?,
        }),
        "return" => Ok(Expr::Return(bx(&l[1])?)),
        _ => err("expression", s),
    }
}

// ---------------------------------------------------------------------
// Program elements and the whole artifact.
// ---------------------------------------------------------------------

fn op_class(s: &Sexp) -> R<OpClass> {
    if let Some(name) = s.as_sym() {
        return Ok(OpClass::Plain(name.to_string()));
    }
    let [name, coin, recipient] = fixed(s, "operation class")?;
    Ok(OpClass::CoinCheck {
        name: sym(name, "operation class")?,
        coin: nat(coin, "coin argument index")?,
        recipient: nat(recipient, "recipient argument index")?,
    })
}

fn ledger_binding(s: &Sexp) -> R<LedgerBinding> {
    let [name, path, exported, t] = fixed(s, "ledger binding")?;
    Ok(LedgerBinding {
        name: ident(name, "ledger binding name")?,
        path: list(path, "ledger binding path")?
            .iter()
            .map(|i| nat(i, "ledger binding index"))
            .collect::<R<_>>()?,
        exported: boolean(
            keyed(exported, "exported", "ledger binding")?,
            "ledger binding exported",
        )?,
        ty: ty(t)?,
    })
}

fn flatten_pl_array(s: &Sexp, out: &mut Vec<LedgerBinding>) -> R<()> {
    for e in tagged(s, "public-ledger-array", "public-ledger-array")? {
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
            let [tag, entry_fn, class] = fixed(&l[2], "native entry")?;
            if tag.as_sym() != Some("entry") {
                return err("native entry", &l[2]);
            }
            let type_args = tagged(&l[3], "type-arguments", "native type arguments")?;
            Ok(ProgramElement::Native(Native {
                name: ident(&l[1], "native name")?,
                entry: string(entry_fn, "native entry fn")?,
                class: sym(class, "native entry class")?,
                type_arguments: type_args.iter().map(ty).collect::<R<_>>()?,
                arguments: arguments(&l[4])?,
                result_type: ty(&l[5])?,
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
            let [tag, params, body] = fixed(&l[2], "constructor")?;
            if tag.as_sym() != Some("constructor") {
                return err("constructor", &l[2]);
            }
            Ok(ProgramElement::PublicLedgerDeclaration(LedgerDeclaration {
                fields,
                constructor: Constructor {
                    arguments: arguments(params)?,
                    body: expr(body)?,
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

fn version(s: &Sexp, key: &str) -> R<String> {
    string(keyed(s, key, key)?, key)
}

pub fn parse(s: &Sexp) -> R<AnalyzedIr> {
    let rest = tagged(s, "analyzed-ir", "analyzed-ir")?;
    let mut compiler_version = None;
    let mut language_version = None;
    let mut runtime_version = None;
    let mut exports = Vec::new();
    let mut contract_types = Vec::new();
    let mut elements = Vec::new();
    for e in rest {
        match e.head() {
            Some("compiler-version") => compiler_version = Some(version(e, "compiler-version")?),
            Some("language-version") => language_version = Some(version(e, "language-version")?),
            Some("runtime-version") => runtime_version = Some(version(e, "runtime-version")?),
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
