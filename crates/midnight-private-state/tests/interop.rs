//! Cross-SDK interop tests: take an `EncryptedExport` produced by this crate's
//! `FsPrivateStateProvider`, push it through midnight-js's
//! `levelPrivateStateProvider`, and verify the original bytes survive the round
//! trip.
//!
//! `#[ignore]`d by default because they need Node + pnpm + an `npm install` of
//! the midnight-js package. The canonical entry point is `make test-interop`
//! from the repo root.

use std::path::{Path, PathBuf};
use std::process::Command;

use midnight_private_state::{
    EncryptedExport, ExportOptions, FsPrivateStateProvider, ImportOptions, PrivateStateProvider,
};
use tempfile::TempDir;

/// 16+ characters, no `1234`/`abcd`-style sequences (midnight-js's
/// `validatePassword` rejects those).
const PASSWORD: &str = "correct-horse-battery-staple-x7Q";

/// Used both as the on-disk address in Rust and as the `setContractAddress`
/// scope on the midnight-js side; midnight-js stores entries under
/// `${contractAddress}:${PSI}`, so reusing the same string for both makes the
/// re-export's key strip cleanly back to ours.
const CONTRACT_ADDR: &str = "0200aabbccddeeff00112233445566778899aabbccddeeff00112233445566";

fn interop_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/interop")
}

/// Skip the test (with a stderr hint, not a panic) if Node or the JS deps
/// aren't available. We don't want CI without Node failing the suite — the
/// `--ignored` gate already keeps it out of the default `make ci` run.
fn require_interop_env() -> bool {
    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
        .not()
    {
        eprintln!("skipping interop test: `node` not found in PATH");
        return false;
    }
    if !interop_dir().join("node_modules").exists() {
        eprintln!(
            "skipping interop test: `{}/node_modules` missing — run `make test-interop` instead, \
             which installs deps first",
            interop_dir().display()
        );
        return false;
    }
    true
}

trait BoolExt {
    fn not(self) -> bool;
}
impl BoolExt for bool {
    fn not(self) -> bool {
        !self
    }
}

fn run_node(args: &[&str]) -> std::io::Result<bool> {
    let script = interop_dir().join("interop.mjs");
    let mut cmd = Command::new("node");
    cmd.arg(&script);
    cmd.args(args);
    cmd.current_dir(interop_dir());
    let status = cmd.status()?;
    Ok(status.success())
}

fn write_export(path: &Path, export: &EncryptedExport) {
    let bytes = serde_json::to_vec_pretty(export).expect("EncryptedExport serializes");
    std::fs::write(path, bytes).expect("write export to disk");
}

fn read_export(path: &Path) -> EncryptedExport {
    let bytes = std::fs::read(path).expect("read export from disk");
    serde_json::from_slice(&bytes).expect("export is valid JSON")
}

#[tokio::test]
#[ignore = "requires node + pnpm + npm registry access; run via `make test-interop`"]
async fn private_states_round_trip_through_midnight_js() {
    if !require_interop_env() {
        return;
    }

    // Pick a payload that isn't valid UTF-8 — the SuperJSON Uint8Array
    // envelope preserves raw bytes regardless of encoding, so this catches a
    // regression where we'd accidentally encode the value as a string.
    let payload: Vec<u8> = vec![0x00, 0xFF, 0x80, b'h', b'i', 0x7F, 0x42, 0x00];

    let dir = TempDir::new().unwrap();
    let src = FsPrivateStateProvider::new(dir.path().join("rust-src"));
    src.set(CONTRACT_ADDR, &payload).await.unwrap();

    // 1. Rust → file
    let rust_path = dir.path().join("rust-export.json");
    let export = src
        .export_private_states(&ExportOptions::new(PASSWORD))
        .await
        .expect("Rust export");
    write_export(&rust_path, &export);

    // 2. midnight-js: import that file, re-export → another file
    let mjs_path = dir.path().join("mjs-reexport.json");
    let ok = run_node(&[
        "round-trip-states",
        rust_path.to_str().unwrap(),
        mjs_path.to_str().unwrap(),
        PASSWORD,
        CONTRACT_ADDR,
    ])
    .expect("spawn node");
    assert!(ok, "midnight-js round-trip script exited non-zero");

    // 3. Rust: import what midnight-js re-exported
    let mjs_export = read_export(&mjs_path);
    let dst = FsPrivateStateProvider::new(dir.path().join("rust-dst"));
    let result = dst
        .import_private_states(&mjs_export, &ImportOptions::new(PASSWORD))
        .await
        .expect("Rust re-import");
    assert_eq!(result.imported, 1, "expected 1 imported, got {result:?}");

    // 4. Bytes survived intact through Rust → midnight-js → Rust.
    let got = dst.get(CONTRACT_ADDR).await.unwrap();
    assert_eq!(got.as_deref(), Some(payload.as_slice()));
}

#[tokio::test]
#[ignore = "requires node + pnpm + npm registry access; run via `make test-interop`"]
async fn signing_keys_round_trip_through_midnight_js() {
    if !require_interop_env() {
        return;
    }

    // A realistic 32-byte signing key.
    let key: Vec<u8> = (0..32u8)
        .map(|i| i.wrapping_mul(7).wrapping_add(11))
        .collect();

    let dir = TempDir::new().unwrap();
    let src = FsPrivateStateProvider::new(dir.path().join("rust-src"));
    src.set_signing_key(CONTRACT_ADDR, &key).await.unwrap();

    let rust_path = dir.path().join("rust-export.json");
    let export = src
        .export_signing_keys(&ExportOptions::new(PASSWORD))
        .await
        .expect("Rust export");
    write_export(&rust_path, &export);

    let mjs_path = dir.path().join("mjs-reexport.json");
    let ok = run_node(&[
        "round-trip-keys",
        rust_path.to_str().unwrap(),
        mjs_path.to_str().unwrap(),
        PASSWORD,
    ])
    .expect("spawn node");
    assert!(ok, "midnight-js round-trip script exited non-zero");

    let mjs_export = read_export(&mjs_path);
    let dst = FsPrivateStateProvider::new(dir.path().join("rust-dst"));
    let result = dst
        .import_signing_keys(&mjs_export, &ImportOptions::new(PASSWORD))
        .await
        .expect("Rust re-import");
    assert_eq!(result.imported, 1, "expected 1 imported, got {result:?}");

    assert_eq!(
        dst.get_signing_key(CONTRACT_ADDR).await.unwrap().as_deref(),
        Some(key.as_slice()),
    );
}
