//! The fixtures are real compiler output: regenerate with
//! `compactc --skip-zk --analyzed-ir <src> <out>`
//! (the hook lives in midnight-rs) and copy `<out>/compiler/analyzed-ir.sexp`.

use compact_analyzed_ir::*;

fn fixture(name: &str) -> AnalyzedIr {
    let path = format!("{}/tests/fixtures/{name}.sexp", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).expect("fixture readable");
    parse_str(&src).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn every_fixture_parses() {
    for name in [
        "counter",
        "bboard",
        "events",
        "ser",
        "ccc",
        "loops",
        "slices",
        "mint-probe",
        "zerocash",
        "adt-defaults",
    ] {
        let ir = fixture(name);
        assert!(!ir.elements.is_empty(), "{name}: no elements");
        assert_eq!(ir.compiler_version, "0.33.122");
    }
}

#[test]
fn counter_shape() {
    let ir = fixture("counter");
    assert_eq!(
        ir.exports
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["increment", "round"]
    );
    let inc = ir.circuit("increment").expect("increment");
    assert!(inc.exported && !inc.pure && inc.proof);

    let ledger = ir.ledger().expect("ledger");
    assert_eq!(ledger.fields.len(), 1);
    assert_eq!(ledger.fields[0].name.name(), "round");
    assert_eq!(ledger.fields[0].path, [0]);
    assert!(matches!(&ledger.fields[0].ty, Type::Adt { name, .. } if name == "Counter"));

    // The increment site: idx / addi / ins against state index 0.
    let mut sites: Vec<(String, Vec<Instruction>)> = Vec::new();
    walk_deep(&inc.body, &mut |e| {
        if let Expr::PublicLedger {
            op, instructions, ..
        } = e
        {
            sites.push((op.clone(), instructions.clone()));
        }
    });
    assert_eq!(sites.len(), 1);
    let (op, instructions) = &sites[0];
    assert_eq!(op, "increment");
    assert_eq!(
        instructions
            .iter()
            .map(|i| i.op.as_str())
            .collect::<Vec<_>>(),
        ["idx", "addi", "ins"]
    );
    assert_eq!(instructions[2].arg("n"), Some(&Operand::Int(1.into())));
}

#[test]
fn mint_probe_carries_dup_arities() {
    let ir = fixture("mint-probe");
    let mut arities = Vec::new();
    for c in ir.circuits() {
        collect_dups(&c.body, &mut arities);
    }
    // kernel.self is dup{n:2}; the mint effects use dup{n:1} and dup{n:2}.
    assert!(
        arities.contains(&1.into()) && arities.contains(&2.into()),
        "{arities:?}"
    );
}

#[test]
fn events_emit() {
    let ir = fixture("events");
    let c = ir.circuit("mint_event").expect("mint_event");
    let mut found = false;
    walk_deep(&c.body, &mut |e| {
        if let Expr::Emit {
            event_version,
            event_tag,
            len,
            instructions,
            ..
        } = e
        {
            assert_eq!((*event_version, *event_tag, *len), (1, 6, 80));
            assert_eq!(instructions.last().map(|i| i.op.as_str()), Some("log"));
            found = true;
        }
    });
    assert!(found, "no emit node");
}

#[test]
fn bboard_types() {
    let ir = fixture("bboard");
    let ledger = ir.ledger().expect("ledger");
    let message = ledger
        .fields
        .iter()
        .find(|f| f.name.name() == "message")
        .expect("message");
    let Type::Adt { name, args } = &message.ty else {
        panic!("not an ADT")
    };
    assert_eq!(name, "__compact_Cell");
    let AdtArg::Type(Type::Struct { name, fields }) = &args[0] else {
        panic!("not a struct cell")
    };
    assert_eq!(name, "Maybe");
    assert_eq!(fields[0].0, "is_some");
}

// -- helpers ----------------------------------------------------------

fn collect_dups(e: &Expr, out: &mut Vec<num_bigint::BigInt>) {
    walk_deep(e, &mut |e| {
        if let Expr::PublicLedger { instructions, .. } = e {
            for i in instructions {
                if i.op != "dup" {
                    continue;
                }
                if let Some(Operand::Int(n)) = i.arg("n") {
                    out.push(n.clone());
                }
            }
        }
    });
}

/// Visit `e` and every subexpression.
fn walk_deep(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    walk(e, &mut |c| walk_deep(c, f));
}

/// Visit the direct subexpressions of `e`.
fn walk(e: &Expr, f: &mut impl FnMut(&Expr)) {
    use Expr::*;
    let mut go = |x: &Expr| f(x);
    match e {
        Quote(_) | VarRef(_) | Default(_) | EnumRef { .. } => {}
        If { cond, then, els } => {
            go(cond);
            go(then);
            go(els);
        }
        EltRef { expr, .. }
        | TupleRef { expr, .. }
        | TupleSlice { expr, .. }
        | VectorToBytes { expr, .. }
        | BytesToVector { expr, .. }
        | FieldToBytes { expr, .. }
        | CastFromBytes { expr, .. }
        | CastFromEnum { expr, .. }
        | CastToEnum { expr, .. }
        | CastToField { expr, .. }
        | CastFromField { expr, .. }
        | SafeCast { expr, .. }
        | DowncastUnsigned { expr, .. }
        | Assert { expr, .. }
        | Return(expr) => go(expr),
        VectorRef { expr, index, .. }
        | VectorSlice { expr, index, .. }
        | BytesRef { expr, index, .. }
        | BytesSlice { expr, index, .. } => {
            go(expr);
            go(index);
        }
        Add { left, right, .. }
        | Sub { left, right, .. }
        | Mul { left, right, .. }
        | Lt { left, right, .. }
        | Le { left, right, .. }
        | Gt { left, right, .. }
        | Ge { left, right, .. }
        | Eq { left, right, .. }
        | Neq { left, right, .. } => {
            go(left);
            go(right);
        }
        Tuple(args) | VectorLit(args) => {
            for a in args {
                match a {
                    TupleArg::Single(x) | TupleArg::Spread { expr: x, .. } => go(x),
                }
            }
        }
        Map { fun, args, .. } => {
            if let Fun::Circuit { body, .. } = fun {
                go(body);
            }
            for a in args {
                go(&a.expr);
            }
        }
        Fold {
            fun, init, args, ..
        } => {
            if let Fun::Circuit { body, .. } = fun {
                go(body);
            }
            go(init);
            for a in args {
                go(&a.expr);
            }
        }
        Call { args, .. } | New { elements: args, .. } => {
            for a in args {
                go(a);
            }
        }
        Seq(items) => {
            for i in items {
                go(i);
            }
        }
        LetStar { bindings, body } => {
            for (_, v) in bindings {
                go(v);
            }
            go(body);
        }
        ContractCall { receiver, args, .. } => {
            go(receiver);
            for a in args {
                go(a);
            }
        }
        Emit { payload, .. } => go(payload),
        PublicLedger { path, args, .. } => {
            for p in path {
                if let PathElement::Computed { expr, .. } = p {
                    go(expr);
                }
            }
            for a in args {
                go(a);
            }
        }
    }
}

/// `List.head` and `Map.insertDefault` are the operations whose VM code
/// carries a type where a value usually sits, and an addition.
#[test]
fn adt_default_operands_carry_types() {
    let ir = fixture("adt-defaults");
    let mut saw_add = false;
    let mut saw_null = false;
    let mut saw_max_sizeof = false;
    let mut saw_adt = false;

    fn walk(o: &Operand, add: &mut bool, null: &mut bool, max: &mut bool, adt: &mut bool) {
        match o {
            Operand::Add(a, b) => {
                *add = true;
                walk(a, add, null, max, adt);
                walk(b, add, null, max, adt);
            }
            Operand::Null(Type::Unsigned(_) | Type::Adt { .. }) => *null = true,
            Operand::MaxSizeof(Type::Unsigned(_)) => *max = true,
            Operand::StateValue(StateValue::Adt(v, Type::Adt { .. })) => {
                *adt = true;
                walk(v, add, null, max, adt);
            }
            Operand::ValueToInt(x) | Operand::LeafHash(x) => walk(x, add, null, max, adt),
            Operand::AlignedConcat(xs) | Operand::List(xs) => {
                for x in xs {
                    walk(x, add, null, max, adt);
                }
            }
            _ => {}
        }
    }

    for c in ir.circuits() {
        walk_deep(&c.body, &mut |e| {
            if let Expr::PublicLedger { instructions, .. } = e {
                for i in instructions {
                    for (_, o) in &i.args {
                        walk(
                            o,
                            &mut saw_add,
                            &mut saw_null,
                            &mut saw_max_sizeof,
                            &mut saw_adt,
                        );
                    }
                }
            }
        });
    }

    assert!(saw_add, "no `+` operand");
    assert!(saw_null, "no typed `null` operand");
    assert!(saw_max_sizeof, "no typed `max-sizeof` operand");
    assert!(saw_adt, "no typed `state-value ADT` operand");
}
