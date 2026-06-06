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
        block_height: u64,
        block_hash: [u8; 32],
    ) -> Result<(), PrivateStateError> {
        let ext_hex = hex::encode(extrinsic_hash);
        let path = self.find_snapshot_path(address, &ext_hex)?.ok_or_else(|| {
            PrivateStateError::SnapshotNotFound {
                address: address.to_string(),
                extrinsic_hash: ext_hex.clone(),
            }
        })?;
        let mut snap: Snapshot = read_json_opt(&path)?.ok_or_else(|| {
            PrivateStateError::Io(format!("snapshot disappeared at {}", path.display()))
        })?;
        snap.status = SnapshotStatus::Confirmed;
        snap.block_height = Some(block_height);
        snap.block_hash = Some(hex::encode(block_hash));
        write_json_atomic(&path, &snap)?;
        debug!(
            address,
            extrinsic_hash = %ext_hex,
            block_height,
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
        cascade_drop(self, address, &ext_hex)?;
        debug!(address, extrinsic_hash = %ext_hex, "marked snapshot failed");
        Ok(())
    }

    async fn head(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError> {
        // Find the journal leaf: the snapshot whose extrinsic_hash isn't in
        // any other snapshot's `depends_on`. In sequential mode there's
        // exactly one such snapshot. Using the dependency graph (rather than
        // filename order) keeps head correct after `import_private_states`
        // rewrites filenames with new timestamps.
        let snaps: Vec<Snapshot> = self
            .load_snapshots(address)?
            .into_iter()
            .map(|(_, s)| s)
            .collect();
        Ok(find_leaf(snaps).map(|s| s.data))
    }

    async fn head_extrinsic(&self, address: &str) -> Result<Option<[u8; 32]>, PrivateStateError> {
        let snaps: Vec<Snapshot> = self
            .load_snapshots(address)?
            .into_iter()
            .map(|(_, s)| s)
            .collect();
        let Some(snap) = find_leaf(snaps) else {
            return Ok(None);
        };
        Ok(Some(parse_hash(&snap.extrinsic_hash)?))
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
        cascade_drop(self, address, &ext_hex)?;
        Ok(())
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
        if let Ok(rd) = fs::read_dir(self.keys_dir()) {
            for entry in rd {
                let path = entry
                    .map_err(|e| PrivateStateError::Io(e.to_string()))?
                    .path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                if let Some(rec) = read_json_opt::<KeyRecord>(&path)? {
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
        let records: Vec<KeyRecord> = serde_json::from_slice(&payload)
            .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
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
/// any other snapshot's `depends_on`. In sequential mode there's exactly
/// one; in pipelined mode any leaf is acceptable and we return the first one
/// found in input order. Returns `None` for an empty journal.
fn find_leaf(snapshots: Vec<Snapshot>) -> Option<Snapshot> {
    if snapshots.is_empty() {
        return None;
    }
    let referenced: HashSet<String> = snapshots
        .iter()
        .filter_map(|s| s.depends_on.clone())
        .collect();
    snapshots
        .into_iter()
        .find(|s| !referenced.contains(&s.extrinsic_hash))
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

    for (path, snap) in snapshots {
        if failed.contains(&snap.extrinsic_hash) {
            remove_file_opt(&path)?;
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
    }
    Ok(result)
}

/// Verify that `field` (when present) is a 32-byte hex string. Used by the
/// import path so a malformed payload errors out before mutating the store.
fn validate_hash_field(name: &str, field: Option<&str>) -> Result<(), PrivateStateError> {
    let Some(s) = field else {
        return Ok(());
    };
    if s.len() != 64 || hex::decode(s).is_err() {
        return Err(PrivateStateError::InvalidFormat(format!(
            "field `{name}` is not 32 bytes of hex: {s}"
        )));
    }
    Ok(())
}

/// Write the plaintext address to `<dir>/address.txt` if it isn't already
/// there, so an export can recover the address from a hashed directory.
fn ensure_address_marker(dir: &Path, address: &str) -> Result<(), PrivateStateError> {
    fs::create_dir_all(dir)
        .map_err(|e| PrivateStateError::Io(format!("create dir {}: {e}", dir.display())))?;
    let marker = dir.join(ADDRESS_MARKER);
    if marker.exists() {
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
        p.confirm("0200aa", ext(1), 42, [0xbb; 32]).await.unwrap();
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
        src.confirm("0200aa", ext(1), 10, [0xee; 32]).await.unwrap();
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
}
