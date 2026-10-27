use std::time::Duration;

use midnight_provider::{NotApplied, ProviderError, SubmitError, TransactionHash, TxInBlock};
use midnight_types::ExtrinsicHash;

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
    /// A failure below the contract layer.
    ///
    /// It comes from the node RPC, the wallet sync, coin selection, fee
    /// balancing, or proving. It never holds
    /// [`ProviderError::NotApplied`]: the conversion from [`ProviderError`]
    /// turns that into [`TransactionFailed`](ContractError::TransactionFailed).
    ///
    /// Match the inner [`ProviderError`] to find the cause. When the proof of a
    /// deploy, a call or a maintenance update fails, the error is
    /// `ProviderError::Wallet(WalletError::Proving(_))`. When a call or the
    /// shielded offer of a deploy names a shielded coin that another build
    /// holds, the error is
    /// `ProviderError::Wallet(WalletError::InputsReserved { .. })`. When it
    /// names a shielded coin twice, the error is
    /// `ProviderError::Wallet(WalletError::DuplicateInput { .. })`. When the
    /// node would refuse the transaction as outside its time to dismiss, the
    /// error is `ProviderError::Wallet(WalletError::OutsideTimeToDismiss { .. })`.
    /// See
    /// [`WalletError`](midnight_provider::WalletError) for the other wallet
    /// failures.
    #[error(transparent)]
    Provider(ProviderError),

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

    /// A deploy's shielded offer pays more of a token than its coins hold.
    ///
    /// The deploy stops before it proves anything, and the wallet gets the
    /// offer's coins back. Add coins of `token_type` to the offer, or pay
    /// less of it.
    #[error("the shielded offer pays {paid} of token {token_type} but its coins hold {held}")]
    ShieldedOfferShortfall {
        /// The token that the coins do not cover.
        token_type: midnight_types::ShieldedTokenType,
        /// The value of the token that the offer's coins hold, zero when no
        /// coin holds it.
        held: u128,
        /// The value of the token that the offer's outputs pay.
        paid: u128,
    },

    #[error("serialization failed: {0}")]
    Serialization(String),

    #[error("state fetch failed: {0}")]
    StateFetch(String),

    #[error("invalid address: {0}")]
    InvalidAddress(String),

    /// The SDK submitted a circuit-call transaction, or tried to, and got no verdict.
    ///
    /// A failed submit or wait does **not** retract the transaction, so it can
    /// still land. A wait that reads a verdict other than `Success` gives
    /// [`TransactionFailed`](ContractError::TransactionFailed) instead. Match
    /// the [`SubmitError`] kind in `source` to choose the recovery:
    ///
    /// - `Invalid`: the node rejected the transaction, and the rejection is
    ///   final. Mark any pending private-state snapshot failed, then build
    ///   again. The wallet has the inputs that the call reserved back before
    ///   the error returns.
    /// - `SubmitRpc`, `Dropped`, `NodeError` or `WatchStream`: the fate of the
    ///   transaction is unknown, and it can still land until its TTL passes. A
    ///   `SubmitRpc` comes from the submit call, before any wait. It does not
    ///   say whether the node holds the transaction (see
    ///   [`SubmitRpc`](SubmitError::SubmitRpc)). The inputs that the call
    ///   reserved stay reserved.
    /// - [`VerdictFetch`]: the transaction is in a block, and only its verdict
    ///   is unknown. Do not resubmit. Find the verdict with the check below.
    ///
    /// The SDK has no lookup of a transaction by its hash, so find the fate on
    /// the chain. Read the events of the finalized blocks from the submission
    /// on. A block whose `Midnight::TxApplied` event names `transaction_hash`
    /// applied the transaction. A `Midnight::TxPartialSuccess` event that names
    /// it means that the transaction did not apply. When a finalized block is
    /// later than the TTL, and no finalized block names the transaction in a
    /// `TxApplied` event, the transaction did not apply. If `snapshot_written`
    /// is true, call `confirm` when it applied, or `mark_failed` when it did
    /// not, with `extrinsic_hash`. Until you reconcile the snapshot, the next
    /// call on this contract builds on it. A `mark_failed` of it then also
    /// drops the snapshots of the later calls.
    ///
    /// ```rust
    /// # use midnight_contract::{ContractError, SubmitError};
    /// # fn handle(err: ContractError) {
    /// match err {
    ///     ContractError::SubmissionWait {
    ///         source: SubmitError::Invalid { .. },
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
    ///         /* fate or verdict unknown: look for a finalized block whose
    ///            TxApplied event names transaction_hash; then, if
    ///            snapshot_written, confirm (it applied) or mark_failed (it did
    ///            not) the snapshot keyed by extrinsic_hash */
    ///     }
    ///     _ => { /* ... */ }
    /// }
    /// # }
    /// ```
    ///
    /// [`VerdictFetch`]: midnight_provider::SubmitError::VerdictFetch
    #[error(
        "transaction {transaction_hash} (extrinsic {extrinsic_hash:x}) has no verdict: \
         {source}.{}",
        if *snapshot_written { PENDING_SNAPSHOT_HINT } else { "" }
    )]
    SubmissionWait {
        /// The Midnight transaction hash. See
        /// [`PendingTx::transaction_hash`](midnight_provider::PendingTx::transaction_hash).
        transaction_hash: TransactionHash,
        /// The extrinsic hash of the in-flight transaction. The pending
        /// private-state snapshot uses it as its key.
        extrinsic_hash: ExtrinsicHash,
        /// The failure of the submit or of the wait.
        source: SubmitError,
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
    /// Do not build or submit again from the same inputs before you know its
    /// fate. Find the fate with the check that [`SubmissionWait`] describes.
    /// Then, if `snapshot_written` is true, call `confirm` (the transaction
    /// applied) or `mark_failed` (it did not) with `extrinsic_hash`. Until you
    /// reconcile the snapshot, the next call on this contract builds on it. A
    /// `mark_failed` of it then also drops the snapshots of the later calls.
    ///
    /// [`SubmissionWait`]: ContractError::SubmissionWait
    #[error(
        "transaction {transaction_hash} (extrinsic {extrinsic_hash:x}) did not \
         finalize within {timeout:?}. It can be in the mempool, or in a block \
         that is not final yet. The timeout does not retract it, so it can \
         still land.{}",
        if *snapshot_written { PENDING_SNAPSHOT_HINT } else { "" }
    )]
    FinalizeTimeout {
        /// The Midnight transaction hash. See
        /// [`PendingTx::transaction_hash`](midnight_provider::PendingTx::transaction_hash).
        transaction_hash: TransactionHash,
        /// The extrinsic hash of the in-flight transaction. The pending
        /// private-state snapshot uses it as its key.
        extrinsic_hash: ExtrinsicHash,
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
    /// verdict. `Contract::call_with` and `PendingCall::wait_finalized` drop the
    /// orphan `Pending` snapshot, when the call recorded one, with `mark_failed`
    /// before they return this error.
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
    /// The deadline bounds the wait for a block and the read of the deployed
    /// contract together, and `in_block` tells which stage timed out. Each
    /// stage has a different recovery:
    ///
    /// - `None`: no block included the deploy before the deadline. The deploy
    ///   can still land until its TTL passes, and a transaction that is not in
    ///   a block before its TTL never lands. A read that finds the contract at
    ///   `address` shows that it landed. Do not deploy again before the TTL
    ///   passes: a second deploy pays a second fee and makes a second contract.
    /// - `Some`: the chain applied the deploy in that block, and only the read
    ///   of the contract's state did not succeed. Connect to `address` with
    ///   [`Contract::at`]. A verdict from a best-block wait is provisional
    ///   until the block is final.
    ///
    /// [`Contract::at`]: crate::Contract::at
    #[error(
        "deploy of contract {address} did not complete within {timeout:?}. {}",
        match in_block {
            None => format!(
                "No block included transaction {transaction_hash} yet, and it can \
                 still land until its TTL passes: do not deploy again before then."
            ),
            Some(in_block) => format!(
                "Block {} applied transaction {transaction_hash}, but the read of \
                 the contract did not succeed: connect with `Contract::at`.",
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
    /// stopped before the submit. Nothing reached the chain, no snapshot
    /// exists, and the wallet has the inputs that the call reserved back. Fix
    /// the store error in `source`, then call again.
    #[error(
        "`append_pending` failed for extrinsic {extrinsic_hash:x}: {source}. The \
         transaction was not submitted, and nothing reached the chain: fix the \
         store error, then call again."
    )]
    PendingSnapshotFailed {
        /// The extrinsic hash of the transaction that the call did not submit.
        extrinsic_hash: ExtrinsicHash,
        /// The private-state store error that prevented the snapshot.
        source: midnight_provider::PrivateStateError,
    },

    /// A build-only call ran witnesses that change the contract's private
    /// state, and a build-only call has no way to record the new state.
    ///
    /// The generated `.build()` and `.without_dust()`, and
    /// [`Contract::build_call_with`](crate::Contract::build_call_with), give
    /// this error after the local run of the circuit. The call reserves no
    /// input, makes no proof, and writes no private-state snapshot. Submit the
    /// call with `.await` or `.send()`, which record the new state.
    #[error(
        "the witnesses of this call change the private state of contract {address}, and a \
         build-only call does not record it: submit the call with `.await` or `.send()`"
    )]
    UnrecordedPrivateState {
        /// The address of the contract whose private state the call changes.
        address: String,
    },

    #[error("maintenance error: {0}")]
    Maintenance(String),
}

impl From<ProviderError> for ContractError {
    fn from(error: ProviderError) -> Self {
        match error {
            ProviderError::NotApplied(not_applied) => Self::TransactionFailed(*not_applied),
            other => Self::Provider(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_provider::PrivateStateError;

    /// The call wait errors name the transaction hash, which the chain's
    /// `TxApplied` event names. A snapshot failure says that the call did not
    /// submit the transaction. Each names the whole extrinsic hash, the key of
    /// the snapshot.
    /// thiserror does not warn when a Display leaves out a field. A snapshot
    /// failure that says "submitted" sends the caller to look for a
    /// transaction that never left the process.
    #[test]
    fn call_errors_name_what_the_caller_reconciles_with() {
        // Distinct bytes, so a Display that prints only the extrinsic hash
        // does not match the transaction hash.
        let transaction_hash = TransactionHash::from([0xab; 32]);
        let extrinsic_hash = ExtrinsicHash::repeat_byte(0xcd);
        let cases = [
            (
                ContractError::SubmissionWait {
                    transaction_hash,
                    extrinsic_hash,
                    source: SubmitError::Dropped {
                        message: "pool full".into(),
                    },
                    snapshot_written: false,
                },
                transaction_hash.to_string(),
            ),
            (
                ContractError::FinalizeTimeout {
                    transaction_hash,
                    extrinsic_hash,
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
            assert!(
                message.contains(&format!("{extrinsic_hash:x}")),
                "expected the whole extrinsic hash in: {message}"
            );
        }
    }
}
