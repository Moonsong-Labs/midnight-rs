//! Filesystem-backed [`PrivateStateProvider`].
//!
//! Layout (one store root per wallet, one directory per contract address,
//! one file per snapshot):
//!
//! ```text
//! <base>/
//!   <sha256(wallet_address)>/                        # the store root of one wallet
//!     states/
//!       <sha256(address)>/
//!         address.txt                                # plaintext address marker (for export)
//!         <020-padded-unix-nanos>-<extrinsic_hash_hex>.json
//!           { status, extrinsicHash, blockHeight?, blockHash?, dependsOn?, data: base64 }
//!     signing-keys/
//!       <sha256(address)>.json   { address, data: base64 }
//! ```
//!
//! On Unix, the store creates its directories 0700 and its files 0600. Each
//! write first narrows the store root to 0700, so the root also bars other
//! users from a file that an earlier version wrote at the process umask.
//!
//! Snapshot filenames are prefixed with a 020-padded unix-nanos timestamp
//! mostly for human inspection (sorting a directory listing gives an
//! append-time order). The journal head is found by walking the `dependsOn`
//! graph (the leaf snapshot is the one nothing else depends on), not by
//! filename, so `head` stays correct after `import_private_states` rewrites
//! filenames with new timestamps. `mark_failed` / `rollback_from` likewise
//! walk the graph to cascade through dependents.
//!
//! A write syncs a `.tmp` sibling to disk and then `rename`s it into place
//! (see `midnight_owner_only`), so a crash, or a power loss on a file system
//! that can sync, never leaves a half-written file. The wallet uses the same
//! discipline.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use crate::{
    ConflictStrategy, EncryptedExport, ExportOptions, FORMAT_KEYS, FORMAT_STATES, ImportOptions,
    ImportResult, PrivateStateError, PrivateStateProvider, Snapshot, SnapshotStatus, crypto,
};

const STATES_SUBDIR: &str = "states";
const KEYS_SUBDIR: &str = "signing-keys";
const ADDRESS_MARKER: &str = "address.txt";

/// Signing-key record (signing keys are a flat per-address slot; no
/// journaling). `deny_unknown_fields` rejects malformed records on import.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyRecord {
    address: String,
    #[serde(with = "crate::base64_bytes")]
    data: Vec<u8>,
}

/// One entry in a private-state export: the address it belongs to plus the
/// full [`Snapshot`]. The address is kept alongside so an import can recover
/// the per-address directory.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportEntry {
    address: String,
    snapshot: Snapshot,
}

/// Filesystem [`PrivateStateProvider`] for one wallet.
///
/// State lives under `<root>/states/` (one directory per address) and signing
/// keys under `<root>/signing-keys/`, plaintext at rest and owner-only on Unix.
/// [`Self::for_wallet`] puts the root at `<base>/<sha256(wallet_address)>`, and
/// [`Self::with_default_dir`] uses `~/.midnight/private-state/` as the base.
///
/// `forget_all`, `clear_signing_keys`, `export_private_states` and
/// `export_signing_keys` act on this one wallet's store. They do not touch the
/// store of another wallet under the same base.
#[derive(Debug, Clone)]
pub struct FsPrivateStateProvider {
    root: PathBuf,
    /// Per-address mutexes serializing the journal read-modify-write
    /// operations (`append_pending` / `confirm` / `mark_failed` /
    /// `rollback_from`). Without this, two concurrent `call_with`s on the
    /// same contract could both pass `append_pending`'s duplicate check and
    /// write sibling pending snapshots, branching the journal so `find_leaf`
    /// rejects every later read. Wrapped in `Arc` so cloned providers share
    /// the same lock map (and so the struct stays `Clone`).
    locks: Arc<StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

/// `~/.midnight/private-state/`, the base that holds one store per wallet.
fn default_dir() -> Option<PathBuf> {
    std::env::home_dir().map(|h| h.join(".midnight").join("private-state"))
}

/// The on-disk name of `s`: the hex of its sha256.
fn hashed_name(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Whether `base` holds a store from before the per-wallet layout, which no
/// store reads.
fn unscoped_store_exists(base: &Path) -> bool {
    [STATES_SUBDIR, KEYS_SUBDIR]
        .iter()
        .any(|subdir| base.join(subdir).exists())
}

impl FsPrivateStateProvider {
    /// A store rooted at `root`.
    ///
    /// The root belongs to one wallet. Every caller that opens it reads every
    /// journal and signing key under it. On Unix, each write sets `root`
    /// itself to mode 0700, so give the store a directory of its own. Use
    /// [`Self::for_wallet`] to give each wallet its own root under one base.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            locks: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    /// A store for one wallet under `base`. Two wallets never share a journal
    /// or a signing key.
    ///
    /// The root is `<base>/<sha256(wallet_address)>`. Give the wallet's
    /// unshielded address: its bech32 prefix names the network, so each
    /// network also gets its own store.
    pub fn for_wallet(base: impl AsRef<Path>, wallet_address: &str) -> Self {
        Self::new(base.as_ref().join(hashed_name(wallet_address)))
    }

    /// [`Self::for_wallet`] under `~/.midnight/private-state/`. `None` when no
    /// home directory is known.
    ///
    /// Logs a warning when that base still holds a store from before the
    /// per-wallet layout. That store is not read: move its `states` and
    /// `signing-keys` directories into the store root of the wallet that wrote
    /// them.
    pub fn with_default_dir(wallet_address: &str) -> Option<Self> {
        let base = default_dir()?;
        let store = Self::for_wallet(&base, wallet_address);
        if unscoped_store_exists(&base) {
            warn!(
                base = %base.display(),
                wallet_root = %store.root.display(),
                "private state under base belongs to no wallet and is not read; if this wallet \
                 wrote it, move its states and signing-keys directories into wallet_root"
            );
        }
        Some(store)
    }

    /// Acquire the per-address journal lock, creating it on first use.
    /// Held across a single read-modify-write so concurrent mutations of
    /// one address's journal can't interleave and branch it. Different
    /// addresses never contend.
    async fn lock_address(&self, address: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut map = self.locks.lock().expect("private-state lock map poisoned");
            map.entry(address.to_string()).or_default().clone()
        };
        lock.lock_owned().await
    }

    fn states_dir(&self) -> PathBuf {
        self.root.join(STATES_SUBDIR)
    }

    fn keys_dir(&self) -> PathBuf {
        self.root.join(KEYS_SUBDIR)
    }

    /// `<states>/<sha256(address)>/`, the per-address journal directory.
    fn address_dir(&self, address: &str) -> PathBuf {
        self.states_dir().join(hashed_name(address))
    }

    /// `<keys>/<sha256(address)>.json`, the flat signing-key file.
    fn key_path(&self, address: &str) -> PathBuf {
        self.keys_dir()
            .join(format!("{}.json", hashed_name(address)))
    }

    /// `<states>/<sha256(address)>/<nanos>-<extrinsic_hash>.json`, the file
    /// of a new journal snapshot. The module docs say what the timestamp is
    /// for.
    fn new_snapshot_path(&self, address: &str, extrinsic_hash: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        // 020-pad covers nanos through year ~2554; lexicographic sort matches
        // chronological order.
        self.address_dir(address)
            .join(format!("{nanos:020}-{extrinsic_hash}.json"))
    }

    /// Read every snapshot file under `<address>/`, oldest first.
    fn load_snapshots(&self, address: &str) -> Result<Vec<(PathBuf, Snapshot)>, PrivateStateError> {
        read_json_dir(&self.address_dir(address))
    }

    /// The journal head snapshot for `address`, picked from the `depends_on`
    /// graph rather than filename order so it survives an import that
    /// rewrites timestamps. A well-formed sequential journal has exactly one
    /// leaf; `find_leaf` surfaces `InvalidFormat` for cycle / branching so
    /// `head` and `head_extrinsic` don't silently build against an ambiguous
    /// baseline.
    fn head_snapshot(&self, address: &str) -> Result<Option<Snapshot>, PrivateStateError> {
        let snaps = self
            .load_snapshots(address)?
            .into_iter()
            .map(|(_, s)| s)
            .collect();
        find_leaf(snaps)
    }

    /// Find the file path for the snapshot with `extrinsic_hash`, if any.
    fn find_snapshot_path(
        &self,
        address: &str,
        extrinsic_hash_hex: &str,
    ) -> Result<Option<PathBuf>, PrivateStateError> {
        for (path, snap) in self.load_snapshots(address)? {
            if snap.extrinsic_hash == extrinsic_hash_hex {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    /// Create the store root owner-only, or narrow it when it exists. Every
    /// writer calls this first, because the root is the barrier for older,
    /// wider files under it (see the module docs).
    fn narrow_root(&self) -> Result<(), PrivateStateError> {
        create_private_dir(&self.root)
    }

    /// Write the plaintext address to `<address dir>/address.txt` if it isn't
    /// already there, so an export can recover the address from a hashed
    /// directory. If the marker exists but its content doesn't match
    /// `address`, error out rather than silently let the wrong-address record
    /// propagate into export payloads.
    fn ensure_address_marker(&self, address: &str) -> Result<(), PrivateStateError> {
        self.narrow_root()?;
        let dir = self.address_dir(address);
        create_private_dir(&dir)?;
        let marker = dir.join(ADDRESS_MARKER);
        // Read first instead of `exists()` then read: the prior shape was a
        // TOCTOU between the two syscalls (a concurrent `forget` could delete
        // the marker between the check and the read, surfacing an Io error
        // where the recreate path was the right answer).
        match not_found_as_none(fs::read_to_string(&marker), "read", &marker)? {
            Some(existing) if existing.trim() != address => {
                Err(PrivateStateError::InvalidFormat(format!(
                    "address marker at {} holds {:?} but caller passed {:?}; \
                     the per-address directory does not match the address it \
                     was created for. Resolve by deleting the directory or \
                     repairing the marker.",
                    marker.display(),
                    existing.trim(),
                    address,
                )))
            }
            Some(_) => Ok(()),
            None => write_atomic(&marker, address.as_bytes()),
        }
    }

    fn write_json_atomic<T: Serialize>(
        &self,
        path: &Path,
        value: &T,
    ) -> Result<(), PrivateStateError> {
        self.narrow_root()?;
        let dir = path
            .parent()
            .ok_or_else(|| PrivateStateError::Io("path has no parent directory".into()))?;
        create_private_dir(dir)?;

        let json = serde_json::to_vec_pretty(value)
            .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
        write_atomic(path, &json)
    }
}

#[async_trait]
impl PrivateStateProvider for FsPrivateStateProvider {
    async fn append_pending(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
        depends_on: Option<[u8; 32]>,
        state: &[u8],
    ) -> Result<(), PrivateStateError> {
        let _journal_guard = self.lock_address(address).await;
        let ext_hex = hex::encode(extrinsic_hash);
        self.ensure_address_marker(address)?;
        // Reject duplicates: every lookup by tx id assumes one file per id.
        if self.find_snapshot_path(address, &ext_hex)?.is_some() {
            return Err(PrivateStateError::SnapshotAlreadyExists {
                address: address.to_string(),
                extrinsic_hash: ext_hex,
            });
        }
        // Under the per-address lock the leaf can't move between this check
        // and the write below, so requiring `depends_on` to equal the
        // current leaf is what actually prevents branching: two concurrent
        // calls that read the same baseline serialize here, and the second
        // sees the first's snapshot as the new leaf and is rejected with
        // `JournalConflict` (it must rebuild on the fresh baseline) instead
        // of writing a sibling.
        let expected = depends_on.map(hex::encode);
        let current_leaf = self.head_snapshot(address)?.map(|s| s.extrinsic_hash);
        if current_leaf != expected {
            return Err(PrivateStateError::JournalConflict {
                address: address.to_string(),
                expected,
                found: current_leaf,
            });
        }
        let snapshot = Snapshot {
            status: SnapshotStatus::Pending,
            extrinsic_hash: ext_hex.clone(),
            block_hash: None,
            block_height: None,
            depends_on: expected.clone(),
            data: state.to_vec(),
        };
        let path = self.new_snapshot_path(address, &ext_hex);
        // An error tells the caller that no snapshot exists, but a failed
        // directory sync comes after the rename. The path is new, so remove it.
        if let Err(err) = self.write_json_atomic(&path, &snapshot) {
            if let Err(remove) = remove_file_opt(&path) {
                warn!(
                    path = %path.display(),
                    error = %remove,
                    "a failed append left its snapshot; remove the file before the next call"
                );
            }
            return Err(err);
        }
        debug!(
            address,
            extrinsic_hash = %ext_hex,
            "appended pending snapshot"
        );
        Ok(())
    }

    async fn confirm(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
        block_height: Option<u64>,
        block_hash: [u8; 32],
    ) -> Result<(), PrivateStateError> {
        let _journal_guard = self.lock_address(address).await;
        let ext_hex = hex::encode(extrinsic_hash);
        let block_hash_hex = hex::encode(block_hash);
        let path = self.find_snapshot_path(address, &ext_hex)?.ok_or_else(|| {
            PrivateStateError::SnapshotNotFound {
                address: address.to_string(),
                extrinsic_hash: ext_hex.clone(),
            }
        })?;
        let mut snap: Snapshot = read_json_opt(&path)?.ok_or_else(|| {
            PrivateStateError::Io(format!("snapshot disappeared at {}", path.display()))
        })?;
        // Confirmed is terminal. Idempotency keys on `block_hash` only,
        // because the block hash uniquely identifies the block on chain;
        // `block_height` is a derived property of that block, so a stored
        // `None` height with a matching `block_hash` is just a less
        // populated copy of the same record. Same-hash re-confirms succeed,
        // and a None → Some(h) upgrade fills in the height without an
        // error. Conflicting hashes (genuine corruption) still error.
        if snap.status == SnapshotStatus::Confirmed {
            let same_hash = snap.block_hash.as_deref() == Some(&block_hash_hex);
            if !same_hash {
                return Err(PrivateStateError::InvalidFormat(format!(
                    "snapshot already confirmed with a different block; \
                     address={address}, extrinsic_hash={ext_hex}, \
                     existing=(height={:?}, hash={:?}), \
                     requested=(height={block_height:?}, hash={block_hash_hex})",
                    snap.block_height, snap.block_hash,
                )));
            }
            // Same block, possibly enriching the height. Persist only when
            // we'd actually add information, so the file isn't rewritten
            // for a pure no-op.
            if snap.block_height.is_none() && block_height.is_some() {
                snap.block_height = block_height;
                self.write_json_atomic(&path, &snap)?;
            }
            return Ok(());
        }
        snap.status = SnapshotStatus::Confirmed;
        snap.block_height = block_height;
        snap.block_hash = Some(block_hash_hex);
        self.write_json_atomic(&path, &snap)?;
        debug!(
            address,
            extrinsic_hash = %ext_hex,
            block_height = ?block_height,
            "confirmed snapshot"
        );
        Ok(())
    }

    async fn mark_failed(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
    ) -> Result<(), PrivateStateError> {
        self.rollback_from(address, extrinsic_hash).await?;
        debug!(address, extrinsic_hash = %hex::encode(extrinsic_hash), "marked snapshot failed");
        Ok(())
    }

    async fn snapshots(&self, address: &str) -> Result<Vec<Snapshot>, PrivateStateError> {
        // Topologically sorted by `depends_on` so callers see a causal order
        // that survives import (which rewrites filename timestamps).
        Ok(topo_sort(
            self.load_snapshots(address)?
                .into_iter()
                .map(|(_, s)| s)
                .collect(),
        ))
    }

    async fn rollback_from(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
    ) -> Result<(), PrivateStateError> {
        let _journal_guard = self.lock_address(address).await;
        cascade_drop(self, address, &hex::encode(extrinsic_hash))
    }

    /// Both fields come from a single `head_snapshot` call so a concurrent
    /// `append_pending` can't produce a torn read where `data` and
    /// `extrinsic_hash` come from different journal versions.
    async fn head_with_extrinsic(
        &self,
        address: &str,
    ) -> Result<Option<(Vec<u8>, [u8; 32])>, PrivateStateError> {
        let Some(snap) = self.head_snapshot(address)? else {
            return Ok(None);
        };
        let ext = parse_hash(&snap.extrinsic_hash)?;
        Ok(Some((snap.data, ext)))
    }

    async fn forget(&self, address: &str) -> Result<(), PrivateStateError> {
        clear_dir(&self.address_dir(address))
    }

    async fn forget_all(&self) -> Result<(), PrivateStateError> {
        clear_dir(&self.states_dir())
    }

    async fn set_signing_key(&self, address: &str, key: &[u8]) -> Result<(), PrivateStateError> {
        let rec = KeyRecord {
            address: address.to_string(),
            data: key.to_vec(),
        };
        self.write_json_atomic(&self.key_path(address), &rec)
    }

    async fn get_signing_key(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError> {
        Ok(read_json_opt::<KeyRecord>(&self.key_path(address))?.map(|rec| rec.data))
    }

    async fn remove_signing_key(&self, address: &str) -> Result<(), PrivateStateError> {
        remove_file_opt(&self.key_path(address))
    }

    async fn clear_signing_keys(&self) -> Result<(), PrivateStateError> {
        clear_dir(&self.keys_dir())
    }

    async fn export_private_states(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError> {
        opts.check_password()?;
        let entries = collect_export_entries(self)?;
        let export = crypto::seal(FORMAT_STATES, opts, &entries)?;
        debug!(count = entries.len(), "exported snapshot journal");
        Ok(export)
    }

    async fn import_private_states(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        let entries = crypto::open(FORMAT_STATES, data, opts)?;
        apply_import_entries(self, entries, opts.conflict)
    }

    async fn export_signing_keys(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError> {
        opts.check_password()?;
        let mut records: Vec<KeyRecord> = Vec::new();
        for (path, rec) in read_json_dir::<KeyRecord>(&self.keys_dir())? {
            // Cross-check the filename: a signing-key file at
            // `<sha256(address)>.json` whose JSON `address` field
            // has been edited would otherwise migrate the key to
            // the edited address through an export/import
            // round-trip.
            let expected = self.key_path(&rec.address);
            if path != expected {
                return Err(PrivateStateError::InvalidFormat(format!(
                    "signing-key file at {} has an address that does not \
                     match its filename hash (expected {})",
                    path.display(),
                    expected.display(),
                )));
            }
            records.push(rec);
        }
        crypto::seal(FORMAT_KEYS, opts, &records)
    }

    async fn import_signing_keys(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        let records: Vec<KeyRecord> = crypto::open(FORMAT_KEYS, data, opts)?;

        let resolved: Vec<(PathBuf, KeyRecord)> = records
            .into_iter()
            .map(|rec| (self.key_path(&rec.address), rec))
            .collect();
        reject_duplicate_paths(&resolved)?;

        // Decide every record before the first write, so a conflict leaves
        // the store unchanged.
        let mut result = ImportResult::default();
        let mut admitted = Vec::with_capacity(resolved.len());
        for (path, rec) in &resolved {
            if result.admit(path.exists(), opts.conflict, || rec.address.clone())? {
                admitted.push((path, rec));
            }
        }
        for (path, rec) in admitted {
            self.write_json_atomic(path, rec)?;
        }
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// The journal leaf: the snapshot whose extrinsic_hash isn't referenced by
/// any other snapshot's `depends_on`. A well-formed sequential journal has
/// exactly one leaf AND every snapshot is reachable from a root via
/// `depends_on`. Returns:
///
/// - `Ok(None)` if the journal is empty.
/// - `Ok(Some(snap))` if there is exactly one leaf and the journal is
///   acyclic (every snapshot is reachable from a root).
/// - `Err(InvalidFormat)` if the journal is non-empty but malformed:
///   - zero leaves (every snapshot is referenced; only possible if the
///     graph is one big cycle),
///   - more than one leaf (branching, e.g., from two concurrent
///     `append_pending` calls sharing a parent),
///   - an isolated cycle alongside a valid root-leaf (some snapshots
///     unreachable from any root; detected via topo-sort reachability).
///
/// In every error case the caller can't safely pick a witness baseline.
/// Resolve via `rollback_from` / `mark_failed` until the journal is
/// single-leafed and fully reachable again.
fn find_leaf(snapshots: Vec<Snapshot>) -> Result<Option<Snapshot>, PrivateStateError> {
    if snapshots.is_empty() {
        return Ok(None);
    }
    let total = snapshots.len();
    let referenced: HashSet<String> = snapshots
        .iter()
        .filter_map(|s| s.depends_on.clone())
        .collect();
    let mut leaves: Vec<Snapshot> = snapshots
        .iter()
        .filter(|s| !referenced.contains(&s.extrinsic_hash))
        .cloned()
        .collect();

    if leaves.is_empty() {
        return Err(PrivateStateError::InvalidFormat(
            "malformed journal: no leaf snapshot (depends_on cycle)".into(),
        ));
    }
    if leaves.len() > 1 {
        // Sort leaf hashes so the error is deterministic across directory
        // iteration orderings.
        let mut hashes: Vec<String> = leaves.into_iter().map(|s| s.extrinsic_hash).collect();
        hashes.sort();
        return Err(PrivateStateError::InvalidFormat(format!(
            "malformed journal: {} leaves (branching); resolve via \
             rollback_from / mark_failed. leaves=[{}]",
            hashes.len(),
            hashes.join(", ")
        )));
    }
    // One leaf, but we still need to rule out an isolated cycle hiding
    // alongside this valid root-leaf chain (e.g., {A_root, B<->C}). Do a
    // BFS from every root (snapshot with `depends_on=None`) following
    // child edges; anything not reached belongs to a cycle.
    let by_hash: HashMap<&str, &Snapshot> = snapshots
        .iter()
        .map(|s| (s.extrinsic_hash.as_str(), s))
        .collect();
    let roots = snapshots
        .iter()
        .filter(|s| s.depends_on.is_none())
        .map(|s| s.extrinsic_hash.as_str())
        .collect();
    let reachable: HashSet<&str> = walk_children(&snapshots, roots).into_iter().collect();
    if reachable.len() != total {
        let mut orphans: Vec<String> = by_hash
            .keys()
            .filter(|h| !reachable.contains(*h))
            .map(|h| h.to_string())
            .collect();
        orphans.sort();
        // Distinguish two unreachable shapes so the message points the
        // user at the right tool. "Cycle" is the case where every
        // unreachable snapshot's `depends_on` parent IS in the journal,
        // but follows a cycle that excludes it from any root walk.
        // "Orphan chain" is the case where at least one unreachable
        // snapshot's parent is missing entirely (corrupted import /
        // hand-edited file) so the chain can't be re-rooted without
        // restoring the missing parent.
        let all_parents_present = orphans.iter().all(|h| {
            // SAFETY: orphan came from by_hash.keys, so it's a known hash.
            by_hash[h.as_str()]
                .depends_on
                .as_deref()
                .is_none_or(|p| by_hash.contains_key(p))
        });
        let reason = if all_parents_present {
            "cycle (every unreachable snapshot's parent is in the journal \
             but forms a closed loop)"
        } else {
            "orphan chain (at least one unreachable snapshot's \
             `depends_on` parent is missing from the journal)"
        };
        return Err(PrivateStateError::InvalidFormat(format!(
            "malformed journal: {} unreachable snapshot(s); {reason}. \
             Resolve via rollback_from / mark_failed. unreachable=[{}]",
            total - reachable.len(),
            orphans.join(", ")
        )));
    }
    Ok(leaves.pop())
}

/// Topologically sort snapshots so roots (no `depends_on`) come first and
/// each snapshot follows its parent. Within one parent the order is
/// extrinsic_hash-lexicographic so the result is deterministic.
fn topo_sort(snapshots: Vec<Snapshot>) -> Vec<Snapshot> {
    let mut by_hash: HashMap<String, Snapshot> = snapshots
        .into_iter()
        .map(|s| (s.extrinsic_hash.clone(), s))
        .collect();

    let mut roots: Vec<&str> = by_hash
        .values()
        .filter(|s| s.depends_on.is_none())
        .map(|s| s.extrinsic_hash.as_str())
        .collect();
    roots.sort_unstable();
    let order: Vec<String> = walk_children(by_hash.values(), roots)
        .into_iter()
        .map(str::to_owned)
        .collect();

    let mut out: Vec<Snapshot> = order.iter().filter_map(|h| by_hash.remove(h)).collect();
    // Orphans (snapshot whose `depends_on` points at a missing parent) won't
    // be reached by the BFS; append them in hash order so they're still
    // included in the inventory.
    let mut orphans: Vec<Snapshot> = by_hash.into_values().collect();
    orphans.sort_by(|a, b| a.extrinsic_hash.cmp(&b.extrinsic_hash));
    out.extend(orphans);
    out
}

/// The hashes reachable from `starts` along child edges (a child's
/// `depends_on` names its parent), breadth first. Each hash appears once, and
/// every child comes after its parent. Siblings come in hash order.
fn walk_children<'a>(
    snapshots: impl IntoIterator<Item = &'a Snapshot>,
    starts: Vec<&'a str>,
) -> Vec<&'a str> {
    use std::collections::VecDeque;

    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for s in snapshots {
        if let Some(parent) = s.depends_on.as_deref() {
            children.entry(parent).or_default().push(&s.extrinsic_hash);
        }
    }
    for siblings in children.values_mut() {
        siblings.sort_unstable();
    }

    let mut visited = HashSet::new();
    let mut order = Vec::new();
    let mut queue: VecDeque<&str> = starts.into();
    while let Some(hash) = queue.pop_front() {
        if !visited.insert(hash) {
            continue;
        }
        order.push(hash);
        if let Some(cs) = children.get(hash) {
            queue.extend(cs);
        }
    }
    order
}

/// Cascade-drop a snapshot and every snapshot that transitively depends on
/// it. Walks the dependency graph (`depends_on` edges) starting from
/// `start_hash`, collects every reachable extrinsic_hash, then deletes the
/// corresponding files. Errors with [`PrivateStateError::SnapshotNotFound`]
/// if `start_hash` is not present.
fn cascade_drop(
    provider: &FsPrivateStateProvider,
    address: &str,
    start_hash: &str,
) -> Result<(), PrivateStateError> {
    let snapshots = provider.load_snapshots(address)?;
    if !snapshots
        .iter()
        .any(|(_, s)| s.extrinsic_hash == start_hash)
    {
        return Err(PrivateStateError::SnapshotNotFound {
            address: address.to_string(),
            extrinsic_hash: start_hash.to_string(),
        });
    }
    let path_by_hash: HashMap<&str, &Path> = snapshots
        .iter()
        .map(|(path, s)| (s.extrinsic_hash.as_str(), path.as_path()))
        .collect();
    let doomed = walk_children(snapshots.iter().map(|(_, s)| s), vec![start_hash]);

    // Delete child-first (the reverse of the walk order) and stop at the
    // first failure, so a partial cascade is always recoverable. The doomed
    // set is the start snapshot plus everything that (transitively) depends
    // on it. Deleting a parent before its child and then failing would leave
    // the surviving child an orphan whose `depends_on` points at a
    // now-missing parent, which `find_leaf` rejects. Deleting children
    // before parents means every surviving doomed snapshot still has its
    // parent on disk, so the journal stays a valid chain and re-running
    // `mark_failed` resumes the drop from where it stopped.
    for hash in doomed.into_iter().rev() {
        if let Some(path) = path_by_hash.get(hash) {
            remove_file_opt(path).map_err(|e| {
                PrivateStateError::Io(format!(
                    "cascade_drop on {address} stopped at snapshot {hash} ({}): {e}; \
                     re-run mark_failed to resume",
                    path.display()
                ))
            })?;
        }
    }
    Ok(())
}

/// Walk the per-address journal directories, collecting every snapshot
/// alongside the address it belongs to. The address is recovered from the
/// `address.txt` marker each per-address dir holds.
fn collect_export_entries(
    provider: &FsPrivateStateProvider,
) -> Result<Vec<ExportEntry>, PrivateStateError> {
    let mut out: Vec<ExportEntry> = Vec::new();
    for dir in list_dir(&provider.states_dir())? {
        if !dir.is_dir() {
            continue;
        }
        let addr_marker = dir.join(ADDRESS_MARKER);
        let Some(address) =
            not_found_as_none(fs::read_to_string(&addr_marker), "read", &addr_marker)?
        else {
            continue;
        };
        let address = address.trim().to_string();
        // Cross-check the marker against the directory name: the directory
        // hash is the source of truth, so a tampered or stale marker must
        // not silently rebind snapshots through an export/import round-trip.
        let expected = provider.address_dir(&address);
        if dir != expected {
            return Err(PrivateStateError::InvalidFormat(format!(
                "address marker at {} holds {:?} whose sha256 does not match \
                 the directory name (expected directory {})",
                addr_marker.display(),
                address,
                expected.display(),
            )));
        }
        for (_, snap) in read_json_dir::<Snapshot>(&dir)? {
            out.push(ExportEntry {
                address: address.clone(),
                snapshot: snap,
            });
        }
    }
    Ok(out)
}

/// Replay a snapshot stream into the per-address journal, honouring the
/// conflict strategy. Snapshots for the same `(address, extrinsic_hash)`
/// collide; everything else lives side-by-side.
///
/// Detect-before-mutate posture: every validation pass runs against an
/// in-memory simulation of the merged journal, so any failure (hex parse,
/// payload dedupe, conflict under `Error`, status monotonicity under
/// `Overwrite`, post-merge `find_leaf` graph health) aborts before any
/// file is written. The destination is therefore either fully updated or
/// untouched.
///
/// Performance: snapshots for each touched address are loaded exactly
/// once into a `HashMap<extrinsic_hash, PathBuf>`, so an N-entry import
/// against an M-snapshot directory costs O(M + N) reads instead of the
/// previous O(N * M) per-entry directory scans.
fn apply_import_entries(
    provider: &FsPrivateStateProvider,
    entries: Vec<ExportEntry>,
    conflict: ConflictStrategy,
) -> Result<ImportResult, PrivateStateError> {
    use std::collections::HashMap;

    // Pass 1: validate hex on every snapshot's hash fields.
    for entry in &entries {
        validate_hash_field("extrinsic_hash", Some(&entry.snapshot.extrinsic_hash))?;
        validate_hash_field("depends_on", entry.snapshot.depends_on.as_deref())?;
        validate_hash_field("block_hash", entry.snapshot.block_hash.as_deref())?;
    }

    // Pass 2: reject duplicate (address, extrinsic_hash) pairs within the
    // payload itself.
    let mut seen_in_payload: HashSet<(String, String)> = HashSet::with_capacity(entries.len());
    for entry in &entries {
        let key = (entry.address.clone(), entry.snapshot.extrinsic_hash.clone());
        if !seen_in_payload.insert(key) {
            return Err(PrivateStateError::InvalidFormat(format!(
                "duplicate entry in import payload: {} / {}",
                entry.address, entry.snapshot.extrinsic_hash
            )));
        }
    }

    // Pass 3: load each touched address's directory once into a HashMap of
    // (extrinsic_hash -> (path, snapshot)). Reused for the conflict check
    // (Pass 4) and the merged-journal simulation (Pass 5).
    let mut addresses_in_payload: HashSet<String> = HashSet::new();
    for entry in &entries {
        addresses_in_payload.insert(entry.address.clone());
    }
    let mut per_address: HashMap<String, HashMap<String, (PathBuf, Snapshot)>> = HashMap::new();
    for address in &addresses_in_payload {
        let mut by_hash: HashMap<String, (PathBuf, Snapshot)> = HashMap::new();
        for (path, snap) in provider.load_snapshots(address)? {
            by_hash.insert(snap.extrinsic_hash.clone(), (path, snap));
        }
        per_address.insert(address.clone(), by_hash);
    }

    // Pass 4: detect-before-mutate. For each entry, decide against the
    // destination (using the cached HashMap) whether to write it, and stage
    // the target path of the write. No on-disk changes yet.
    let mut staged: Vec<(ExportEntry, Option<PathBuf>)> = Vec::with_capacity(entries.len());
    let mut result = ImportResult::default();
    for entry in entries {
        let by_hash = per_address.get(&entry.address).expect("populated above");
        let existing = by_hash.get(&entry.snapshot.extrinsic_hash);
        // Status monotonicity: don't let an Overwrite import
        // silently downgrade a Confirmed snapshot back to
        // Pending and drop its `block_hash` / `block_height`.
        if conflict == ConflictStrategy::Overwrite
            && let Some((_, existing_snap)) = existing
            && existing_snap.status == SnapshotStatus::Confirmed
            && entry.snapshot.status == SnapshotStatus::Pending
        {
            return Err(PrivateStateError::InvalidFormat(format!(
                "import would downgrade a Confirmed snapshot to \
                 Pending at {} / {}; refusing to discard the \
                 existing block_hash / block_height",
                entry.address, entry.snapshot.extrinsic_hash
            )));
        }
        let write = result.admit(existing.is_some(), conflict, || {
            format!("{} / {}", entry.address, entry.snapshot.extrinsic_hash)
        })?;
        let target = write.then(|| match existing {
            Some((path, _)) => path.clone(),
            None => provider.new_snapshot_path(&entry.address, &entry.snapshot.extrinsic_hash),
        });
        // Update the in-memory view so Pass 5 can validate the merged
        // journal without touching disk.
        if target.is_some() {
            per_address
                .get_mut(&entry.address)
                .expect("populated above")
                .insert(
                    entry.snapshot.extrinsic_hash.clone(),
                    (PathBuf::new(), entry.snapshot.clone()),
                );
        }
        staged.push((entry, target));
    }

    // Pass 5: run `find_leaf` on every address in the payload (even ones
    // that only saw `Skip` actions), so a pre-existing malformed journal
    // surfaces here instead of at the next `call_with`. Validating Skip
    // addresses also catches the case where the import silently no-op'd
    // against a broken destination.
    for address in &addresses_in_payload {
        let snaps: Vec<Snapshot> = per_address[address]
            .values()
            .map(|(_, s)| s.clone())
            .collect();
        if let Err(e) = find_leaf(snaps) {
            return Err(PrivateStateError::InvalidFormat(format!(
                "import produced a malformed journal at {address}: {e}"
            )));
        }
    }

    // Pass 6: commit. Every prior pass ran on borrowed data; the only IO
    // failures left are filesystem-level (ENOSPC etc.), which we surface
    // as `Io` like any other write.
    for (entry, target) in staged {
        provider.ensure_address_marker(&entry.address)?;
        if let Some(path) = target {
            provider.write_json_atomic(&path, &entry.snapshot)?;
        }
    }
    Ok(result)
}

/// Verify that `field` (when present) is a 32-byte hex string. Used by the
/// import path so a malformed payload errors out before mutating the store.
fn validate_hash_field(name: &str, field: Option<&str>) -> Result<(), PrivateStateError> {
    let Some(s) = field else {
        return Ok(());
    };
    // Internally produced hashes always go through `hex::encode` (lowercase).
    // Reject uppercase here so an import with mixed-case hex can't end up
    // byte-unequal to an internally produced hash with the same value,
    // which would silently break `HashSet<String>`-based comparisons in
    // `find_leaf` and `find_snapshot_path`.
    let is_lowercase_hex = s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !is_lowercase_hex {
        return Err(PrivateStateError::InvalidFormat(format!(
            "field `{name}` is not 32 bytes of lowercase hex: {s}"
        )));
    }
    Ok(())
}

/// The paths of the entries in `dir`, or none when `dir` does not exist. Any
/// other failure surfaces, so a store that cannot be listed never exports as
/// an empty backup.
fn list_dir(dir: &Path) -> Result<Vec<PathBuf>, PrivateStateError> {
    let io_err =
        |e: std::io::Error| PrivateStateError::Io(format!("read dir {}: {e}", dir.display()));
    let Some(entries) = not_found_as_none(fs::read_dir(dir), "read dir", dir)? else {
        return Ok(Vec::new());
    };
    entries
        .map(|entry| entry.map(|e| e.path()).map_err(io_err))
        .collect()
}

/// Every `*.json` record in `dir` with its path, sorted by file name. Empty
/// when `dir` does not exist.
fn read_json_dir<T: DeserializeOwned>(dir: &Path) -> Result<Vec<(PathBuf, T)>, PrivateStateError> {
    let mut out = Vec::new();
    for path in list_dir(dir)? {
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Some(record) = read_json_opt(&path)? {
            out.push((path, record));
        }
    }
    out.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));
    Ok(out)
}

fn create_private_dir(dir: &Path) -> Result<(), PrivateStateError> {
    midnight_owner_only::create_dir_all(dir)
        .map_err(|e| PrivateStateError::Io(format!("create dir {}: {e}", dir.display())))
}

/// Write `contents` to a `.tmp` sibling of `path`, rename it into place, and
/// sync the directory (see the module docs).
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), PrivateStateError> {
    let tmp = path.with_extension("tmp");
    midnight_owner_only::write(&tmp, contents)
        .map_err(|e| PrivateStateError::Io(format!("write {}: {e}", tmp.display())))?;
    fs::rename(&tmp, path)
        .map_err(|e| PrivateStateError::Io(format!("rename into {}: {e}", path.display())))?;
    let dir = path
        .parent()
        .ok_or_else(|| PrivateStateError::Io(format!("{} has no parent", path.display())))?;
    midnight_owner_only::sync_dir(dir)
        .map_err(|e| PrivateStateError::Io(format!("sync {}: {e}", dir.display())))
}

/// `None` when `result` failed with `NotFound`. Any other error becomes
/// [`PrivateStateError::Io`] that names `op` and `path`.
fn not_found_as_none<T>(
    result: std::io::Result<T>,
    op: &str,
    path: &Path,
) -> Result<Option<T>, PrivateStateError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PrivateStateError::Io(format!(
            "{op} {}: {e}",
            path.display()
        ))),
    }
}

fn read_json_opt<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, PrivateStateError> {
    not_found_as_none(fs::read(path), "read", path)?
        .map(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|e| PrivateStateError::Serialize(format!("{}: {e}", path.display())))
        })
        .transpose()
}

fn remove_file_opt(path: &Path) -> Result<(), PrivateStateError> {
    not_found_as_none(fs::remove_file(path), "remove", path)?;
    Ok(())
}

fn clear_dir(dir: &Path) -> Result<(), PrivateStateError> {
    not_found_as_none(fs::remove_dir_all(dir), "clear", dir)?;
    Ok(())
}

fn parse_hash(s: &str) -> Result<[u8; 32], PrivateStateError> {
    let bytes = hex::decode(s)
        .map_err(|e| PrivateStateError::InvalidFormat(format!("hash is not valid hex: {e}")))?;
    bytes.try_into().map_err(|v: Vec<u8>| {
        PrivateStateError::InvalidFormat(format!("hash must be 32 bytes; got {}", v.len()))
    })
}

fn reject_duplicate_paths<T>(resolved: &[(PathBuf, T)]) -> Result<(), PrivateStateError> {
    let mut seen = HashSet::with_capacity(resolved.len());
    for (path, _) in resolved {
        if !seen.insert(path.as_path()) {
            return Err(PrivateStateError::InvalidFormat(
                "export contains duplicate entries for the same key".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "a-sufficiently-long-password";

    fn provider() -> (tempfile::TempDir, FsPrivateStateProvider) {
        let dir = tempfile::TempDir::new().unwrap();
        let p = FsPrivateStateProvider::new(dir.path());
        (dir, p)
    }

    fn ext(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    fn pending(hash: u8, parent: Option<u8>, data: &[u8]) -> Snapshot {
        Snapshot {
            status: SnapshotStatus::Pending,
            extrinsic_hash: hex::encode(ext(hash)),
            block_hash: None,
            block_height: None,
            depends_on: parent.map(|p| hex::encode(ext(p))),
            data: data.to_vec(),
        }
    }

    /// Write `snap` into the journal of `address` in the layout of
    /// `append_pending`, but without its checks.
    fn write_raw(p: &FsPrivateStateProvider, address: &str, snap: &Snapshot) {
        p.ensure_address_marker(address).unwrap();
        let path = p.new_snapshot_path(address, &snap.extrinsic_hash);
        p.write_json_atomic(&path, snap).unwrap();
    }

    // Seal and open by hand, not with `crypto::seal` and `crypto::open`, so
    // that these fixtures check the aad == format rule on their own.
    fn envelope(format: &str, payload: &impl Serialize) -> EncryptedExport {
        let bytes = serde_json::to_vec(payload).unwrap();
        let (salt, ciphertext) = crypto::encrypt(PW, format.as_bytes(), &bytes).unwrap();
        EncryptedExport {
            format: format.to_string(),
            salt,
            ciphertext,
        }
    }

    fn entries_of(export: &EncryptedExport) -> Vec<ExportEntry> {
        let payload = crypto::decrypt(
            PW,
            FORMAT_STATES.as_bytes(),
            &export.salt,
            &export.ciphertext,
        )
        .unwrap();
        serde_json::from_slice(&payload).unwrap()
    }

    #[tokio::test]
    async fn head_returns_latest_pending_snapshot() {
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        p.append_pending("0200aa", ext(2), Some(ext(1)), b"s2")
            .await
            .unwrap();
        assert_eq!(p.head("0200aa").await.unwrap().as_deref(), Some(&b"s2"[..]));
        assert_eq!(p.head_extrinsic("0200aa").await.unwrap(), Some(ext(2)));
    }

    #[tokio::test]
    async fn confirm_promotes_pending_to_confirmed() {
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        p.confirm("0200aa", ext(1), Some(42), [0xbb; 32])
            .await
            .unwrap();
        let snaps = p.snapshots("0200aa").await.unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].status, SnapshotStatus::Confirmed);
        assert_eq!(snaps[0].block_height, Some(42));
    }

    #[tokio::test]
    async fn mark_failed_cascades_to_dependents() {
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        p.append_pending("0200aa", ext(2), Some(ext(1)), b"s2")
            .await
            .unwrap();
        p.append_pending("0200aa", ext(3), Some(ext(2)), b"s3")
            .await
            .unwrap();

        // Mark the middle one as failed → head becomes the snapshot before
        // it. The third snapshot, which transitively depends on the failed
        // one, also gets dropped.
        p.mark_failed("0200aa", ext(2)).await.unwrap();
        let snaps = p.snapshots("0200aa").await.unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].extrinsic_hash, hex::encode(ext(1)));
        assert_eq!(p.head_extrinsic("0200aa").await.unwrap(), Some(ext(1)));
    }

    #[tokio::test]
    async fn forget_drops_everything_for_address() {
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        p.append_pending("0200bb", ext(2), None, b"s2")
            .await
            .unwrap();
        p.forget("0200aa").await.unwrap();
        assert!(p.head("0200aa").await.unwrap().is_none());
        assert_eq!(p.head("0200bb").await.unwrap().as_deref(), Some(&b"s2"[..]));
    }

    #[tokio::test]
    async fn signing_keys_roundtrip() {
        let (_dir, p) = provider();
        assert!(p.get_signing_key("0200aa").await.unwrap().is_none());
        p.set_signing_key("0200aa", b"key").await.unwrap();
        assert_eq!(
            p.get_signing_key("0200aa").await.unwrap().as_deref(),
            Some(&b"key"[..])
        );
        p.remove_signing_key("0200aa").await.unwrap();
        assert!(p.get_signing_key("0200aa").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn export_import_roundtrip_preserves_journal() {
        let (_src_dir, src) = provider();
        // `append_pending` writes the address marker; the export path reads
        // it to recover the plaintext address.
        src.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        src.confirm("0200aa", ext(1), Some(10), [0xee; 32])
            .await
            .unwrap();
        src.append_pending("0200aa", ext(2), Some(ext(1)), b"s2")
            .await
            .unwrap();

        let exp = src
            .export_private_states(&ExportOptions::new(PW))
            .await
            .unwrap();
        assert_eq!(exp.format, FORMAT_STATES);

        let (_dst_dir, dst) = provider();
        let res = dst
            .import_private_states(&exp, &ImportOptions::new(PW))
            .await
            .unwrap();
        assert_eq!(res.imported, 2);

        let snaps = dst.snapshots("0200aa").await.unwrap();
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].status, SnapshotStatus::Confirmed);
        assert_eq!(snaps[1].status, SnapshotStatus::Pending);
    }

    #[tokio::test]
    async fn head_uses_depends_on_graph_not_filename_order() {
        // `head` must return the journal leaf (the snapshot nothing else
        // depends on), not just the lexicographically-last filename. The
        // export/import path rewrites filenames with new timestamps, which
        // would silently corrupt the head if it relied on filename order.
        //
        // Import names the files in payload order. The leaf has the smallest
        // hash, so a payload in hash order gives the leaf the first filename,
        // also when two timestamps tie.
        let (_src_dir, src) = provider();
        src.append_pending("0200aa", ext(3), None, b"s3")
            .await
            .unwrap();
        src.append_pending("0200aa", ext(2), Some(ext(3)), b"s2")
            .await
            .unwrap();
        src.append_pending("0200aa", ext(1), Some(ext(2)), b"s1")
            .await
            .unwrap();

        let exp = src
            .export_private_states(&ExportOptions::new(PW))
            .await
            .unwrap();
        let mut entries = entries_of(&exp);
        entries.sort_by(|a, b| a.snapshot.extrinsic_hash.cmp(&b.snapshot.extrinsic_hash));
        let leaf_first = envelope(FORMAT_STATES, &entries);

        let (_dst_dir, dst) = provider();
        dst.import_private_states(&leaf_first, &ImportOptions::new(PW))
            .await
            .unwrap();

        assert_eq!(dst.head_extrinsic("0200aa").await.unwrap(), Some(ext(1)));
        assert_eq!(
            dst.head("0200aa").await.unwrap().as_deref(),
            Some(&b"s1"[..])
        );
    }

    #[tokio::test]
    async fn import_rejects_duplicate_extrinsic_hash_in_payload() {
        // Two entries with the same `(address, extrinsic_hash)` in one
        // payload must be rejected before mutating the store. Without this
        // check the first write would land, and only the second would error
        // under `ConflictStrategy::Error`.
        let (_src_dir, src) = provider();
        src.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        let exp = src
            .export_private_states(&ExportOptions::new(PW))
            .await
            .unwrap();

        // Decrypt, duplicate the entry, re-encrypt, then try to import.
        let mut entries = entries_of(&exp);
        entries.push(entries[0].clone());
        let dup = envelope(FORMAT_STATES, &entries);

        let (_dst_dir, dst) = provider();
        let err = dst
            .import_private_states(&dup, &ImportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(_)),
            "expected InvalidFormat, got {err:?}"
        );
        // The store is untouched: no partial write from the first copy.
        assert!(dst.head("0200aa").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn import_rejects_malformed_hash_fields() {
        // A snapshot whose `extrinsic_hash` isn't 32 bytes of hex would
        // round-trip through serde fine but break `head_extrinsic` /
        // rollback logic later. The import path must reject these up front.
        let bad = vec![ExportEntry {
            address: "0200aa".into(),
            snapshot: Snapshot {
                extrinsic_hash: "not-hex".into(),
                ..pending(0, None, b"bytes")
            },
        }];
        let exp = envelope(FORMAT_STATES, &bad);

        let (_dir, dst) = provider();
        let err = dst
            .import_private_states(&exp, &ImportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(_)),
            "expected InvalidFormat, got {err:?}"
        );
    }

    #[tokio::test]
    async fn append_pending_rejects_duplicate_extrinsic_hash() {
        // A second append with the same extrinsic_hash would leave two files
        // that claim one tx id, and each lookup by id would then pick one of
        // them arbitrarily.
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        let err = p
            .append_pending("0200aa", ext(1), None, b"s1-retry")
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::SnapshotAlreadyExists { .. }),
            "expected SnapshotAlreadyExists, got {err:?}"
        );
        // Only one file ended up on disk.
        assert_eq!(p.snapshots("0200aa").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn append_pending_rejects_branching_with_journal_conflict() {
        // A second append whose `depends_on` is no longer the leaf is
        // rejected, so the journal can't branch through the API.
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        p.append_pending("0200aa", ext(2), Some(ext(1)), b"s2")
            .await
            .unwrap();
        // ext(1) is no longer the leaf (ext(2) is).
        let err = p
            .append_pending("0200aa", ext(3), Some(ext(1)), b"branch")
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::JournalConflict { .. }),
            "expected JournalConflict, got {err:?}"
        );
        assert_eq!(p.snapshots("0200aa").await.unwrap().len(), 2);
        // The leaf is still unambiguous.
        assert!(p.head("0200aa").await.is_ok());
    }

    #[tokio::test]
    async fn concurrent_appends_keep_a_single_leaf() {
        // Two calls that captured the same baseline race to extend it. The
        // per-address lock serializes them; exactly one wins and the other
        // gets JournalConflict, so the journal never branches.
        //
        // One race can miss the overlap, so the test runs many rounds.
        for round in 0..50 {
            let (_dir, p) = provider();
            p.append_pending("0200aa", ext(1), None, b"s1")
                .await
                .unwrap();
            // A std barrier releases both threads together. A tokio barrier
            // queues the woken task behind the other append on one worker.
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let append = |hash: u8, state: &'static [u8]| {
                let (p, barrier) = (p.clone(), barrier.clone());
                let rt = tokio::runtime::Handle::current();
                tokio::task::spawn_blocking(move || {
                    barrier.wait();
                    rt.block_on(p.append_pending("0200aa", ext(hash), Some(ext(1)), state))
                })
            };
            let (a, b) = (append(2, b"a"), append(3, b"b"));
            let (ra, rb) = (a.await.unwrap(), b.await.unwrap());
            let oks = [&ra, &rb].iter().filter(|r| r.is_ok()).count();
            let conflicts = [&ra, &rb]
                .iter()
                .filter(|r| matches!(r, Err(PrivateStateError::JournalConflict { .. })))
                .count();
            assert_eq!(
                oks, 1,
                "round {round}: exactly one append should win: {ra:?} / {rb:?}"
            );
            assert_eq!(
                conflicts, 1,
                "round {round}: the loser should see JournalConflict"
            );
            assert_eq!(p.snapshots("0200aa").await.unwrap().len(), 2);
            assert!(p.head("0200aa").await.is_ok(), "leaf must stay unique");
        }
    }

    #[test]
    fn a_cascade_from_mid_chain_removes_children_before_parents() {
        // The chain is 1 <- 5 <- 3, given out of order. The cascade starts
        // at 5, which is not a root, and its child 3 sorts below it.
        let journal = [
            pending(5, Some(1), b""),
            pending(3, Some(5), b""),
            pending(1, None, b""),
        ];
        let start = hex::encode(ext(5));
        let delete_order: Vec<&str> = walk_children(&journal, vec![start.as_str()])
            .into_iter()
            .rev()
            .collect();
        assert_eq!(delete_order, [3, 5].map(|h| hex::encode(ext(h))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mark_failed_that_cannot_remove_a_file_asks_for_a_rerun_and_resumes() {
        // A failed removal must leave a valid journal, return an Io error
        // that asks for a re-run, and finish the drop on that re-run. The
        // read-only address directory makes the removal fail.
        use std::os::unix::fs::PermissionsExt;
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        p.append_pending("0200aa", ext(2), Some(ext(1)), b"s2")
            .await
            .unwrap();
        p.confirm("0200aa", ext(1), None, ext(9)).await.unwrap();

        let dir = p.address_dir("0200aa");
        let orig = std::fs::metadata(&dir).unwrap().permissions();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();

        // Drop ext(2): removal fails under the read-only dir.
        let err = p.mark_failed("0200aa", ext(2)).await.unwrap_err();
        assert!(
            matches!(err, PrivateStateError::Io(ref m) if m.contains("re-run mark_failed")),
            "expected a recoverable Io error, got {err:?}"
        );

        // Journal is still intact and readable (nothing orphaned).
        std::fs::set_permissions(&dir, orig).unwrap();
        assert_eq!(p.snapshots("0200aa").await.unwrap().len(), 2);
        assert!(p.head("0200aa").await.is_ok());

        // Re-run completes the drop.
        p.mark_failed("0200aa", ext(2)).await.unwrap();
        let remaining = p.snapshots("0200aa").await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].extrinsic_hash, hex::encode(ext(1)));
    }

    #[tokio::test]
    async fn export_signing_keys_rejects_corrupt_on_disk_record() {
        // A hand-edited (or otherwise corrupted) on-disk record with invalid
        // base64 in `data` must not silently propagate into an export that
        // would later fail to import.
        let (_dir, p) = provider();
        p.set_signing_key("0200aa", b"k").await.unwrap();
        // Corrupt the file in place: replace `data` with non-base64.
        let key_path = p.key_path("0200aa");
        fs::write(
            &key_path,
            r#"{"address":"0200aa","data":"!!!not-base64!!!"}"#,
        )
        .unwrap();

        let err = p
            .export_signing_keys(&ExportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::Serialize(ref m)
                if m.contains(&key_path.display().to_string())),
            "expected Serialize naming the file, got {err:?}"
        );
    }

    #[tokio::test]
    async fn head_errors_on_branching_journal() {
        // Two snapshots sharing the same `depends_on` parent: both are leaves,
        // so the journal has no unique head. `head` / `head_extrinsic` must
        // surface this rather than silently picking one based on filename
        // order.
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        p.append_pending("0200aa", ext(2), Some(ext(1)), b"a")
            .await
            .unwrap();
        // `append_pending` now rejects a sibling (its `depends_on` no longer
        // matches the leaf), so write the branching snapshot straight to
        // disk to exercise `find_leaf`'s defense against an already-branched
        // journal (one produced by import, manual editing, or a pre-fix
        // version of the SDK).
        write_raw(&p, "0200aa", &pending(3, Some(1), b"b"));

        let err = p.head("0200aa").await.unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m) if m.contains("2 leaves")),
            "expected InvalidFormat with branching message, got {err:?}"
        );
        let err = p.head_extrinsic("0200aa").await.unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m) if m.contains("2 leaves")),
            "expected InvalidFormat with branching message, got {err:?}"
        );
    }

    #[tokio::test]
    async fn head_errors_on_depends_on_cycle() {
        // Construct a cycle by hand on disk: A.depends_on = B,
        // B.depends_on = A. `append_pending` won't let us do this through the
        // public API (it would require a future child to predate its parent),
        // but a corrupted directory could.
        let (_dir, p) = provider();
        write_raw(&p, "0200aa", &pending(0xAA, Some(0xBB), b"a"));
        write_raw(&p, "0200aa", &pending(0xBB, Some(0xAA), b"b"));

        let err = p.head("0200aa").await.unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m) if m.contains("no leaf")),
            "expected InvalidFormat with cycle message, got {err:?}"
        );
    }

    #[tokio::test]
    async fn import_signing_keys_decode_error_is_invalid_format() {
        // A payload that decrypts cleanly but isn't a valid `Vec<KeyRecord>`
        // (e.g., wrong shape) should surface as `InvalidFormat`, matching
        // the snapshot-import error variant.
        let exp = envelope(FORMAT_KEYS, &serde_json::json!({"not": "an array"}));

        let (_dir, dst) = provider();
        let err = dst
            .import_signing_keys(&exp, &ImportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(_)),
            "expected InvalidFormat, got {err:?}"
        );
    }

    #[tokio::test]
    async fn import_signing_keys_conflict_writes_nothing() {
        let (_dir, dst) = provider();
        dst.set_signing_key("0200aa", b"old").await.unwrap();
        // The new key comes first, so an import that writes before it meets
        // the conflict leaves that key behind.
        let records = [
            KeyRecord {
                address: "0200bb".into(),
                data: b"new".to_vec(),
            },
            KeyRecord {
                address: "0200aa".into(),
                data: b"other".to_vec(),
            },
        ];
        let exp = envelope(FORMAT_KEYS, &records);

        let err = dst
            .import_signing_keys(&exp, &ImportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::ImportConflict(ref a) if a == "0200aa"),
            "expected ImportConflict(0200aa), got {err:?}"
        );
        assert_eq!(dst.get_signing_key("0200bb").await.unwrap(), None);
        assert_eq!(
            dst.get_signing_key("0200aa").await.unwrap().as_deref(),
            Some(&b"old"[..])
        );
    }

    #[tokio::test]
    async fn mark_failed_errors_on_unknown_hash() {
        // Matches `confirm`'s SnapshotNotFound semantics so a typo doesn't
        // silently no-op.
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        let err = p.mark_failed("0200aa", ext(42)).await.unwrap_err();
        assert!(
            matches!(err, PrivateStateError::SnapshotNotFound { .. }),
            "expected SnapshotNotFound, got {err:?}"
        );
    }

    #[tokio::test]
    async fn confirm_is_idempotent_but_rejects_conflicting_reconfirm() {
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        p.confirm("0200aa", ext(1), Some(42), [0xbb; 32])
            .await
            .unwrap();
        // Same block info: idempotent Ok.
        p.confirm("0200aa", ext(1), Some(42), [0xbb; 32])
            .await
            .unwrap();
        // Same block hash but height absent: still idempotent, since the
        // hash uniquely identifies the block and `None` is just a less
        // populated copy of the same record.
        p.confirm("0200aa", ext(1), None, [0xbb; 32]).await.unwrap();
        let snap = &p.snapshots("0200aa").await.unwrap()[0];
        assert_eq!(snap.block_height, Some(42));
        // Different block hash: refuse.
        let err = p
            .confirm("0200aa", ext(1), Some(42), [0xcc; 32])
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("already confirmed")),
            "expected InvalidFormat(already confirmed...), got {err:?}"
        );
    }

    #[tokio::test]
    async fn confirm_upgrades_none_height_to_some_when_block_hash_matches() {
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        // First confirm with no height (the typical Contract::call_with
        // path: subxt's TxInBlock only exposes the hash).
        p.confirm("0200aa", ext(1), None, [0xbb; 32]).await.unwrap();
        assert_eq!(p.snapshots("0200aa").await.unwrap()[0].block_height, None);
        // Later out-of-band reconciliation fills in the height. The block
        // hash is the same, so this upgrades the record rather than
        // erroring on Some vs None mismatch.
        p.confirm("0200aa", ext(1), Some(42), [0xbb; 32])
            .await
            .unwrap();
        assert_eq!(
            p.snapshots("0200aa").await.unwrap()[0].block_height,
            Some(42)
        );
    }

    #[tokio::test]
    async fn find_leaf_detects_isolated_cycle_alongside_root_leaf() {
        // {A_clean_root, B<->C}: A is the only non-referenced snapshot, so
        // a leaf-only check returns A. The reachability check must catch
        // that {B, C} are unreachable from any root via depends_on and
        // surface InvalidFormat.
        let (_dir, p) = provider();
        for s in [
            pending(0xAA, None, b"a"),
            pending(0xBB, Some(0xCC), b"b"),
            pending(0xCC, Some(0xBB), b"c"),
        ] {
            write_raw(&p, "0200aa", &s);
        }
        let err = p.head("0200aa").await.unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("cycle (every unreachable snapshot's parent")),
            "expected InvalidFormat(cycle...), got {err:?}"
        );
    }

    #[tokio::test]
    async fn find_leaf_distinguishes_orphan_chain_from_cycle() {
        // {C<-MISSING}: a single snapshot whose `depends_on` points at a
        // hash not in the journal. Single leaf (C), so the branching
        // check passes. Reachable={} (no roots), total=1; the orphan-
        // chain branch fires because C's parent is missing from the
        // journal. The cycle case (all parents present) is covered by
        // `find_leaf_detects_isolated_cycle_alongside_root_leaf`.
        let (_dir, p) = provider();
        write_raw(&p, "0200aa", &pending(0xCC, Some(0xFF), b"c"));
        let err = p.head("0200aa").await.unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("orphan chain")),
            "expected InvalidFormat(orphan chain...), got {err:?}"
        );
    }

    #[tokio::test]
    async fn import_rejects_uppercase_hex() {
        let bad = vec![ExportEntry {
            address: "0200aa".into(),
            snapshot: Snapshot {
                // 64 chars, valid hex content, but uppercase: would not be
                // byte-equal to an internally produced lowercase encoding
                // of the same bytes.
                extrinsic_hash: "AA".repeat(32),
                ..pending(0, None, b"x")
            },
        }];
        let exp = envelope(FORMAT_STATES, &bad);
        let (_dir, dst) = provider();
        let err = dst
            .import_private_states(&exp, &ImportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("lowercase hex")),
            "expected InvalidFormat(lowercase hex), got {err:?}"
        );
    }

    #[tokio::test]
    async fn ensure_address_marker_rejects_mismatched_existing_marker() {
        let (_dir, p) = provider();
        // First write seeds the directory + marker via append_pending.
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        // Corrupt the marker by hand.
        let dir = p.address_dir("0200aa");
        fs::write(dir.join(ADDRESS_MARKER), "0200ff").unwrap();
        // Second write to the same address must reject rather than
        // silently let the stale marker persist.
        let err = p
            .append_pending("0200aa", ext(2), Some(ext(1)), b"s2")
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("address marker") && m.contains("does not match")
                    || m.contains("address marker") && m.contains("holds")),
            "expected InvalidFormat(address marker mismatch), got {err:?}"
        );
    }

    #[tokio::test]
    async fn export_rejects_tampered_marker() {
        let (_dir, p) = provider();
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        // Tamper with the marker so its sha256 no longer matches the dir.
        let dir = p.address_dir("0200aa");
        fs::write(dir.join(ADDRESS_MARKER), "evil-address").unwrap();
        let err = p
            .export_private_states(&ExportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("sha256") && m.contains("does not match")),
            "expected InvalidFormat(sha256 mismatch), got {err:?}"
        );
    }

    #[tokio::test]
    async fn export_signing_keys_rejects_filename_mismatch() {
        let (_dir, p) = provider();
        p.set_signing_key("0200aa", b"k").await.unwrap();
        // Edit the JSON address field so its sha256 no longer matches the
        // filename. The export must reject rather than rebind the key.
        let key_path = p.key_path("0200aa");
        let mut rec: KeyRecord = read_json_opt(&key_path).unwrap().unwrap();
        rec.address = "0200ff".into();
        p.write_json_atomic(&key_path, &rec).unwrap();
        let err = p
            .export_signing_keys(&ExportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("does not match its filename hash")),
            "expected InvalidFormat(filename mismatch), got {err:?}"
        );
    }

    #[tokio::test]
    async fn import_rejects_post_merge_branching() {
        // Destination already has B<-A. Import a payload containing
        // C<-A. The merged journal has two leaves under A; the importer
        // must reject so the user discovers the breakage at the import
        // call site instead of the next call_with.
        let (_src_dir, src) = provider();
        // Populate the destination's pre-import state by importing a
        // single-tx history to make sure we end up with a valid sibling
        // path; the helper does that fine.
        src.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        src.append_pending("0200aa", ext(2), Some(ext(1)), b"s2")
            .await
            .unwrap();
        let dst_exp = src
            .export_private_states(&ExportOptions::new(PW))
            .await
            .unwrap();

        let (_dst_dir, dst) = provider();
        dst.import_private_states(&dst_exp, &ImportOptions::new(PW))
            .await
            .unwrap();
        // Now build an external payload with C<-ext(1) (a second child of A).
        let bad = vec![ExportEntry {
            address: "0200aa".into(),
            snapshot: pending(3, Some(1), b"s3"),
        }];
        let exp = envelope(FORMAT_STATES, &bad);
        let err = dst
            .import_private_states(&exp, &ImportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("malformed journal") && m.contains("0200aa")),
            "expected InvalidFormat(malformed journal at 0200aa), got {err:?}"
        );
    }

    #[tokio::test]
    async fn import_overwrite_refuses_to_downgrade_confirmed_to_pending() {
        // Destination has X Confirmed with a recorded block. An older
        // backup tries to overwrite with X Pending. The destination's
        // block_hash / block_height would be lost; refuse before any
        // write.
        let (_dir, dst) = provider();
        dst.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        dst.confirm("0200aa", ext(1), Some(42), [0xbb; 32])
            .await
            .unwrap();

        let payload = vec![ExportEntry {
            address: "0200aa".into(),
            snapshot: pending(1, None, b"s1"),
        }];
        let exp = envelope(FORMAT_STATES, &payload);
        let err = dst
            .import_private_states(
                &exp,
                &ImportOptions::new(PW).with_conflict(ConflictStrategy::Overwrite),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("would downgrade a Confirmed snapshot")),
            "expected InvalidFormat(would downgrade...), got {err:?}"
        );
        // Destination remained intact.
        let snap = &dst.snapshots("0200aa").await.unwrap()[0];
        assert_eq!(snap.status, SnapshotStatus::Confirmed);
        assert_eq!(snap.block_height, Some(42));
    }

    #[tokio::test]
    async fn import_skip_still_validates_destination_journal_health() {
        // Destination has a pre-existing branched journal at address A
        // (two leaves sharing a parent, constructed by direct file
        // writes). A Skip-strategy import whose entries collide with
        // existing snapshots no-ops every write, but the importer must
        // still surface the pre-existing branching so the user discovers
        // the breakage at the import call site instead of at the next
        // call_with.
        let (_dir, dst) = provider();
        // A is the shared root; B and C are siblings under A.
        for (h, parent) in [(1, None), (2, Some(1)), (3, Some(1))] {
            write_raw(&dst, "0200aa", &pending(h, parent, b"x"));
        }

        // Skip-strategy import containing only an entry that already
        // exists (ext(1)). Every iteration takes the Skip arm.
        let payload = vec![ExportEntry {
            address: "0200aa".into(),
            snapshot: pending(1, None, b"x"),
        }];
        let exp = envelope(FORMAT_STATES, &payload);
        let err = dst
            .import_private_states(
                &exp,
                &ImportOptions::new(PW).with_conflict(ConflictStrategy::Skip),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("malformed journal at 0200aa")
                    && m.contains("branching")),
            "expected InvalidFormat(malformed journal at 0200aa, branching), got {err:?}"
        );
    }

    /// One seed's private state can hold its secrets, so a second wallet on
    /// the same machine must start from an empty journal.
    #[tokio::test]
    async fn for_wallet_isolates_wallets_and_survives_a_restart() {
        let base = tempfile::TempDir::new().unwrap();
        let a = FsPrivateStateProvider::for_wallet(base.path(), "wallet-a");
        a.append_pending("0200aa", ext(1), None, b"a-secret")
            .await
            .unwrap();
        a.confirm("0200aa", ext(1), None, ext(9)).await.unwrap();

        let b = FsPrivateStateProvider::for_wallet(base.path(), "wallet-b");
        assert_eq!(b.head("0200aa").await.unwrap(), None);

        let a2 = FsPrivateStateProvider::for_wallet(base.path(), "wallet-a");
        assert_eq!(a2.head("0200aa").await.unwrap(), Some(b"a-secret".to_vec()));
    }

    /// The `with_default_dir` warning is the only pointer to a store from
    /// before the per-wallet layout. Either old directory alone must trigger
    /// it, and a per-wallet store under the same base must not.
    #[tokio::test]
    async fn unscoped_store_exists_detects_only_the_old_layout() {
        let states_only = tempfile::TempDir::new().unwrap();
        FsPrivateStateProvider::new(states_only.path())
            .append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        assert!(
            unscoped_store_exists(states_only.path()),
            "an old store with only private state"
        );

        let keys_only = tempfile::TempDir::new().unwrap();
        FsPrivateStateProvider::new(keys_only.path())
            .set_signing_key("0200aa", b"k")
            .await
            .unwrap();
        assert!(
            unscoped_store_exists(keys_only.path()),
            "an old store with only signing keys"
        );

        let per_wallet = tempfile::TempDir::new().unwrap();
        let w = FsPrivateStateProvider::for_wallet(per_wallet.path(), "wallet-a");
        w.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        w.set_signing_key("0200aa", b"k").await.unwrap();
        assert!(
            !unscoped_store_exists(per_wallet.path()),
            "a per-wallet store under the base"
        );
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The journals and the signing keys are plaintext. Written at the
    /// process umask, they are readable by every local user.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_store_is_owner_only() {
        let base = tempfile::TempDir::new().unwrap();
        let p = FsPrivateStateProvider::for_wallet(base.path(), "wallet-a");
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        p.set_signing_key("0200aa", b"k").await.unwrap();

        let address_dir = p.address_dir("0200aa");
        let snapshot = p
            .find_snapshot_path("0200aa", &hex::encode(ext(1)))
            .unwrap()
            .unwrap();
        for (path, mode) in [
            (p.root.clone(), 0o700),
            (p.states_dir(), 0o700),
            (address_dir.clone(), 0o700),
            (address_dir.join(ADDRESS_MARKER), 0o600),
            (snapshot, 0o600),
            (p.keys_dir(), 0o700),
            (p.key_path("0200aa"), 0o600),
        ] {
            let actual = mode_of(&path);
            assert_eq!(
                actual,
                mode,
                "{} reads {actual:o}, expected {mode:o}",
                path.display()
            );
        }
    }

    /// A store that an earlier version wrote at the umask keeps its modes
    /// until a write narrows them, because a mode given at create does not
    /// touch what already exists. This includes a temporary file that a crash
    /// left before its rename, which the next write of that snapshot reuses.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_next_write_narrows_an_existing_store() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::TempDir::new().unwrap();
        let p = FsPrivateStateProvider::for_wallet(base.path(), "wallet-a");
        p.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        let snapshot = p
            .find_snapshot_path("0200aa", &hex::encode(ext(1)))
            .unwrap()
            .unwrap();
        fs::set_permissions(&p.root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&snapshot, fs::Permissions::from_mode(0o644)).unwrap();
        let stale_tmp = snapshot.with_extension("tmp");
        fs::write(&stale_tmp, b"{}").unwrap();
        fs::set_permissions(&stale_tmp, fs::Permissions::from_mode(0o644)).unwrap();

        p.confirm("0200aa", ext(1), None, ext(9)).await.unwrap();

        assert_eq!(mode_of(&p.root), 0o700);
        assert_eq!(mode_of(&snapshot), 0o600);
    }
}
