use midnight_private_state::{
    ConflictStrategy, EncryptedExport, ExportOptions, FsPrivateStateProvider, ImportOptions,
    PrivateStateError, PrivateStateProvider,
};
use tempfile::TempDir;

const PASSWORD: &str = "a-sufficiently-long-password";
const ADDR_A: &str = "0200aabbccdd";
const ADDR_B: &str = "0200eeff0011";

fn provider() -> (TempDir, FsPrivateStateProvider) {
    let dir = TempDir::new().unwrap();
    let provider = FsPrivateStateProvider::new(dir.path());
    (dir, provider)
}

#[tokio::test]
async fn private_state_set_get_remove() {
    let (_dir, p) = provider();

    assert_eq!(p.get(ADDR_A).await.unwrap(), None);

    p.set(ADDR_A, b"state-bytes").await.unwrap();
    assert_eq!(
        p.get(ADDR_A).await.unwrap().as_deref(),
        Some(&b"state-bytes"[..])
    );

    // A different contract address is a distinct entry.
    assert_eq!(p.get(ADDR_B).await.unwrap(), None);

    p.set(ADDR_A, b"updated").await.unwrap();
    assert_eq!(
        p.get(ADDR_A).await.unwrap().as_deref(),
        Some(&b"updated"[..])
    );

    p.remove(ADDR_A).await.unwrap();
    assert_eq!(p.get(ADDR_A).await.unwrap(), None);
    // Removing again is a no-op.
    p.remove(ADDR_A).await.unwrap();
}

#[tokio::test]
async fn clear_removes_all_states() {
    let (_dir, p) = provider();
    p.set(ADDR_A, b"1").await.unwrap();
    p.set(ADDR_B, b"2").await.unwrap();
    p.clear().await.unwrap();
    assert_eq!(p.get(ADDR_A).await.unwrap(), None);
    assert_eq!(p.get(ADDR_B).await.unwrap(), None);
    // Clear on an empty store is fine.
    p.clear().await.unwrap();
}

#[tokio::test]
async fn signing_keys_set_get_remove_clear() {
    let (_dir, p) = provider();
    assert_eq!(p.get_signing_key(ADDR_A).await.unwrap(), None);

    p.set_signing_key(ADDR_A, b"key-a").await.unwrap();
    p.set_signing_key(ADDR_B, b"key-b").await.unwrap();
    assert_eq!(
        p.get_signing_key(ADDR_A).await.unwrap().as_deref(),
        Some(&b"key-a"[..])
    );
    assert_eq!(
        p.get_signing_key(ADDR_B).await.unwrap().as_deref(),
        Some(&b"key-b"[..])
    );

    p.remove_signing_key(ADDR_A).await.unwrap();
    assert_eq!(p.get_signing_key(ADDR_A).await.unwrap(), None);
    assert_eq!(
        p.get_signing_key(ADDR_B).await.unwrap().as_deref(),
        Some(&b"key-b"[..])
    );

    p.clear_signing_keys().await.unwrap();
    assert_eq!(p.get_signing_key(ADDR_B).await.unwrap(), None);
}

#[tokio::test]
async fn export_import_round_trip() {
    let (_src_dir, src) = provider();
    src.set(ADDR_A, b"42").await.unwrap();
    src.set(ADDR_B, &[0u8, 1, 2, 255]).await.unwrap();

    let export = src
        .export_private_states(&ExportOptions::new(PASSWORD))
        .await
        .unwrap();
    assert_eq!(export.format, "midnight-private-state-export");

    let (_dst_dir, dst) = provider();
    let result = dst
        .import_private_states(&export, &ImportOptions::new(PASSWORD))
        .await
        .unwrap();
    assert_eq!(result.imported, 2);
    assert_eq!(result.skipped, 0);
    assert_eq!(result.overwritten, 0);

    assert_eq!(dst.get(ADDR_A).await.unwrap().as_deref(), Some(&b"42"[..]));
    assert_eq!(
        dst.get(ADDR_B).await.unwrap().as_deref(),
        Some(&[0u8, 1, 2, 255][..])
    );
}

#[tokio::test]
async fn signing_keys_export_import_round_trip() {
    let (_src_dir, src) = provider();
    src.set_signing_key(ADDR_A, b"key-a").await.unwrap();

    let export = src
        .export_signing_keys(&ExportOptions::new(PASSWORD))
        .await
        .unwrap();
    assert_eq!(export.format, "midnight-signing-key-export");

    let (_dst_dir, dst) = provider();
    let result = dst
        .import_signing_keys(&export, &ImportOptions::new(PASSWORD))
        .await
        .unwrap();
    assert_eq!(result.imported, 1);
    assert_eq!(
        dst.get_signing_key(ADDR_A).await.unwrap().as_deref(),
        Some(&b"key-a"[..])
    );
}

#[tokio::test]
async fn import_with_wrong_password_fails() {
    let (_src_dir, src) = provider();
    src.set(ADDR_A, b"1").await.unwrap();
    let export = src
        .export_private_states(&ExportOptions::new(PASSWORD))
        .await
        .unwrap();

    let (_dst_dir, dst) = provider();
    let err = dst
        .import_private_states(&export, &ImportOptions::new("a-different-strong-password"))
        .await
        .unwrap_err();
    assert!(matches!(err, PrivateStateError::Decrypt));
}

#[tokio::test]
async fn export_password_too_short() {
    let (_dir, p) = provider();
    p.set(ADDR_A, b"1").await.unwrap();
    let err = p
        .export_private_states(&ExportOptions::new("short"))
        .await
        .unwrap_err();
    assert!(matches!(err, PrivateStateError::PasswordTooShort));
}

#[tokio::test]
async fn export_too_many_entries() {
    let (_dir, p) = provider();
    p.set(ADDR_A, b"1").await.unwrap();
    p.set(ADDR_B, b"2").await.unwrap();
    let err = p
        .export_private_states(&ExportOptions::new(PASSWORD).with_max_entries(1))
        .await
        .unwrap_err();
    assert!(matches!(err, PrivateStateError::TooManyEntries));
}

#[tokio::test]
async fn import_conflict_error_strategy() {
    let (_src_dir, src) = provider();
    src.set(ADDR_A, b"new").await.unwrap();
    let export = src
        .export_private_states(&ExportOptions::new(PASSWORD))
        .await
        .unwrap();

    let (_dst_dir, dst) = provider();
    dst.set(ADDR_A, b"existing").await.unwrap();

    let err = dst
        .import_private_states(&export, &ImportOptions::new(PASSWORD))
        .await
        .unwrap_err();
    assert!(matches!(err, PrivateStateError::ImportConflict(_)));
    // The existing value is untouched (detect-before-mutate).
    assert_eq!(
        dst.get(ADDR_A).await.unwrap().as_deref(),
        Some(&b"existing"[..])
    );
}

#[tokio::test]
async fn import_conflict_skip_strategy() {
    let (_src_dir, src) = provider();
    src.set(ADDR_A, b"new").await.unwrap();
    src.set(ADDR_B, b"fresh").await.unwrap();
    let export = src
        .export_private_states(&ExportOptions::new(PASSWORD))
        .await
        .unwrap();

    let (_dst_dir, dst) = provider();
    dst.set(ADDR_A, b"existing").await.unwrap();

    let result = dst
        .import_private_states(
            &export,
            &ImportOptions::new(PASSWORD).with_conflict(ConflictStrategy::Skip),
        )
        .await
        .unwrap();
    assert_eq!(result.imported, 1);
    assert_eq!(result.skipped, 1);
    assert_eq!(result.overwritten, 0);
    // Conflicting entry kept; non-conflicting entry added.
    assert_eq!(
        dst.get(ADDR_A).await.unwrap().as_deref(),
        Some(&b"existing"[..])
    );
    assert_eq!(
        dst.get(ADDR_B).await.unwrap().as_deref(),
        Some(&b"fresh"[..])
    );
}

#[tokio::test]
async fn import_conflict_overwrite_strategy() {
    let (_src_dir, src) = provider();
    src.set(ADDR_A, b"new").await.unwrap();
    let export = src
        .export_private_states(&ExportOptions::new(PASSWORD))
        .await
        .unwrap();

    let (_dst_dir, dst) = provider();
    dst.set(ADDR_A, b"existing").await.unwrap();

    let result = dst
        .import_private_states(
            &export,
            &ImportOptions::new(PASSWORD).with_conflict(ConflictStrategy::Overwrite),
        )
        .await
        .unwrap();
    assert_eq!(result.imported, 0);
    assert_eq!(result.overwritten, 1);
    assert_eq!(dst.get(ADDR_A).await.unwrap().as_deref(), Some(&b"new"[..]));
}

#[tokio::test]
async fn import_rejects_format_mismatch() {
    let (_src_dir, src) = provider();
    src.set_signing_key(ADDR_A, b"key").await.unwrap();
    let key_export = src
        .export_signing_keys(&ExportOptions::new(PASSWORD))
        .await
        .unwrap();

    let (_dst_dir, dst) = provider();
    // Importing a signing-key export as private states must be rejected.
    let err = dst
        .import_private_states(&key_export, &ImportOptions::new(PASSWORD))
        .await
        .unwrap_err();
    assert!(matches!(err, PrivateStateError::InvalidFormat(_)));
}

#[tokio::test]
async fn import_rejects_format_retag() {
    // A private-state export retagged as a signing-key export decrypts cleanly
    // (the envelope binds the salt, not the format), but the inner payload
    // shape doesn't match `SigningKeyPayload` — so the import fails with
    // `InvalidFormat` rather than silently writing private-state bytes into
    // the signing-key store.
    let (_src_dir, src) = provider();
    src.set(ADDR_A, b"secret").await.unwrap();
    let states_export = src
        .export_private_states(&ExportOptions::new(PASSWORD))
        .await
        .unwrap();

    let retagged = EncryptedExport {
        format: "midnight-signing-key-export".to_string(),
        salt: states_export.salt.clone(),
        encrypted_payload: states_export.encrypted_payload.clone(),
    };

    let (_dst_dir, dst) = provider();
    let err = dst
        .import_signing_keys(&retagged, &ImportOptions::new(PASSWORD))
        .await
        .unwrap_err();
    assert!(matches!(err, PrivateStateError::InvalidFormat(_)));
    assert_eq!(dst.get_signing_key(ADDR_A).await.unwrap(), None);
}
