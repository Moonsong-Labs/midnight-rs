//! Per-contract private state example.
//!
//! The `PrivateStateProvider` is a durable, contract-scoped store for the
//! off-chain state that stateful witnesses read between calls, plus contract
//! maintenance signing keys. This example is fully local — it needs no node or
//! indexer — and shows:
//!
//!   1. storing and reading a contract's private state and signing key,
//!   2. a password-encrypted export, and
//!   3. importing that export into a fresh store (device migration / backup).
//!
//! Run with: `cargo run -p example-private-state`.

use midnight_provider::{
    ExportOptions, FsPrivateStateProvider, ImportOptions, PrivateStateId, PrivateStateProvider,
};

// A contract address (the hex string this SDK uses throughout) and a private
// state id under it. A single contract can hold several private states under
// different ids.
const CONTRACT: &str = "0200a1b2c3d4e5f6";
const PASSWORD: &str = "correct-horse-battery-staple";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Store under a temp directory so the example is repeatable. A real app
    // would use `FsPrivateStateProvider::with_default_dir()`
    // (`~/.midnight/private-state/`).
    let dir = std::env::temp_dir().join("midnight-private-state-example");
    let _ = std::fs::remove_dir_all(&dir);
    let store = FsPrivateStateProvider::new(&dir);

    let id = PrivateStateId::from("counter");

    // 1. Store some opaque private-state bytes and a maintenance signing key.
    //    The caller owns the encoding of its private-state type; here it's just
    //    a little-endian counter.
    store.set(CONTRACT, &id, &7u64.to_le_bytes()).await?;
    store
        .set_signing_key(CONTRACT, b"maintenance-key-bytes")
        .await?;

    let loaded = store.get(CONTRACT, &id).await?.expect("just stored");
    let counter = u64::from_le_bytes(loaded.try_into().unwrap());
    println!("loaded private state for {CONTRACT}/{id}: counter = {counter}");

    // 2. Export everything under a password. The envelope is AES-256-GCM
    //    encrypted with an Argon2id-derived key; signing keys are exported
    //    separately from private states.
    let states_export = store
        .export_private_states(&ExportOptions::new(PASSWORD))
        .await?;
    let keys_export = store
        .export_signing_keys(&ExportOptions::new(PASSWORD))
        .await?;
    println!(
        "exported (format = {}, salt = {}…)",
        states_export.format,
        &states_export.salt[..8]
    );

    // 3. Import into a fresh store, as if restoring on another device.
    let restore_dir = std::env::temp_dir().join("midnight-private-state-example-restore");
    let _ = std::fs::remove_dir_all(&restore_dir);
    let restored = FsPrivateStateProvider::new(&restore_dir);

    let r1 = restored
        .import_private_states(&states_export, &ImportOptions::new(PASSWORD))
        .await?;
    let r2 = restored
        .import_signing_keys(&keys_export, &ImportOptions::new(PASSWORD))
        .await?;
    println!(
        "restored {} private state(s) and {} signing key(s)",
        r1.imported, r2.imported
    );

    let restored_state = restored.get(CONTRACT, &id).await?.expect("imported");
    let restored_counter = u64::from_le_bytes(restored_state.try_into().unwrap());
    assert_eq!(restored_counter, counter);
    println!("restored counter matches: {restored_counter}");

    // A wrong password fails authentication rather than returning garbage.
    let wrong = restored
        .import_private_states(
            &states_export,
            &ImportOptions::new("not-the-password-at-all"),
        )
        .await;
    println!("import with wrong password -> {}", err_label(&wrong));

    Ok(())
}

fn err_label<T>(r: &Result<T, midnight_provider::PrivateStateError>) -> String {
    match r {
        Ok(_) => "unexpectedly succeeded".into(),
        Err(e) => e.to_string(),
    }
}
