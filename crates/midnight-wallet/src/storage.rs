//! A wallet's snapshot directory: its layout, `metadata.json`, and
//! `pending.json`.
//!
//! What goes in the binary state files is per ledger generation (see the
//! per-ledger `snapshot` modules). The metadata names the ledger generation
//! that wrote them, because their tags are the same in every ledger
//! generation.

use std::path::{Path, PathBuf};

use midnight_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use midnight_types::LedgerVersion;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::WalletError;
use crate::chain_pin::ChainPin;
use midnight_types::TrackedUtxo;

const METADATA_FILE: &str = "metadata.json";
const PENDING_FILE: &str = "pending.json";

pub(crate) fn zswap_file(generation: u64) -> String {
    format!("zswap-{generation}.bin")
}

pub(crate) fn dust_wallet_file(generation: u64) -> String {
    format!("dust_wallet-{generation}.bin")
}

/// Public identity that names a wallet's on-disk storage directory.
///
/// Derived from the wallet's public (unshielded) address, not its seed: the
/// address uniquely identifies the wallet, is safe to put in a path, and can be
/// supplied by an external signer (e.g. a hardware wallet) that never releases
/// the seed. So the `storage` module never handles seed material, and the seed
/// stays purely a signing concern.
///
/// The invariant covers every file in the directory, not just its name. A
/// persisted record that needs to name a wallet names this id; most need no
/// wallet identity at all, because the directory already scopes them, which is
/// why `pending.json` stores none. The pending module's
/// `pending_json_contains_no_seed_material` test is what holds the line.
pub(crate) fn wallet_storage_id(address: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(address.as_bytes()))
}

/// Every snapshot and pending file written before the SDK carried ledger 9
/// came from a ledger 8 chain, and carries no `ledger_version`.
fn ledger_8() -> LedgerVersion {
    LedgerVersion::V8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredMetadata {
    /// Monotonically increasing version of the wallet snapshot. Each save
    /// writes new `zswap-{generation}.bin` / `dust_wallet-{generation}.bin`
    /// files and commits a new metadata.json referencing them, then deletes
    /// the previous generation's files. metadata.json is renamed atomically
    /// from a temp file, so a crash before/after that rename leaves the
    /// metadata pointing at a generation whose binary files exist on disk.
    #[serde(default)]
    pub generation: u64,
    /// The ledger generation whose types wrote the binary files.
    #[serde(default = "ledger_8")]
    pub ledger_version: LedgerVersion,
    pub zswap_event_id: i64,
    pub dust_event_id: i64,
    pub last_block_height: i64,
    pub last_tx_id: Option<i64>,
    /// The finalized block this snapshot last saw, so a resume can ask the
    /// node whether it is still on that chain. Absent in snapshots written
    /// before the pin existed, which simply skips the check.
    #[serde(default)]
    pub chain_pin: Option<ChainPin>,
    pub unshielded_utxos: Vec<StoredUtxo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredUtxo {
    owner: String,
    token_type: String,
    value: String,
    intent_hash: Option<String>,
    output_index: Option<i64>,
    // A snapshot written before these fields existed carries neither, and the
    // next sync refills them from the indexer.
    #[serde(default)]
    ctime: Option<i64>,
    #[serde(default)]
    registered_for_dust_generation: Option<bool>,
}

impl From<&TrackedUtxo> for StoredUtxo {
    fn from(u: &TrackedUtxo) -> Self {
        Self {
            owner: u.owner.clone(),
            token_type: u.token_type.clone(),
            value: u.value.to_string(),
            intent_hash: u.intent_hash.clone(),
            output_index: u.output_index,
            ctime: u.ctime,
            registered_for_dust_generation: u.registered_for_dust_generation,
        }
    }
}

impl TryFrom<StoredUtxo> for TrackedUtxo {
    type Error = WalletError;

    fn try_from(u: StoredUtxo) -> Result<Self, Self::Error> {
        let value: u128 = u.value.parse().map_err(|e| {
            WalletError::Storage(format!(
                "failed to parse stored UTXO value '{}': {e}",
                u.value
            ))
        })?;
        Ok(Self {
            owner: u.owner,
            token_type: u.token_type,
            value,
            intent_hash: u.intent_hash,
            output_index: u.output_index,
            ctime: u.ctime,
            registered_for_dust_generation: u.registered_for_dust_generation,
        })
    }
}

/// A wallet's storage directory, keyed on a public `wallet_id` (see
/// [`wallet_storage_id`]) rather than the seed. The directory name
/// is the identity, so nothing secret, and nothing else, needs to be persisted
/// to tell one wallet's snapshot from another's.
pub(crate) fn storage_dir(base: &Path, network: &str, wallet_id: &str) -> PathBuf {
    base.join(network).join(wallet_id)
}

/// Create the wallet's storage directory, readable only by its owner on unix.
///
/// The tree holds the wallet's UTXO set, spend history and reserved
/// nullifiers. None of that is secret in the key sense, but it is a full
/// picture of the wallet's activity and there is no reason for another local
/// user to have it. `mode` applies only to directories this call creates, so
/// an existing one is narrowed explicitly.
///
/// Only the wallet's own directory is narrowed. An ancestor an earlier version
/// or another tool created keeps its mode, so the set of wallet ids under a
/// network may stay listable. Permissions are unchanged on other platforms.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// Write a file readable only by its owner on unix. See
/// [`create_private_dir`] for why. Callers write to a temporary path and
/// rename, and rename preserves the mode, so setting it here covers the final
/// file too.
///
/// Permissions are unchanged on other platforms.
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        // `mode` applies only when this call creates the file, so a path left
        // wider by an earlier version is still wide here. Narrow it before the
        // contents land rather than after, or they are briefly readable.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        file.write_all(contents)?;
        Ok(())
    }
    #[cfg(not(unix))]
    std::fs::write(path, contents)
}

pub(crate) fn tagged_to_file<
    T: midnight_helpers::midnight_serialize::Serializable
        + midnight_helpers::midnight_serialize::Tagged,
>(
    dir: &Path,
    filename: &str,
    value: &T,
) -> Result<(), WalletError> {
    let path = dir.join(filename);
    let tmp = dir.join(format!("{filename}.tmp"));
    let mut buf = Vec::new();
    tagged_serialize(value, &mut buf)
        .map_err(|e| WalletError::Storage(format!("serialize {filename}: {e}")))?;
    write_private(&tmp, &buf)
        .map_err(|e| WalletError::Storage(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| WalletError::Storage(format!("rename {filename}: {e}")))?;
    Ok(())
}

pub(crate) fn tagged_from_file<
    T: midnight_helpers::midnight_serialize::Deserializable
        + midnight_helpers::midnight_serialize::Tagged,
>(
    dir: &Path,
    filename: &str,
) -> Result<T, WalletError> {
    let path = dir.join(filename);
    let bytes = std::fs::read(&path)
        .map_err(|e| WalletError::Storage(format!("read {}: {e}", path.display())))?;
    tagged_deserialize(&bytes[..])
        .map_err(|e| WalletError::Storage(format!("deserialize {filename}: {e}")))
}

/// Read a snapshot's `metadata.json`, or `None` when there is no snapshot.
pub(crate) fn read_metadata(dir: &Path) -> Result<Option<StoredMetadata>, WalletError> {
    let meta_path = dir.join(METADATA_FILE);
    if !meta_path.exists() {
        return Ok(None);
    }
    let meta_json = std::fs::read_to_string(&meta_path)
        .map_err(|e| WalletError::Storage(format!("read {}: {e}", meta_path.display())))?;
    serde_json::from_str(&meta_json)
        .map(Some)
        .map_err(|e| WalletError::Storage(format!("parse metadata: {e}")))
}

/// Read only the chain pin from a snapshot, without loading the wallet state
/// behind it. The caller has to check the pin before a resume commits to the
/// cache, and deserializing the zswap and dust binaries first would be work
/// thrown away when the chain turns out to be a different one.
pub(crate) fn load_chain_pin(
    base: &Path,
    network: &str,
    wallet_id: &str,
) -> Result<Option<ChainPin>, WalletError> {
    let dir = storage_dir(base, network, wallet_id);
    Ok(read_metadata(&dir)?.and_then(|m| m.chain_pin))
}

/// Where a wallet's snapshot lives, for an error that tells a reader what to
/// remove.
pub(crate) fn snapshot_path(base: &Path, network: &str, wallet_id: &str) -> PathBuf {
    storage_dir(base, network, wallet_id)
}

/// The ledger generation of a snapshot's state files, or `None` when there
/// is no snapshot.
pub(crate) fn load_ledger_version(
    base: &Path,
    network: &str,
    wallet_id: &str,
) -> Result<Option<LedgerVersion>, WalletError> {
    let dir = storage_dir(base, network, wallet_id);
    Ok(read_metadata(&dir)?.map(|m| m.ledger_version))
}

/// Commit one snapshot. `write_state` writes the state files for the
/// `generation` number it gets. Then this function commits `metadata` as the
/// one that references them, and removes the files of the number before.
pub(crate) fn save_snapshot(
    base: &Path,
    network: &str,
    wallet_id: &str,
    mut metadata: StoredMetadata,
    write_state: impl FnOnce(&Path, u64) -> Result<(), WalletError>,
) -> Result<(), WalletError> {
    let dir = storage_dir(base, network, wallet_id);
    create_private_dir(&dir)
        .map_err(|e| WalletError::Storage(format!("create dir {}: {e}", dir.display())))?;

    // Read the current metadata (if any) so we can bump the generation and
    // clean up the previous binary files only after the new metadata commit.
    let meta_path = dir.join(METADATA_FILE);
    let previous_generation: Option<u64> = std::fs::read_to_string(&meta_path)
        .ok()
        .and_then(|json| serde_json::from_str::<StoredMetadata>(&json).ok())
        .map(|m| m.generation);
    let generation = previous_generation.map(|g| g + 1).unwrap_or(1);

    // Write the new generation's binary files first. They are referenced only
    // once the metadata rename commits, so a crash here leaves orphan files
    // that the next save will clean up but does not break the load path.
    write_state(&dir, generation)?;

    metadata.generation = generation;
    let meta_tmp = dir.join("metadata.json.tmp");
    let meta_json = serde_json::to_string_pretty(&metadata)
        .map_err(|e| WalletError::Storage(format!("serialize metadata: {e}")))?;
    write_private(&meta_tmp, meta_json.as_bytes())
        .map_err(|e| WalletError::Storage(format!("write {}: {e}", meta_tmp.display())))?;
    // Atomic commit: from this point on, the wallet sees the new state.
    std::fs::rename(&meta_tmp, &meta_path)
        .map_err(|e| WalletError::Storage(format!("rename metadata: {e}")))?;

    // Best-effort: remove the previous generation's binary files. Failure
    // here is non-fatal (the next save will retry or overwrite).
    if let Some(prev) = previous_generation {
        let _ = std::fs::remove_file(dir.join(zswap_file(prev)));
        let _ = std::fs::remove_file(dir.join(dust_wallet_file(prev)));
    }

    info!(
        generation,
        ledger_version = %metadata.ledger_version,
        zswap_event_id = metadata.zswap_event_id,
        dust_event_id = metadata.dust_event_id,
        path = %dir.display(),
        "saved wallet state to disk"
    );

    Ok(())
}

/// On-disk representation of a wallet's pending reservations. `DustSpend` and
/// `Sp<DustLocalState<D>, D>` are both Tagged + Serializable, so we
/// hex-encode their `tagged_serialize` bytes to round-trip through JSON
/// without dragging the tagged-codec into the schema.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPending {
    /// The ledger generation whose types encoded the hex fields.
    #[serde(default = "ledger_8")]
    pub ledger_version: LedgerVersion,
    #[serde(default)]
    pub dust: Vec<StoredPendingDustBatch>,
    #[serde(default)]
    pub unshielded: Vec<StoredPendingUnshielded>,
    #[serde(default)]
    pub shielded: Vec<StoredPendingShielded>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPendingDustBatch {
    /// Tagged-serialized `Vec<DustSpend<ProofPreimageMarker, DefaultDB>>`, hex.
    pub spends_hex: String,
    /// Tagged-serialized `Sp<DustLocalState<DefaultDB>, DefaultDB>`, hex.
    pub updated_state_hex: String,
    /// `Timestamp::to_secs()` value.
    pub reserved_at_secs: u64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPendingUnshielded {
    pub intent_hash: String,
    pub output_index: u32,
    pub reserved_at_secs: u64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPendingShielded {
    /// Tagged-serialized `Nullifier`, hex.
    pub nullifier_hex: String,
    pub reserved_at_secs: u64,
}

impl StoredPending {
    fn is_empty(&self) -> bool {
        self.dust.is_empty() && self.unshielded.is_empty() && self.shielded.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Pending reservations (separate from confirmed state, see pending.rs).
// ---------------------------------------------------------------------------

/// Persist in-flight reservations to a per-wallet `pending.json`.
///
/// Confirmed-state files (`metadata.json`, `zswap-N.bin`, `dust_wallet-N.bin`)
/// never carry pending entries; `pending.json` is overwritten in place via
/// atomic rename. If `pending` is empty and a previous file exists, this
/// removes the file rather than writing an empty record, so the on-disk
/// surface stays clean.
pub(crate) fn save_pending(
    base: &Path,
    network: &str,
    wallet_id: &str,
    pending: &StoredPending,
) -> Result<(), WalletError> {
    let dir = storage_dir(base, network, wallet_id);
    create_private_dir(&dir)
        .map_err(|e| WalletError::Storage(format!("create dir {}: {e}", dir.display())))?;

    let path = dir.join(PENDING_FILE);

    if pending.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(WalletError::Storage(format!(
                    "remove empty pending file {}: {e}",
                    path.display()
                )));
            }
        }
        return Ok(());
    }

    let json = serde_json::to_string(pending)
        .map_err(|e| WalletError::Storage(format!("serialize pending: {e}")))?;

    let tmp = dir.join(format!("{PENDING_FILE}.tmp"));
    write_private(&tmp, json.as_bytes())
        .map_err(|e| WalletError::Storage(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| WalletError::Storage(format!("rename {PENDING_FILE}: {e}")))?;

    info!(path = %path.display(), "saved pending reservations");
    Ok(())
}

/// Load pending reservations if a `pending.json` exists. Returns `Ok(None)`
/// when the file is absent (the common case for a fresh wallet).
pub(crate) fn load_pending(
    base: &Path,
    network: &str,
    wallet_id: &str,
) -> Result<Option<StoredPending>, WalletError> {
    let dir = storage_dir(base, network, wallet_id);
    let path = dir.join(PENDING_FILE);

    if !path.exists() {
        return Ok(None);
    }

    let json = std::fs::read_to_string(&path)
        .map_err(|e| WalletError::Storage(format!("read {}: {e}", path.display())))?;
    let stored: StoredPending = serde_json::from_str(&json)
        .map_err(|e| WalletError::Storage(format!("parse pending: {e}")))?;

    info!(path = %path.display(), "loaded pending reservations");
    Ok(Some(stored))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A non-empty set, since saving an empty one removes the file.
    fn some_pending() -> StoredPending {
        StoredPending {
            ledger_version: LedgerVersion::V9,
            dust: Vec::new(),
            unshielded: vec![StoredPendingUnshielded {
                intent_hash: "abcd".to_string(),
                output_index: 0,
                reserved_at_secs: 100,
            }],
            shielded: Vec::new(),
        }
    }

    /// A snapshot written before the chain pin existed has no such key, and
    /// it has to keep loading: the alternative is every persisted wallet
    /// refusing to resume after an upgrade.
    #[test]
    fn a_snapshot_without_a_chain_pin_still_loads() {
        let json = r#"{
            "generation": 3,
            "zswap_event_id": 178,
            "dust_event_id": 179,
            "last_block_height": 0,
            "last_tx_id": 9,
            "unshielded_utxos": []
        }"#;
        let metadata: StoredMetadata = serde_json::from_str(json).expect("parse");
        assert_eq!(metadata.chain_pin, None, "an absent pin skips the check");
        assert_eq!(metadata.zswap_event_id, 178, "the rest still parses");
    }

    /// The pin's field names are the on-disk contract. A rename would not
    /// fail to compile: `#[serde(default)]` turns an unrecognised field into
    /// `None`, so every stored pin would quietly stop guarding anything.
    #[test]
    fn a_snapshot_reads_the_chain_pin_back_by_name() {
        let json = r#"{
            "generation": 1,
            "zswap_event_id": 1,
            "dust_event_id": 1,
            "last_block_height": 0,
            "last_tx_id": null,
            "chain_pin": { "height": 633, "hash": "0xd886b98e" },
            "unshielded_utxos": []
        }"#;
        let pin = serde_json::from_str::<StoredMetadata>(json)
            .expect("parse")
            .chain_pin
            .expect("the pin parses");
        assert_eq!(pin.height, 633);
        assert_eq!(pin.hash, "0xd886b98e");
    }

    /// The field's name and its default are the on-disk contract. A default
    /// of ledger 9 would decode every snapshot an earlier build wrote with
    /// the wrong ledger generation's types.
    #[test]
    fn a_snapshot_names_the_ledger_that_wrote_it() {
        let written_before = r#"{
            "generation": 1,
            "zswap_event_id": 1,
            "dust_event_id": 1,
            "last_block_height": 0,
            "last_tx_id": null,
            "unshielded_utxos": []
        }"#;
        let metadata: StoredMetadata = serde_json::from_str(written_before).expect("parse");
        assert_eq!(metadata.ledger_version, LedgerVersion::V8);

        let written_on_ledger_9 = written_before.replace(
            r#""generation": 1,"#,
            r#""generation": 1, "ledger_version": "V9","#,
        );
        let metadata: StoredMetadata = serde_json::from_str(&written_on_ledger_9).expect("parse");
        assert_eq!(metadata.ledger_version, LedgerVersion::V9);
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The snapshot tree records the wallet's UTXO set, spend history and
    /// reserved nullifiers. Written at the process umask it lands world
    /// readable, which hands every local user a full picture of the wallet.
    #[test]
    fn snapshot_tree_is_owner_only() {
        let base = tempfile::TempDir::new().unwrap();
        save_pending(base.path(), "undeployed", "testwallet", &some_pending()).unwrap();

        let dir = storage_dir(base.path(), "undeployed", "testwallet");
        assert_eq!(mode_of(&dir), 0o700, "wallet directory must be owner-only");
        assert_eq!(
            mode_of(&dir.join(PENDING_FILE)),
            0o600,
            "pending.json must be owner-only"
        );
    }

    /// A directory or file from an earlier version carries the old mode, and
    /// creating with a mode does not touch what already exists.
    #[test]
    fn existing_permissions_are_narrowed() {
        let base = tempfile::TempDir::new().unwrap();
        let dir = storage_dir(base.path(), "undeployed", "testwallet");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.join(PENDING_FILE);
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        save_pending(base.path(), "undeployed", "testwallet", &some_pending()).unwrap();

        assert_eq!(mode_of(&dir), 0o700);
        assert_eq!(mode_of(&path), 0o600);
    }
}
