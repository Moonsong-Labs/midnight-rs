//! User-defined types named like the runtime re-exports (`Value`, `Bytes`,
//! `StateValue`) next to a flat `contract!` invocation must compile: the
//! generated code's imports are scoped to a hidden module, so they neither
//! collide with the user items (E0255) nor get shadowed by them.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Value(pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bytes(pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateValue(pub u8);

midnight_bindgen::contract!(
    "../../../../crates/midnight-contract/tests/fixtures/counter/compiler/contract-info.json"
);

fn main() {
    // The user types are intact and still refer to the local definitions.
    assert_eq!(Value(1), Value(1));
    assert_eq!(Bytes(2), Bytes(2));
    assert_eq!(StateValue(3), StateValue(3));

    // The generated bindings are visible at the call site and functional.
    let ledger = LedgerInitialState { round: 42 }.into_ledger();
    assert_eq!(ledger.round().unwrap(), 42);
}
