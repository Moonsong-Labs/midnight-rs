//! User-defined types named like the runtime re-exports (`Value`, `Bytes`,
//! `StateValue`) next to a flat `contract!` invocation must compile: the
//! generated code's imports are scoped to a hidden module, so they neither
//! collide with the user items (E0255) nor get shadowed by them.

pub struct Value(pub u8);

pub struct Bytes(pub u8);

pub struct StateValue(pub u8);

compact_bindgen::contract!(
    "../../../../crates/midnight-contract/tests/fixtures/counter/compiler/analyzed-ir.sexp"
);

fn main() {
    // The generated bindings are visible at the call site and functional.
    let ledger = LedgerInitialState { round: 42 }.into_ledger();
    assert_eq!(ledger.round().unwrap(), 42);
}
