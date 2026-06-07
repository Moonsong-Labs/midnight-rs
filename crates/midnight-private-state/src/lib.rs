//! Per-contract private state and signing-key storage.
//!
//! Some Compact contracts use stateful witnesses: the value a witness feeds
//! the circuit depends on data kept off-chain between calls. This crate
//! provides a durable, contract-scoped journal of private-state snapshots,
//! one per submitted transaction, plus a per-contract signing-key slot.
//!
//! Each snapshot is keyed by the transaction that produced it
//! (`extrinsic_hash`) and, once the chain finalizes that transaction, by the
//! block it landed in (`block_height` + `block_hash`). Snapshots progress
//! through a small lifecycle:
//!
//! - **Pending**. The SDK has submitted the transaction; finality is not yet
//!   established. The snapshot is the SDK's best guess at the post-call
//!   state. Subsequent calls can chain off it (using its bytes as the next
//!   witness baseline); a later failure cascade-rolls the chain back.
//! - **Confirmed**. The transaction is finalized on the chain. The contract
//!   path's `confirm` is optimistic today, in that the SDK does not parse the
//!   block's events to verify the fallible phase reported `Success`; a tx
//!   that finalized with `PartialSuccess` or `Failure` still gets promoted
//!   to `Confirmed`. Callers who learn out of band that a snapshot doesn't
//!   match the chain can drop it (and its dependents) via
//!   [`PrivateStateProvider::mark_failed`].
//!
//! See `docs/private-state.md` for the call flow and recovery semantics.

mod crypto;
mod fs;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use fs::FsPrivateStateProvider;

/// Maximum number of records (snapshots or signing keys) a single export may
/// contain. A guard against memory-exhaustion on import.
pub(crate) const MAX_EXPORT_ENTRIES: usize = 10_000;

/// Minimum length of an export password, in characters.
pub(crate) const MIN_PASSWORD_LEN: usize = 16;

const FORMAT_STATES: &str = "midnight-rs-private-state-journal-export-v1";
const FORMAT_KEYS: &str = "midnight-rs-signing-key-export-v1";

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

    /// A snapshot the caller named was not found.
    #[error("snapshot not found: address={address}, extrinsic_hash={extrinsic_hash}")]
    SnapshotNotFound {
        address: String,
        extrinsic_hash: String,
    },

    /// A snapshot with this extrinsic_hash already exists at this address.
    /// `append_pending` rejects duplicates rather than recording two files
    /// for the same tx (which would leave `confirm` / `mark_failed` walking
    /// an ambiguous match set).
    #[error("snapshot already exists: address={address}, extrinsic_hash={extrinsic_hash}")]
    SnapshotAlreadyExists {
        address: String,
        extrinsic_hash: String,
    },
}

/// Lifecycle state of a snapshot in the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotStatus {
    /// The transaction has been submitted but has not yet finalized.
    /// Subsequent calls may chain off this snapshot's bytes; a later
    /// `mark_failed` will cascade-roll back this and any descendants.
    Pending,
    /// The transaction is finalized on chain. The contract path's `confirm`
    /// is optimistic: it does not parse block events to verify the fallible
    /// phase reported `Success`, so a tx that finalized with
    /// `PartialSuccess` or `Failure` is still marked `Confirmed` here.
    /// Callers who learn out of band that a snapshot doesn't reflect the
    /// chain can invoke `mark_failed` to cascade-roll back this and any
    /// descendants.
    Confirmed,
}

/// One recorded snapshot.
///
/// `data` is the opaque post-call state bytes the witness layer would replay
/// for the next call. `extrinsic_hash` is the unique identifier (subxt's
/// extrinsic hash) of the transaction the snapshot was recorded against.
/// The SDK does not verify on-chain execution succeeded when promoting to
/// `Confirmed` (see [`SnapshotStatus::Confirmed`]). `block_height` /
/// `block_hash` are filled in by [`PrivateStateProvider::confirm`] once the
/// tx finalizes. `depends_on` is the `extrinsic_hash` of the previous
/// snapshot the new state was built on top of (or `None` if this was the
/// first snapshot at this address), used to cascade rollbacks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub status: SnapshotStatus,
    /// Hex of the 32-byte extrinsic hash.
    pub extrinsic_hash: String,
    /// Hex of the 32-byte block hash, set once the tx is finalized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<String>,
    /// Set once the tx is finalized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_height: Option<u64>,
    /// Extrinsic hash of the previous snapshot at this address, or `None` for
    /// the first snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<String>,
    /// Opaque post-call state bytes.
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&BASE64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        BASE64.decode(s).map_err(serde::de::Error::custom)
    }
}

/// How [`import_private_states`](PrivateStateProvider::import_private_states)
/// resolves an entry that already exists in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictStrategy {
    /// Keep the existing snapshot; ignore the imported one.
    Skip,
    /// Replace the existing snapshot with the imported one.
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
    pub fn new(password: impl Into<String>) -> Self {
        Self {
            password: password.into(),
            max_entries: MAX_EXPORT_ENTRIES,
        }
    }

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

/// Encrypted, JSON-serializable envelope shared between private-state and
/// signing-key exports. The `format` tag distinguishes the two.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedExport {
    pub format: String,
    pub salt: String,
    pub ciphertext: String,
}

/// A journaled key-value store for contract private state and a per-contract
/// signing-key slot.
///
/// Private states form a per-address append-only journal: each circuit call
/// records a [`Snapshot`] tagged with the transaction that produced it.
/// Subsequent calls read the latest snapshot's `data` as their witness
/// baseline. The journal supports rollback (cascading through `depends_on`)
/// so a reorg or post-finalization failure can unwind dependent pending
/// snapshots.
///
/// Signing keys are a flat per-address slot, since Compact contracts have at
/// most one signing key per address.
#[async_trait]
pub trait PrivateStateProvider: Send + Sync {
    /// Append a new pending snapshot. `extrinsic_hash` is the unique tx id;
    /// `depends_on` should be the current head's extrinsic_hash (or `None`
    /// if this is the first snapshot at this address).
    async fn append_pending(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
        depends_on: Option<[u8; 32]>,
        state: &[u8],
    ) -> Result<(), PrivateStateError>;

    /// Promote a pending snapshot to confirmed, recording the block it
    /// landed in. `block_height` is optional, since some subxt code paths
    /// only know the block hash and would otherwise have to pass a sentinel
    /// that's indistinguishable from a genuine genesis-block confirmation.
    ///
    /// Errors with [`PrivateStateError::SnapshotNotFound`] if no snapshot
    /// with `extrinsic_hash` exists at `address`. Re-confirming a snapshot
    /// that is already [`SnapshotStatus::Confirmed`] succeeds only when the
    /// new `(block_height, block_hash)` matches the existing record; a
    /// conflicting re-confirm errors with
    /// [`PrivateStateError::InvalidFormat`] instead of silently overwriting
    /// the durable record.
    async fn confirm(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
        block_height: Option<u64>,
        block_hash: [u8; 32],
    ) -> Result<(), PrivateStateError>;

    /// Mark a snapshot as failed and remove it. Cascading: any snapshots
    /// that transitively `depends_on` this one are removed too.
    ///
    /// Errors with [`PrivateStateError::SnapshotNotFound`] if no snapshot
    /// with `extrinsic_hash` exists at `address`, matching the semantics of
    /// [`Self::confirm`] so callers see a consistent error variant when
    /// addressing a missing snapshot.
    async fn mark_failed(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
    ) -> Result<(), PrivateStateError>;

    /// The most recent snapshot's `data` (the next call's witness baseline),
    /// or `None` if no snapshots are recorded.
    async fn head(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError>;

    /// The most recent snapshot's `extrinsic_hash`, for use as the next
    /// call's `depends_on`.
    async fn head_extrinsic(&self, address: &str) -> Result<Option<[u8; 32]>, PrivateStateError>;

    /// The most recent snapshot's `data` together with its `extrinsic_hash`,
    /// obtained from a single read of the underlying store. Prefer this over
    /// calling [`Self::head`] and [`Self::head_extrinsic`] back to back when
    /// you need both values: under concurrent mutation those two calls can
    /// return values from different journal versions, producing a torn read
    /// where the baseline data and the parent extrinsic_hash disagree.
    ///
    /// The default implementation sequences `head` then `head_extrinsic`;
    /// backends that can answer in one read (such as
    /// [`FsPrivateStateProvider`]) should override.
    async fn head_with_extrinsic(
        &self,
        address: &str,
    ) -> Result<Option<(Vec<u8>, [u8; 32])>, PrivateStateError> {
        let Some(data) = self.head(address).await? else {
            return Ok(None);
        };
        let Some(ext) = self.head_extrinsic(address).await? else {
            return Ok(None);
        };
        Ok(Some((data, ext)))
    }

    /// All snapshots recorded for `address`, oldest first.
    async fn snapshots(&self, address: &str) -> Result<Vec<Snapshot>, PrivateStateError>;

    /// Drop the snapshot identified by `extrinsic_hash` and every snapshot
    /// that transitively depends on it.
    ///
    /// Errors with [`PrivateStateError::SnapshotNotFound`] if no snapshot
    /// with `extrinsic_hash` exists at `address`, matching [`Self::confirm`]
    /// and [`Self::mark_failed`].
    async fn rollback_from(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
    ) -> Result<(), PrivateStateError>;

    /// Drop every snapshot for `address`.
    async fn forget(&self, address: &str) -> Result<(), PrivateStateError>;

    /// Drop every snapshot for every address.
    async fn forget_all(&self) -> Result<(), PrivateStateError>;

    /// Store the signing `key` for `address`, replacing any existing value.
    async fn set_signing_key(&self, address: &str, key: &[u8]) -> Result<(), PrivateStateError>;

    /// Fetch the signing key for `address`, or `None` if unset.
    async fn get_signing_key(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError>;

    /// Remove the signing key for `address`. A no-op if it does not exist.
    async fn remove_signing_key(&self, address: &str) -> Result<(), PrivateStateError>;

    /// Remove every signing key.
    async fn clear_signing_keys(&self) -> Result<(), PrivateStateError>;

    /// Export the full snapshot journal (every address, every snapshot) as
    /// a password-encrypted envelope. Signing keys are never included; export
    /// them separately via [`Self::export_signing_keys`].
    async fn export_private_states(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError>;

    /// Restore a snapshot journal from an envelope produced by
    /// [`Self::export_private_states`].
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

    /// Import signing keys from an envelope produced by
    /// [`Self::export_signing_keys`].
    async fn import_signing_keys(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError>;
}
