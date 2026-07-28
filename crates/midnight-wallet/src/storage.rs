use std::path::{Path, PathBuf};

use midnight_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use midnight_helpers::{DefaultDB, DustWallet, WalletState as ZswapLocalState};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::WalletError;
use crate::pending::{PendingReservations, StoredPending};
use crate::state::TrackedUtxo;

const METADATA_FILE: &str = "metadata.json";
const PENDING_FILE: &str = "pending.json";

fn zswap_file(generation: u64) -> String {
    format!("zswap-{generation}.bin")
}

fn dust_wallet_file(generation: u64) -> String {
    format!("dust_wallet-{generation}.bin")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMetadata {
    /// Monotonically increasing version of the wallet snapshot. Each save
    /// writes new `zswap-{generation}.bin` / `dust_wallet-{generation}.bin`
    /// files and commits a new metadata.json referencing them, then deletes
    /// the previous generation's files. metadata.json is renamed atomically
    /// from a temp file, so a crash before/after that rename leaves the
    /// metadata pointing at a generation whose binary files exist on disk.
    #[serde(default)]
    generation: u64,
    zswap_event_id: i64,
    dust_event_id: i64,
    last_block_height: i64,
    last_tx_id: Option<i64>,
    unshielded_utxos: Vec<StoredUtxo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredUtxo {
    owner: String,
    token_type: String,
    value: String,
    intent_hash: Option<String>,
    output_index: Option<i64>,
}

impl From<&TrackedUtxo> for StoredUtxo {
    fn from(u: &TrackedUtxo) -> Self {
        Self {
            owner: u.owner.clone(),
            token_type: u.token_type.clone(),
            value: u.value.to_string(),
            intent_hash: u.intent_hash.clone(),
            output_index: u.output_index,
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
        })
    }
}

/// A wallet's storage directory, keyed on a public `wallet_id` (see
/// [`crate::state::wallet_storage_id`]) rather than the seed. The directory name
/// is the identity, so nothing secret, and nothing else, needs to be persisted
/// to tell one wallet's snapshot from another's.
fn storage_dir(base: &Path, network: &str, wallet_id: &str) -> PathBuf {
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

fn tagged_to_file<
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

fn tagged_from_file<
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

pub(crate) struct LoadedState {
    pub zswap_state: ZswapLocalState<DefaultDB>,
    pub dust_wallet: DustWallet<DefaultDB>,
    pub zswap_event_id: i64,
    pub dust_event_id: i64,
    pub last_block_height: i64,
    pub last_tx_id: Option<i64>,
    pub unshielded_utxos: Vec<TrackedUtxo>,
}

pub(crate) fn load(
    base: &Path,
    network: &str,
    wallet_id: &str,
) -> Result<Option<LoadedState>, WalletError> {
    let dir = storage_dir(base, network, wallet_id);
    let meta_path = dir.join(METADATA_FILE);

    if !meta_path.exists() {
        return Ok(None);
    }

    let meta_json = std::fs::read_to_string(&meta_path)
        .map_err(|e| WalletError::Storage(format!("read {}: {e}", meta_path.display())))?;
    let metadata: StoredMetadata = serde_json::from_str(&meta_json)
        .map_err(|e| WalletError::Storage(format!("parse metadata: {e}")))?;

    // No identity check here: the directory name is derived from the wallet's
    // public id, so reaching a metadata file already means it is this wallet's.

    let zswap_state = tagged_from_file(&dir, &zswap_file(metadata.generation))?;
    let dust_wallet = tagged_from_file(&dir, &dust_wallet_file(metadata.generation))?;

    let unshielded_utxos: Vec<TrackedUtxo> = metadata
        .unshielded_utxos
        .into_iter()
        .map(TrackedUtxo::try_from)
        .collect::<Result<_, _>>()?;

    info!(
        zswap_event_id = metadata.zswap_event_id,
        dust_event_id = metadata.dust_event_id,
        unshielded_utxos = unshielded_utxos.len(),
        "loaded wallet state from disk"
    );

    Ok(Some(LoadedState {
        zswap_state,
        dust_wallet,
        zswap_event_id: metadata.zswap_event_id,
        dust_event_id: metadata.dust_event_id,
        last_block_height: metadata.last_block_height,
        last_tx_id: metadata.last_tx_id,
        unshielded_utxos,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn save(
    base: &Path,
    network: &str,
    wallet_id: &str,
    zswap_state: &ZswapLocalState<DefaultDB>,
    dust_wallet: &DustWallet<DefaultDB>,
    zswap_event_id: i64,
    dust_event_id: i64,
    last_block_height: i64,
    last_tx_id: Option<i64>,
    unshielded_utxos: &[TrackedUtxo],
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
    tagged_to_file(&dir, &zswap_file(generation), zswap_state)?;
    tagged_to_file(&dir, &dust_wallet_file(generation), dust_wallet)?;

    let metadata = StoredMetadata {
        generation,
        zswap_event_id,
        dust_event_id,
        last_block_height,
        last_tx_id,
        unshielded_utxos: unshielded_utxos.iter().map(StoredUtxo::from).collect(),
    };
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
        zswap_event_id,
        dust_event_id,
        path = %dir.display(),
        "saved wallet state to disk"
    );

    Ok(())
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
    pending: &PendingReservations,
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

    let stored = pending.to_stored()?;
    let json = serde_json::to_string(&stored)
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
) -> Result<Option<PendingReservations>, WalletError> {
    let dir = storage_dir(base, network, wallet_id);
    let path = dir.join(PENDING_FILE);

    if !path.exists() {
        return Ok(None);
    }

    let json = std::fs::read_to_string(&path)
        .map_err(|e| WalletError::Storage(format!("read {}: {e}", path.display())))?;
    let stored: StoredPending = serde_json::from_str(&json)
        .map_err(|e| WalletError::Storage(format!("parse pending: {e}")))?;

    let pending = PendingReservations::from_stored(stored)?;
    info!(path = %path.display(), "loaded pending reservations");
    Ok(Some(pending))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::pending::PendingReservations;
    use crate::transfer::SpentUtxoKey;
    use midnight_helpers::Timestamp;
    use std::os::unix::fs::PermissionsExt;

    /// A non-empty set, since saving an empty one removes the file.
    fn some_pending() -> PendingReservations {
        let mut p = PendingReservations::default();
        p.reserve(
            Vec::new(),
            vec![SpentUtxoKey {
                intent_hash: "abcd".to_string(),
                output_index: 0,
            }],
            Vec::new(),
            Timestamp::from_secs(100),
        );
        p
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
