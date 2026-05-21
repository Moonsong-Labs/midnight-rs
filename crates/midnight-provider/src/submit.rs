//! Transaction submission over the node's WebSocket RPC.
//!
//! Lives on the provider because it's pure transport — connects to
//! [`MidnightProvider::node_url`], submits the proven tx bytes as an
//! unsigned `Midnight::send_mn_transaction` extrinsic, and returns a
//! [`PendingTx`] handle that drives the watch stream to inclusion /
//! finalization.

use crate::ProviderError;

/// Inclusion details for a transaction that landed in a block.
#[derive(Debug, Clone, Copy)]
pub struct TxInBlock {
    pub block_hash: [u8; 32],
    pub extrinsic_hash: [u8; 32],
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
    pub async fn wait_best(mut self) -> Result<(TxInBlock, Self), ProviderError> {
        use subxt::tx::TransactionStatus;
        while let Some(status) = self.progress.next().await {
            match status.map_err(|e| ProviderError::Submission(format!("watch: {e}")))? {
                TransactionStatus::InBestBlock(in_block) => {
                    let tx = TxInBlock {
                        block_hash: in_block.block_hash().0,
                        extrinsic_hash: in_block.extrinsic_hash().0,
                    };
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
    pub async fn wait_finalized(mut self) -> Result<(TxInBlock, Self), ProviderError> {
        use subxt::tx::TransactionStatus;
        while let Some(status) = self.progress.next().await {
            match status.map_err(|e| ProviderError::Submission(format!("watch: {e}")))? {
                TransactionStatus::InFinalizedBlock(in_block) => {
                    let tx = TxInBlock {
                        block_hash: in_block.block_hash().0,
                        extrinsic_hash: in_block.extrinsic_hash().0,
                    };
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
