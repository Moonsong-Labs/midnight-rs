//! Circuit call integration tests.
//!
//! These tests run circuits through the interpreter that `midnight_contract`
//! re-exports. They cover witness dispatch, captured Zswap outputs,
//! `kernel.self()`, and struct arguments.

mod common;

use common::{epoch, with_witnesses};
use compact_bindgen::AlignedValue;
use midnight_coin_structure::contract::ContractAddress;
use midnight_contract::interpreter;

use compact_codegen::ir::{self, Argument, Expr, Ident, Literal, Type};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn id(name: &str) -> Ident {
    Ident(name.to_string())
}

fn uint(maxval: &str) -> Type {
    Type::Unsigned(maxval.parse().expect("a decimal bound"))
}

/// A proof circuit with the given signature and body, named as the compiler
/// names an exported circuit.
fn circuit(arguments: Vec<Argument>, result_type: Type, body: Expr) -> ir::Circuit {
    ir::Circuit {
        name: id("%test.0"),
        exported: true,
        pure: false,
        proof: true,
        arguments,
        result_type,
        body,
    }
}

// ---------------------------------------------------------------------------
// Witness calls
// ---------------------------------------------------------------------------

/// A witness declaration whose name collides with the interpreter builtin of
/// the same name.
fn persistent_hash_witness() -> ir::Witness {
    ir::Witness {
        name: id("%persistentHash.1"),
        arguments: vec![Argument {
            name: id("%value.2"),
            ty: uint("65535"),
        }],
        result_type: Type::Bytes(32),
    }
}

/// IR whose result is a `persistentHash` witness call over a literal.
fn persistent_hash_witness_ir() -> ir::Circuit {
    circuit(
        Vec::new(),
        Type::Bytes(32),
        Expr::Call {
            name: id("%persistentHash.1"),
            args: vec![Expr::Quote(Literal::Int(7.into()))],
        },
    )
}

/// A real provider failure (HSM down, decode error, ...) must propagate even
/// when the witness name collides with a builtin: the builtin must NOT run.
#[test]
fn witness_failure_on_builtin_name_propagates() {
    use midnight_contract::interpreter;
    use midnight_contract::runtime::{InterpreterError, Value, WitnessOutcome, WitnessProvider};

    struct FailingHsm;
    impl WitnessProvider for FailingHsm {
        fn call_witness(
            &self,
            _ctx: &mut midnight_contract::runtime::WitnessContext<'_>,
            name: &str,
            _args: &[Value],
        ) -> Result<WitnessOutcome, InterpreterError> {
            Err(InterpreterError::Witness(format!(
                "hsm unreachable: {name}"
            )))
        }
    }

    let witnesses = vec![persistent_hash_witness()];
    let program = interpreter::Program::new(&[], &witnesses, &[]);
    let state = common::contract_state(vec![]);
    match interpreter::execute(
        &persistent_hash_witness_ir(),
        &program,
        state,
        &[],
        with_witnesses(&FailingHsm),
    ) {
        Ok(_) => panic!("a witness-level failure must propagate, not fall through to the builtin"),
        Err(InterpreterError::Witness(msg)) => {
            assert_eq!(msg, "hsm unreachable: persistentHash");
        }
        Err(other) => panic!("expected the provider's witness error, got {other:?}"),
    }
}

/// A declared witness that the provider does not implement is an error, also
/// when a builtin has its name: the builtin must not answer for the witness.
#[test]
fn unknown_witness_on_builtin_name_is_an_error() {
    use midnight_contract::interpreter;
    use midnight_contract::runtime::InterpreterError;

    let witnesses = vec![persistent_hash_witness()];
    let program = interpreter::Program::new(&[], &witnesses, &[]);
    let state = common::contract_state(vec![]);
    // `epoch()` carries `NoWitnesses`, which answers `Unknown` for every name.
    match interpreter::execute(&persistent_hash_witness_ir(), &program, state, &[], epoch()) {
        Err(InterpreterError::Witness(_)) => {}
        Ok(_) => panic!("an unknown witness must not fall back to the builtin"),
        Err(other) => panic!("expected a witness error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Build-only calls
// ---------------------------------------------------------------------------

/// A fee-less call of a contract with witnesses needs a private-state store,
/// as a submitted call does, and it checks for one before it queries the node.
/// The provider's node URL is a closed port, so a check after the state read
/// fails with a node error instead. The wallet is never reached. A handle that
/// declares no witnesses needs no store, so its call reaches the node query.
#[tokio::test]
async fn a_fee_less_call_with_witnesses_needs_a_store_before_any_node_query() {
    use midnight_base_crypto::hash::HashOutput;
    use midnight_contract::runtime::NoWitnesses;
    use midnight_contract::{Contract, ContractError, ShieldedInputs};
    use midnight_provider::MidnightProvider;

    let provider = MidnightProvider::new("ws://127.0.0.1:1").with_wallet(common::UnreachedWallet);
    let contract = Contract::at(
        &provider,
        midnight_contract::ContractAddress(HashOutput([7; 32])),
    )
    .with_declared_witnesses(true)
    .with_zk_config("compiled")
    .build();
    let witnesses = vec![persistent_hash_witness()];
    let program = interpreter::Program::new(&[], &witnesses, &[]);

    let err = contract
        .without_fee_call_with(
            &persistent_hash_witness_ir(),
            &program,
            "test",
            &[],
            &NoWitnesses,
            &[],
            ShieldedInputs::default(),
        )
        .await
        .expect_err("a contract with witnesses and no store must not build");
    assert!(
        matches!(&err, ContractError::Construction(message) if message.contains("with_private_state")),
        "expected the store error, got {err:?}"
    );

    // A URL with no scheme fails the connect at once, where the closed port
    // waits out the query bound.
    let unconnectable = MidnightProvider::new("no-scheme").with_wallet(common::UnreachedWallet);
    let undeclared = Contract::at(
        &unconnectable,
        midnight_contract::ContractAddress(HashOutput([7; 32])),
    )
    .with_zk_config("compiled")
    .build();
    let err = undeclared
        .without_fee_call_with(
            &persistent_hash_witness_ir(),
            &program,
            "test",
            &[],
            &NoWitnesses,
            &[],
            ShieldedInputs::default(),
        )
        .await
        .expect_err("the closed port fails the state query");
    assert!(
        matches!(err, ContractError::Provider(_)),
        "a handle that declares no witnesses needs no store, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Shielded mint: full `mintShieldedToken` circuit
// ---------------------------------------------------------------------------

/// A probe whose recipient is a runtime `Either`, so the interpreter has to
/// destructure it; the devnet mint folds that branch away.
fn mint_probe_info() -> compact_codegen::types::ContractInfo {
    let text =
        include_str!("../../../tests/fixtures/compiled/mint-probe/compiler/analyzed-ir.sexp");
    compact_codegen::artifact::load_str(text).unwrap()
}

/// The `recipient` argument's declared type must carry the nested `Either` /
/// `ZswapCoinPublicKey` / `ContractAddress` layout the interpreter needs to
/// slice it. The type is the only source of that layout, so a compiler that
/// stopped emitting fields inline would break the funded call path here
/// rather than somewhere deep in execution.
#[test]
fn the_recipient_argument_type_carries_its_nested_layout() {
    let info = mint_probe_info();
    let mint = common::exported_circuit(&info, "mint");

    let recipient = &mint
        .arguments()
        .iter()
        .find(|a| a.name.name() == "recipient")
        .expect("mint takes a recipient argument")
        .ty;

    let Type::Struct { name, fields } = recipient else {
        panic!("recipient must be a struct type, got {recipient:?}")
    };
    assert_eq!(name, "Either");
    let field_names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(field_names, ["is_left", "left", "right"]);

    for (branch, expected) in [(1, "ZswapCoinPublicKey"), (2, "ContractAddress")] {
        let Type::Struct { name, fields } = &fields[branch].1 else {
            panic!("branch {branch} must be a struct type")
        };
        assert_eq!(name, expected);
        assert!(
            fields.iter().any(|(n, _)| n == "bytes"),
            "{expected} must carry its `bytes` field"
        );
    }
}

/// Encode an `Either::left(cpk)` recipient as the interpreter sees a
/// struct-typed argument: a flat `AlignedValue` of `[is_left, left.bytes,
/// right.bytes]`.
fn either_left(cpk: [u8; 32]) -> AlignedValue {
    AlignedValue::concat(
        [
            AlignedValue::from(true),
            AlignedValue::from(cpk),
            AlignedValue::from([0u8; 32]),
        ]
        .iter(),
    )
}

/// The mint circuit's arguments, as the funded call path passes them.
fn mint_args(domain_sep: [u8; 32]) -> [(&'static str, midnight_contract::runtime::Value); 4] {
    use midnight_contract::runtime::Value;
    [
        (
            "domain_sep",
            Value::AlignedValue(AlignedValue::from(domain_sep)),
        ),
        ("value", Value::Integer(1000)),
        ("nonce", Value::AlignedValue(AlignedValue::from([2u8; 32]))),
        ("recipient", Value::AlignedValue(either_left([3u8; 32]))),
    ]
}

/// Run the mint circuit against an EMPTY contract state, passing the real
/// contract address. `kernel.self()` (`dup{n:2} idx[0] popeq`) reads the
/// address from the VM **context**, not user state, so the deployed `data` is
/// an empty array. The interpreter must resolve `kernel.self()` to the supplied
/// address; the minted coin's color is `tokenType(domain_sep, address)`, so the
/// captured output's color depends on the address — proving the resolution uses
/// the real address rather than a zero/dummy one.
fn run_mint(
    domain_sep: [u8; 32],
    address: midnight_coin_structure::contract::ContractAddress,
) -> midnight_contract::runtime::CircuitZswapOutput {
    use midnight_contract::interpreter;

    let info = mint_probe_info();
    let mint = common::exported_circuit(&info, "mint");
    let program = common::program(&info);

    // Harvest the inline `Either` / `ZswapCoinPublicKey` / `ContractAddress`
    // defs from the circuit arguments, exactly as the funded call path does.

    // Deployed mint contract has no user ledger fields: data is an empty array.
    let state = common::contract_state(vec![]);

    let args = mint_args(domain_sep);

    let result = interpreter::execute(
        &mint.def,
        &program,
        state,
        &args,
        interpreter::Env {
            address: Some(address),
            ..epoch()
        },
    )
    .expect("mint circuit must execute");

    assert_eq!(
        result.zswap_outputs.len(),
        1,
        "mintShieldedToken creates exactly one Zswap output"
    );
    let out = result.zswap_outputs.into_iter().next().unwrap();
    assert_eq!(
        out.recipient.try_to_aligned_value().unwrap(),
        either_left([3u8; 32]),
        "createZswapOutput must capture the recipient argument"
    );
    out
}

/// Full mint circuit, end to end against an empty deployed state, using the
/// `dup` arities the patched compiler emits. Exercises `kernel.self()`
/// resolution, the `persistentCommit` token-color derivation, the Either
/// destructuring, and the `mintShieldedToken`/`claimZswapCoinSpend` effect ops
/// (which need `dup{n:1}`/`dup{n:2}`), and proves the captured coin's color
/// depends on the contract address.
#[test]
fn interpreter_runs_mint_shielded_token_circuit() {
    fn addr(b: u8) -> midnight_coin_structure::contract::ContractAddress {
        ContractAddress(midnight_base_crypto::hash::HashOutput([b; 32]))
    }
    fn color_of(out: &midnight_contract::runtime::CircuitZswapOutput) -> [u8; 32] {
        // coin AlignedValue atoms: [nonce(32), color(32), value]. Color is
        // atom 1, FAB-trimmed of trailing zeros.
        let av = out.coin.try_to_aligned_value().unwrap();
        midnight_base_crypto::hash::HashOutput::try_from(&av.value.0[1])
            .expect("a 32-byte color atom")
            .0
    }

    let domain = [1u8; 32];
    let color_a = color_of(&run_mint(domain, addr(0xAA)));
    let color_b = color_of(&run_mint(domain, addr(0xBB)));

    assert_ne!(
        color_a, [0u8; 32],
        "minted coin color must be a real token type, not zero"
    );
    assert_ne!(
        color_a, color_b,
        "coin color = tokenType(domain_sep, address): different addresses must give \
         different colors, proving kernel.self() resolves to the real contract address"
    );
}
