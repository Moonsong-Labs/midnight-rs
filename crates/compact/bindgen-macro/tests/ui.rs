//! trybuild ui tests for the `contract!` macro.
//!
//! The fixture paths inside the ui test files are relative to trybuild's
//! scratch project (`$CARGO_TARGET_DIR/tests/trybuild/compact-bindgen-macro`,
//! four levels below the workspace root), because the macro resolves them
//! against the `CARGO_MANIFEST_DIR` of the crate being compiled.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass/*.rs");
    t.compile_fail("tests/ui/fail/*.rs");
}
