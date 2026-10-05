use std::time::Duration;

use midnight_provider::{NotApplied, ProviderError, TransactionHash, TxInBlock};

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
    #[error(transparent)]
    Provider(#[from] ProviderError),

    #[error("contract not found at address {0}")]
    NotFound(String),

    #[error("state deserialization error: {0}")]
    State(#[from] midnight_typed_state::StateError),

    #[error("interpreter error: {0}")]
    Interpreter(#[from] crate::runtime::InterpreterError),

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

    /// The SDK submitted a circuit-call transaction, but the wait for finality failed.
    ///
    /// A failed wait does **not** retract the transaction, so it can still
    /// land. `source` is always a [`ProviderError::Submission`] that carries a
    /// [`SubmitError`](midnight_provider::SubmitError). Match the inner kind
    /// to choose the recovery:
    ///
    /// - `Invalid`: the node rejected the transaction, and the rejection is
    ///   final. Mark any pending private-state snapshot failed, then build
    ///   again.
    /// - `Dropped`, `NodeError` or `WatchStream`: the fate of the transaction
    ///   is unknown, and it can still land.
    /// - [`VerdictFetch`]: the transaction is in a block, and only its verdict
    ///   is unknown. Do not resubmit.
    ///
    /// For an unknown fate or verdict, query the indexer by
    /// `transaction_hash` with [`MidnightProvider::get_transactions`]. If
    /// `snapshot_written` is true, call `confirm` (the transaction applied) or
    /// `mark_failed` (it did not) with `extrinsic_hash`. An empty result does
    /// not mean that the transaction failed: the indexer can lag, and the
    /// transaction can still land until its TTL passes. Call `mark_failed`
    /// only on a `PartialSuccess` or `Failure` result, or after the TTL.
    ///
    /// ```rust
    /// # use midnight_contract::{ContractError, SubmitError};
    /// # use midnight_provider::ProviderError;
    /// # fn handle(err: ContractError) {
    /// match err {
    ///     ContractError::SubmissionWait {
    ///         source: ProviderError::Submission(SubmitError::Invalid { .. }),
    ///         snapshot_written,
    ///         ..
    ///     } => { /* definitive rejection: if snapshot_written, mark_failed;
    ///               then build again */ }
    ///     ContractError::SubmissionWait {
    ///         transaction_hash,
    ///         extrinsic_hash,
    ///         snapshot_written,
    ///         ..
    ///     } => {
    ///         /* fate or verdict unknown: query the indexer by transaction_hash;
    ///            then, if snapshot_written, confirm (it applied) or mark_failed
    ///            (it did not) the snapshot keyed by extrinsic_hash */
    ///     }
    ///     _ => { /* ... */ }
    /// }
    /// # }
    /// ```
    ///
    /// [`VerdictFetch`]: midnight_provider::SubmitError::VerdictFetch
    /// [`MidnightProvider::get_transactions`]: midnight_provider::MidnightProvider::get_transactions
    #[error(
        "transaction {transaction_hash} (extrinsic {extrinsic_hash}): the wait for \
         finality failed: {source}.{}",
        if *snapshot_written { PENDING_SNAPSHOT_HINT } else { "" }
    )]
    SubmissionWait {
        /// The Midnight transaction hash. An indexer query by hash takes it.
        // Boxed: unboxed, this variant takes `ContractError` past clippy's
        // `result_large_err` limit, and every `Result` here pays that size.
        transaction_hash: Box<TransactionHash>,
        /// Hex extrinsic hash (no `0x` prefix) of the in-flight transaction.
        /// The pending private-state snapshot uses it as its key.
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

    /// The SDK submitted a circuit-call transaction, but it did not finalize in time.
    ///
    /// The transaction can be in the mempool, or in a block that is not final
    /// yet. The timeout does not retract it, so it can still land.
    ///
    /// Before you build or submit again, query the indexer by
    /// `transaction_hash` with [`MidnightProvider::get_transactions`] to learn
    /// its fate. If `snapshot_written` is true, call `confirm` (the transaction
    /// applied) or `mark_failed` (it did not) with `extrinsic_hash`. An empty
    /// result does not mean that the transaction failed: the indexer can lag,
    /// and the transaction can still land until its TTL passes. Call
    /// `mark_failed` only on a `PartialSuccess` or `Failure` result, or after
    /// the TTL.
    ///
    /// [`MidnightProvider::get_transactions`]: midnight_provider::MidnightProvider::get_transactions
    #[error(
        "transaction {transaction_hash} (extrinsic {extrinsic_hash}) did not \
         finalize within {timeout:?}. It can be in the mempool, or in a block \
         that is not final yet. The timeout does not retract it, so it can \
         still land.{}",
        if *snapshot_written { PENDING_SNAPSHOT_HINT } else { "" }
    )]
    FinalizeTimeout {
        /// The Midnight transaction hash. An indexer query by hash takes it.
        // Boxed like `SubmissionWait::transaction_hash`, so one or-pattern
        // binds the field of both variants.
        transaction_hash: Box<TransactionHash>,
        /// Hex extrinsic hash (no `0x` prefix) of the in-flight transaction.
        /// The pending private-state snapshot uses it as its key.
        extrinsic_hash: String,
        /// The deadline the finalization wait was bounded by.
        timeout: Duration,
        /// Whether a pending private-state snapshot was recorded for this
        /// transaction. When `true`, the Display appends reconciliation
        /// guidance for the on-disk snapshot.
        snapshot_written: bool,
    },

    /// The transaction landed in a block, but the chain did not apply it.
    /// The wrapped [`NotApplied`] carries the [`TxInBlock`], whose `verdict`
    /// tells the two cases apart: [`Verdict::PartialSuccess`] (the guaranteed
    /// phase committed, at least one fallible segment failed) and
    /// [`Verdict::Failure`] (the dispatch errored entirely, so no phase ran).
    /// Unlike [`SubmissionWait`] and [`FinalizeTimeout`], the chain gave a
    /// verdict. For `Contract::call_with`, the orphan `Pending` snapshot (when
    /// one was recorded) has already been cascade-dropped via `mark_failed` by
    /// the time the caller sees this error.
    ///
    /// [`SubmissionWait`]: ContractError::SubmissionWait
    /// [`FinalizeTimeout`]: ContractError::FinalizeTimeout
    /// [`TxInBlock`]: midnight_provider::TxInBlock
    /// [`Verdict::PartialSuccess`]: midnight_provider::Verdict::PartialSuccess
    /// [`Verdict::Failure`]: midnight_provider::Verdict::Failure
    #[error(transparent)]
    TransactionFailed(#[from] NotApplied),

    /// A deploy did not complete before the deploy deadline.
    ///
    /// The deadline bounds the wait for a block and the indexer poll together,
    /// and `in_block` tells which stage timed out. Each stage has a different
    /// recovery:
    ///
    /// - `None`: no block included the deploy before the deadline. The deploy
    ///   can still land, so query `transaction_hash` before you deploy again.
    ///   A second deploy pays a second fee and makes a second contract.
    /// - `Some`: the chain applied the deploy in that block, and only the
    ///   indexer lags. Connect to `address` with [`Contract::at`]. A verdict
    ///   from a best-block wait is provisional until the block is final.
    ///
    /// [`Contract::at`]: crate::Contract::at
    #[error(
        "deploy of contract {address} did not complete within {timeout:?}. {}",
        match in_block {
            None => format!(
                "No block included transaction {transaction_hash} yet, and it can \
                 still land: query it before you deploy again."
            ),
            Some(in_block) => format!(
                "Block {} applied transaction {transaction_hash}, but the indexer \
                 does not show the contract yet: connect with `Contract::at`.",
                hex::encode(in_block.block_hash)
            ),
        }
    )]
    DeployTimeout {
        /// The address of the deployed contract.
        address: String,
        /// The Midnight transaction that carries the deploy. When `in_block`
        /// is `Some`, it is the same as `in_block.transaction_hash`.
        transaction_hash: TransactionHash,
        /// The length of the deadline, from the start of
        /// [`PendingDeploy::into_contract`](crate::PendingDeploy::into_contract).
        timeout: Duration,
        /// Where the chain applied the deploy, or `None` when no block
        /// included it before the deadline.
        // Boxed: unboxed, this variant takes `ContractError` past clippy's
        // `result_large_err` limit, and every `Result` here pays that size.
        in_block: Option<Box<TxInBlock>>,
    },

    /// A circuit call could not record its pending private-state snapshot.
    ///
    /// The call records the snapshot before it submits the transaction, so it
    /// stopped before the submit. Nothing reached the chain, and no snapshot
    /// exists. Fix the store error in `source`, then call again.
    #[error(
        "`append_pending` failed for extrinsic {extrinsic_hash}: {source}. The \
         transaction was not submitted, and nothing reached the chain: fix the \
         store error, then call again."
    )]
    PendingSnapshotFailed {
        /// Hex extrinsic hash (no `0x` prefix) of the transaction that the call
        /// did not submit.
        extrinsic_hash: String,
        /// The private-state store error that prevented the snapshot.
        source: midnight_provider::PrivateStateError,
    },

    #[error("maintenance error: {0}")]
    Maintenance(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_provider::{PrivateStateError, SubmitError};

    /// The call wait errors name the transaction hash, the key that an indexer
    /// query by hash takes. A snapshot failure says that the call did not
    /// submit the transaction.
    /// thiserror does not warn when a Display leaves out a field. A snapshot
    /// failure that says "submitted" sends the caller to look for a
    /// transaction that never left the process.
    #[test]
    fn call_errors_name_what_the_caller_reconciles_with() {
        // Distinct bytes, so a Display that prints only the extrinsic hash
        // does not match the transaction hash.
        let transaction_hash = TransactionHash::from([0xab; 32]);
        let extrinsic_hash = hex::encode([0xcd; 32]);
        let cases = [
            (
                ContractError::SubmissionWait {
                    transaction_hash: Box::new(transaction_hash),
                    extrinsic_hash: extrinsic_hash.clone(),
                    source: ProviderError::Submission(SubmitError::Dropped {
                        message: "pool full".into(),
                    }),
                    snapshot_written: false,
                },
                transaction_hash.to_string(),
            ),
            (
                ContractError::FinalizeTimeout {
                    transaction_hash: Box::new(transaction_hash),
                    extrinsic_hash: extrinsic_hash.clone(),
                    timeout: Duration::from_secs(60),
                    snapshot_written: false,
                },
                transaction_hash.to_string(),
            ),
            (
                ContractError::PendingSnapshotFailed {
                    extrinsic_hash,
                    source: PrivateStateError::Io("disk full".into()),
                },
                "not submitted".to_string(),
            ),
        ];
        for (err, expected) in cases {
            let message = err.to_string();
            assert!(
                message.contains(&expected),
                "expected {expected:?} in: {message}"
            );
        }
    }
}
