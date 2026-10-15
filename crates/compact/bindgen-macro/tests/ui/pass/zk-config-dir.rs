//! An artifact at `compiler/analyzed-ir.sexp` gets `ZK_CONFIG_DIR`, the
//! absolute compactc output directory that holds it.

compact_bindgen::contract!(
    Counter,
    "../../../../crates/midnight-contract/tests/fixtures/counter/compiler/analyzed-ir.sexp"
);

fn main() {
    let dir = std::path::Path::new(counter::ZK_CONFIG_DIR);
    assert!(dir.is_absolute(), "{}", dir.display());
    assert!(
        dir.join("compiler/analyzed-ir.sexp").is_file(),
        "{}",
        dir.display()
    );
}
