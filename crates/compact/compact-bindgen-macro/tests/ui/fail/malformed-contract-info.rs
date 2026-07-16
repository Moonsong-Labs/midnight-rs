//! A contract-info.json that does not match the schema must fail compilation
//! with the serde parse error, not generate broken code.

compact_bindgen::contract!(
    "../../../../crates/compact/compact-bindgen-macro/tests/ui/fixtures/malformed.json"
);

fn main() {}
