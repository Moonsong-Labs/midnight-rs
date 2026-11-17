//! A circuit result decodes through its type. The interpreter holds a vector or
//! a tuple as a tuple of values, an enum member as an integer, and a ledger
//! read as an aligned value.

compact_bindgen::contract!("../../../../tests/fixtures/compiled/results/compiler/analyzed-ir.sexp");

fn main() {
    let field = |n: u64| compact_bindgen::TransientFr::from(n);
    let mut simulator = Simulator::new(LedgerInitialState::default(), std::time::UNIX_EPOCH);

    assert_eq!(
        simulator.pair(field(2), field(3)).unwrap(),
        (field(5), compact_bindgen::Vector([field(2), field(3)]))
    );
    assert_eq!(simulator.close().unwrap(), Phase::Closed);
    assert_eq!(simulator.tick().unwrap(), 1);
    assert!(simulator.raise().unwrap());
}
