//! Transaction submission over the node's WebSocket RPC.
//!
//! Lives on the provider because it is pure transport — rides the
//! provider's shared node connection, submits the proven tx bytes as an
//! unsigned `Midnight::send_mn_transaction` extrinsic, and returns a
//! [`PendingTx`] handle that drives the watch stream to inclusion /
//! finalization.

use std::sync::Arc;

use crate::types::TransactionHash;

use midnight_types::SpentInputs;
use midnight_wallet_facade::WalletFacade;
use sha2::{Digest, Sha256};

use crate::{HeldInputs, ProviderError};

/// Whether a terminal status proves the transaction will never be included, so
/// the inputs it reserved can be handed back.
///
/// Only [`SubmitError::Invalid`] does. Dropped and node-error statuses leave a
/// transaction that may still be gossiped and included, so releasing on those
/// would let a later build spend the same coins, and the loser is rejected on
/// chain. Being wrong the other way costs only the TTL window.
pub(crate) fn rejection_is_definitive(err: &SubmitError) -> bool {
    matches!(err, SubmitError::Invalid { .. })
}

/// Whether a failed submission proves the transaction never reached the node,
/// so the inputs it reserved are safe to hand back immediately.
///
/// Only [`SubmitError::NotSubmitted`] says that: it failed before the RPC call.
/// [`SubmitError::SubmitRpc`] is ambiguous, since a transport failure mid-call
/// may still have delivered the transaction, and every other variant is
/// reported by a wait rather than by submission.
pub(crate) fn never_reached_the_node(err: &ProviderError) -> bool {
    matches!(
        err,
        ProviderError::Submission(SubmitError::NotSubmitted { .. })
    )
}

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
    /// via the chain before resubmitting.
    #[error("submit RPC: {message}")]
    SubmitRpc { message: String },

    /// The node reported the transaction as invalid (bad nonce, signature,
    /// failed guaranteed phase, ...). This is a **definitive rejection**:
    /// the transaction will not be included. It is safe to rebuild and
    /// resubmit with fresh inputs.
    ///
    /// `code` is the node's `InvalidTransaction::Custom` byte when the
    /// message carries one. Match on it rather than on the message text; see
    /// [`crate::node_error`] for what a code is called.
    #[error("invalid: {}{message}", match code.and_then(crate::node_error::name_of) {
        Some(name) => format!("{name}: "),
        None => String::new(),
    })]
    Invalid { message: String, code: Option<u8> },

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
    /// Re-query the chain instead of resubmitting.
    #[error("watch stream: {message}")]
    WatchStream { message: String },

    /// The transaction landed in a block, but fetching or decoding the
    /// extrinsic's events failed, so the chain's [`Verdict`] could not be
    /// derived. The transaction is on chain (provisionally, for a
    /// best-block wait); do **not** resubmit. Re-query the chain for the
    /// extrinsic's events to learn whether it applied.
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
                code: crate::node_error::code_in(message),
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
    /// The Midnight transaction the extrinsic carried, the ledger's own
    /// identity for it. The chain names this hash in its `TxApplied` /
    /// `TxPartialSuccess` events, and the indexer keys transactions by it.
    pub transaction_hash: TransactionHash,
    pub verdict: Verdict,
}

/// Return `in_block` when the chain applied the transaction, and
/// [`NotApplied`] when it did not. Only [`Verdict::Success`] counts as applied:
/// a partial success paid the fee, but a fallible segment did not apply.
fn applied(in_block: TxInBlock) -> Result<TxInBlock, NotApplied> {
    match in_block.verdict {
        Verdict::Success => Ok(in_block),
        Verdict::PartialSuccess | Verdict::Failure => Err(NotApplied(in_block)),
    }
}

/// A transaction that landed in a block, but that the chain did not apply.
///
/// [`PendingTx::wait_best`] and [`PendingTx::wait_finalized`] return it, inside
/// [`ProviderError::NotApplied`], for a [`Verdict::PartialSuccess`] or a
/// [`Verdict::Failure`]. The wrapped [`TxInBlock`] names the transaction, the
/// block and the verdict, so a caller who handles a partial success reads the
/// verdict here. A `NotApplied` from `wait_best` is provisional: a reorg can
/// change the verdict before finality.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error(
    "transaction {} landed in block {} but the chain did not apply it ({})",
    .0.transaction_hash,
    hex::encode(.0.block_hash),
    .0.verdict
)]
pub struct NotApplied(pub TxInBlock);

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

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Success => "success",
            Self::PartialSuccess => "partial success",
            Self::Failure => "failure",
        })
    }
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
/// Both waits return `Ok` only when the chain applied the transaction, so the
/// returned [`TxInBlock`] has the verdict [`Verdict::Success`].
///
/// # Errors
///
/// Both wait methods fail with [`ProviderError::NotApplied`] when the
/// transaction landed but the chain did not apply it. Its [`NotApplied`] holds
/// the [`TxInBlock`] with the verdict.
///
/// Every other failure is [`ProviderError::Submission`] carrying a
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
/// ```rust,no_run
/// # async fn f(pending: midnight_provider::PendingTx) -> anyhow::Result<()> {
/// use std::time::Duration;
///
/// let (best, pending) = tokio::time::timeout(
///     Duration::from_secs(60),
///     pending.wait_best(),
/// ).await??;
/// # Ok(())
/// # }
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
    reservation: Option<Reservation>,
    transaction_hash: TransactionHash,
}

/// What a build reserved, kept alive so it can be handed back if the
/// transaction turns out to be dead.
///
/// A node's rejection arrives as a terminal status while awaiting inclusion,
/// long after the builder returned, so the reservation has to outlive the
/// builder for anything to release it.
///
/// One entry per reservation the build made. A release matches on each
/// entry's own `reserved_at`, so the entries stay apart.
pub(crate) struct Reservation {
    wallet: Arc<dyn WalletFacade>,
    spent: Vec<SpentInputs>,
}

impl Reservation {
    /// Take over what `held` guards, so that the guards no longer release.
    ///
    /// `None` when no guard has a wallet to hand the inputs back to. Every
    /// guard of one [`PreparedTx`] holds the wallet of the provider that
    /// prepared it.
    fn take_over(held: Vec<HeldInputs>) -> Option<Self> {
        let mut wallet = None;
        let spent = held
            .into_iter()
            .filter_map(HeldInputs::disarm)
            .map(|(held_by, spent)| {
                wallet = Some(held_by);
                spent
            })
            .collect();
        Some(Self {
            wallet: wallet?,
            spent,
        })
    }

    /// Hand back every entry. A caller that drops this future midway leaves
    /// the entry in flight to its own task, and the entries after it to the
    /// `Drop` of [`HeldInputs`].
    async fn release(self) {
        let Self { wallet, spent } = self;
        let held: Vec<HeldInputs> = spent
            .into_iter()
            .map(|spent| HeldInputs::of(spent, Some(Arc::clone(&wallet))))
            .collect();
        for guard in held {
            guard.release().await;
        }
    }
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

    /// The hash of the Midnight transaction the extrinsic carries.
    ///
    /// Substrate identifies the extrinsic; the Midnight ledger identifies the
    /// transaction inside it. The latter is what an explorer shows, what the
    /// chain's own `TxApplied` event names, and what an indexer query keyed by
    /// transaction hash takes. `to_string()` gives the form those expect.
    pub fn transaction_hash(&self) -> TransactionHash {
        self.transaction_hash
    }

    /// Drive the watch stream until the transaction lands in the best block,
    /// and return the inclusion when the chain applied it there.
    ///
    /// The verdict of this wait is provisional. A reorg can drop the block,
    /// and the transaction can then land again with another verdict, which
    /// this wait does not follow. [`Self::wait_finalized`] gives the final
    /// verdict. An `Err` consumes the handle. When the final verdict matters,
    /// call [`Self::wait_finalized`] in place of `wait_best`.
    ///
    /// # Errors
    ///
    /// [`ProviderError::NotApplied`] when the verdict in the best block is not
    /// [`Verdict::Success`]. See the [type-level docs](PendingTx#errors) for
    /// the [`SubmitError`] kinds of the other failures and what each implies
    /// about retrying.
    pub async fn wait_best(mut self) -> Result<(TxInBlock, Self), ProviderError> {
        use subxt::tx::TransactionStatus;
        while let Some(status) = self.progress.next().await {
            let status = status.map_err(SubmitError::watch)?;
            if let Some(err) = SubmitError::from_terminal_status(&status) {
                // Taken by value: holding a borrow of `self` across the await
                // would make this future require `PendingTx: Sync`, which the
                // watch stream is not.
                if rejection_is_definitive(&err)
                    && let Some(reservation) = self.reservation.take()
                {
                    reservation.release().await;
                }
                return Err(err.into());
            }
            if let TransactionStatus::InBestBlock(in_block) = status {
                let tx = tx_in_block_with_verdict(&in_block, self.transaction_hash).await?;
                return Ok((applied(tx)?, self));
            }
        }
        Err(SubmitError::stream_ended("reaching best block").into())
    }

    /// Drive the watch stream until the transaction is in a finalized block,
    /// and return the inclusion when the chain applied it there.
    ///
    /// Past finality the block can't be reorged out under honest-majority
    /// assumptions, so the verdict of this wait is final.
    ///
    /// # Errors
    ///
    /// [`ProviderError::NotApplied`] when the final verdict is not
    /// [`Verdict::Success`]. See the [type-level docs](PendingTx#errors) for
    /// the [`SubmitError`] kinds of the other failures and what each implies
    /// about retrying.
    pub async fn wait_finalized(mut self) -> Result<(TxInBlock, Self), ProviderError> {
        use subxt::tx::TransactionStatus;
        while let Some(status) = self.progress.next().await {
            let status = status.map_err(SubmitError::watch)?;
            if let Some(err) = SubmitError::from_terminal_status(&status) {
                // Taken by value: holding a borrow of `self` across the await
                // would make this future require `PendingTx: Sync`, which the
                // watch stream is not.
                if rejection_is_definitive(&err)
                    && let Some(reservation) = self.reservation.take()
                {
                    reservation.release().await;
                }
                return Err(err.into());
            }
            if let TransactionStatus::InFinalizedBlock(in_block) = status {
                let tx = tx_in_block_with_verdict(&in_block, self.transaction_hash).await?;
                return Ok((applied(tx)?, self));
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
    transaction_hash: TransactionHash,
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
        transaction_hash,
        verdict,
    })
}

/// A transaction wrapped in its unsigned extrinsic but **not yet submitted**.
/// The extrinsic comes from the node's metadata, which checks only the call's
/// shape: the node validates the transaction only at
/// [`submit`](Self::submit). Its [`extrinsic_hash`](Self::extrinsic_hash) is
/// already known, so a caller can durably record state keyed by that hash
/// (e.g. a private-state journal entry) *before* the transaction hits the
/// mempool, then [`submit`](Self::submit) it. This closes the window where
/// a crash between submit and record would leave a transaction on the wire
/// with no local handle to reconcile it.
///
/// A `PreparedTx` from [`MidnightProvider::prepare_reserved`] guards the
/// inputs its build reserved. Dropped before [`submit`](Self::submit), it hands
/// them back, because the node never saw the transaction.
///
/// [`MidnightProvider::prepare_reserved`]: crate::MidnightProvider::prepare_reserved
pub struct PreparedTx {
    tx: subxt::tx::SubmittableTransaction<
        subxt::SubstrateConfig,
        subxt::client::OnlineClientAtBlockImpl<subxt::SubstrateConfig>,
    >,
    transaction_hash: TransactionHash,
    held: Vec<HeldInputs>,
}

impl PreparedTx {
    /// The hash subxt will report for this extrinsic once submitted,
    /// computed here from the encoded transaction without contacting the
    /// node. Identical to the eventual [`PendingTx::extrinsic_hash`].
    pub fn extrinsic_hash(&self) -> [u8; 32] {
        self.tx.hash().0
    }

    /// The hash of the Midnight transaction these bytes carry, known before
    /// submission. See [`PendingTx::transaction_hash`].
    pub fn transaction_hash(&self) -> TransactionHash {
        self.transaction_hash
    }

    /// Submit the prepared transaction and return a [`PendingTx`] for
    /// awaiting inclusion / finalization. On failure the transaction never
    /// reached the node (or its fate is ambiguous per [`SubmitError`]).
    ///
    /// The inputs this guards move to the returned [`PendingTx`], which hands
    /// them back on a definitive rejection. A failed call keeps them reserved,
    /// because the node may have received the transaction. They stay reserved
    /// until a sync sees the transaction land, or until their TTL elapses.
    pub async fn submit(self) -> Result<PendingTx, ProviderError> {
        let Self {
            tx,
            transaction_hash,
            held,
        } = self;
        // Disarm before the call: a caller that drops this future mid-call
        // leaves the same ambiguity as a failed call.
        let reservation = Reservation::take_over(held);
        let progress = tx
            .submit_and_watch()
            .await
            .map_err(|e| SubmitError::SubmitRpc {
                message: e.to_string(),
            })?;
        Ok(PendingTx {
            progress,
            reservation,
            transaction_hash,
        })
    }

    /// Guard `held` until this transaction is submitted.
    pub(crate) fn holding(mut self, held: Vec<HeldInputs>) -> Self {
        self.held = held;
        self
    }
}

/// Wrap proven transaction bytes in the unsigned `send_mn_transaction`
/// extrinsic, built locally from the node's metadata, and compute its hashes.
/// It does not submit, and the node does not see the transaction. The
/// returned [`PreparedTx`] exposes the hashes and a `submit` step.
pub(crate) async fn prepare_bytes(
    client: &subxt::OnlineClient<subxt::SubstrateConfig>,
    tx_bytes: &[u8],
) -> Result<PreparedTx, ProviderError> {
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
    Ok(PreparedTx {
        tx,
        transaction_hash: midnight_transaction_hash(tx_bytes),
        held: Vec::new(),
    })
}

/// The Midnight transaction hash of already-serialized transaction bytes.
///
/// The ledger defines it as SHA-256 over the transaction's tagged
/// serialization, and `tx_bytes` is exactly that serialization, so hashing the
/// bytes reproduces the value the chain and the indexer report.
fn midnight_transaction_hash(tx_bytes: &[u8]) -> TransactionHash {
    <[u8; 32]>::from(Sha256::digest(tx_bytes)).into()
}

#[cfg(test)]
mod tests {

    fn err(e: SubmitError) -> ProviderError {
        ProviderError::Submission(e)
    }

    /// The wait site is where a node's rejection arrives, and it is the only
    /// place a reservation can be freed on one. Freeing on a status that still
    /// permits inclusion would let a later build spend the same coins.
    #[test]
    fn only_a_definitive_rejection_frees_inputs_at_the_wait_site() {
        let m = || "x".to_string();

        assert!(rejection_is_definitive(&SubmitError::Invalid {
            message: m(),
            code: None
        }));

        for still_possible in [
            SubmitError::Dropped { message: m() },
            SubmitError::NodeError { message: m() },
            SubmitError::WatchStream { message: m() },
            SubmitError::VerdictFetch { message: m() },
            SubmitError::SubmitRpc { message: m() },
            SubmitError::NotSubmitted { message: m() },
        ] {
            assert!(
                !rejection_is_definitive(&still_possible),
                "only Invalid rules out inclusion"
            );
        }
    }

    /// Releasing a transaction that can still land lets a later build spend
    /// the same inputs, so the submit site frees them only when the
    /// transaction provably never left this process. `SubmitRpc` is the
    /// interesting one: a transport failure mid-call may still have delivered
    /// it.
    #[test]
    fn only_a_failed_submission_frees_inputs_at_the_submit_site() {
        let m = || "x".to_string();

        assert!(never_reached_the_node(&err(SubmitError::NotSubmitted {
            message: m()
        })));

        for ambiguous in [
            SubmitError::SubmitRpc { message: m() },
            SubmitError::Invalid {
                message: m(),
                code: None,
            },
            SubmitError::Dropped { message: m() },
            SubmitError::NodeError { message: m() },
            SubmitError::WatchStream { message: m() },
            SubmitError::VerdictFetch { message: m() },
        ] {
            assert!(
                !never_reached_the_node(&err(ambiguous)),
                "only NotSubmitted proves the transaction never left"
            );
        }

        assert!(!never_reached_the_node(&ProviderError::NoWallet));
    }

    use super::*;
    use midnight_types::chain_pin::ChainView;
    use midnight_types::{
        ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, HashOutput, LedgerVersion,
        Network, Nullifier, SpendableShieldedCoin, SyncCursors, Timestamp, TrackedUtxo,
        WalletBalance, WalletError, WalletSeed,
    };
    use subxt::SubstrateConfig;
    use subxt::tx::TransactionStatus;
    use tokio::sync::Semaphore;

    /// The client type parameter is irrelevant for the terminal-status
    /// variants, which only carry a message.
    type Status = TransactionStatus<SubstrateConfig, ()>;

    #[test]
    fn invalid_status_maps_to_invalid() {
        for (message, code) in [
            ("bad nonce", None),
            ("Invalid transaction with custom error: 168", Some(168)),
        ] {
            let status = Status::Invalid {
                message: message.into(),
            };
            assert_eq!(
                SubmitError::from_terminal_status(&status),
                Some(SubmitError::Invalid {
                    message: message.into(),
                    code
                })
            );
        }
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
    fn a_rejection_names_the_code_it_carries() {
        let named = SubmitError::Invalid {
            message: "Invalid transaction with custom error: 168".into(),
            code: Some(168),
        };
        assert_eq!(
            named.to_string(),
            "invalid: MalformedError::FeeCalculation: Invalid transaction with custom error: 168"
        );

        // A refusal that never reached the ledger's mapping reads as before.
        let bare = SubmitError::Invalid {
            message: "bad signature".into(),
            code: None,
        };
        assert_eq!(bare.to_string(), "invalid: bad signature");
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

    fn in_block(verdict: Verdict) -> TxInBlock {
        TxInBlock {
            block_hash: [1; 32],
            extrinsic_hash: [2; 32],
            transaction_hash: [3; 32].into(),
            verdict,
        }
    }

    /// A partial success paid its fee and committed its guaranteed phase, so it
    /// is easy to count as applied. The error names the transaction hash, the
    /// key an indexer query takes, not the extrinsic hash, which no indexer
    /// stores.
    #[test]
    fn only_success_counts_as_applied() {
        assert!(applied(in_block(Verdict::Success)).is_ok());

        for verdict in [Verdict::PartialSuccess, Verdict::Failure] {
            let tx = in_block(verdict);
            let err = applied(tx).expect_err("only Success counts as applied");
            let message = err.to_string();
            assert!(
                message.contains(&tx.transaction_hash.to_string()),
                "the error should name the transaction hash, got: {message}"
            );
        }
    }

    /// A wallet that records each release as started, takes a permit, and
    /// records it as finished. Its state changes before its last await, and a
    /// test chooses which release is still in flight.
    struct RecordingWallet {
        started: std::sync::Mutex<Vec<SpentInputs>>,
        finished: std::sync::Mutex<Vec<SpentInputs>>,
        permits: Semaphore,
    }

    impl RecordingWallet {
        fn with_permits(permits: usize) -> Arc<Self> {
            Arc::new(Self {
                started: std::sync::Mutex::default(),
                finished: std::sync::Mutex::default(),
                permits: Semaphore::new(permits),
            })
        }

        fn started(&self) -> Vec<(Timestamp, Vec<Nullifier>)> {
            entries_of(&self.started.lock().expect("not poisoned"))
        }

        fn finished(&self) -> Vec<(Timestamp, Vec<Nullifier>)> {
            entries_of(&self.finished.lock().expect("not poisoned"))
        }
    }

    #[async_trait::async_trait]
    impl WalletFacade for RecordingWallet {
        async fn network(&self) -> Network {
            unimplemented!("only releases")
        }

        async fn ledger_version(&self) -> LedgerVersion {
            unimplemented!("only releases")
        }

        async fn seed(&self) -> WalletSeed {
            unimplemented!("only releases")
        }

        async fn shielded_public_keys(&self) -> (CoinPublicKey, EncryptionPublicKey) {
            unimplemented!("only releases")
        }

        async fn balance(&self) -> WalletBalance {
            unimplemented!("only releases")
        }

        async fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
            unimplemented!("only releases")
        }

        async fn unshielded_utxos(&self) -> Vec<TrackedUtxo> {
            unimplemented!("only releases")
        }

        async fn parameters(&self) -> ChainParameters {
            unimplemented!("only releases")
        }

        async fn sync_cursors(&self) -> SyncCursors {
            unimplemented!("only releases")
        }

        async fn dust_synced(&self) -> bool {
            unimplemented!("only releases")
        }

        async fn release(&self, spent: &SpentInputs) {
            self.started
                .lock()
                .expect("not poisoned")
                .push(spent.clone());
            self.permits
                .acquire()
                .await
                .expect("the semaphore stays open")
                .forget();
            self.finished
                .lock()
                .expect("not poisoned")
                .push(spent.clone());
        }

        async fn resync(&self, _chain: &dyn ChainView) -> Result<(), WalletError> {
            unimplemented!("only releases")
        }

        async fn rescan_shielded(&self) -> Result<(), WalletError> {
            unimplemented!("only releases")
        }

        async fn watch_for_coins(&self, _coins: Vec<CoinInfo>) -> Result<(), WalletError> {
            unimplemented!("only releases")
        }

        async fn forget_coins(&self, _coins: Vec<CoinInfo>) -> Result<(), WalletError> {
            unimplemented!("only releases")
        }
    }

    /// One reservation entry: a shielded coin, stamped at `secs`.
    fn entry(coin: u8, secs: u64) -> SpentInputs {
        SpentInputs::from_shielded(
            vec![Nullifier(HashOutput([coin; 32]))],
            Timestamp::from_secs(secs),
        )
    }

    /// What a release matches on, in an order that does not depend on the
    /// order of the releases.
    fn entries_of(spent: &[SpentInputs]) -> Vec<(Timestamp, Vec<Nullifier>)> {
        let mut entries: Vec<_> = spent
            .iter()
            .map(|s| (s.reserved_at, s.shielded.clone()))
            .collect();
        entries.sort();
        entries
    }

    fn reservation_of(wallet: &Arc<RecordingWallet>, entries: &[SpentInputs]) -> Reservation {
        let wallet: Arc<dyn WalletFacade> = wallet.clone();
        let held = entries
            .iter()
            .map(|spent| HeldInputs::of(spent.clone(), Some(wallet.clone())))
            .collect();
        Reservation::take_over(held).expect("every guard holds the wallet")
    }

    /// Let the spawned tasks run on this single-threaded runtime.
    async fn settle() {
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
    }

    /// A release matches each entry on its own `reserved_at`, so one merged
    /// entry, or one stamp for all, hands back nothing for the rest. Until a
    /// rejection, the submitted transaction can still land, so nothing may go
    /// back before the release.
    #[tokio::test]
    async fn a_reservation_hands_back_each_entry_on_release_only() {
        let wallet = RecordingWallet::with_permits(Semaphore::MAX_PERMITS);
        let entries = [entry(1, 10), entry(2, 20)];

        let reservation = reservation_of(&wallet, &entries);
        settle().await;
        assert!(
            wallet.started().is_empty(),
            "taking over the guards must disarm them"
        );

        reservation.release().await;
        assert_eq!(wallet.finished(), entries_of(&entries));
    }

    /// A deadline around the wait can drop the release while the release of
    /// an entry is in flight, in a wallet that changes its state before its
    /// last await. Every entry must still go back, and only once: a second
    /// release can drop the entry of a later build stamped the same.
    #[tokio::test]
    async fn a_release_cut_short_hands_back_each_entry_once() {
        // No permit, so the release of the first entry records it and waits.
        let wallet = RecordingWallet::with_permits(0);
        let entries = [entry(1, 10), entry(2, 20), entry(3, 30)];

        let mut release = Box::pin(reservation_of(&wallet, &entries).release());
        std::future::poll_fn(|cx| {
            assert!(
                release.as_mut().poll(cx).is_pending(),
                "the release of the first entry is in flight"
            );
            std::task::Poll::Ready(())
        })
        .await;
        settle().await;
        assert_eq!(wallet.started(), entries_of(&entries[..1]));
        drop(release);

        // Room for a release of every entry twice, so a repeat shows.
        wallet.permits.add_permits(2 * entries.len());
        settle().await;
        assert_eq!(
            wallet.started(),
            entries_of(&entries),
            "each entry starts one release"
        );
        assert_eq!(
            wallet.finished(),
            entries_of(&entries),
            "each release runs to its end, also the one in flight at the cut"
        );
    }
}
