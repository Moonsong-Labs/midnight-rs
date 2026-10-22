//! Struct instantiations that the bindings accept. Two instantiations that
//! give the same Rust struct share it, and a tree's element type is never
//! named. A `Maybe` of a vector of enum members encodes as the interpreter
//! reads it by the Compact layout, and the typed accessor reads back one that
//! the interpreter built.

compact_bindgen::contract!("../../../../tests/fixtures/compiled/struct-shapes/compiler/analyzed-ir.sexp");

use compact_bindgen::Vector;
use compact_bindgen::midnight_contract::runtime::Maybe;

fn main() {
    let mut simulator = Simulator::new(LedgerInitialState::default(), std::time::UNIX_EPOCH);

    let colors = Maybe {
        is_some: true,
        value: Vector([Color::Red, Color::Green]),
    };
    assert_eq!(simulator.second_color(colors).unwrap(), Color::Green);

    simulator.paint().unwrap();
    assert_eq!(
        simulator.ledger().colors().unwrap(),
        Maybe {
            is_some: true,
            value: Vector([Color::Green, Color::Red]),
        }
    );
}
