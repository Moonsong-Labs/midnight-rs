//! Per-contract private state and signing-key storage.
//!
//! Some Compact contracts use stateful witnesses: the value a witness feeds the
//! circuit depends on data kept off-chain between calls. This crate provides a
//! durable, contract-scoped place to keep that data — the [`PrivateStateProvider`]
//! trait plus a filesystem default ([`FsPrivateStateProvider`]) — together with
//! password-encrypted [export](PrivateStateProvider::export_private_states) /
//! [import](PrivateStateProvider::import_private_states) for backup and migration.
//!
//! See `docs/private-state.md` for the design and how it maps to midnight-js's
//! `PrivateStateProvider`.
//!
//! # Threading
//!
//! This crate is the storage layer. The wiring that threads private state through
//! witness execution lives in `midnight-contract`: when a provider is attached via
//! `MidnightProvider::with_private_state`, a circuit call loads the contract's state
//! before execution, hands it to each witness through a `WitnessContext`, and
//! persists the updated state after the transaction lands. Used directly (without
//! that wiring), this is a plain contract-scoped key-value store.

mod crypto;
mod fs;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use fs::FsPrivateStateProvider;

/// Maximum number of entries a single export may contain. Mirrors midnight-js's
/// `MAX_EXPORT_STATES`; a guard against memory-exhaustion on import.
pub(crate) const MAX_EXPORT_ENTRIES: usize = 10_000;

/// Minimum length of an export password, in characters.
pub(crate) const MIN_PASSWORD_LEN: usize = 16;

/// Inner-payload schema version. The export is wire-compatible with midnight-js
/// at version 1; bump in lockstep if either side adds a field.
pub(crate) const EXPORT_VERSION: u32 = 1;

const FORMAT_STATES: &str = "midnight-private-state-export";
const FORMAT_KEYS: &str = "midnight-signing-key-export";

/// Errors surfaced by a [`PrivateStateProvider`].
#[derive(Debug, thiserror::Error)]
pub enum PrivateStateError {
    #[error("storage I/O error: {0}")]
    Io(String),

    #[error("serialization error: {0}")]
    Serialize(String),

    #[error("export password must be at least {MIN_PASSWORD_LEN} characters")]
    PasswordTooShort,

    #[error("export exceeds the maximum of {MAX_EXPORT_ENTRIES} entries")]
    TooManyEntries,

    #[error("encryption failed: {0}")]
    Encrypt(String),

    #[error("decryption failed: wrong password or corrupted data")]
    Decrypt,

    #[error("key derivation failed: {0}")]
    KeyDerivation(String),

    #[error("invalid export format: {0}")]
    InvalidFormat(String),

    #[error("import conflict for {0}")]
    ImportConflict(String),
}

/// How [`import`](PrivateStateProvider::import_private_states) resolves an entry
/// that already exists in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictStrategy {
    /// Keep the existing value; ignore the imported one.
    Skip,
    /// Replace the existing value with the imported one.
    Overwrite,
    /// Fail the whole import if any conflict is detected.
    #[default]
    Error,
}

/// Options for an encrypted export.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    password: String,
    max_entries: usize,
}

impl ExportOptions {
    /// Encrypt with `password` (must be at least [`MIN_PASSWORD_LEN`] characters).
    pub fn new(password: impl Into<String>) -> Self {
        Self {
            password: password.into(),
            max_entries: MAX_EXPORT_ENTRIES,
        }
    }

    /// Lower the entry cap below [`MAX_EXPORT_ENTRIES`].
    pub fn with_max_entries(mut self, max: usize) -> Self {
        self.max_entries = max.min(MAX_EXPORT_ENTRIES);
        self
    }
}

/// Options for an encrypted import.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    password: String,
    conflict: ConflictStrategy,
    max_entries: usize,
}

impl ImportOptions {
    /// Decrypt with `password`; default conflict strategy is [`ConflictStrategy::Error`].
    pub fn new(password: impl Into<String>) -> Self {
        Self {
            password: password.into(),
            conflict: ConflictStrategy::default(),
            max_entries: MAX_EXPORT_ENTRIES,
        }
    }

    pub fn with_conflict(mut self, strategy: ConflictStrategy) -> Self {
        self.conflict = strategy;
        self
    }

    pub fn with_max_entries(mut self, max: usize) -> Self {
        self.max_entries = max.min(MAX_EXPORT_ENTRIES);
        self
    }
}

/// Outcome counts from an import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportResult {
    pub imported: usize,
    pub skipped: usize,
    pub overwritten: usize,
}

/// Encrypted, JSON-serializable export envelope. Wire-compatible with
/// midnight-js's `PrivateStateExport` / `SigningKeyExport`: same field names,
/// same encoding, same inner-payload shape. The `format` tag distinguishes
/// states from keys so a key export cannot be imported as private states.
///
/// JSON shape:
///
/// ```json
/// {
///   "format": "midnight-private-state-export",
///   "encryptedPayload": "<base64 of PBKDF2-SHA256 + AES-256-GCM envelope>",
///   "salt": "<32-byte salt as 64 hex chars>"
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedExport {
    /// Format identifier: `midnight-private-state-export` or
    /// `midnight-signing-key-export`.
    pub format: String,
    /// Base64 of the binary envelope
    /// `version(1) || salt(32) || iv(12) || tag(16) || ciphertext`.
    pub encrypted_payload: String,
    /// Key-derivation salt, hex-encoded (32 bytes / 64 hex chars). Duplicated
    /// inside `encryptedPayload`; the two are compared on decrypt as a
    /// sanity check.
    pub salt: String,
}

/// A key-value store for contract private state and a per-contract signing-key
/// slot, both keyed by contract address. Addresses are the hex strings used
/// throughout this SDK.
///
/// The signing-key slot is a general per-contract key store, distinct from the
/// wallet's spending keys. This SDK's contract governance signs maintenance
/// updates externally and does not use it; it's here for apps that manage their
/// own per-contract keys.
///
/// A contract has exactly one private-state object (the Compact `PS` type, with one
/// field per private variable), so one entry per address is the whole model. The
/// caller serializes that object to the opaque bytes stored here.
#[async_trait]
pub trait PrivateStateProvider: Send + Sync {
    /// Store the private state for `address`, replacing any existing value.
    async fn set(&self, address: &str, state: &[u8]) -> Result<(), PrivateStateError>;

    /// Fetch the private state for `address`, or `None` if unset.
    async fn get(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError>;

    /// Remove the private state for `address`. A no-op if it does not exist.
    async fn remove(&self, address: &str) -> Result<(), PrivateStateError>;

    /// Remove every private state.
    async fn clear(&self) -> Result<(), PrivateStateError>;

    /// Store the signing `key` for `address`, replacing any existing value.
    async fn set_signing_key(&self, address: &str, key: &[u8]) -> Result<(), PrivateStateError>;

    /// Fetch the signing key for `address`, or `None` if unset.
    async fn get_signing_key(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError>;

    /// Remove the signing key for `address`. A no-op if it does not exist.
    async fn remove_signing_key(&self, address: &str) -> Result<(), PrivateStateError>;

    /// Remove every signing key.
    async fn clear_signing_keys(&self) -> Result<(), PrivateStateError>;

    /// Export all private states as a password-encrypted envelope. Signing keys are
    /// never included (export them separately via [`Self::export_signing_keys`]).
    async fn export_private_states(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError>;

    /// Import private states from an envelope produced by [`Self::export_private_states`].
    async fn import_private_states(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError>;

    /// Export all signing keys as a password-encrypted envelope.
    async fn export_signing_keys(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError>;

    /// Import signing keys from an envelope produced by [`Self::export_signing_keys`].
    async fn import_signing_keys(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError>;
}
