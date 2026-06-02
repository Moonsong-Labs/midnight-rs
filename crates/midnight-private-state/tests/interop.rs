//! Cross-SDK interop: load real exports produced by midnight-js's
//! `level-private-state-provider` and verify our import path round-trips
//! their content. The fixtures under `interop/fixtures/` were produced by
//! `interop/regenerate-fixtures.mjs`; rerun it (`pnpm install
//! --frozen-lockfile && node regenerate-fixtures.mjs`) if anything about
//! the wire format changes.
//!
//! The encrypted bytes in each fixture differ on every regeneration
//! (random salt + IV per call) — the assertions intentionally target the
//! decrypted *content*, not stable byte-for-byte envelopes.

use midnight_private_state::{
    EncryptedExport, ExportOptions, FsPrivateStateProvider, ImportOptions, PrivateStateProvider,
};
use tempfile::TempDir;

/// Must match `regenerate-fixtures.mjs`.
const PASSWORD: &str = "correct-horse-battery-staple-x7Q";
const CONTRACT_ADDR: &str = "0200aabbccddeeff00112233445566778899aabbccddeeff00112233445566";

/// Arbitrary 32-byte signing key for the fixture; mirrored in
/// `regenerate-fixtures.mjs`.
const SIGNING_KEY_BYTES: [u8; 32] = [0x42; 32];

const STATES_FIXTURE: &str = include_str!("interop/fixtures/midnight-js-private-state-export.json");
const KEYS_FIXTURE: &str = include_str!("interop/fixtures/midnight-js-signing-key-export.json");

/// Imports a midnight-js export of a real typed private state — a contract
/// with two witness-backed `Map`s — and verifies the stored bytes are the
/// SuperJSON envelope midnight-js wrote, with both `Map` type tags and the
/// expected keys preserved. Then re-exports through our own path and asserts
/// the inner envelope matches exactly: byte-for-byte round-trip through our
/// SDK, with full fidelity for whatever typed shape midnight-js chose.
#[tokio::test]
async fn imports_midnight_js_private_state_export() {
    let export: EncryptedExport =
        serde_json::from_str(STATES_FIXTURE).expect("fixture parses as EncryptedExport");

    let dir = TempDir::new().unwrap();
    let provider = FsPrivateStateProvider::new(dir.path());
    let result = provider
        .import_private_states(&export, &ImportOptions::new(PASSWORD))
        .await
        .expect("import the midnight-js fixture");
    assert_eq!(result.imported, 1, "expected 1 imported, got {result:?}");

    // The stored bytes are the SuperJSON envelope midnight-js wrote for our
    // `{ votes: Map, deposits: Map }` value. Parse it and check the shape.
    let bytes = provider
        .get(CONTRACT_ADDR)
        .await
        .unwrap()
        .expect("imported state present");
    let envelope: serde_json::Value =
        serde_json::from_slice(&bytes).expect("imported bytes parse as JSON");

    // Both top-level fields are present.
    assert!(envelope["json"]["votes"].is_array());
    assert!(envelope["json"]["deposits"].is_array());

    // Each field is tagged as a `Map` in the SuperJSON meta. For per-field
    // type tags (no subtype) SuperJSON emits `["map"]`, not the nested
    // `[["typed-array", "Uint8Array"]]` shape used for root-level typed
    // arrays.
    assert_eq!(envelope["meta"]["values"]["votes"][0].as_str(), Some("map"));
    assert_eq!(
        envelope["meta"]["values"]["deposits"][0].as_str(),
        Some("map"),
    );

    // Spot-check the entries.
    let votes = envelope["json"]["votes"].as_array().unwrap();
    assert!(
        votes.iter().any(|pair| pair[0] == "alice" && pair[1] == 3),
        "votes preserves the (alice, 3) entry; got {votes:?}",
    );

    // Round-trip the bytes through our own export and back, and verify the
    // inner envelope string is unchanged. Re-import into a fresh provider so
    // the assertion is on the wire-level content, not the on-disk record.
    let re_export = provider
        .export_private_states(&ExportOptions::new(PASSWORD))
        .await
        .expect("Rust re-export");
    let dst_dir = TempDir::new().unwrap();
    let dst = FsPrivateStateProvider::new(dst_dir.path());
    dst.import_private_states(&re_export, &ImportOptions::new(PASSWORD))
        .await
        .expect("Rust re-import");
    let round_tripped = dst.get(CONTRACT_ADDR).await.unwrap().unwrap();
    assert_eq!(
        round_tripped, bytes,
        "envelope bytes must survive a Rust round-trip"
    );
}

#[tokio::test]
async fn imports_midnight_js_signing_key_export() {
    let export: EncryptedExport =
        serde_json::from_str(KEYS_FIXTURE).expect("fixture parses as EncryptedExport");

    let dir = TempDir::new().unwrap();
    let provider = FsPrivateStateProvider::new(dir.path());
    let result = provider
        .import_signing_keys(&export, &ImportOptions::new(PASSWORD))
        .await
        .expect("import the midnight-js fixture");

    assert_eq!(result.imported, 1);
    assert_eq!(
        provider
            .get_signing_key(CONTRACT_ADDR)
            .await
            .unwrap()
            .as_deref(),
        Some(SIGNING_KEY_BYTES.as_slice()),
    );
}
