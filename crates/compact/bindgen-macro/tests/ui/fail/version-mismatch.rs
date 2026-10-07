//! A compiler-version outside the supported major.minor family must fail
//! compilation naming the field, the found value, and the supported family.

compact_bindgen::contract!(
    "../../../../crates/compact/bindgen-macro/tests/ui/fixtures/version-mismatch.sexp"
);

fn main() {}
