//! A analyzed-ir.sexp that does not parse must fail compilation with
//! the reader's error, not generate broken code.

compact_bindgen::contract!(
    "../../../../crates/compact/bindgen-macro/tests/ui/fixtures/malformed.sexp"
);

fn main() {}
