//! Filesystem-backed [`PrivateStateProvider`].
//!
//! Each entry is a small self-describing JSON record (so an export can recover
//! the original address from a hashed filename). Writes go to a `.tmp` sibling
//! and are `rename`d into place, so a crash never leaves a half-written file —
//! the same discipline the wallet uses for its own state.
//!
//! The export/import wire format is described on [`crate::EncryptedExport`]
//! and is interoperable with midnight-js's `level-private-state-provider`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::debug;

use crate::crypto::SALT_LEN;
use crate::{
    ConflictStrategy, EXPORT_VERSION, EncryptedExport, ExportOptions, FORMAT_KEYS, FORMAT_STATES,
    ImportOptions, ImportResult, MIN_PASSWORD_LEN, PrivateStateError, PrivateStateProvider, crypto,
};

const STATES_SUBDIR: &str = "states";
const KEYS_SUBDIR: &str = "signing-keys";

/// One stored entry (a private state or a signing key), keyed by contract
/// address. `data` is base64-encoded opaque bytes. `deny_unknown_fields` rejects
/// malformed records on import.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    address: String,
    data: String,
}

// ---------------------------------------------------------------------------
// Wire-format payload types (matches midnight-js
// `PrivateStatePayload` / `SigningKeyPayload`)
// ---------------------------------------------------------------------------

/// Inner payload of a `midnight-private-state-export`. Each value in `states`
/// is whatever midnight-js's `superjson.stringify(value)` emitted for that
/// contract's private state — an opaque string from our side's perspective.
/// We store it verbatim as the bytes a caller gets from
/// [`PrivateStateProvider::get`]; a Rust consumer who needs to inspect the
/// typed shape parses the SuperJSON envelope themselves.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrivateStatePayload {
    version: u32,
    exported_at: String,
    state_count: usize,
    states: HashMap<String, String>,
}

/// Inner payload of a `midnight-signing-key-export`. Each value in `keys` is
/// hex-encoded — matches midnight-js's `validateSigningKeyValue` (hex chars,
/// even length).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SigningKeyPayload {
    version: u32,
    exported_at: String,
    key_count: usize,
    keys: HashMap<String, String>,
}

/// Filesystem [`PrivateStateProvider`]. State lives under `<root>/states/` and
/// signing keys under `<root>/signing-keys/`, plaintext at rest. Default root
/// is `~/.midnight/private-state/`.
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

    /// JSON-serialize `payload`, encrypt under `password`, wrap in an
    /// [`EncryptedExport`] tagged with `format`. Validates password length.
    fn encrypt_export<P: Serialize>(
        password: &str,
        format: &str,
        payload: &P,
    ) -> Result<EncryptedExport, PrivateStateError> {
        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(PrivateStateError::PasswordTooShort);
        }
        let plaintext =
            serde_json::to_vec(payload).map_err(|e| PrivateStateError::Serialize(e.to_string()))?;
        let (salt, encrypted_payload) = crypto::encrypt(password, &plaintext)?;
        Ok(EncryptedExport {
            format: format.to_string(),
            encrypted_payload,
            salt: hex::encode(salt),
        })
    }

    /// Verify the envelope's `format` tag and decrypt+deserialize its inner
    /// payload as `P`.
    fn decrypt_export<P: DeserializeOwned>(
        password: &str,
        expected_format: &str,
        data: &EncryptedExport,
    ) -> Result<P, PrivateStateError> {
        if data.format != expected_format {
            return Err(PrivateStateError::InvalidFormat(format!(
                "expected format {expected_format}, got {}",
                data.format
            )));
        }
        let salt = parse_salt(&data.salt)?;
        let plaintext = crypto::decrypt(password, &salt, &data.encrypted_payload)?;
        // A successful decrypt with a malformed plaintext is not a wrong-password
        // failure but a corrupt-payload one — keep the `Decrypt` variant for the
        // wrong-password case and only use it here too if you'd prefer to mask
        // the distinction.
        serde_json::from_slice(&plaintext)
            .map_err(|e| PrivateStateError::InvalidFormat(format!("decrypted payload: {e}")))
    }

    /// Write each `(address, bytes)` into `dir`, honoring `conflict`.
    fn apply_import(
        &self,
        dir: &Path,
        entries: Vec<(String, Vec<u8>)>,
        conflict: ConflictStrategy,
    ) -> Result<ImportResult, PrivateStateError> {
        // Pre-resolve paths so the Error strategy can detect-before-mutate.
        let resolved: Vec<(PathBuf, String, Vec<u8>)> = entries
            .into_iter()
            .map(|(addr, data)| (entry_path(dir, &addr), addr, data))
            .collect();

        if conflict == ConflictStrategy::Error {
            if let Some((_, addr, _)) = resolved.iter().find(|(p, _, _)| p.exists()) {
                return Err(PrivateStateError::ImportConflict(addr.clone()));
            }
        }

        let mut result = ImportResult::default();
        for (path, address, data) in &resolved {
            if path.exists() {
                match conflict {
                    ConflictStrategy::Skip => {
                        result.skipped += 1;
                        continue;
                    }
                    ConflictStrategy::Overwrite => result.overwritten += 1,
                    // A concurrent writer can create the file between the
                    // pre-check and here (TOCTOU); fail with the address
                    // rather than silently overwriting.
                    ConflictStrategy::Error => {
                        return Err(PrivateStateError::ImportConflict(address.clone()));
                    }
                }
            } else {
                result.imported += 1;
            }
            self.write_record(path, address, data)?;
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

/// Decode and validate the salt field of an [`EncryptedExport`]: must be exactly
/// `2 * SALT_LEN` hex chars.
fn parse_salt(hex_salt: &str) -> Result<[u8; SALT_LEN], PrivateStateError> {
    if hex_salt.len() != 2 * SALT_LEN {
        return Err(PrivateStateError::InvalidFormat(format!(
            "salt must be {} hex chars ({} bytes); got {} chars",
            2 * SALT_LEN,
            SALT_LEN,
            hex_salt.len()
        )));
    }
    let bytes = hex::decode(hex_salt)
        .map_err(|e| PrivateStateError::InvalidFormat(format!("salt is not valid hex: {e}")))?;
    let mut arr = [0u8; SALT_LEN];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

/// Verify that an imported payload's `version` is one we understand. We accept
/// exactly `EXPORT_VERSION` today; expand to a slice if we ever need a window.
fn validate_payload_version(version: u32) -> Result<(), PrivateStateError> {
    if version != EXPORT_VERSION {
        return Err(PrivateStateError::InvalidFormat(format!(
            "export version {version} is not supported (only {EXPORT_VERSION})",
        )));
    }
    Ok(())
}

/// `YYYY-MM-DDTHH:MM:SSZ` for `now`, matching the shape midnight-js's
/// `new Date().toISOString()` emits (modulo sub-second precision). Hand-rolled
/// to avoid a chrono / time dependency for one metadata field.
fn iso8601_utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil_from_epoch_secs(secs)
}

/// Howard Hinnant's [`days_from_civil`](https://howardhinnant.github.io/date_algorithms.html)
/// inverse: epoch seconds → `YYYY-MM-DDTHH:MM:SSZ` (UTC, no leap-second fudge,
/// proleptic Gregorian).
fn civil_from_epoch_secs(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    let hour = sod / 3600;
    let minute = (sod / 60) % 60;
    let second = sod % 60;

    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
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
        let records: Vec<Record> = read_records(&self.states_dir())?;
        if records.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }
        // The bytes we stored are the SuperJSON envelope string midnight-js
        // wrote (or that a Rust caller produced with the same convention). Put
        // them back on the wire as a UTF-8 string; a non-UTF-8 payload means a
        // caller stored raw bytes that wouldn't be midnight-js-importable, so
        // surface that as an explicit error rather than corrupting the envelope.
        let mut states = HashMap::with_capacity(records.len());
        for rec in records {
            let bytes = decode_data(&rec.data)?;
            let envelope = String::from_utf8(bytes).map_err(|e| {
                PrivateStateError::InvalidFormat(format!(
                    "state for {} is not a UTF-8 SuperJSON envelope: {e}",
                    rec.address,
                ))
            })?;
            states.insert(rec.address, envelope);
        }
        let state_count = states.len();
        let payload = PrivateStatePayload {
            version: EXPORT_VERSION,
            exported_at: iso8601_utc_now(),
            state_count,
            states,
        };
        debug!(
            count = state_count,
            format = FORMAT_STATES,
            "exported records"
        );
        Self::encrypt_export(&opts.password, FORMAT_STATES, &payload)
    }

    async fn import_private_states(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        let payload: PrivateStatePayload =
            Self::decrypt_export(&opts.password, FORMAT_STATES, data)?;
        validate_payload_version(payload.version)?;
        if payload.states.len() != payload.state_count {
            return Err(PrivateStateError::InvalidFormat(format!(
                "stateCount ({}) does not match number of entries ({})",
                payload.state_count,
                payload.states.len()
            )));
        }
        if payload.states.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }
        // Store each SuperJSON envelope string as the per-address blob. A Rust
        // consumer that needs to inspect the typed value parses it themselves;
        // re-exporting the same bytes round-trips byte-for-byte through
        // midnight-js.
        let entries = payload
            .states
            .into_iter()
            .map(|(address, envelope)| (address, envelope.into_bytes()))
            .collect();
        self.apply_import(&self.states_dir(), entries, opts.conflict)
    }

    async fn export_signing_keys(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError> {
        let records: Vec<Record> = read_records(&self.keys_dir())?;
        if records.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }
        // Signing keys go on the wire as hex strings — matches midnight-js's
        // `validateSigningKeyValue` (hex chars, even length).
        let mut keys = HashMap::with_capacity(records.len());
        for rec in records {
            let bytes = decode_data(&rec.data)?;
            keys.insert(rec.address, hex::encode(bytes));
        }
        let key_count = keys.len();
        let payload = SigningKeyPayload {
            version: EXPORT_VERSION,
            exported_at: iso8601_utc_now(),
            key_count,
            keys,
        };
        debug!(count = key_count, format = FORMAT_KEYS, "exported records");
        Self::encrypt_export(&opts.password, FORMAT_KEYS, &payload)
    }

    async fn import_signing_keys(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        let payload: SigningKeyPayload = Self::decrypt_export(&opts.password, FORMAT_KEYS, data)?;
        validate_payload_version(payload.version)?;
        if payload.keys.len() != payload.key_count {
            return Err(PrivateStateError::InvalidFormat(format!(
                "keyCount ({}) does not match number of entries ({})",
                payload.key_count,
                payload.keys.len()
            )));
        }
        if payload.keys.len() > opts.max_entries {
            return Err(PrivateStateError::TooManyEntries);
        }
        let mut entries = Vec::with_capacity(payload.keys.len());
        for (address, hex_str) in payload.keys {
            let bytes = hex::decode(&hex_str).map_err(|e| {
                PrivateStateError::InvalidFormat(format!(
                    "signing key for {address} is not valid hex: {e}"
                ))
            })?;
            entries.push((address, bytes));
        }
        self.apply_import(&self.keys_dir(), entries, opts.conflict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // End-to-end round-trip / conflict / password coverage lives in
    // `tests/fs.rs`. The unit tests here exercise the in-crate helpers that
    // aren't reachable from there.

    #[test]
    fn iso8601_formats_known_epochs() {
        // 2020-01-01T00:00:00Z = 1577836800
        assert_eq!(civil_from_epoch_secs(1_577_836_800), "2020-01-01T00:00:00Z");
        // 2026-06-05T12:34:56Z = 1780662896
        assert_eq!(civil_from_epoch_secs(1_780_662_896), "2026-06-05T12:34:56Z");
        // Epoch itself.
        assert_eq!(civil_from_epoch_secs(0), "1970-01-01T00:00:00Z");
    }
}
