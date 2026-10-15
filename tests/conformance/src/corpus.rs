//! The layout of the conformance corpus: the case files in `cases/`, the
//! goldens that `ts-driver/driver.mjs` writes to `expected/`, and the compiled
//! fixtures in `fixtures/`.

use std::path::Path;

use serde_json::Value as Json;

/// The root directory of the corpus.
pub fn corpus_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The JSON document in the file at `path`.
///
/// # Panics
///
/// When the file cannot be read or is not JSON.
pub fn read_json(path: &Path) -> Json {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// The golden that the canonical TS runtime recorded for `case` of `fixture`.
///
/// # Panics
///
/// When the golden is missing, cannot be read or is not JSON.
pub fn golden(fixture: &str, case: &str) -> Json {
    let path = corpus_dir()
        .join("expected")
        .join(fixture)
        .join(format!("{case}.json"));
    assert!(
        path.is_file(),
        "missing golden {}; run `make conformance-regen`",
        path.display()
    );
    read_json(&path)
}
