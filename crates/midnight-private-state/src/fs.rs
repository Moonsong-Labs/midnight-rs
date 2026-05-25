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
    ImportResult, MIN_PASSWORD_LEN, PrivateStateError, PrivateStateId, PrivateStateProvider,
    crypto,
};

const STATES_SUBDIR: &str = "states";
const KEYS_SUBDIR: &str = "signing-keys";

/// One stored private state. `data` is base64-encoded opaque bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateRecord {
    address: String,
    id: String,
    data: String,
}

/// One stored signing key. `data` is base64-encoded opaque bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeyRecord {
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

    fn state_path(&self, address: &str, id: &PrivateStateId) -> PathBuf {
        let mut h = Sha256::new();
        h.update(address.as_bytes());
        // Unit separator so distinct (address, id) pairs can't collide by
        // concatenation (e.g. ("ab","c") vs ("a","bc")).
        h.update([0x1f]);
        h.update(id.as_str().as_bytes());
        self.states_dir()
            .join(format!("{}.json", hex::encode(h.finalize())))
    }

    fn key_path(&self, address: &str) -> PathBuf {
        self.keys_dir().join(format!(
            "{}.json",
            hex::encode(Sha256::digest(address.as_bytes()))
        ))
    }
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

#[async_trait]
impl PrivateStateProvider for FsPrivateStateProvider {
    async fn set(
        &self,
        address: &str,
        id: &PrivateStateId,
        state: &[u8],
    ) -> Result<(), PrivateStateError> {
        let rec = StateRecord {
            address: address.to_string(),
            id: id.as_str().to_string(),
            data: BASE64.encode(state),
        };
        write_json_atomic(&self.state_path(address, id), &rec)
    }

    async fn get(
        &self,
        address: &str,
        id: &PrivateStateId,
    ) -> Result<Option<Vec<u8>>, PrivateStateError> {
        match read_json_opt::<StateRecord>(&self.state_path(address, id))? {
            Some(rec) => Ok(Some(decode_data(&rec.data)?)),
            None => Ok(None),
        }
    }

    async fn remove(&self, address: &str, id: &PrivateStateId) -> Result<(), PrivateStateError> {
        remove_file_opt(&self.state_path(address, id))
    }

    async fn clear(&self) -> Result<(), PrivateStateError> {
        clear_dir(&self.states_dir())
    }

    async fn set_signing_key(&self, address: &str, key: &[u8]) -> Result<(), PrivateStateError> {
        let rec = KeyRecord {
            address: address.to_string(),
            data: BASE64.encode(key),
        };
        write_json_atomic(&self.key_path(address), &rec)
    }

    async fn get_signing_key(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError> {
        match read_json_opt::<KeyRecord>(&self.key_path(address))? {
            Some(rec) => Ok(Some(decode_data(&rec.data)?)),
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
        let records: Vec<StateRecord> = read_records(&self.states_dir())?;
        if records.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }
        let payload = serde_json::to_vec(&records)
            .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
        let (salt, ciphertext) = crypto::encrypt(&opts.password, &payload)?;
        debug!(count = records.len(), "exported private states");
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
        let payload = crypto::decrypt(&opts.password, &data.salt, &data.ciphertext)?;
        let records: Vec<StateRecord> = serde_json::from_slice(&payload)
            .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
        if records.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }

        // Resolve each record to its (path, id). Validates base64 up front so a
        // corrupt entry fails before any file is written.
        let mut resolved = Vec::with_capacity(records.len());
        for rec in records {
            decode_data(&rec.data)?;
            let id = PrivateStateId::from(rec.id.clone());
            let path = self.state_path(&rec.address, &id);
            resolved.push((path, rec));
        }

        // Detect-before-mutate for the Error strategy.
        if opts.conflict == ConflictStrategy::Error {
            if let Some((_, rec)) = resolved.iter().find(|(p, _)| p.exists()) {
                return Err(PrivateStateError::ImportConflict(format!(
                    "{}:{}",
                    rec.address, rec.id
                )));
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
                    // Pre-checked above.
                    ConflictStrategy::Error => unreachable!(),
                }
            } else {
                result.imported += 1;
            }
            write_json_atomic(path, rec)?;
        }
        Ok(result)
    }

    async fn export_signing_keys(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError> {
        if opts.password.chars().count() < MIN_PASSWORD_LEN {
            return Err(PrivateStateError::PasswordTooShort);
        }
        let records: Vec<KeyRecord> = read_records(&self.keys_dir())?;
        if records.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }
        let payload = serde_json::to_vec(&records)
            .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
        let (salt, ciphertext) = crypto::encrypt(&opts.password, &payload)?;
        debug!(count = records.len(), "exported signing keys");
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
        let payload = crypto::decrypt(&opts.password, &data.salt, &data.ciphertext)?;
        let records: Vec<KeyRecord> = serde_json::from_slice(&payload)
            .map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
        if records.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }

        let mut resolved = Vec::with_capacity(records.len());
        for rec in records {
            decode_data(&rec.data)?;
            let path = self.key_path(&rec.address);
            resolved.push((path, rec));
        }

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
                    ConflictStrategy::Error => unreachable!(),
                }
            } else {
                result.imported += 1;
            }
            write_json_atomic(path, rec)?;
        }
        Ok(result)
    }
}
