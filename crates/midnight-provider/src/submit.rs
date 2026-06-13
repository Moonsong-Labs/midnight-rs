//! Transaction submission over the node's WebSocket RPC.
//!
//! Lives on the provider because it's pure transport — connects to
//! [`MidnightProvider::node_url`], submits the proven tx bytes as an
//! unsigned `Midnight::send_mn_transaction` extrinsic, and returns a
//! [`PendingTx`] handle that drives the watch stream to inclusion /
//! finalization.

use crate::ProviderError;

/// Why a transaction submission (or the wait for its inclusion) failed.
///
/// Carried by [`ProviderError::Submission`]. The variants matter because
/// they imply different recovery paths — in particular, [`Invalid`] is the
/// only definitive rejection; everything else leaves the transaction's fate
/// ambiguous and resubmitting the same inputs risks a double spend.
/// The one exception is [`VerdictFetch`]: the transaction is known to be in
/// a block, only its outcome could not be learned.
///
/// [`Invalid`]: SubmitError::Invalid
/// [`VerdictFetch`]: SubmitError::VerdictFetch
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubmitError {
    /// The submission pipeline failed before the transaction was handed to
    /// the node: connecting, fetching metadata, or encoding the unsigned
    /// extrinsic. The transaction never left this process, so resubmitting
    /// the same bytes is always safe.
    #[error("not submitted: {message}")]
    NotSubmitted { message: String },

    /// The `submit_and_watch` RPC call itself failed. On a clean error
    /// response the node refused the transaction at submission and it is
    /// not in the pool (safe to rebuild and resubmit); on a transport
    /// failure mid-call the node may have received it anyway — confirm
    /// via the chain (e.g. `wait_transaction_result`) before resubmitting.
    #[error("submit RPC: {message}")]
    SubmitRpc { message: String },

    /// The node reported the transaction as invalid (bad nonce, signature,
    /// failed guaranteed phase, ...). This is a **definitive rejection**:
    /// the transaction will not be included. It is safe to rebuild and
    /// resubmit with fresh inputs.
    #[error("invalid: {message}")]
    Invalid { message: String },

    /// The node dropped the transaction from its pool. **Not** a definitive
    /// rejection: the transaction may already have been gossiped to peers
    /// and can still be included in a later block. Resubmitting a
    /// transaction that spends the same inputs risks a double-spend
    /// conflict — confirm the original is absent from the chain (or let its
    /// TTL expire) before rebuilding.
    #[error("dropped: {message}")]
    Dropped { message: String },

    /// The node hit an internal error while tracking the transaction. Its
    /// fate is unknown — treat like [`Dropped`](SubmitError::Dropped):
    /// the transaction may still be included, so resubmitting the same
    /// inputs risks a double spend.
    #[error("node error: {message}")]
    NodeError { message: String },

    /// The watch subscription failed or ended before the transaction
    /// reached the awaited status (a transport/stream issue, or the stream
    /// was already consumed by a previous wait). Says nothing about the
    /// transaction itself: it stays in the node's pool and may still land.
    /// Re-query the chain (e.g. `wait_transaction_result`) instead of
    /// resubmitting.
    #[error("watch stream: {message}")]
    WatchStream { message: String },

    /// The transaction landed in a block, but fetching or decoding the
    /// extrinsic's events failed, so the chain's [`Verdict`] could not be
    /// derived. The transaction is on chain (provisionally, for a
    /// best-block wait); do **not** resubmit. Re-query the chain for the
    /// extrinsic's events (e.g. `wait_transaction_result` against the
    /// indexer) to learn whether it applied.
    #[error("verdict fetch: {message}")]
    VerdictFetch { message: String },
}

impl SubmitError {
    /// Map a terminal subxt watch status to the structured error it
    /// surfaces. Returns `None` for non-terminal statuses and for the two
    /// success terminals (`InBestBlock` / `InFinalizedBlock`), which the
    /// wait loops handle themselves.
    fn from_terminal_status<T: subxt::Config, C>(
        status: &subxt::tx::TransactionStatus<T, C>,
    ) -> Option<Self> {
        use subxt::tx::TransactionStatus;
        match status {
            TransactionStatus::Invalid { message } => Some(Self::Invalid {
                message: message.clone(),
            }),
            TransactionStatus::Dropped { message } => Some(Self::Dropped {
                message: message.clone(),
            }),
            TransactionStatus::Error { message } => Some(Self::NodeError {
                message: message.clone(),
            }),
            // Explicit non-terminal arms (no `_`) so a new terminal variant
            // in a future subxt bump fails to compile here instead of
            // degrading into a misleading `WatchStream` error.
            TransactionStatus::Validated
            | TransactionStatus::Broadcasted
            | TransactionStatus::NoLongerInBestBlock
            | TransactionStatus::InBestBlock(_)
            | TransactionStatus::InFinalizedBlock(_) => None,
        }
    }

    /// The watch stream itself yielded an error.
    fn watch(e: impl std::fmt::Display) -> Self {
        Self::WatchStream {
            message: e.to_string(),
        }
    }

    /// The watch stream ended without a terminal status.
    fn stream_ended(awaiting: &str) -> Self {
        Self::WatchStream {
            message: format!("stream ended before {awaiting}"),
        }
    }
}

/// Inclusion details for a transaction that landed in a block, together with
/// the chain's verdict on whether it actually applied.
///
/// `block_hash` and `extrinsic_hash` come from subxt's
/// `TransactionStatus::InBestBlock` / `InFinalizedBlock`. `verdict` is the
/// SDK's interpretation of the Midnight pallet's outcome events: the
/// `Midnight` pallet always emits `TxApplied` (all segments applied) or
/// `TxPartialSuccess` (at least one fallible segment failed) for a
/// successful dispatch, and falls back to `System::ExtrinsicFailed` when the
/// dispatch errored entirely. See [`Verdict`].
#[derive(Debug, Clone, Copy)]
pub struct TxInBlock {
    pub block_hash: [u8; 32],
    pub extrinsic_hash: [u8; 32],
    pub verdict: Verdict,
}

/// What actually happened to a Midnight transaction once it landed in a block.
///
/// All Midnight transactions (deploys, contract calls, maintenance, shielded
/// transfers, unshielded transfers, dust registration) go through the same
/// `Midnight::send_mn_transaction` entrypoint, so every finalized tx emits
/// exactly one of these outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// `Midnight::TxApplied`: every fallible segment succeeded; the chain
    /// state advanced fully.
    Success,
    /// `Midnight::TxPartialSuccess`: the guaranteed phase committed
    /// (Zswap I/O, fees, signatures landed on chain), but at least one
    /// fallible segment failed and was not applied.
    ///
    /// The on-chain event doesn't carry a per-segment breakdown, but the
    /// SDK's tx shapes hold one fallible segment each
    /// (`Contract::call_with` -> one contract call;
    /// `DeployBuilder` -> one deploy; maintenance -> one update), so within
    /// those flows `PartialSuccess` unambiguously means "my segment didn't
    /// apply". Callers that build multi-segment txs and need a per-segment
    /// map should query the indexer's `TransactionResult::segments`.
    PartialSuccess,
    /// The dispatch errored entirely (`System::ExtrinsicFailed`). Nothing
    /// landed on chain; no Zswap I/O, no fees taken. Rare in normal
    /// operation, since guaranteed-phase failures are normally rejected at
    /// submission.
    Failure,
}

/// Handle to a submitted transaction whose progress can be awaited.
///
/// Returned by [`crate::MidnightProvider::submit`]. Both
/// [`PendingTx::wait_best`] and [`PendingTx::wait_finalized`] consume
/// `self` and return the handle back alongside the inclusion details,
/// so callers re-bind the same name through each step without needing
/// `let mut`. Either may be called first; `wait_finalized` skips the
/// best-block status if `wait_best` was not used. Calling either method
/// twice (or `wait_best` after `wait_finalized`) returns a
/// [`SubmitError::WatchStream`] error because subxt closes the stream once
/// the transaction reaches a terminal state.
///
/// # Errors
///
/// Both wait methods fail with [`ProviderError::Submission`] carrying a
/// [`SubmitError`]; match its variants instead of parsing error text. The
/// distinction matters for recovery:
///
/// - [`SubmitError::Invalid`] — definitively rejected; safe to rebuild and
///   resubmit with fresh inputs.
/// - [`SubmitError::Dropped`] / [`SubmitError::NodeError`] — the tx may
///   still be re-included; resubmitting the same inputs risks a double
///   spend.
/// - [`SubmitError::WatchStream`] — transport/stream trouble only; the tx
///   stays in the pool and may still land.
/// - [`SubmitError::VerdictFetch`] — the tx is in a block, but its outcome
///   events could not be fetched or decoded; re-query the chain for the
///   verdict instead of resubmitting.
///
/// # Timeouts and cancellation
///
/// Neither wait method imposes a deadline. If the node accepts the
/// transaction but the chain stalls (no block production, or no
/// finalization after inclusion), the underlying subxt stream stays open
/// and the wait future blocks indefinitely. Callers that need a deadline
/// should wrap the wait in [`tokio::time::timeout`]:
///
/// ```rust,ignore
/// use std::time::Duration;
///
/// let (best, pending) = tokio::time::timeout(
///     Duration::from_secs(60),
///     pending.wait_best(),
/// ).await??;
/// ```
///
/// Cancelling the wait future (drop, `tokio::select!`, timeout) is safe
/// and asynchronously closes the subxt subscription. It does **not**
/// retract the transaction from the mempool; the node keeps it queued
/// until it lands in a block or is dropped by the node itself.
pub struct PendingTx {
    progress: subxt::tx::TransactionProgress<
        subxt::SubstrateConfig,
        subxt::client::OnlineClientAtBlockImpl<subxt::SubstrateConfig>,
    >,
}

impl PendingTx {
    /// The hash of the submitted extrinsic.
    pub fn extrinsic_hash(&self) -> [u8; 32] {
        self.progress.extrinsic_hash().0
    }

    /// The extrinsic hash formatted as a hex string (no `0x` prefix).
    pub fn extrinsic_hash_hex(&self) -> String {
        hex::encode(self.extrinsic_hash())
    }

    /// Drive the watch stream until the transaction lands in the best block.
    ///
    /// Best-block inclusion is provisional: the block can still be reorged
    /// out before finalization. The returned [`TxInBlock::verdict`] reflects
    /// the events the block author emitted, so it can change if a different
    /// block wins the chain race. Use [`Self::wait_finalized`] when you
    /// need an authoritative verdict.
    ///
    /// See the [type-level docs](PendingTx#errors) for the [`SubmitError`]
    /// kinds a failed wait surfaces and what each implies about retrying.
    pub async fn wait_best(mut self) -> Result<(TxInBlock, Self), ProviderError> {
        use subxt::tx::TransactionStatus;
        while let Some(status) = self.progress.next().await {
            let status = status.map_err(SubmitError::watch)?;
            if let Some(err) = SubmitError::from_terminal_status(&status) {
                return Err(err.into());
            }
            if let TransactionStatus::InBestBlock(in_block) = status {
                let tx = tx_in_block_with_verdict(&in_block).await?;
                return Ok((tx, self));
            }
        }
        Err(SubmitError::stream_ended("reaching best block").into())
    }

    /// Drive the watch stream until the transaction is in a finalized block.
    /// Past finality the block can't be reorged out under honest-majority
    /// assumptions, so the returned [`TxInBlock::verdict`] is authoritative.
    ///
    /// See the [type-level docs](PendingTx#errors) for the [`SubmitError`]
    /// kinds a failed wait surfaces and what each implies about retrying.
    pub async fn wait_finalized(mut self) -> Result<(TxInBlock, Self), ProviderError> {
        use subxt::tx::TransactionStatus;
        while let Some(status) = self.progress.next().await {
            let status = status.map_err(SubmitError::watch)?;
            if let Some(err) = SubmitError::from_terminal_status(&status) {
                return Err(err.into());
            }
            if let TransactionStatus::InFinalizedBlock(in_block) = status {
                let tx = tx_in_block_with_verdict(&in_block).await?;
                return Ok((tx, self));
            }
        }
        Err(SubmitError::stream_ended("finalization").into())
    }
}

/// Fetch the extrinsic's events and derive the [`Verdict`] from the
/// `Midnight` pallet's `TxApplied` / `TxPartialSuccess` events. Default to
/// `Failure` when neither is present (the dispatch errored and only
/// `System::ExtrinsicFailed` was emitted).
async fn tx_in_block_with_verdict(
    in_block: &subxt::tx::TransactionInBlock<
        subxt::SubstrateConfig,
        subxt::client::OnlineClientAtBlockImpl<subxt::SubstrateConfig>,
    >,
) -> Result<TxInBlock, ProviderError> {
    let block_hash = in_block.block_hash().0;
    let extrinsic_hash = in_block.extrinsic_hash().0;
    let events = in_block
        .fetch_events()
        .await
        .map_err(|e| SubmitError::VerdictFetch {
            message: format!("fetch events: {e}"),
        })?;
    let mut verdict = Verdict::Failure;
    for ev in events.iter() {
        let ev = ev.map_err(|e| SubmitError::VerdictFetch {
            message: format!("decode event: {e}"),
        })?;
        match (ev.pallet_name(), ev.event_name()) {
            ("Midnight", "TxApplied") => {
                verdict = Verdict::Success;
                break;
            }
            ("Midnight", "TxPartialSuccess") => {
                verdict = Verdict::PartialSuccess;
                break;
            }
            _ => {}
        }
    }
    Ok(TxInBlock {
        block_hash,
        extrinsic_hash,
        verdict,
    })
}

/// A transaction that has been built and validated against the node but
/// **not yet submitted**. Its [`extrinsic_hash`](Self::extrinsic_hash) is
/// already known, so a caller can durably record state keyed by that hash
/// (e.g. a private-state journal entry) *before* the transaction hits the
/// mempool, then [`submit`](Self::submit) it. This closes the window where
/// a crash between submit and record would leave a transaction on the wire
/// with no local handle to reconcile it.
pub struct PreparedTx {
    tx: subxt::tx::SubmittableTransaction<
        subxt::SubstrateConfig,
        subxt::client::OnlineClientAtBlockImpl<subxt::SubstrateConfig>,
    >,
}

impl PreparedTx {
    /// The hash subxt will report for this extrinsic once submitted,
    /// computed here from the encoded transaction without contacting the
    /// node. Identical to the eventual [`PendingTx::extrinsic_hash`].
    pub fn extrinsic_hash(&self) -> [u8; 32] {
        self.tx.hash().0
    }

    /// Submit the prepared transaction and return a [`PendingTx`] for
    /// awaiting inclusion / finalization. On failure the transaction never
    /// reached the node (or its fate is ambiguous per [`SubmitError`]).
    pub async fn submit(self) -> Result<PendingTx, ProviderError> {
        let progress = self
            .tx
            .submit_and_watch()
            .await
            .map_err(|e| SubmitError::SubmitRpc {
                message: e.to_string(),
            })?;
        Ok(PendingTx { progress })
    }
}

/// Build and validate proven transaction bytes against a Midnight node
/// without submitting them. The returned [`PreparedTx`] exposes the
/// extrinsic hash and a `submit` step.
pub(crate) async fn prepare_bytes(
    node_url: &str,
    tx_bytes: &[u8],
) -> Result<PreparedTx, ProviderError> {
    use subxt::{OnlineClient, SubstrateConfig};

    let client = OnlineClient::<SubstrateConfig>::from_insecure_url(node_url)
        .await
        .map_err(|e| SubmitError::NotSubmitted {
            message: format!("connect: {e}"),
        })?;

    let call = subxt::dynamic::tx(
        "Midnight",
        "send_mn_transaction",
        vec![subxt::dynamic::Value::from_bytes(tx_bytes)],
    );

    let tx_client = client.tx().await.map_err(|e| SubmitError::NotSubmitted {
        message: format!("tx client: {e}"),
    })?;
    let tx = tx_client
        .create_unsigned(&call)
        .map_err(|e| SubmitError::NotSubmitted {
            message: format!("create unsigned: {e}"),
        })?;
    Ok(PreparedTx { tx })
}

/// Submit proven transaction bytes to a Midnight node and return a handle
/// for awaiting inclusion / finalization.
pub(crate) async fn submit_bytes(
    node_url: &str,
    tx_bytes: &[u8],
) -> Result<PendingTx, ProviderError> {
    prepare_bytes(node_url, tx_bytes).await?.submit().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use subxt::SubstrateConfig;
    use subxt::tx::TransactionStatus;

    /// The client type parameter is irrelevant for the terminal-status
    /// variants, which only carry a message.
    type Status = TransactionStatus<SubstrateConfig, ()>;

    #[test]
    fn invalid_status_maps_to_invalid() {
        let status = Status::Invalid {
            message: "bad nonce".into(),
        };
        assert_eq!(
            SubmitError::from_terminal_status(&status),
            Some(SubmitError::Invalid {
                message: "bad nonce".into()
            })
        );
    }

    #[test]
    fn dropped_status_maps_to_dropped() {
        let status = Status::Dropped {
            message: "pool full".into(),
        };
        assert_eq!(
            SubmitError::from_terminal_status(&status),
            Some(SubmitError::Dropped {
                message: "pool full".into()
            })
        );
    }

    #[test]
    fn error_status_maps_to_node_error() {
        let status = Status::Error {
            message: "node exploded".into(),
        };
        assert_eq!(
            SubmitError::from_terminal_status(&status),
            Some(SubmitError::NodeError {
                message: "node exploded".into()
            })
        );
    }

    #[test]
    fn non_terminal_statuses_are_not_errors() {
        for status in [
            Status::Validated,
            Status::Broadcasted,
            Status::NoLongerInBestBlock,
        ] {
            assert_eq!(SubmitError::from_terminal_status(&status), None);
        }
    }

    #[test]
    fn watch_stream_failures_map_to_watch_stream() {
        assert_eq!(
            SubmitError::watch("connection reset"),
            SubmitError::WatchStream {
                message: "connection reset".into()
            }
        );
        assert_eq!(
            SubmitError::stream_ended("finalization"),
            SubmitError::WatchStream {
                message: "stream ended before finalization".into()
            }
        );
    }

    #[test]
    fn submit_error_converts_into_provider_submission() {
        let err: ProviderError = SubmitError::Invalid {
            message: "bad signature".into(),
        }
        .into();
        match err {
            ProviderError::Submission(SubmitError::Invalid { message }) => {
                assert_eq!(message, "bad signature");
            }
            other => panic!("expected Submission(Invalid), got {other:?}"),
        }
    }

    #[test]
    fn display_keeps_the_node_message() {
        let err: ProviderError = SubmitError::Dropped {
            message: "usurped by tx 0xabc".into(),
        }
        .into();
        assert_eq!(err.to_string(), "submission: dropped: usurped by tx 0xabc");
    }
}
