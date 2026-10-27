//! Shapes that the bindings decode through Rust tuples, which decode at most 11
//! elements. A struct of 12 fields reads back from a ledger cell with each
//! field in its place, and a one-element tuple of a struct encodes as its
//! element.

compact_bindgen::contract!("../../../../tests/fixtures/compiled/tuple-shapes/compiler/analyzed-ir.sexp");

use compact_bindgen::{Bytes, TransientFr};

fn main() {
    let record = Record {
        f01: TransientFr::from(1u64),
        f02: true,
        f03: 3,
        f04: Bytes([4; 4]),
        f05: 5,
        f06: TransientFr::from(6u64),
        f07: false,
        f08: 8,
        f09: Bytes([9; 2]),
        f10: 10,
        f11: TransientFr::from(11u64),
        f12: true,
    };
    let mut simulator = Simulator::new(LedgerInitialState::default(), std::time::UNIX_EPOCH);

    simulator.store((record.clone(),)).unwrap();
    assert_eq!(simulator.ledger().record().unwrap(), record);
}
