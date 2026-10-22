//! A Compact struct at two types gets one Rust struct per name, so the
//! bindings refuse it by name rather than give one of the two the wrong
//! fields.

compact_bindgen::contract!("../../../../tests/fixtures/compiled/pairs/compiler/analyzed-ir.sexp");

fn main() {}
