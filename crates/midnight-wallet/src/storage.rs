//! A wallet's snapshot directory: its layout, `metadata.json`, and
//! `pending.json`.
//!
//! The binary state files hold ledger 9 state (see `ledger_9::snapshot`). The
//! metadata names the ledger generation that wrote them, because their tags
//! are the same in ledger 8 and ledger 9.

use std::path::{Path, PathBuf};

use midnight_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::WalletError;
use crate::chain_pin::ChainPin;
use crate::generation::Generation;
use hex::FromHex;
use midnight_types::{HashOutput, TrackedUtxo, UnshieldedTokenType};

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
/// the seed. So the `storage` module never handles seed material.
///
/// No file in the directory holds the seed either. A persisted record that
/// needs to name a wallet names this id; most need no wallet identity at all,
/// because the directory already scopes them, which is why `pending.json`
/// stores none. The pending module's `pending_json_contains_no_seed_material`
/// test holds that line. `dust_wallet-N.bin` does hold the Dust secret key: the
/// helpers' `DustWallet` serializes it, and a resumed wallet needs it to apply
/// Dust events.
pub(crate) fn wallet_storage_id(address: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(address.as_bytes()))
}

/// A snapshot or pending file that carries no `ledger_version` came from a
/// ledger 8 chain.
fn ledger_8() -> Generation {
    Generation::V8
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
    pub ledger_version: Generation,
    pub zswap_event_id: i64,
    pub dust_event_id: i64,
    pub last_block_height: i64,
    pub last_tx_id: Option<i64>,
    /// The finalized block this snapshot last saw, so a resume can ask the
    /// node whether it is still on that chain. Absent in snapshots written
    /// before the pin existed. Ledger 8 wrote those, and the default lets
    /// them parse, so that [`read_metadata`] refuses them by name.
    #[serde(default)]
    pub chain_pin: Option<ChainPin>,
    pub unshielded_utxos: Vec<StoredUtxo>,
}

/// A [`TrackedUtxo`] as `metadata.json` holds it. The hashes stay hex
/// strings, because the serde form of `HashOutput` is a list of 32 numbers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredUtxo {
    owner: String,
    token_type: String,
    value: String,
    intent_hash: String,
    output_index: u32,
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
            token_type: hex::encode(u.token_type.0.0),
            value: u.value.to_string(),
            intent_hash: hex::encode(u.intent_hash.0),
            output_index: u.output_index,
            ctime: u.ctime,
            registered_for_dust_generation: u.registered_for_dust_generation,
        }
    }
}

impl TryFrom<StoredUtxo> for TrackedUtxo {
    type Error = WalletError;

    fn try_from(u: StoredUtxo) -> Result<Self, Self::Error> {
        let malformed = |field, value: &str, reason: &dyn std::fmt::Display| {
            WalletError::Storage(format!(
                "{METADATA_FILE}: stored UTXO {field} {value:?}: {reason}"
            ))
        };
        let token_type = <[u8; 32]>::from_hex(&u.token_type)
            .map_err(|e| malformed("token_type", &u.token_type, &e))?;
        let value = u
            .value
            .parse::<u128>()
            .map_err(|e| malformed("value", &u.value, &e))?;
        let intent_hash = <[u8; 32]>::from_hex(&u.intent_hash)
            .map_err(|e| malformed("intent_hash", &u.intent_hash, &e))?;
        Ok(Self {
            owner: u.owner,
            token_type: UnshieldedTokenType(HashOutput(token_type)),
            value,
            intent_hash: HashOutput(intent_hash),
            output_index: u.output_index,
            ctime: u.ctime,
            registered_for_dust_generation: u.registered_for_dust_generation,
        })
    }
}

/// A wallet's snapshot directory, keyed on a public `wallet_id` (see
/// [`wallet_storage_id`]) rather than the seed. The directory name
/// is the identity, so nothing secret, and nothing else, needs to be persisted
/// to tell one wallet's snapshot from another's.
pub(crate) fn snapshot_path(base: &Path, network: &str, wallet_id: &str) -> PathBuf {
    base.join(network).join(wallet_id)
}

/// Creates the wallet's storage directory and its missing ancestors. On Unix,
/// each directory that the call creates gets mode 0700, and an existing
/// directory keeps its mode.
///
/// The tree holds the key that spends the wallet's Dust, in
/// `dust_wallet-N.bin`. It also holds the wallet's UTXO set, spend history and
/// reserved nullifiers, a full picture of the wallet's activity. No other local
/// user has a reason to read either.
fn create_private_dir(dir: &Path) -> Result<(), WalletError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .map_err(|e| WalletError::Storage(format!("create dir {}: {e}", dir.display())))
}

fn write_atomic(dir: &Path, filename: &str, contents: &[u8]) -> Result<(), WalletError> {
    let tmp = dir.join(format!("{filename}.tmp"));
    create_owner_only_file(&tmp, contents)
        .map_err(|e| WalletError::Storage(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, dir.join(filename))
        .map_err(|e| WalletError::Storage(format!("rename {filename}: {e}")))?;
    sync_dir(dir).map_err(|e| WalletError::Storage(format!("sync {}: {e}", dir.display())))
}

/// Writes `contents` to a new file at `path` and syncs it to disk. On Unix, the
/// file gets mode 0600.
///
/// The call removes an existing file at `path`, and the create fails if one
/// appears again. So the contents never go into a file or a symbolic link that
/// another user put there.
fn create_owner_only_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Syncs the entries of `dir` to disk, so a rename or a removal in `dir`
/// persists after a power loss. Other platforms skip it, because
/// `File::open` fails on a directory on Windows.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, WalletError> {
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(path)
        .map_err(|e| WalletError::Storage(format!("read {}: {e}", path.display())))?;
    serde_json::from_str(&json)
        .map(Some)
        .map_err(|e| WalletError::Storage(format!("parse {}: {e}", path.display())))
}

pub(crate) fn tagged_to_file<
    T: midnight_helpers::midnight_serialize::Serializable
        + midnight_helpers::midnight_serialize::Tagged,
>(
    dir: &Path,
    filename: &str,
    value: &T,
) -> Result<(), WalletError> {
    let mut buf = Vec::new();
    tagged_serialize(value, &mut buf)
        .map_err(|e| WalletError::Storage(format!("serialize {filename}: {e}")))?;
    write_atomic(dir, filename, &buf)
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

/// `value`'s tagged serialization, hex-encoded. `what` names it in an error.
pub(crate) fn tagged_to_hex<
    T: midnight_helpers::midnight_serialize::Serializable
        + midnight_helpers::midnight_serialize::Tagged,
>(
    value: &T,
    what: &str,
) -> Result<String, WalletError> {
    let mut buf = Vec::new();
    tagged_serialize(value, &mut buf)
        .map_err(|e| WalletError::Storage(format!("serialize {what}: {e}")))?;
    Ok(hex::encode(buf))
}

/// The value that [`tagged_to_hex`] encoded as `encoded`.
pub(crate) fn tagged_from_hex<
    T: midnight_helpers::midnight_serialize::Deserializable
        + midnight_helpers::midnight_serialize::Tagged,
>(
    encoded: &str,
    what: &str,
) -> Result<T, WalletError> {
    let bytes =
        hex::decode(encoded).map_err(|e| WalletError::Storage(format!("decode {what}: {e}")))?;
    tagged_deserialize(&bytes[..])
        .map_err(|e| WalletError::Storage(format!("deserialize {what}: {e}")))
}

/// Read a snapshot's `metadata.json`, or `None` when there is no snapshot.
///
/// # Errors
///
/// [`WalletError::Ledger8Snapshot`] when ledger 8 wrote the snapshot. Its
/// state files carry the same tags as those of ledger 9, so no later read
/// refuses them.
pub(crate) fn read_metadata(dir: &Path) -> Result<Option<StoredMetadata>, WalletError> {
    let metadata: Option<StoredMetadata> = read_json(&dir.join(METADATA_FILE))?;
    if metadata
        .as_ref()
        .is_some_and(|m| m.ledger_version == Generation::V8)
    {
        return Err(WalletError::Ledger8Snapshot {
            path: dir.display().to_string(),
        });
    }
    Ok(metadata)
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
    let dir = snapshot_path(base, network, wallet_id);
    Ok(read_metadata(&dir)?.and_then(|m| m.chain_pin))
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
    let dir = snapshot_path(base, network, wallet_id);
    create_private_dir(&dir)?;

    // Read the current metadata (if any) so we can bump the generation and
    // clean up the previous binary files only after the new metadata commit.
    // Read past the ledger 8 refusal: a save replaces a ledger 8 snapshot too.
    let previous_generation = read_json::<StoredMetadata>(&dir.join(METADATA_FILE))
        .ok()
        .flatten()
        .map(|m| m.generation);
    let generation = previous_generation.map(|g| g + 1).unwrap_or(1);

    // Write the new generation's binary files first. They are referenced only
    // once the metadata rename commits, so a crash here leaves orphan files
    // that the next save will clean up but does not break the load path.
    write_state(&dir, generation)?;

    metadata.generation = generation;
    let meta_json = serde_json::to_string_pretty(&metadata)
        .map_err(|e| WalletError::Storage(format!("serialize metadata: {e}")))?;
    // Atomic commit: from this point on, the wallet sees the new state.
    write_atomic(&dir, METADATA_FILE, meta_json.as_bytes())?;

    // Best-effort: a failed removal, or a save that fails after the metadata
    // rename, leaves the previous generation on disk, and no later save
    // removes it.
    if let Some(prev) = previous_generation {
        let _ = std::fs::remove_file(dir.join(zswap_file(prev)));
        let _ = std::fs::remove_file(dir.join(dust_wallet_file(prev)));
    }

    info!(
        generation,
        zswap_event_id = metadata.zswap_event_id,
        dust_event_id = metadata.dust_event_id,
        path = %dir.display(),
        "saved wallet state to disk"
    );

    Ok(())
}

/// On-disk representation of a wallet's pending reservations. The hex fields
/// hold the `tagged_serialize` bytes of the Dust nullifiers and the shielded
/// nullifiers, so the JSON schema stays free of the tagged codec.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPending {
    /// The ledger generation whose types encoded the hex fields.
    #[serde(default = "ledger_8")]
    pub ledger_version: Generation,
    #[serde(default)]
    pub dust: Vec<StoredPendingDustBatch>,
    #[serde(default)]
    pub unshielded: Vec<StoredPendingUnshielded>,
    #[serde(default)]
    pub shielded: Vec<StoredPendingShielded>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPendingDustBatch {
    /// Tagged-serialized `Vec<DustNullifier>`, hex.
    pub nullifiers_hex: String,
    /// `Timestamp::to_secs()` value.
    pub reserved_at_secs: u64,
    /// `Timestamp::to_secs()` value.
    pub expires_at_secs: u64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPendingUnshielded {
    pub intent_hash: String,
    pub output_index: u32,
    pub reserved_at_secs: u64,
    pub expires_at_secs: u64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPendingShielded {
    /// Tagged-serialized `Nullifier`, hex.
    pub nullifier_hex: String,
    pub reserved_at_secs: u64,
    pub expires_at_secs: u64,
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
    let dir = snapshot_path(base, network, wallet_id);
    create_private_dir(&dir)?;

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

    write_atomic(&dir, PENDING_FILE, json.as_bytes())?;

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
    let path = snapshot_path(base, network, wallet_id).join(PENDING_FILE);
    let stored = read_json(&path)?;
    if stored.is_some() {
        info!(path = %path.display(), "loaded pending reservations");
    }
    Ok(stored)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A non-empty set, since saving an empty one removes the file.
    fn some_pending() -> StoredPending {
        StoredPending {
            ledger_version: Generation::V9,
            dust: Vec::new(),
            unshielded: vec![StoredPendingUnshielded {
                intent_hash: "abcd".to_string(),
                output_index: 0,
                reserved_at_secs: 100,
                expires_at_secs: 200,
            }],
            shielded: Vec::new(),
        }
    }

    /// A stored UTXO keeps its hashes as hex and its value as a decimal
    /// string, and the load parses that form. A write in another form, such
    /// as the ten characters of the `Display` of `HashOutput`, refuses the
    /// next resume.
    #[test]
    fn a_stored_utxo_keeps_its_form_on_disk() {
        let stored = serde_json::json!({
            "owner": "mn_addr_undeployed1owner",
            "token_type": "02".repeat(32),
            "value": u128::MAX.to_string(),
            "intent_hash": "ab".repeat(32),
            "output_index": 3,
            "ctime": 1_700_000_000,
            "registered_for_dust_generation": true,
        });
        let parsed: StoredUtxo = serde_json::from_value(stored.clone()).expect("parse");
        let utxo = TrackedUtxo::try_from(parsed).expect("a stored UTXO in this form loads");
        assert_eq!(utxo.token_type, UnshieldedTokenType(HashOutput([0x02; 32])));
        assert_eq!(utxo.value, u128::MAX);
        assert_eq!(utxo.intent_hash, HashOutput([0xab; 32]));
        assert_eq!(utxo.output_index, 3);

        let written = serde_json::to_value(StoredUtxo::from(&utxo)).expect("serialize");
        assert_eq!(written, stored, "a save writes the form a load reads");
    }

    /// The pin's field names are the on-disk contract. A rename would not
    /// fail to compile: `#[serde(default)]` turns an unrecognised field into
    /// `None`, so every stored pin would quietly stop guarding anything.
    #[test]
    fn a_snapshot_reads_the_chain_pin_back_by_name() {
        let json = r#"{
            "generation": 1,
            "ledger_version": "V9",
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

    /// A save over a ledger 8 snapshot replaces it. A save that read the old
    /// metadata through the ledger 8 refusal would start again at number 1
    /// and leave the old state files, Dust secret key included, on disk.
    #[test]
    fn a_save_over_a_ledger_8_snapshot_removes_its_state_files() {
        let base = tempfile::TempDir::new().unwrap();
        let dir = snapshot_path(base.path(), "undeployed", "testwallet");
        std::fs::create_dir_all(&dir).unwrap();
        let ledger_8_metadata = r#"{
            "generation": 4,
            "zswap_event_id": 1,
            "dust_event_id": 1,
            "last_block_height": 0,
            "last_tx_id": null,
            "unshielded_utxos": []
        }"#;
        std::fs::write(dir.join(METADATA_FILE), ledger_8_metadata).unwrap();
        let old_files = [zswap_file(4), dust_wallet_file(4)];
        for file in &old_files {
            std::fs::write(dir.join(file), b"ledger 8").unwrap();
        }

        let mut metadata: StoredMetadata = serde_json::from_str(ledger_8_metadata).unwrap();
        metadata.ledger_version = Generation::V9;
        save_snapshot(base.path(), "undeployed", "testwallet", metadata, |_, _| {
            Ok(())
        })
        .unwrap();

        for file in &old_files {
            assert!(!dir.join(file).exists(), "{file} stays on disk");
        }
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The snapshot tree records the Dust secret key, and the wallet's UTXO
    /// set, spend history and reserved nullifiers. Written at the process umask
    /// it lands world readable, which hands every local user the key and a full
    /// picture of the wallet.
    #[test]
    fn snapshot_tree_is_owner_only() {
        let base = tempfile::TempDir::new().unwrap();
        save_pending(base.path(), "undeployed", "testwallet", &some_pending()).unwrap();

        let dir = snapshot_path(base.path(), "undeployed", "testwallet");
        assert_eq!(mode_of(&dir), 0o700, "wallet directory must be owner-only");
        assert_eq!(
            mode_of(&dir.join(PENDING_FILE)),
            0o600,
            "pending.json must be owner-only"
        );
    }

    #[test]
    fn a_save_does_not_follow_a_planted_link() {
        let base = tempfile::TempDir::new().unwrap();
        let dir = snapshot_path(base.path(), "undeployed", "testwallet");
        std::fs::create_dir_all(&dir).unwrap();
        let target = base.path().join("target");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, dir.join(format!("{PENDING_FILE}.tmp"))).unwrap();

        save_pending(base.path(), "undeployed", "testwallet", &some_pending()).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }
}
