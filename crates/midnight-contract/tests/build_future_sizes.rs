//! Every public entry point that builds, proves, or resyncs must hand back a
//! small future.
//!
//! A caller's own future stores each future it awaits, so an inlined
//! build-and-prove frame (tens of kilobytes at `opt-level = 0`) travels all the
//! way up to whatever owns the stack. libtest gives a test thread 2 MiB, and a
//! debug-build deploy plus one call already needs most of that.
//!
//! The futures here are built and dropped, never polled, so this needs no
//! devnet.

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/analyzed-ir.sexp");
}

use std::future::IntoFuture;

use midnight_contract::interpreter::Program;
use midnight_contract::runtime::NoWitnesses;
use midnight_contract::{Contract, ShieldedInputs};
use midnight_provider::{
    DustlessBuilder, HashOutput, MidnightProvider, ShieldedTokenType, UnshieldedTokenType,
};

/// Room for a builder to gain a field, far below the tens of kilobytes an
/// inlined build frame costs.
const MAX_FUTURE_BYTES: usize = 4096;

const ADDR: &str = "0200aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";

fn provider() -> &'static MidnightProvider {
    Box::leak(Box::new(
        MidnightProvider::new("ws://test", "http://test").unwrap(),
    ))
}

#[track_caller]
fn assert_small<F>(entry_point: &str, future: F) {
    let bytes = std::mem::size_of_val(&future);
    drop(future);
    assert!(
        bytes <= MAX_FUTURE_BYTES,
        "{entry_point} returns a {bytes}-byte future; box it so the caller's frame stays small"
    );
}

#[test]
fn contract_entry_points_return_small_futures() {
    let p = provider();

    assert_small(
        "DeployBuilder::send",
        midnight_contract::Contract::deploy(p)
            .with_zk_config("compiled")
            .send(),
    );
    assert_small(
        "generated DeployBuilder::send",
        counter::Contract::deploy(p)
            .with_zk_config("compiled")
            .send(),
    );
    assert_small(
        "generated DeployBuilder::into_future",
        counter::Contract::deploy(p)
            .with_zk_config("compiled")
            .into_future(),
    );

    let contract = counter::Contract::at(p, ADDR)
        .with_zk_config("compiled")
        .build();

    assert_small(
        "generated circuit call::into_future",
        contract.circuits().increment().into_future(),
    );
    assert_small(
        "generated circuit call::build",
        contract.circuits().increment().build(),
    );
    assert_small(
        "generated circuit call::without_dust",
        contract.circuits().increment().without_dust(),
    );
    assert_small(
        "ContractMaintenance::prepare",
        contract
            .maintenance()
            .remove_verifier_key("increment")
            .prepare(),
    );
}

/// The generated call builders box their own async blocks, so they stay small
/// whatever the methods beneath them do. These are those methods, sized as a
/// hand-written caller awaits them.
#[test]
fn the_base_contract_call_methods_return_small_futures() {
    let p = provider();
    let contract: Contract<&MidnightProvider> =
        Contract::at(p, ADDR).with_zk_config("compiled").build();

    let helpers = counter::Ledger::__helpers();
    let witnesses = counter::Ledger::__witnesses();
    let natives = counter::Ledger::__natives();
    let program = Program::new(&helpers, &witnesses, &natives);
    // Any circuit sizes the future the same way, and none of these are polled.
    let circuit = helpers
        .first()
        .expect("the counter artifact declares a circuit");

    assert_small(
        "Contract::call_with",
        contract.call_with(
            circuit,
            &program,
            "increment",
            &[],
            &NoWitnesses,
            &[],
            ShieldedInputs::default(),
        ),
    );
    assert_small(
        "Contract::build_call_with",
        contract.build_call_with(
            circuit,
            &program,
            "increment",
            &[],
            &NoWitnesses,
            &[],
            ShieldedInputs::default(),
            true,
        ),
    );
    assert_small(
        "Contract::call",
        contract.call(circuit, &program, "increment"),
    );
}

#[test]
fn provider_entry_points_return_small_futures() {
    let p = provider();

    assert_small("MidnightProvider::resync_wallet", p.resync_wallet());
    assert_small(
        "MidnightProvider::balance_transaction",
        p.balance_transaction(&[]),
    );
    assert_small(
        "ShieldedTransfer::build",
        p.transfer_shielded(ShieldedTokenType(HashOutput([0u8; 32])), 1, "recipient")
            .build(),
    );
    assert_small(
        "UnshieldedTransfer::build",
        p.transfer_unshielded(UnshieldedTokenType(HashOutput([0u8; 32])), 1, "recipient")
            .build(),
    );
    assert_small(
        "ShieldedSwap::build",
        p.shielded_swap(
            ShieldedTokenType(HashOutput([0u8; 32])),
            1,
            ShieldedTokenType(HashOutput([1u8; 32])),
            1,
        )
        .build(),
    );
    assert_small("DustRegistration::build", p.register_dust(None).build());
}
