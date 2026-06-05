//! Filesystem-backed [`PrivateStateProvider`].
//!
//! Each entry is a small self-describing JSON record (so an export can recover the
//! original address/id from a hashed filename). Writes go to a `.tmp` sibling and
//! are `rename`d into place, so a crash never leaves a half-written file — the same
//! discipline the wallet uses for its own state.

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::debug;

use crate::{
    ConflictStrategy, EncryptedExport, ExportOptions, FORMAT_KEYS, FORMAT_STATES, ImportOptions,
    ImportResult, MIN_PASSWORD_LEN, PrivateStateError, PrivateStateProvider, crypto,
};

const STATES_SUBDIR: &str = "states";
const KEYS_SUBDIR: &str = "signing-keys";

/// One stored entry (a private state or a signing key), keyed by contract
/// address. `data` is base64-encoded opaque bytes. `deny_unknown_fields`
/// rejects malformed records on import.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    address: String,
    data: String,
}

/// Filesystem [`PrivateStateProvider`]. State lives under `<root>/states/` and
/// signing keys under `<root>/signing-keys/`, plaintext at rest. Default root is
/// `~/.midnight/private-state/`.
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
    /// Store private state and signing keys under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The default root, `~/.midnight/private-state/`, or `None` if no home
    /// directory can be resolved.
    pub fn default_dir() -> Option<PathBuf> {
        home_dir().map(|h| h.join(".midnight").join("private-state"))
    }

    /// Construct against [`Self::default_dir`], or `None` if no home directory
    /// can be resolved.
    pub fn with_default_dir() -> Option<Self> {
        Self::default_dir().map(Self::new)
    }

    fn states_dir(&self) -> PathBuf {
        self.root.join(STATES_SUBDIR)
    }

    fn keys_dir(&self) -> PathBuf {
        self.root.join(KEYS_SUBDIR)
    }

    fn state_path(&self, address: &str) -> PathBuf {
        entry_path(&self.states_dir(), address)
    }

    fn key_path(&self, address: &str) -> PathBuf {
        entry_path(&self.keys_dir(), address)
    }

    /// Write `data` at `path` as a self-describing JSON record that pairs the
    /// original `address` with base64-encoded `data`.
    fn write_record(
        &self,
        path: &Path,
        address: &str,
        data: &[u8],
    ) -> Result<(), PrivateStateError> {
        let rec = Record {
            address: address.to_string(),
            data: BASE64.encode(data),
        };
        write_json_atomic(path, &rec)
    }

    /// Read the JSON record at `path` and decode its base64 payload.
    fn read_record(&self, path: &Path) -> Result<Option<Vec<u8>>, PrivateStateError> {
        match read_json_opt::<Record>(path)? {
            Some(rec) => Ok(Some(decode_data(&rec.data)?)),
            None => Ok(None),
        }
    }

    /// Encrypt every record under `dir` into the provided `format` envelope.
    /// Used by both `export_private_states` and `export_signing_keys`; the two
    /// differ only in the source directory and the format constant they tag the
    /// payload with.
    fn export_records(
        &self,
        dir: &Path,
        format: &str,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError> {
        if opts.password.chars().count() < MIN_PASSWORD_LEN {
            return Err(PrivateStateError::PasswordTooShort);
        }
        let records: Vec<Record> = read_records(dir)?;
        if records.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }
        let payload = serde_json::to_vec(&records)
            .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
        let (salt, ciphertext) = crypto::encrypt(&opts.password, format.as_bytes(), &payload)?;
        debug!(count = records.len(), format, "exported records");
        Ok(EncryptedExport {
            format: format.to_string(),
            salt,
            ciphertext,
        })
    }

    /// Decrypt `data` (verifying its envelope `format` matches `expected_format`)
    /// and write the records into `dir`, honoring `opts.conflict`. Used by both
    /// `import_private_states` and `import_signing_keys`.
    fn import_records(
        &self,
        dir: &Path,
        expected_format: &str,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        if data.format != expected_format {
            return Err(PrivateStateError::InvalidFormat(format!(
                "expected format {expected_format}, got {}",
                data.format
            )));
        }
        let payload = crypto::decrypt(
            &opts.password,
            expected_format.as_bytes(),
            &data.salt,
            &data.ciphertext,
        )?;
        let records: Vec<Record> = serde_json::from_slice(&payload)
            .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
        if records.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }

        // Resolve each record to its path. Validates base64 up front so a
        // corrupt entry fails before any file is written.
        let mut resolved = Vec::with_capacity(records.len());
        for rec in records {
            decode_data(&rec.data)?;
            let path = entry_path(dir, &rec.address);
            resolved.push((path, rec));
        }
        reject_duplicate_paths(&resolved)?;

        // Detect-before-mutate for the Error strategy.
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
                    // Pre-checked above and duplicate targets were rejected, so
                    // this normally can't happen — but a concurrent writer could
                    // create the file between the pre-check and here (TOCTOU), so
                    // return the conflicting address rather than panic.
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

/// `<dir>/<sha256(address)>.json`. Hashing keeps the filename fixed-length and
/// path-safe regardless of the address string.
fn entry_path(dir: &Path, address: &str) -> PathBuf {
    dir.join(format!(
        "{}.json",
        hex::encode(Sha256::digest(address.as_bytes()))
    ))
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

fn read_records<T: DeserializeOwned>(dir: &Path) -> Result<Vec<T>, PrivateStateError> {
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
    let mut out = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| PrivateStateError::Io(e.to_string()))?
            .path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Some(rec) = read_json_opt::<T>(&path)? {
            out.push(rec);
        }
    }
    Ok(out)
}

fn decode_data(data: &str) -> Result<Vec<u8>, PrivateStateError> {
    BASE64
        .decode(data)
        .map_err(|e| PrivateStateError::InvalidFormat(format!("entry data is not base64: {e}")))
}

/// Reject an import payload whose entries resolve to the same target path (a
/// malformed export). Without this, two duplicate entries under the `Error`
/// strategy would write the first then hit `unreachable!()` on the second after
/// it had already partially mutated the store.
fn reject_duplicate_paths<T>(resolved: &[(PathBuf, T)]) -> Result<(), PrivateStateError> {
    let mut seen = std::collections::HashSet::with_capacity(resolved.len());
    for (path, _) in resolved {
        if !seen.insert(path.as_path()) {
            return Err(PrivateStateError::InvalidFormat(
                "export contains duplicate entries for the same key".into(),
            ));
        }
    }
    Ok(())
}

#[async_trait]
impl PrivateStateProvider for FsPrivateStateProvider {
    async fn set(&self, address: &str, state: &[u8]) -> Result<(), PrivateStateError> {
        self.write_record(&self.state_path(address), address, state)
    }

    async fn get(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError> {
        self.read_record(&self.state_path(address))
    }

    async fn remove(&self, address: &str) -> Result<(), PrivateStateError> {
        remove_file_opt(&self.state_path(address))
    }

    async fn clear(&self) -> Result<(), PrivateStateError> {
        clear_dir(&self.states_dir())
    }

    async fn set_signing_key(&self, address: &str, key: &[u8]) -> Result<(), PrivateStateError> {
        self.write_record(&self.key_path(address), address, key)
    }

    async fn get_signing_key(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError> {
        self.read_record(&self.key_path(address))
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
        self.export_records(&self.states_dir(), FORMAT_STATES, opts)
    }

    async fn import_private_states(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        self.import_records(&self.states_dir(), FORMAT_STATES, data, opts)
    }

    async fn export_signing_keys(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError> {
        self.export_records(&self.keys_dir(), FORMAT_KEYS, opts)
    }

    async fn import_signing_keys(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        self.import_records(&self.keys_dir(), FORMAT_KEYS, data, opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImportOptions;

    const PW: &str = "a-sufficiently-long-password";

    // A crafted export with two entries resolving to the same key must be
    // rejected outright — not panic on the second write (the old `unreachable!()`
    // path) or leave the first write behind as a partial mutation.
    #[tokio::test]
    async fn import_rejects_duplicate_entries() {
        let records = vec![
            Record {
                address: "0200aa".into(),
                data: BASE64.encode(b"one"),
            },
            Record {
                address: "0200aa".into(),
                data: BASE64.encode(b"two"),
            },
        ];
        let payload = serde_json::to_vec(&records).unwrap();
        let (salt, ciphertext) = crypto::encrypt(PW, FORMAT_STATES.as_bytes(), &payload).unwrap();
        let export = EncryptedExport {
            format: FORMAT_STATES.to_string(),
            salt,
            ciphertext,
        };

        let dir = tempfile::TempDir::new().unwrap();
        let provider = FsPrivateStateProvider::new(dir.path());
        let err = provider
            .import_private_states(&export, &ImportOptions::new(PW))
            .await
            .unwrap_err();
        assert!(matches!(err, PrivateStateError::InvalidFormat(_)));
        // Nothing was written: no partial mutation from the first entry.
        assert_eq!(provider.get("0200aa").await.unwrap(), None);
    }
}
