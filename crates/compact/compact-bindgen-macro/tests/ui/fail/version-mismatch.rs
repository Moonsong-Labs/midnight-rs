//! A compiler-version outside the supported major.minor families must fail
//! compilation naming the field, the found value, and the supported range.

compact_bindgen::contract!(
    "../../../../crates/compact/compact-bindgen-macro/tests/ui/fixtures/version-mismatch.json"
);

fn main() {}
