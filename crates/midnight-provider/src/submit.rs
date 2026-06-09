//! Transaction submission over the node's WebSocket RPC.
//!
//! Lives on the provider because it's pure transport — connects to
//! [`MidnightProvider::node_url`], submits the proven tx bytes as an
//! unsigned `Midnight::send_mn_transaction` extrinsic, and returns a
//! [`PendingTx`] handle that drives the watch stream to inclusion /
//! finalization.

use crate::ProviderError;

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
/// twice (or `wait_best` after `wait_finalized`) returns a "watch stream
/// ended" error because subxt closes the stream once the transaction
/// reaches a terminal state.
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
    pub async fn wait_best(mut self) -> Result<(TxInBlock, Self), ProviderError> {
        use subxt::tx::TransactionStatus;
        while let Some(status) = self.progress.next().await {
            match status.map_err(|e| ProviderError::Submission(format!("watch: {e}")))? {
                TransactionStatus::InBestBlock(in_block) => {
                    let tx = tx_in_block_with_verdict(&in_block).await?;
                    return Ok((tx, self));
                }
                TransactionStatus::Error { message } => {
                    return Err(ProviderError::Submission(format!("error: {message}")));
                }
                TransactionStatus::Invalid { message } => {
                    return Err(ProviderError::Submission(format!("invalid: {message}")));
                }
                TransactionStatus::Dropped { message } => {
                    return Err(ProviderError::Submission(format!("dropped: {message}")));
                }
                _ => continue,
            }
        }
        Err(ProviderError::Submission(
            "watch stream ended before reaching best block".into(),
        ))
    }

    /// Drive the watch stream until the transaction is in a finalized block.
    /// Past finality the block can't be reorged out under honest-majority
    /// assumptions, so the returned [`TxInBlock::verdict`] is authoritative.
    pub async fn wait_finalized(mut self) -> Result<(TxInBlock, Self), ProviderError> {
        use subxt::tx::TransactionStatus;
        while let Some(status) = self.progress.next().await {
            match status.map_err(|e| ProviderError::Submission(format!("watch: {e}")))? {
                TransactionStatus::InFinalizedBlock(in_block) => {
                    let tx = tx_in_block_with_verdict(&in_block).await?;
                    return Ok((tx, self));
                }
                TransactionStatus::Error { message } => {
                    return Err(ProviderError::Submission(format!("error: {message}")));
                }
                TransactionStatus::Invalid { message } => {
                    return Err(ProviderError::Submission(format!("invalid: {message}")));
                }
                TransactionStatus::Dropped { message } => {
                    return Err(ProviderError::Submission(format!("dropped: {message}")));
                }
                _ => continue,
            }
        }
        Err(ProviderError::Submission(
            "watch stream ended before finalization".into(),
        ))
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
        .map_err(|e| ProviderError::Submission(format!("fetch events: {e}")))?;
    let mut verdict = Verdict::Failure;
    for ev in events.iter() {
        let ev = ev.map_err(|e| ProviderError::Submission(format!("decode event: {e}")))?;
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

/// Submit proven transaction bytes to a Midnight node and return a handle
/// for awaiting inclusion / finalization.
pub(crate) async fn submit_bytes(
    node_url: &str,
    tx_bytes: &[u8],
) -> Result<PendingTx, ProviderError> {
    use subxt::{OnlineClient, SubstrateConfig};

    let client = OnlineClient::<SubstrateConfig>::from_insecure_url(node_url)
        .await
        .map_err(|e| ProviderError::Submission(format!("connect: {e}")))?;

    let call = subxt::dynamic::tx(
        "Midnight",
        "send_mn_transaction",
        vec![subxt::dynamic::Value::from_bytes(tx_bytes)],
    );

    let tx_client = client
        .tx()
        .await
        .map_err(|e| ProviderError::Submission(format!("tx client: {e}")))?;
    let unsigned = tx_client
        .create_unsigned(&call)
        .map_err(|e| ProviderError::Submission(format!("create unsigned: {e}")))?;
    let progress = unsigned
        .submit_and_watch()
        .await
        .map_err(|e| ProviderError::Submission(format!("submit_and_watch: {e}")))?;

    Ok(PendingTx { progress })
}
