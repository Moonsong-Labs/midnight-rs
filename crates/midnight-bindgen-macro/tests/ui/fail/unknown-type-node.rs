//! An unrecognized `type-name` must fail compilation naming the type and the
//! field that used it, instead of the old eprintln + `Vec<u8>` fallback.

midnight_bindgen::contract!(
    "../../../../crates/midnight-bindgen-macro/tests/ui/fixtures/unknown-type.json"
);

fn main() {}
