//! Cross-SDK interop: load a real export produced by midnight-js's
//! `level-private-state-provider` and verify our import path recovers the
//! original bytes. The fixtures under `interop/fixtures/` were generated
//! once by `interop/regenerate-fixtures.mjs`; regenerate that script (`pnpm
//! install --frozen-lockfile && node regenerate-fixtures.mjs`) if anything
//! about the wire format changes.
//!
//! The encrypted bytes in each fixture differ on every regeneration (random
//! salt + IV per call) — the tests intentionally assert on the *decrypted*
//! content, not on a stable byte-for-byte envelope.

use midnight_private_state::{
    EncryptedExport, FsPrivateStateProvider, ImportOptions, PrivateStateProvider,
};
use tempfile::TempDir;

/// Must match `regenerate-fixtures.mjs`.
const PASSWORD: &str = "correct-horse-battery-staple-x7Q";
const CONTRACT_ADDR: &str = "0200aabbccddeeff00112233445566778899aabbccddeeff00112233445566";
const STATE_BYTES: &[u8] = &[0x00, 0xFF, 0x80, b'h', b'i', 0x7F, 0x42, 0x00];

const STATES_FIXTURE: &str = include_str!("interop/fixtures/midnight-js-private-state-export.json");
const KEYS_FIXTURE: &str = include_str!("interop/fixtures/midnight-js-signing-key-export.json");

fn signing_key_bytes() -> Vec<u8> {
    (0..32u8)
        .map(|i| i.wrapping_mul(7).wrapping_add(11))
        .collect()
}

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

    assert_eq!(result.imported, 1, "expected 1 imported, got {result:?}",);
    assert_eq!(
        provider.get(CONTRACT_ADDR).await.unwrap().as_deref(),
        Some(STATE_BYTES),
        "decrypted state bytes don't match the fixture's input",
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
        Some(signing_key_bytes().as_slice()),
    );
}
