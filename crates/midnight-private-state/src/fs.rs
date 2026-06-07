//! Filesystem-backed [`PrivateStateProvider`].
//!
//! Layout (one directory per contract address; one file per snapshot):
//!
//! ```text
//! <root>/
//!   states/
//!     <sha256(address)>/
//!       address.txt                                # plaintext address marker (for export)
//!       <020-padded-unix-nanos>-<extrinsic_hash_hex>.json
//!         { status, extrinsicHash, blockHeight?, blockHash?, dependsOn?, data: base64 }
//!   signing-keys/
//!     <sha256(address)>.json   { address, data: base64 }
//! ```
//!
//! Snapshot filenames are prefixed with a 020-padded unix-nanos timestamp
//! mostly for human inspection (sorting a directory listing gives an
//! append-time order). The journal head is found by walking the `dependsOn`
//! graph (the leaf snapshot is the one nothing else depends on), not by
//! filename, so `head` stays correct after `import_private_states` rewrites
//! filenames with new timestamps. `mark_failed` / `rollback_from` likewise
//! walk the graph to cascade through dependents.
//!
//! Writes go to a `.tmp` sibling and are `rename`d into place, so a crash
//! never leaves a half-written file. The wallet uses the same discipline.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tracing::debug;

use crate::{
    ConflictStrategy, EncryptedExport, ExportOptions, FORMAT_KEYS, FORMAT_STATES, ImportOptions,
    ImportResult, MIN_PASSWORD_LEN, PrivateStateError, PrivateStateProvider, Snapshot,
    SnapshotStatus, crypto,
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
    data: String,
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

/// Filesystem [`PrivateStateProvider`]. State lives under `<root>/states/`
/// (one directory per address) and signing keys under `<root>/signing-keys/`,
/// plaintext at rest. Default root is `~/.midnight/private-state/`.
#[derive(Debug, Clone)]
pub struct FsPrivateStateProvider {
    root: PathBuf,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

impl FsPrivateStateProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_dir() -> Option<PathBuf> {
        home_dir().map(|h| h.join(".midnight").join("private-state"))
    }

    pub fn with_default_dir() -> Option<Self> {
        Self::default_dir().map(Self::new)
    }

    fn states_dir(&self) -> PathBuf {
        self.root.join(STATES_SUBDIR)
    }

    fn keys_dir(&self) -> PathBuf {
        self.root.join(KEYS_SUBDIR)
    }

    /// `<states>/<sha256(address)>/`, the per-address journal directory.
    fn address_dir(&self, address: &str) -> PathBuf {
        self.states_dir()
            .join(hex::encode(Sha256::digest(address.as_bytes())))
    }

    /// `<keys>/<sha256(address)>.json`, the flat signing-key file.
    fn key_path(&self, address: &str) -> PathBuf {
        self.keys_dir().join(format!(
            "{}.json",
            hex::encode(Sha256::digest(address.as_bytes()))
        ))
    }

    fn snapshot_filename(extrinsic_hash: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        // 020-pad covers nanos through year ~2554; lexicographic sort matches
        // chronological order.
        format!("{nanos:020}-{extrinsic_hash}.json")
    }

    /// Read every snapshot file under `<address>/`, oldest first.
    fn load_snapshots(&self, address: &str) -> Result<Vec<(PathBuf, Snapshot)>, PrivateStateError> {
        load_snapshots_in(&self.address_dir(address))
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
        let ext_hex = hex::encode(extrinsic_hash);
        let dir = self.address_dir(address);
        ensure_address_marker(&dir, address)?;
        // Reject duplicates so we never write two files for the same tx id.
        // `find_snapshot_path` is the same lookup `confirm` / `mark_failed`
        // use, so this guarantees those operations remain unambiguous.
        if self.find_snapshot_path(address, &ext_hex)?.is_some() {
            return Err(PrivateStateError::SnapshotAlreadyExists {
                address: address.to_string(),
                extrinsic_hash: ext_hex,
            });
        }
        let snapshot = Snapshot {
            status: SnapshotStatus::Pending,
            extrinsic_hash: ext_hex.clone(),
            block_hash: None,
            block_height: None,
            depends_on: depends_on.map(hex::encode),
            data: state.to_vec(),
        };
        let path = dir.join(Self::snapshot_filename(&ext_hex));
        write_json_atomic(&path, &snapshot)?;
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
        // Confirmed is a terminal state: a second confirm with the same
        // (block_height, block_hash) is idempotent, but a conflicting record
        // must not silently overwrite the durable one.
        if snap.status == SnapshotStatus::Confirmed {
            let same_hash = snap.block_hash.as_deref() == Some(&block_hash_hex);
            let same_height = snap.block_height == block_height;
            if same_hash && same_height {
                return Ok(());
            }
            return Err(PrivateStateError::InvalidFormat(format!(
                "snapshot already confirmed with a different block; \
                 address={address}, extrinsic_hash={ext_hex}, \
                 existing=(height={:?}, hash={:?}), \
                 requested=(height={block_height:?}, hash={block_hash_hex})",
                snap.block_height, snap.block_hash,
            )));
        }
        snap.status = SnapshotStatus::Confirmed;
        snap.block_height = block_height;
        snap.block_hash = Some(block_hash_hex);
        write_json_atomic(&path, &snap)?;
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
        let ext_hex = hex::encode(extrinsic_hash);
        // Surface SnapshotNotFound for unknown hashes so callers see a
        // consistent error variant across confirm / mark_failed /
        // rollback_from. cascade_drop's own early-return semantics aren't
        // load-bearing here.
        if self.find_snapshot_path(address, &ext_hex)?.is_none() {
            return Err(PrivateStateError::SnapshotNotFound {
                address: address.to_string(),
                extrinsic_hash: ext_hex,
            });
        }
        cascade_drop(self, address, &ext_hex)?;
        debug!(address, extrinsic_hash = %ext_hex, "marked snapshot failed");
        Ok(())
    }

    async fn head(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError> {
        Ok(self.head_snapshot(address)?.map(|s| s.data))
    }

    async fn head_extrinsic(&self, address: &str) -> Result<Option<[u8; 32]>, PrivateStateError> {
        self.head_snapshot(address)?
            .map(|s| parse_hash(&s.extrinsic_hash))
            .transpose()
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
        let ext_hex = hex::encode(extrinsic_hash);
        if self.find_snapshot_path(address, &ext_hex)?.is_none() {
            return Err(PrivateStateError::SnapshotNotFound {
                address: address.to_string(),
                extrinsic_hash: ext_hex,
            });
        }
        cascade_drop(self, address, &ext_hex)?;
        Ok(())
    }

    /// Single-read override of the default trait impl: both fields come from
    /// the same `head_snapshot` call so a concurrent `append_pending` can't
    /// produce a torn read where `data` and `extrinsic_hash` come from
    /// different journal versions.
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
            data: encode_b64(key),
        };
        write_json_atomic(&self.key_path(address), &rec)
    }

    async fn get_signing_key(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError> {
        match read_json_opt::<KeyRecord>(&self.key_path(address))? {
            Some(rec) => Ok(Some(decode_b64(&rec.data)?)),
            None => Ok(None),
        }
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
        if opts.password.chars().count() < MIN_PASSWORD_LEN {
            return Err(PrivateStateError::PasswordTooShort);
        }
        let entries = collect_export_entries(self)?;
        if entries.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }
        let payload = serde_json::to_vec(&entries)
            .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
        let (salt, ciphertext) =
            crypto::encrypt(&opts.password, FORMAT_STATES.as_bytes(), &payload)?;
        debug!(count = entries.len(), "exported snapshot journal");
        Ok(EncryptedExport {
            format: FORMAT_STATES.to_string(),
            salt,
            ciphertext,
        })
    }

    async fn import_private_states(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        if data.format != FORMAT_STATES {
            return Err(PrivateStateError::InvalidFormat(format!(
                "expected format {FORMAT_STATES}, got {}",
                data.format
            )));
        }
        let payload = crypto::decrypt(
            &opts.password,
            FORMAT_STATES.as_bytes(),
            &data.salt,
            &data.ciphertext,
        )?;
        let entries: Vec<ExportEntry> = serde_json::from_slice(&payload).map_err(|e| {
            PrivateStateError::InvalidFormat(format!("decoded payload is not a snapshot list: {e}"))
        })?;
        if entries.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }
        apply_import_entries(self, entries, opts.conflict)
    }

    async fn export_signing_keys(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError> {
        if opts.password.chars().count() < MIN_PASSWORD_LEN {
            return Err(PrivateStateError::PasswordTooShort);
        }
        let mut records: Vec<KeyRecord> = Vec::new();
        let rd = match fs::read_dir(self.keys_dir()) {
            Ok(rd) => Some(rd),
            // Missing keys dir is "nothing to export"; any other read_dir
            // failure (permission denied, I/O error) is real and must
            // surface, not silently produce an empty backup.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(PrivateStateError::Io(format!(
                    "read_dir {}: {e}",
                    self.keys_dir().display()
                )));
            }
        };
        if let Some(rd) = rd {
            for entry in rd {
                let path = entry
                    .map_err(|e| PrivateStateError::Io(e.to_string()))?
                    .path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                if let Some(rec) = read_json_opt::<KeyRecord>(&path)? {
                    // Validate base64 up front so a corrupt on-disk record
                    // doesn't escape into the export and break the importer.
                    decode_b64(&rec.data).map_err(|_| {
                        PrivateStateError::InvalidFormat(format!(
                            "malformed signing-key record on disk at {}",
                            path.display()
                        ))
                    })?;
                    // Cross-check the filename: a signing-key file at
                    // `<sha256(address)>.json` whose JSON `address` field
                    // has been edited would otherwise migrate the key to
                    // the edited address through an export/import
                    // round-trip.
                    let expected_stem = hex::encode(Sha256::digest(rec.address.as_bytes()));
                    let actual_stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default();
                    if actual_stem != expected_stem {
                        return Err(PrivateStateError::InvalidFormat(format!(
                            "signing-key file at {} has an address that does not \
                             match its filename hash (filename={actual_stem}, \
                             record address hashes to={expected_stem})",
                            path.display(),
                        )));
                    }
                    records.push(rec);
                }
            }
        }
        if records.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }
        let payload = serde_json::to_vec(&records)
            .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
        let (salt, ciphertext) = crypto::encrypt(&opts.password, FORMAT_KEYS.as_bytes(), &payload)?;
        Ok(EncryptedExport {
            format: FORMAT_KEYS.to_string(),
            salt,
            ciphertext,
        })
    }

    async fn import_signing_keys(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        if data.format != FORMAT_KEYS {
            return Err(PrivateStateError::InvalidFormat(format!(
                "expected format {FORMAT_KEYS}, got {}",
                data.format
            )));
        }
        let payload = crypto::decrypt(
            &opts.password,
            FORMAT_KEYS.as_bytes(),
            &data.salt,
            &data.ciphertext,
        )?;
        let records: Vec<KeyRecord> = serde_json::from_slice(&payload).map_err(|e| {
            PrivateStateError::InvalidFormat(format!("malformed import payload: {e}"))
        })?;
        if records.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }

        // Resolve each record to its path; eagerly base64-decode so a corrupt
        // entry aborts before any file is written.
        let mut resolved = Vec::with_capacity(records.len());
        for rec in records {
            decode_b64(&rec.data)?;
            let path = self.key_path(&rec.address);
            resolved.push((path, rec));
        }
        reject_duplicate_paths(&resolved)?;

        if opts.conflict == ConflictStrategy::Error {
            if let Some((_, rec)) = resolved.iter().find(|(p, _)| p.exists()) {
                return Err(PrivateStateError::ImportConflict(rec.address.clone()));
            }
        }

        let mut result = ImportResult::default();
        for (path, rec) in &resolved {
            if path.exists() {
                match opts.conflict {
                    ConflictStrategy::Skip => {
                        result.skipped += 1;
                        continue;
                    }
                    ConflictStrategy::Overwrite => result.overwritten += 1,
                    ConflictStrategy::Error => {
                        return Err(PrivateStateError::ImportConflict(rec.address.clone()));
                    }
                }
            } else {
                result.imported += 1;
            }
            write_json_atomic(path, rec)?;
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
    // child edges; anything not reached belongs to a cycle. We can't
    // reuse `topo_sort` here because it intentionally appends orphans so
    // the snapshot inventory is complete.
    use std::collections::HashMap;
    use std::collections::VecDeque;
    let by_hash: HashMap<&str, &Snapshot> = snapshots
        .iter()
        .map(|s| (s.extrinsic_hash.as_str(), s))
        .collect();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut roots: Vec<&str> = Vec::new();
    for s in &snapshots {
        match s.depends_on.as_deref() {
            None => roots.push(s.extrinsic_hash.as_str()),
            Some(parent) => children
                .entry(parent)
                .or_default()
                .push(s.extrinsic_hash.as_str()),
        }
    }
    let mut reachable: HashSet<&str> = HashSet::with_capacity(total);
    let mut q: VecDeque<&str> = roots.into();
    while let Some(h) = q.pop_front() {
        if !reachable.insert(h) {
            continue;
        }
        if let Some(cs) = children.get(h) {
            for c in cs {
                q.push_back(c);
            }
        }
    }
    if reachable.len() != total {
        let mut orphans: Vec<String> = by_hash
            .keys()
            .filter(|h| !reachable.contains(*h))
            .map(|h| h.to_string())
            .collect();
        orphans.sort();
        // Distinguish two unreachable shapes so the message points the
        // user at the right tool. "No roots at all" means every snapshot
        // has a `depends_on` parent missing from the journal (corrupted
        // import / hand-edited file). The classic isolated-cycle case is
        // a valid root-leaf chain coexisting with a self-referential
        // subgraph.
        let reason = if reachable.is_empty() {
            "no root snapshot (every snapshot's `depends_on` points at a \
             parent missing from the journal)"
        } else {
            "isolated cycle alongside a valid root-leaf chain"
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
    use std::collections::HashMap;
    use std::collections::VecDeque;

    let mut by_hash: HashMap<String, Snapshot> = snapshots
        .into_iter()
        .map(|s| (s.extrinsic_hash.clone(), s))
        .collect();

    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    let mut roots: Vec<String> = Vec::new();
    for s in by_hash.values() {
        match &s.depends_on {
            None => roots.push(s.extrinsic_hash.clone()),
            Some(parent) => children
                .entry(parent.clone())
                .or_default()
                .push(s.extrinsic_hash.clone()),
        }
    }
    roots.sort();
    for v in children.values_mut() {
        v.sort();
    }

    let mut out = Vec::with_capacity(by_hash.len());
    let mut q: VecDeque<String> = roots.into();
    while let Some(h) = q.pop_front() {
        if let Some(s) = by_hash.remove(&h) {
            if let Some(cs) = children.remove(&h) {
                for c in cs {
                    q.push_back(c);
                }
            }
            out.push(s);
        }
    }
    // Orphans (snapshot whose `depends_on` points at a missing parent) won't
    // be reached by the BFS; append them in hash order so they're still
    // included in the inventory.
    let mut orphans: Vec<Snapshot> = by_hash.into_values().collect();
    orphans.sort_by(|a, b| a.extrinsic_hash.cmp(&b.extrinsic_hash));
    out.extend(orphans);
    out
}

/// Cascade-drop a snapshot and every snapshot that transitively depends on
/// it. Walks the dependency graph (`depends_on` edges) starting from
/// `start_hash`, collects every reachable extrinsic_hash, then deletes the
/// corresponding files. A no-op if `start_hash` is not present.
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
        return Ok(());
    }

    let mut failed: HashSet<String> = HashSet::new();
    failed.insert(start_hash.to_string());
    loop {
        let mut grew = false;
        for (_, snap) in &snapshots {
            if failed.contains(&snap.extrinsic_hash) {
                continue;
            }
            if let Some(parent) = &snap.depends_on
                && failed.contains(parent)
            {
                failed.insert(snap.extrinsic_hash.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    // Best-effort delete: a single permission-denied / disk-full failure
    // mid-cascade would otherwise leave the journal partially dropped with
    // orphans whose `depends_on` points at a missing parent. Continue
    // iterating, collect every failure, and surface them together so the
    // caller can retry or diagnose without losing the rest of the work.
    let mut errors: Vec<String> = Vec::new();
    for (path, snap) in snapshots {
        if failed.contains(&snap.extrinsic_hash) {
            if let Err(e) = remove_file_opt(&path) {
                errors.push(format!("{} ({}): {e}", snap.extrinsic_hash, path.display()));
            }
        }
    }
    if !errors.is_empty() {
        return Err(PrivateStateError::Io(format!(
            "cascade_drop on {address} failed for {} snapshot(s): [{}]",
            errors.len(),
            errors.join("; ")
        )));
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
    let states_dir = provider.states_dir();
    let rd = match fs::read_dir(&states_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            return Err(PrivateStateError::Io(format!(
                "read dir {}: {e}",
                states_dir.display()
            )));
        }
    };
    for entry in rd {
        let dir = entry
            .map_err(|e| PrivateStateError::Io(e.to_string()))?
            .path();
        if !dir.is_dir() {
            continue;
        }
        let addr_marker = dir.join(ADDRESS_MARKER);
        let address = match fs::read_to_string(&addr_marker) {
            Ok(s) => s.trim().to_string(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(PrivateStateError::Io(format!(
                    "read {}: {e}",
                    addr_marker.display()
                )));
            }
        };
        // Cross-check the marker against the directory name: the directory
        // hash is the source of truth, so a tampered or stale marker must
        // not silently rebind snapshots through an export/import round-trip.
        let expected_dir = hex::encode(Sha256::digest(address.as_bytes()));
        let actual_dir = dir.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        if actual_dir != expected_dir {
            return Err(PrivateStateError::InvalidFormat(format!(
                "address marker at {} holds {:?} whose sha256 does not match \
                 the directory name (directory={actual_dir}, expected hash \
                 for marker={expected_dir})",
                addr_marker.display(),
                address,
            )));
        }
        for (_, snap) in load_snapshots_in(&dir)? {
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
/// Validation up front (detect-before-mutate):
///
/// 1. Every snapshot's `extrinsic_hash` / `depends_on` / `block_hash` parses
///    as a 32-byte hex string. A malformed payload aborts with
///    `InvalidFormat` before any file is written.
/// 2. The payload itself contains no duplicate `(address, extrinsic_hash)`
///    pairs. Without this guard a duplicate could partial-write under
///    `ConflictStrategy::Error` (first entry writes, second hits an existing
///    file and bails out).
fn apply_import_entries(
    provider: &FsPrivateStateProvider,
    entries: Vec<ExportEntry>,
    conflict: ConflictStrategy,
) -> Result<ImportResult, PrivateStateError> {
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

    // Pass 3: detect-before-mutate for the `Error` strategy.
    if conflict == ConflictStrategy::Error {
        for entry in &entries {
            if provider
                .find_snapshot_path(&entry.address, &entry.snapshot.extrinsic_hash)?
                .is_some()
            {
                return Err(PrivateStateError::ImportConflict(format!(
                    "{} / {}",
                    entry.address, entry.snapshot.extrinsic_hash
                )));
            }
        }
    }

    let mut result = ImportResult::default();
    // Track which addresses the import touched so we can validate the
    // post-import journal once writes finish.
    let mut touched: HashSet<String> = HashSet::new();
    for entry in entries {
        let dir = provider.address_dir(&entry.address);
        ensure_address_marker(&dir, &entry.address)?;
        let existing =
            provider.find_snapshot_path(&entry.address, &entry.snapshot.extrinsic_hash)?;
        let path = match (existing, conflict) {
            (Some(_), ConflictStrategy::Skip) => {
                result.skipped += 1;
                continue;
            }
            (Some(p), ConflictStrategy::Overwrite) => {
                result.overwritten += 1;
                p
            }
            (Some(_), ConflictStrategy::Error) => {
                return Err(PrivateStateError::ImportConflict(format!(
                    "{} / {}",
                    entry.address, entry.snapshot.extrinsic_hash
                )));
            }
            (None, _) => {
                result.imported += 1;
                dir.join(FsPrivateStateProvider::snapshot_filename(
                    &entry.snapshot.extrinsic_hash,
                ))
            }
        };
        write_json_atomic(&path, &entry.snapshot)?;
        touched.insert(entry.address);
    }

    // Pass 4: post-import journal validation. Merging a well-formed export
    // into a non-empty destination can silently produce a branching or
    // cyclic journal (e.g., destination has B<-A, payload has C<-A, post
    // import the journal has two leaves under A). Run `find_leaf` on every
    // touched address so the user discovers the breakage here, where the
    // failed import can be unwound with `mark_failed` or `rollback_from`,
    // rather than at the next `call_with` where the contract path bails out
    // with the same `InvalidFormat`.
    for address in &touched {
        let snapshots: Vec<Snapshot> = provider
            .load_snapshots(address)?
            .into_iter()
            .map(|(_, s)| s)
            .collect();
        if let Err(e) = find_leaf(snapshots) {
            return Err(PrivateStateError::InvalidFormat(format!(
                "import produced a malformed journal at {address}: {e}"
            )));
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

/// Write the plaintext address to `<dir>/address.txt` if it isn't already
/// there, so an export can recover the address from a hashed directory. If
/// the marker exists but its content doesn't match `address`, error out
/// rather than silently let the wrong-address record propagate into export
/// payloads.
fn ensure_address_marker(dir: &Path, address: &str) -> Result<(), PrivateStateError> {
    fs::create_dir_all(dir)
        .map_err(|e| PrivateStateError::Io(format!("create dir {}: {e}", dir.display())))?;
    let marker = dir.join(ADDRESS_MARKER);
    if marker.exists() {
        let existing = fs::read_to_string(&marker)
            .map_err(|e| PrivateStateError::Io(format!("read {}: {e}", marker.display())))?;
        if existing.trim() != address {
            return Err(PrivateStateError::InvalidFormat(format!(
                "address marker at {} holds {:?} but caller passed {:?}; the \
                 per-address directory does not match the address it was \
                 created for. Resolve by deleting the directory or repairing \
                 the marker.",
                marker.display(),
                existing.trim(),
                address,
            )));
        }
        return Ok(());
    }
    let tmp = marker.with_extension("tmp");
    fs::write(&tmp, address.as_bytes())
        .map_err(|e| PrivateStateError::Io(format!("write {}: {e}", tmp.display())))?;
    fs::rename(&tmp, &marker)
        .map_err(|e| PrivateStateError::Io(format!("rename {}: {e}", marker.display())))?;
    Ok(())
}

/// Load every snapshot file under `dir`, oldest first. Free function so the
/// export path (which has only the directory path) can reuse it.
fn load_snapshots_in(dir: &Path) -> Result<Vec<(PathBuf, Snapshot)>, PrivateStateError> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(PrivateStateError::Io(format!(
                "read dir {}: {e}",
                dir.display()
            )));
        }
    };
    let mut out: Vec<(PathBuf, Snapshot)> = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| PrivateStateError::Io(e.to_string()))?
            .path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Some(snap) = read_json_opt::<Snapshot>(&path)? {
            out.push((path, snap));
        }
    }
    // Filenames begin with a 020-padded nanos timestamp, so lexicographic
    // sort is chronological.
    out.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));
    Ok(out)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), PrivateStateError> {
    let dir = path
        .parent()
        .ok_or_else(|| PrivateStateError::Io("path has no parent directory".into()))?;
    fs::create_dir_all(dir)
        .map_err(|e| PrivateStateError::Io(format!("create dir {}: {e}", dir.display())))?;

    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;

    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &json)
        .map_err(|e| PrivateStateError::Io(format!("write {}: {e}", tmp.display())))?;
    fs::rename(&tmp, path)
        .map_err(|e| PrivateStateError::Io(format!("rename into {}: {e}", path.display())))?;
    Ok(())
}

fn read_json_opt<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, PrivateStateError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| PrivateStateError::Serialize(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PrivateStateError::Io(format!(
            "read {}: {e}",
            path.display()
        ))),
    }
}

fn remove_file_opt(path: &Path) -> Result<(), PrivateStateError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(PrivateStateError::Io(format!(
            "remove {}: {e}",
            path.display()
        ))),
    }
}

fn clear_dir(dir: &Path) -> Result<(), PrivateStateError> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(PrivateStateError::Io(format!(
            "clear {}: {e}",
            dir.display()
        ))),
    }
}

fn encode_b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_b64(s: &str) -> Result<Vec<u8>, PrivateStateError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| PrivateStateError::InvalidFormat(format!("data is not base64: {e}")))
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
    async fn rollback_from_drops_snapshot_and_descendants() {
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
        p.rollback_from("0200aa", ext(2)).await.unwrap();
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
        let (_src_dir, src) = provider();
        src.append_pending("0200aa", ext(1), None, b"s1")
            .await
            .unwrap();
        src.append_pending("0200aa", ext(2), Some(ext(1)), b"s2")
            .await
            .unwrap();
        src.append_pending("0200aa", ext(3), Some(ext(2)), b"s3")
            .await
            .unwrap();

        let exp = src
            .export_private_states(&ExportOptions::new(PW))
            .await
            .unwrap();

        let (_dst_dir, dst) = provider();
        dst.import_private_states(&exp, &ImportOptions::new(PW))
            .await
            .unwrap();

        // After import, the leaf of the depends_on chain is still s3, even
        // though filename order is now determined by import-time timestamps.
        assert_eq!(dst.head_extrinsic("0200aa").await.unwrap(), Some(ext(3)));
        assert_eq!(
            dst.head("0200aa").await.unwrap().as_deref(),
            Some(&b"s3"[..])
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
        let payload =
            crypto::decrypt(PW, FORMAT_STATES.as_bytes(), &exp.salt, &exp.ciphertext).unwrap();
        let mut entries: Vec<ExportEntry> = serde_json::from_slice(&payload).unwrap();
        entries.push(entries[0].clone());
        let dup_payload = serde_json::to_vec(&entries).unwrap();
        let (salt, ct) = crypto::encrypt(PW, FORMAT_STATES.as_bytes(), &dup_payload).unwrap();
        let dup = EncryptedExport {
            format: FORMAT_STATES.to_string(),
            salt,
            ciphertext: ct,
        };

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
                status: SnapshotStatus::Pending,
                extrinsic_hash: "not-hex".into(),
                block_hash: None,
                block_height: None,
                depends_on: None,
                data: b"bytes".to_vec(),
            },
        }];
        let payload = serde_json::to_vec(&bad).unwrap();
        let (salt, ct) = crypto::encrypt(PW, FORMAT_STATES.as_bytes(), &payload).unwrap();
        let exp = EncryptedExport {
            format: FORMAT_STATES.to_string(),
            salt,
            ciphertext: ct,
        };

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
        // Two calls to `append_pending` with the same extrinsic_hash would
        // otherwise leave two files in the directory both claiming the same
        // tx id, which makes `confirm` / `mark_failed` operate on whichever
        // one `find_snapshot_path` happens to return first.
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
    async fn export_signing_keys_rejects_corrupt_on_disk_record() {
        // A hand-edited (or otherwise corrupted) on-disk record with invalid
        // base64 in `data` must not silently propagate into an export that
        // would later fail to import.
        let (_dir, p) = provider();
        p.set_signing_key("0200aa", b"k").await.unwrap();
        // Corrupt the file in place: replace `data` with non-base64.
        let key_path = p.key_path("0200aa");
        let mut rec: KeyRecord = read_json_opt(&key_path).unwrap().unwrap();
        rec.data = "!!!not-base64!!!".into();
        write_json_atomic(&key_path, &rec).unwrap();

        let err = p
            .export_signing_keys(&ExportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(_)),
            "expected InvalidFormat, got {err:?}"
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
        // Two siblings, same parent.
        p.append_pending("0200aa", ext(2), Some(ext(1)), b"a")
            .await
            .unwrap();
        p.append_pending("0200aa", ext(3), Some(ext(1)), b"b")
            .await
            .unwrap();

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
        let addr_dir = p.address_dir("0200aa");
        fs::create_dir_all(&addr_dir).unwrap();
        // The address marker is normally written by `append_pending`; write
        // it explicitly here so the path doesn't reject the address on read.
        fs::write(addr_dir.join(ADDRESS_MARKER), "0200aa").unwrap();

        let a_hex = hex::encode(ext(0xAA));
        let b_hex = hex::encode(ext(0xBB));
        let a = Snapshot {
            status: SnapshotStatus::Pending,
            extrinsic_hash: a_hex.clone(),
            block_hash: None,
            block_height: None,
            depends_on: Some(b_hex.clone()),
            data: b"a".to_vec(),
        };
        let b = Snapshot {
            status: SnapshotStatus::Pending,
            extrinsic_hash: b_hex.clone(),
            block_hash: None,
            block_height: None,
            depends_on: Some(a_hex.clone()),
            data: b"b".to_vec(),
        };
        write_json_atomic(
            &addr_dir.join(FsPrivateStateProvider::snapshot_filename(&a_hex)),
            &a,
        )
        .unwrap();
        write_json_atomic(
            &addr_dir.join(FsPrivateStateProvider::snapshot_filename(&b_hex)),
            &b,
        )
        .unwrap();

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
        let bad_payload = serde_json::to_vec(&serde_json::json!({"not": "an array"})).unwrap();
        let (salt, ct) = crypto::encrypt(PW, FORMAT_KEYS.as_bytes(), &bad_payload).unwrap();
        let exp = EncryptedExport {
            format: FORMAT_KEYS.to_string(),
            salt,
            ciphertext: ct,
        };

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
        let err = p.rollback_from("0200aa", ext(42)).await.unwrap_err();
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
        // Different block height: refuse.
        let err = p
            .confirm("0200aa", ext(1), Some(99), [0xbb; 32])
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("already confirmed")),
            "expected InvalidFormat(already confirmed...), got {err:?}"
        );
    }

    #[tokio::test]
    async fn find_leaf_detects_isolated_cycle_alongside_root_leaf() {
        // {A_clean_root, B<->C}: A is the only non-referenced snapshot, so
        // a leaf-only check returns A. The reachability check must catch
        // that {B, C} are unreachable from any root via depends_on and
        // surface InvalidFormat.
        let (_dir, p) = provider();
        let addr_dir = p.address_dir("0200aa");
        fs::create_dir_all(&addr_dir).unwrap();
        fs::write(addr_dir.join(ADDRESS_MARKER), "0200aa").unwrap();
        let a_hex = hex::encode(ext(0xAA));
        let b_hex = hex::encode(ext(0xBB));
        let c_hex = hex::encode(ext(0xCC));
        let a = Snapshot {
            status: SnapshotStatus::Pending,
            extrinsic_hash: a_hex.clone(),
            block_hash: None,
            block_height: None,
            depends_on: None,
            data: b"a".to_vec(),
        };
        let b = Snapshot {
            status: SnapshotStatus::Pending,
            extrinsic_hash: b_hex.clone(),
            block_hash: None,
            block_height: None,
            depends_on: Some(c_hex.clone()),
            data: b"b".to_vec(),
        };
        let c = Snapshot {
            status: SnapshotStatus::Pending,
            extrinsic_hash: c_hex.clone(),
            block_hash: None,
            block_height: None,
            depends_on: Some(b_hex.clone()),
            data: b"c".to_vec(),
        };
        for (h, s) in [(&a_hex, &a), (&b_hex, &b), (&c_hex, &c)] {
            write_json_atomic(
                &addr_dir.join(FsPrivateStateProvider::snapshot_filename(h)),
                s,
            )
            .unwrap();
        }
        let err = p.head("0200aa").await.unwrap_err();
        assert!(
            matches!(err, PrivateStateError::InvalidFormat(ref m)
                if m.contains("isolated cycle")),
            "expected InvalidFormat(isolated cycle...), got {err:?}"
        );
    }

    #[tokio::test]
    async fn import_rejects_uppercase_hex() {
        let bad = vec![ExportEntry {
            address: "0200aa".into(),
            snapshot: Snapshot {
                status: SnapshotStatus::Pending,
                // 64 chars, valid hex content, but uppercase: would not be
                // byte-equal to an internally produced lowercase encoding
                // of the same bytes.
                extrinsic_hash: "AA".repeat(32),
                block_hash: None,
                block_height: None,
                depends_on: None,
                data: b"x".to_vec(),
            },
        }];
        let payload = serde_json::to_vec(&bad).unwrap();
        let (salt, ct) = crypto::encrypt(PW, FORMAT_STATES.as_bytes(), &payload).unwrap();
        let exp = EncryptedExport {
            format: FORMAT_STATES.to_string(),
            salt,
            ciphertext: ct,
        };
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
        write_json_atomic(&key_path, &rec).unwrap();
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
            snapshot: Snapshot {
                status: SnapshotStatus::Pending,
                extrinsic_hash: hex::encode(ext(3)),
                block_hash: None,
                block_height: None,
                depends_on: Some(hex::encode(ext(1))),
                data: b"s3".to_vec(),
            },
        }];
        let payload = serde_json::to_vec(&bad).unwrap();
        let (salt, ct) = crypto::encrypt(PW, FORMAT_STATES.as_bytes(), &payload).unwrap();
        let exp = EncryptedExport {
            format: FORMAT_STATES.to_string(),
            salt,
            ciphertext: ct,
        };
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
}
