//! Transaction submission over the node's WebSocket RPC.
//!
//! Lives on the provider because it is pure transport — rides the
//! provider's shared node connection, submits the proven tx bytes as an
//! unsigned `Midnight::send_mn_transaction` extrinsic, and returns a
//! [`PendingTx`] handle that drives the watch stream to finality.

use crate::types::TransactionHash;

use midnight_types::build::{KeptReservation, Reservation};
use midnight_types::{BlockHash, ExtrinsicHash};
use sha2::{Digest, Sha256};
use subxt::tx::{TransactionInBlock, TransactionStatus};

use crate::ProviderError;
use crate::provider::QUERY_TIMEOUT;

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
    /// wait until its TTL passes. A request made
    /// [`with_ttl`](midnight_types::TransactionRequest::with_ttl) makes that
    /// wait shorter than `global_ttl`.
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
    /// TTL expire) before rebuilding. A request made
    /// [`with_ttl`](midnight_types::TransactionRequest::with_ttl) makes that
    /// wait shorter than `global_ttl`.
    #[error("dropped: {message}")]
    Dropped { message: String },

    /// The node hit an internal error while tracking the transaction. Its
    /// fate is unknown — treat like [`Dropped`](SubmitError::Dropped):
    /// the transaction may still be included, so resubmitting the same
    /// inputs risks a double spend.
    #[error("node error: {message}")]
    NodeError { message: String },

    /// The watch subscription failed or ended before the transaction
    /// reached the awaited status (a transport/stream issue). Says nothing
    /// about the
    /// transaction itself: it stays in the node's pool and may still land.
    /// Re-query the chain instead of resubmitting.
    #[error("watch stream: {message}")]
    WatchStream { message: String },

    /// The transaction landed in a block, but fetching or decoding the
    /// extrinsic's events failed, so the chain's [`Verdict`] could not be
    /// derived. The transaction is on chain; do **not** resubmit. Re-query
    /// the chain for the extrinsic's events to learn whether it applied.
    #[error("verdict fetch: {message}")]
    VerdictFetch { message: String },
}

impl SubmitError {
    /// Map a terminal subxt watch status to the structured error it
    /// surfaces. Returns `None` for non-terminal statuses and for the two
    /// success terminals (`InBestBlock` / `InFinalizedBlock`), which the
    /// wait loop handles itself.
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

/// Where and how the chain included a transaction: its block, its hashes, and
/// the chain's verdict on whether it applied.
///
/// [`PendingTx::get_receipt`] returns it. `block_hash` and `extrinsic_hash`
/// come from subxt's `TransactionStatus::InFinalizedBlock`. `verdict` is the
/// SDK's interpretation of the Midnight pallet's outcome events: the
/// `Midnight` pallet always emits `TxApplied` (all segments applied) or
/// `TxPartialSuccess` (at least one fallible segment failed) for a
/// successful dispatch, and falls back to `System::ExtrinsicFailed` when the
/// dispatch errored entirely. See [`Verdict`].
#[derive(Debug, Clone, Copy)]
pub struct TransactionReceipt {
    pub block_hash: BlockHash,
    pub extrinsic_hash: ExtrinsicHash,
    /// The Midnight transaction the extrinsic carried, the ledger's own
    /// identity for it. The chain names this hash in its `TxApplied` /
    /// `TxPartialSuccess` events, and the indexer keys transactions by it.
    pub transaction_hash: TransactionHash,
    pub verdict: Verdict,
}

/// Return `receipt` when the chain applied the transaction, and
/// [`NotApplied`] when it did not. Only [`Verdict::Success`] counts as applied:
/// a partial success paid the fee, but a fallible segment did not apply.
fn applied(receipt: TransactionReceipt) -> Result<TransactionReceipt, NotApplied> {
    match receipt.verdict {
        Verdict::Success => Ok(receipt),
        Verdict::PartialSuccess | Verdict::Failure => Err(NotApplied(receipt)),
    }
}

/// A transaction that landed in a block, but that the chain did not apply.
///
/// [`PendingTx::get_receipt`] returns it, inside
/// [`ProviderError::NotApplied`], for a [`Verdict::PartialSuccess`] or a
/// [`Verdict::Failure`]. The wrapped [`TransactionReceipt`] names the
/// transaction, the block and the verdict, so a caller who handles a partial
/// success reads the verdict here.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error(
    "transaction {} landed in block {} but the chain did not apply it ({})",
    .0.transaction_hash,
    hex::encode(.0.block_hash),
    .0.verdict
)]
pub struct NotApplied(pub TransactionReceipt);

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

/// A sent transaction. [`get_receipt`](Self::get_receipt) waits for its
/// receipt.
///
/// [`MidnightProvider::send_transaction`],
/// [`MidnightProvider::send_tx_envelope`] and [`PreparedTx::send`] return it.
/// Read the hashes before the wait, because the wait consumes the handle.
///
/// [`MidnightProvider::send_transaction`]: crate::MidnightProvider::send_transaction
/// [`MidnightProvider::send_tx_envelope`]: crate::MidnightProvider::send_tx_envelope
///
/// # Errors
///
/// [`get_receipt`](Self::get_receipt) fails with
/// [`ProviderError::NotApplied`] when the transaction landed but the chain did
/// not apply it. Its [`NotApplied`] holds the [`TransactionReceipt`] with the
/// verdict.
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
/// The SDK sets no deadline on the wait, but subxt can end it. On a node that
/// offers the `chainHead_v1` RPC methods, as the Midnight node does, subxt has
/// a transaction timeout. The timeout starts at the submit, and it is 240 s by
/// default. When it passes, subxt ends the watch, and the wait fails with
/// [`SubmitError::WatchStream`]. subxt checks the timeout only when a block
/// arrives, so a chain that produces no blocks keeps the wait open. Callers
/// that need a deadline should wrap the wait in [`tokio::time::timeout`]:
///
/// ```rust,no_run
/// # async fn f(pending: midnight_provider::PendingTx) -> anyhow::Result<()> {
/// use std::time::Duration;
///
/// let receipt = tokio::time::timeout(Duration::from_secs(60), pending.get_receipt()).await??;
/// # Ok(())
/// # }
/// ```
///
/// Cancelling the wait future (drop, `tokio::select!`, timeout) is safe
/// and asynchronously closes the subxt subscription. It does **not**
/// retract the transaction from the mempool; the node keeps it queued
/// until it lands in a block or is dropped by the node itself.
///
/// A wait that reads [`SubmitError::Invalid`] hands the reserved inputs back
/// to the wallet before it returns (see [`PreparedTx`]). A drop of the handle,
/// or of the wait before it reads `Invalid`, leaves the inputs reserved until
/// a sync sees the chain spend them, or until their TTL passes.
#[must_use = "a sent transaction can still fail; call `get_receipt` for its verdict"]
pub struct PendingTx {
    progress: subxt::tx::TransactionProgress<subxt::SubstrateConfig, AtBlock>,
    reservation: KeptReservation,
    transaction_hash: TransactionHash,
}

impl std::fmt::Debug for PendingTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingTx")
            .field("extrinsic_hash", &self.extrinsic_hash())
            .field("transaction_hash", &self.transaction_hash)
            .field("reserved", &!self.reservation.spent().is_empty())
            .finish_non_exhaustive()
    }
}

impl PendingTx {
    /// The hash of the submitted extrinsic. Format it with `{:x}` for its hex
    /// digest.
    pub fn extrinsic_hash(&self) -> ExtrinsicHash {
        self.progress.extrinsic_hash()
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

    /// Drive the watch stream until the transaction is in a finalized block,
    /// and return its receipt when the chain applied it there.
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
    pub async fn get_receipt(self) -> Result<TransactionReceipt, ProviderError> {
        self.wait_for(TransactionStatus::as_finalized, "finalization")
            .await
    }

    /// Drive the watch stream until the transaction lands in the best block,
    /// and return its receipt when the chain applied it there.
    ///
    /// The verdict is provisional: a reorg can drop the block, and the
    /// transaction can land again with another verdict. midnight-contract
    /// reads a new contract at this block.
    #[doc(hidden)]
    pub async fn best_block_receipt(self) -> Result<TransactionReceipt, ProviderError> {
        self.wait_for(TransactionStatus::as_in_block, "reaching best block")
            .await
    }

    async fn wait_for(
        mut self,
        landed: fn(&Status) -> Option<&InBlock>,
        awaiting: &'static str,
    ) -> Result<TransactionReceipt, ProviderError> {
        while let Some(status) = self.progress.next().await {
            let status = status.map_err(SubmitError::watch)?;
            if let Some(err) = SubmitError::from_terminal_status(&status) {
                if rejection_is_definitive(&err) {
                    std::mem::take(&mut self.reservation).release();
                }
                return Err(err.into());
            }
            if let Some(in_block) = landed(&status) {
                let receipt = receipt_with_verdict(in_block, self.transaction_hash).await?;
                return Ok(applied(receipt)?);
            }
        }
        Err(SubmitError::stream_ended(awaiting).into())
    }
}

/// Fetch the extrinsic's events and derive the [`Verdict`] from the
/// `Midnight` pallet's `TxApplied` / `TxPartialSuccess` events. Default to
/// `Failure` when neither is present (the dispatch errored and only
/// `System::ExtrinsicFailed` was emitted).
async fn receipt_with_verdict(
    in_block: &InBlock,
    transaction_hash: TransactionHash,
) -> Result<TransactionReceipt, ProviderError> {
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
    Ok(TransactionReceipt {
        block_hash,
        extrinsic_hash,
        transaction_hash,
        verdict,
    })
}

/// A signed transaction in its unsigned extrinsic, not sent.
///
/// [`MidnightProvider::prepare`](crate::MidnightProvider::prepare) makes it.
/// The extrinsic comes from the node's metadata, which checks only the call's
/// shape: the node validates the transaction only at [`send`](Self::send).
/// Its [`extrinsic_hash`](Self::extrinsic_hash) is already known, so a caller
/// can durably record state keyed by that hash (e.g. a private-state journal
/// entry) *before* the transaction hits the mempool, then
/// [`send`](Self::send) it. This closes the window where a crash between the
/// send and the record would leave a transaction on the wire with no local
/// handle to reconcile it.
///
/// It holds the reservation that its wallet made:
///
/// - A drop before [`send`](Self::send) hands the inputs back to the wallet
///   at once.
/// - A send moves the reservation to the [`PendingTx`] before the call
///   starts. The `PendingTx` hands the inputs back when
///   [`PendingTx::get_receipt`] reads [`SubmitError::Invalid`].
/// - A send that fails, or a drop of the send future, keeps the inputs
///   reserved, because the node can hold the transaction.
#[must_use = "a prepared transaction does nothing until sent; a drop hands its inputs back"]
pub struct PreparedTx {
    tx: subxt::tx::SubmittableTransaction<subxt::SubstrateConfig, AtBlock>,
    transaction_hash: TransactionHash,
    reservation: Reservation,
}

impl std::fmt::Debug for PreparedTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedTx")
            .field("extrinsic_hash", &self.extrinsic_hash())
            .field("transaction_hash", &self.transaction_hash)
            .field("reservation", &self.reservation)
            .finish_non_exhaustive()
    }
}

impl PreparedTx {
    /// The hash subxt will report for this extrinsic once sent, computed here
    /// from the encoded transaction without contacting the node. Identical to
    /// the eventual [`PendingTx::extrinsic_hash`].
    pub fn extrinsic_hash(&self) -> ExtrinsicHash {
        self.tx.hash()
    }

    /// The hash of the Midnight transaction these bytes carry, known before
    /// the send. See [`PendingTx::transaction_hash`].
    pub fn transaction_hash(&self) -> TransactionHash {
        self.transaction_hash
    }

    /// Submit the transaction, and return the [`PendingTx`] that waits for
    /// its receipt.
    ///
    /// The reservation moves to the [`PendingTx`] before the RPC starts. See
    /// the [type-level docs](PreparedTx).
    ///
    /// # Errors
    ///
    /// [`ProviderError::Submission`] with [`SubmitError::SubmitRpc`] when the
    /// submit call fails. The node can still hold the transaction, so the
    /// inputs stay reserved.
    pub async fn send(self) -> Result<PendingTx, ProviderError> {
        let Self {
            tx,
            transaction_hash,
            reservation,
        } = self;
        // Before the call: once it starts, the node can hold the transaction,
        // so a drop of this future must keep the inputs.
        let reservation = reservation.keep_on_drop();
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
}

/// Wrap proven transaction bytes in the unsigned `send_mn_transaction`
/// extrinsic, built locally from the node's metadata, and compute its hashes.
/// It does not submit, and the node does not see the transaction. The
/// returned [`PreparedTx`] holds `reservation` until its send.
pub(crate) async fn prepare_bytes(
    client: &subxt::OnlineClient<subxt::SubstrateConfig>,
    tx_bytes: Vec<u8>,
    reservation: Reservation,
) -> Result<PreparedTx, ProviderError> {
    let transaction_hash = midnight_transaction_hash(&tx_bytes);
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
        transaction_hash,
        reservation,
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
            ("Invalid transaction with custom error: 231", Some(231)),
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
            message: "Invalid transaction with custom error: 231".into(),
            code: Some(231),
        };
        assert_eq!(
            named.to_string(),
            "invalid: MalformedError::FeeCalculation(OutsideTimeToDismiss): Invalid transaction \
             with custom error: 231"
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

    fn receipt(verdict: Verdict) -> TransactionReceipt {
        TransactionReceipt {
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
        assert!(applied(receipt(Verdict::Success)).is_ok());

        for verdict in [Verdict::PartialSuccess, Verdict::Failure] {
            let tx = receipt(verdict);
            let err = applied(tx).expect_err("only Success counts as applied");
            let message = err.to_string();
            assert!(
                message.contains(&tx.transaction_hash.to_string()),
                "the error should name the transaction hash, got: {message}"
            );
        }
    }
}
