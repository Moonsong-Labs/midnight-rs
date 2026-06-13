use std::time::Duration;

use midnight_provider::ProviderError;

/// Reconciliation guidance appended to the Display of
/// [`ContractError::SubmissionWait`] and [`ContractError::FinalizeTimeout`]
/// when a pending private-state snapshot was recorded for the in-flight
/// transaction (`snapshot_written == true`).
const PENDING_SNAPSHOT_HINT: &str = " The pending snapshot was left on disk; reconcile by calling \
     `PrivateStateProvider::confirm` (if the chain accepted it) \
     or `mark_failed` (if not).";

/// Unified error type for all contract operations: query, call, deploy, submit.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("contract not found at address {0}")]
    NotFound(String),

    #[error("state deserialization error: {0}")]
    State(#[from] midnight_bindgen_runtime::StateError),

    #[error("interpreter error: {0}")]
    Interpreter(#[from] crate::interpreter::InterpreterError),

    #[error("private state error: {0}")]
    PrivateState(#[from] midnight_provider::PrivateStateError),

    #[error("transaction construction failed: {0}")]
    Construction(String),

    #[error("serialization failed: {0}")]
    Serialization(String),

    #[error("state fetch failed: {0}")]
    StateFetch(String),

    #[error("invalid address: {0}")]
    InvalidAddress(String),

    #[error("submission failed: {0}")]
    Submission(String),

    /// A circuit-call transaction was submitted but the bounded wait for
    /// finalization failed. The failed wait does **not** retract the
    /// transaction: it may still land. Every wait error funnels through
    /// [`ProviderError::Submission`] carrying a
    /// [`SubmitError`](midnight_provider::SubmitError), so `source` is
    /// always that pair; match the inner kind to pick the recovery path:
    /// `Invalid` is a definitive rejection (mark any pending private-state
    /// snapshot failed and rebuild), while `Dropped` / `NodeError` /
    /// `WatchStream` leave the transaction's fate unknown (reconcile once
    /// the chain's view of `extrinsic_hash` is known).
    ///
    /// ```rust,ignore
    /// match err {
    ///     ContractError::SubmissionWait {
    ///         source: ProviderError::Submission(SubmitError::Invalid { .. }),
    ///         ..
    ///     } => { /* definitive rejection: mark_failed, rebuild, resubmit */ }
    ///     ContractError::SubmissionWait { extrinsic_hash, .. } => {
    ///         /* fate unknown: query the chain for extrinsic_hash, then
    ///            confirm (it landed) or mark_failed (it didn't) */
    ///     }
    ///     _ => { /* ... */ }
    /// }
    /// ```
    #[error(
        "wait_finalized failed for tx {extrinsic_hash}: {source}.{}",
        if *snapshot_written { PENDING_SNAPSHOT_HINT } else { "" }
    )]
    SubmissionWait {
        /// Hex extrinsic hash (no `0x` prefix) of the in-flight transaction.
        extrinsic_hash: String,
        /// The provider error the wait surfaced: always
        /// [`ProviderError::Submission`] carrying a
        /// [`SubmitError`](midnight_provider::SubmitError).
        source: ProviderError,
        /// Whether a pending private-state snapshot was recorded for this
        /// transaction. When `true`, the Display appends reconciliation
        /// guidance for the on-disk snapshot.
        snapshot_written: bool,
    },

    /// A circuit-call transaction was submitted but did not finalize within
    /// `timeout`. The transaction may be in the mempool or already included
    /// in a non-finalized block; cancelling the wait does not retract it,
    /// so it may still land later. Query the chain for `extrinsic_hash` to
    /// learn its fate before rebuilding or resubmitting.
    #[error(
        "tx {extrinsic_hash} not finalized within {timeout:?}. The tx may be \
         in the mempool or already included in a non-finalized block; \
         cancelling the wait does not retract it, so it may still land \
         later.{}",
        if *snapshot_written { PENDING_SNAPSHOT_HINT } else { "" }
    )]
    FinalizeTimeout {
        /// Hex extrinsic hash (no `0x` prefix) of the in-flight transaction.
        extrinsic_hash: String,
        /// The deadline the finalization wait was bounded by.
        timeout: Duration,
        /// Whether a pending private-state snapshot was recorded for this
        /// transaction. When `true`, the Display appends reconciliation
        /// guidance for the on-disk snapshot.
        snapshot_written: bool,
    },

    /// The transaction landed in a finalized block but the chain didn't
    /// apply it. `status` is `"PartialSuccess"` (guaranteed phase committed,
    /// at least one fallible segment failed) or `"Failure"` (whole dispatch
    /// rejected, nothing on chain). Unlike [`SubmissionWait`] and
    /// [`FinalizeTimeout`], this is a definitive verdict: nothing is left to
    /// reconcile. For `Contract::call_with`, the orphan `Pending` snapshot
    /// (when one was recorded) has already been cascade-dropped via
    /// `mark_failed` by the time the caller sees this error.
    ///
    /// [`SubmissionWait`]: ContractError::SubmissionWait
    /// [`FinalizeTimeout`]: ContractError::FinalizeTimeout
    #[error(
        "transaction {} landed on chain but the fallible phase reported {status}; \
         no state advance",
        hex::encode(extrinsic_hash)
    )]
    TransactionFailed {
        extrinsic_hash: [u8; 32],
        status: String,
    },

    /// A circuit-call transaction was submitted (it is on the wire and may
    /// land) but recording the pending private-state snapshot for it
    /// failed, so **no local snapshot exists**. Query the chain for
    /// `extrinsic_hash` to determine the transaction's status; if it
    /// landed, the post-call private state must be reconstructed manually.
    #[error(
        "tx {extrinsic_hash} was submitted but `append_pending` failed: \
         {source}. The tx is in flight; query the chain to determine its \
         status. No local snapshot was recorded."
    )]
    PendingSnapshotFailed {
        /// Hex extrinsic hash (no `0x` prefix) of the in-flight transaction.
        extrinsic_hash: String,
        /// The private-state store error that prevented the snapshot.
        source: midnight_provider::PrivateStateError,
    },

    #[error("maintenance error: {0}")]
    Maintenance(String),
}
