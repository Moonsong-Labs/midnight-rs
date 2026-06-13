//! A compiler-version outside the supported major.minor families must fail
//! compilation naming the field, the found value, and the supported range.

midnight_bindgen::contract!(
    "../../../../crates/midnight-bindgen-macro/tests/ui/fixtures/version-mismatch.json"
);

fn main() {}
