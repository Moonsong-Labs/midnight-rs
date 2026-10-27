//! Transaction submission over the node's WebSocket RPC.
//!
//! Lives on the provider because it is pure transport — rides the
//! provider's shared node connection, submits the proven tx bytes as an
//! unsigned `Midnight::send_mn_transaction` extrinsic, and returns a
//! [`PendingTx`] handle that drives the watch stream to inclusion /
//! finalization.

use std::sync::Arc;

use crate::types::TransactionHash;

use midnight_types::{BlockHash, ExtrinsicHash, SpentInputs};
use midnight_wallet_facade::WalletFacade;
use sha2::{Digest, Sha256};
use subxt::tx::{TransactionInBlock, TransactionStatus};

use crate::provider::QUERY_TIMEOUT;
use crate::{HeldInputs, ProviderError};

type AtBlock = subxt::client::OnlineClientAtBlockImpl<subxt::SubstrateConfig>;
type Status = TransactionStatus<subxt::SubstrateConfig, AtBlock>;
type InBlock = TransactionInBlock<subxt::SubstrateConfig, AtBlock>;

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
    /// the node: fetching metadata, or encoding the unsigned extrinsic. A
    /// failed connect is [`ProviderError::Rpc`]. The transaction never left this process, so resubmitting
    /// the same bytes is always safe.
    #[error("not submitted: {message}")]
    NotSubmitted { message: String },

    /// The `submit_and_watch` call failed, and the node can still hold the
    /// transaction.
    ///
    /// The error does not say whether the node received the transaction.
    /// subxt's `CombinedBackend` sends it through each of its RPC backends,
    /// one after the other, until one call succeeds. This variant
    /// carries the error of the last backend. So an earlier backend can have
    /// delivered the transaction, and a transport failure can lose the node's
    /// answer.
    ///
    /// Treat the fate as unknown. Before you build again from the same inputs,
    /// make sure that the chain does not hold the transaction. You can also
    /// wait until its TTL passes. A build with `with_ttl` (on the transfer
    /// and contract builders) makes that wait shorter than `global_ttl`.
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
    /// TTL expire) before rebuilding. A build with `with_ttl` (on the
    /// transfer and contract builders) makes that wait shorter than
    /// `global_ttl`.
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
    fn from_terminal_status<T: subxt::Config, C>(status: &TransactionStatus<T, C>) -> Option<Self> {
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
    pub block_hash: BlockHash,
    pub extrinsic_hash: ExtrinsicHash,
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
    /// apply". For a transaction that merges several fallible segments, the
    /// event does not tell which of them failed.
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
/// The SDK sets no deadline on either wait method, but subxt can end a wait.
/// On a node that offers the `chainHead_v1` RPC methods, as the Midnight node
/// does, subxt has a transaction timeout. The timeout starts at the submit,
/// and it is 240 s by default. When it passes, subxt ends the watch, and the
/// wait fails with [`SubmitError::WatchStream`]. subxt checks the timeout only
/// when a block arrives, so a chain that produces no blocks keeps the wait
/// open. Callers that need a deadline should wrap the wait in
/// [`tokio::time::timeout`]:
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
    progress: subxt::tx::TransactionProgress<subxt::SubstrateConfig, AtBlock>,
    reservation: Option<Reservation>,
    transaction_hash: TransactionHash,
}

/// What a build reserved, kept alive so it can be handed back if the
/// transaction turns out to be dead.
///
/// A node's rejection arrives as a terminal status while awaiting inclusion,
/// long after the builder returned, so the reservation has to outlive the
/// builder for anything to release it.
pub(crate) struct Reservation {
    wallet: Arc<dyn WalletFacade>,
    spent: SpentInputs,
}

impl Reservation {
    pub(crate) fn new(wallet: Arc<dyn WalletFacade>, spent: SpentInputs) -> Self {
        Self { wallet, spent }
    }

    async fn release(&self) {
        self.wallet.release(&self.spent).await;
    }
}

impl PendingTx {
    /// The hash of the submitted extrinsic.
    pub fn extrinsic_hash(&self) -> ExtrinsicHash {
        self.progress.extrinsic_hash()
    }

    /// The extrinsic hash formatted as a hex string (no `0x` prefix).
    pub fn extrinsic_hash_hex(&self) -> String {
        format!("{:x}", self.extrinsic_hash())
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
    pub async fn wait_best(self) -> Result<(TxInBlock, Self), ProviderError> {
        self.wait_for(TransactionStatus::as_in_block, "reaching best block")
            .await
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
    pub async fn wait_finalized(self) -> Result<(TxInBlock, Self), ProviderError> {
        self.wait_for(TransactionStatus::as_finalized, "finalization")
            .await
    }

    async fn wait_for(
        mut self,
        landed: fn(&Status) -> Option<&InBlock>,
        awaiting: &'static str,
    ) -> Result<(TxInBlock, Self), ProviderError> {
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
            if let Some(in_block) = landed(&status) {
                let tx = tx_in_block_with_verdict(in_block, self.transaction_hash).await?;
                return Ok((applied(tx)?, self));
            }
        }
        Err(SubmitError::stream_ended(awaiting).into())
    }
}

/// Fetch the extrinsic's events and derive the [`Verdict`] from the
/// `Midnight` pallet's `TxApplied` / `TxPartialSuccess` events. Default to
/// `Failure` when neither is present (the dispatch errored and only
/// `System::ExtrinsicFailed` was emitted).
async fn tx_in_block_with_verdict(
    in_block: &InBlock,
    transaction_hash: TransactionHash,
) -> Result<TxInBlock, ProviderError> {
    let block_hash = in_block.block_hash();
    let extrinsic_hash = in_block.extrinsic_hash();
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
/// A `PreparedTx` from [`MidnightProvider::prepare_reserved`] holds the
/// reservation of its build:
///
/// - A drop before [`submit`](Self::submit) hands the inputs back to the
///   wallet, under a Tokio runtime. [`release`](Self::release) does the same
///   and waits for it.
/// - A submit that succeeds moves the reservation to the [`PendingTx`], which
///   hands the inputs back when a wait reads [`SubmitError::Invalid`].
/// - A submit that fails, or a drop of the submit future, keeps the inputs
///   reserved, because the node can hold the transaction.
///
/// [`MidnightProvider::prepare_reserved`]: crate::MidnightProvider::prepare_reserved
pub struct PreparedTx {
    tx: subxt::tx::SubmittableTransaction<subxt::SubstrateConfig, AtBlock>,
    transaction_hash: TransactionHash,
    held: Option<HeldInputs>,
}

impl PreparedTx {
    /// Hold the reservation of the build of this transaction until the submit.
    pub(crate) fn holding(mut self, held: HeldInputs) -> Self {
        self.held = Some(held);
        self
    }

    /// The hash subxt will report for this extrinsic once submitted,
    /// computed here from the encoded transaction without contacting the
    /// node. Identical to the eventual [`PendingTx::extrinsic_hash`].
    pub fn extrinsic_hash(&self) -> ExtrinsicHash {
        self.tx.hash()
    }

    /// The hash of the Midnight transaction these bytes carry, known before
    /// submission. See [`PendingTx::transaction_hash`].
    pub fn transaction_hash(&self) -> TransactionHash {
        self.transaction_hash
    }

    /// Submit the prepared transaction and return a [`PendingTx`] that waits for
    /// inclusion and finalization.
    ///
    /// The returned [`PendingTx`] carries the reservation that this value
    /// holds. See the [type-level docs](PreparedTx).
    ///
    /// # Errors
    ///
    /// [`ProviderError::Submission`] with [`SubmitError::SubmitRpc`] when the
    /// submit call fails. The node can still hold the transaction, so the
    /// inputs stay reserved.
    pub async fn submit(self) -> Result<PendingTx, ProviderError> {
        let Self {
            tx,
            transaction_hash,
            held,
        } = self;
        // Disarm before the call: once it starts, the node can hold the
        // transaction, so a drop of this future must keep the inputs.
        let reservation = held.and_then(HeldInputs::into_reservation);
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

    /// Drop the transaction without a submit, and hand back the inputs that
    /// its build reserved. The inputs are back when this returns.
    ///
    /// Under a Tokio runtime the release runs as a task of its own, so a drop
    /// of this future does not stop it. A `PreparedTx` from
    /// [`MidnightProvider::prepare`](crate::MidnightProvider::prepare) holds no
    /// reservation, and this only drops it.
    pub async fn release(self) {
        if let Some(held) = self.held {
            held.release().await;
        }
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

    let tx_client = tokio::time::timeout(QUERY_TIMEOUT, client.tx())
        .await
        .map_err(|_| SubmitError::NotSubmitted {
            message: format!("tx client: the node gave no answer within {QUERY_TIMEOUT:?}"),
        })?
        .map_err(|e| SubmitError::NotSubmitted {
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
        held: None,
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

/// Submit proven transaction bytes to a Midnight node and return a handle
/// for awaiting inclusion / finalization.
pub(crate) async fn submit_bytes(
    client: &subxt::OnlineClient<subxt::SubstrateConfig>,
    tx_bytes: &[u8],
) -> Result<PendingTx, ProviderError> {
    prepare_bytes(client, tx_bytes).await?.submit().await
}

#[cfg(test)]
mod tests {

    fn every_variant() -> [SubmitError; 7] {
        let m = || "x".to_string();
        [
            SubmitError::NotSubmitted { message: m() },
            SubmitError::SubmitRpc { message: m() },
            SubmitError::Invalid {
                message: m(),
                code: None,
            },
            SubmitError::Dropped { message: m() },
            SubmitError::NodeError { message: m() },
            SubmitError::WatchStream { message: m() },
            SubmitError::VerdictFetch { message: m() },
        ]
    }

    /// The wait site is where a node's rejection arrives, and it is the only
    /// place a reservation can be freed on one. Freeing on a status that still
    /// permits inclusion would let a later build spend the same coins.
    #[test]
    fn only_a_definitive_rejection_frees_inputs_at_the_wait_site() {
        for e in every_variant() {
            let expected = matches!(e, SubmitError::Invalid { .. });
            assert_eq!(rejection_is_definitive(&e), expected, "{e:?}");
        }
    }

    use super::*;
    use subxt::SubstrateConfig;

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
            block_hash: BlockHash::repeat_byte(1),
            extrinsic_hash: ExtrinsicHash::repeat_byte(2),
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
}
