//! The generation-free half of a replay: the indexer's subscription
//! messages, the reconnect policy that every replay loop and the latest-block
//! query follow, and the unshielded replay, whose events are JSON in every
//! ledger generation.

use midnight_indexer_client::{IndexerError, SubscriptionClient};
use midnight_types::TrackedUtxo;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{SpentUtxoKey, SyncProgress, WalletError};

// ---------------------------------------------------------------------------
// Subscription event types — internal to the sync loop.
//
// These shapes mirror the indexer's GraphQL subscription responses and exist
// to deserialize them. They are not part of the user-facing wallet API: sync
// is `MidnightProvider`'s job, and consumers see only its `SyncProgress`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LedgerEventMessage {
    pub id: i64,
    pub raw: String,
    pub max_id: i64,
}

/// Response envelope for the zswapLedgerEvents subscription.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ZswapEventEnvelope {
    pub zswap_ledger_events: LedgerEventMessage,
}

/// Response envelope for the dustLedgerEvents subscription.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DustEventEnvelope {
    pub dust_ledger_events: LedgerEventMessage,
}

/// Response type for unshielded transaction subscription events.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnshieldedTxEvent {
    pub unshielded_transactions: UnshieldedTxPayload,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "__typename")]
pub(crate) enum UnshieldedTxPayload {
    UnshieldedTransaction(UnshieldedTxData),
    UnshieldedTransactionsProgress(UnshieldedTxProgress),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnshieldedTxData {
    pub transaction: Option<UnshieldedTxRef>,
    #[serde(default)]
    pub created_utxos: Vec<SubscriptionUtxo>,
    #[serde(default)]
    pub spent_utxos: Vec<SubscriptionUtxo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnshieldedTxRef {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub block: Option<SubscriptionBlock>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SubscriptionBlock {
    pub height: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubscriptionUtxo {
    pub owner: String,
    pub token_type: String,
    pub value: String,
    #[serde(default)]
    pub intent_hash: Option<String>,
    #[serde(default)]
    pub output_index: Option<i64>,
    #[serde(default)]
    pub ctime: Option<i64>,
    #[serde(default)]
    pub registered_for_dust_generation: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnshieldedTxProgress {
    pub highest_transaction_id: i64,
}

/// Where a ledger-event replay asks its subscription to start.
///
/// The first attempt asks for the last event already applied, not the next
/// one. The indexer answers at once with that event and its `max_id`, which
/// says whether anything newer exists, and [`already_applied`] drops the
/// event itself. Asking for the next one leaves the stream silent at the tip,
/// and the only way to read that silence is to wait out the idle timeout. A
/// reconnect mid-replay has events waiting, so it resumes from the next one.
pub(crate) fn resume_id(applied_this_replay: u64, last_id: i64) -> i64 {
    if applied_this_replay > 0 {
        last_id + 1
    } else {
        last_id
    }
}

pub(crate) fn last_applied_before(start_id: i64) -> i64 {
    start_id.saturating_sub(1).max(0)
}

// ---------------------------------------------------------------------------
// Reconnect policy for the replay loops.
//
// The indexer client bounds transport liveness (connect/handshake timeout,
// keepalive ping, idle timeout — see `midnight_indexer_client::subscription`)
// and surfaces connection failures as retryable errors. The replay loops own
// the recovery: on a retryable failure they re-subscribe from the next
// unapplied event id with bounded exponential backoff. The retry counter
// resets only on applied progress — an applied event, or (in the unshielded
// loop) any progress update, which signals server liveness — so the bound applies
// to *consecutive failures without applied progress*, not the whole
// (potentially hours-long) initial sync. Deduped re-deliveries of
// already-applied events do not reset it: a non-compliant server that
// re-delivers one duplicate per reconnect and then drops cannot defeat
// the bound.
// ---------------------------------------------------------------------------

/// Maximum consecutive retryable failures before a replay loop gives up.
/// With the initial attempt this allows up to 5 connection attempts.
pub(crate) const RECONNECT_MAX_RETRIES: u32 = 4;

/// Base delay of the reconnect backoff; doubles per consecutive retry:
/// 250ms, 500ms, 1s, 2s.
const RECONNECT_BASE_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

/// Backoff delay before retry number `retry` (1-based).
pub(crate) fn reconnect_delay(retry: u32) -> std::time::Duration {
    RECONNECT_BASE_DELAY * 2u32.saturating_pow(retry.saturating_sub(1))
}

/// Whether an incoming event id was already applied and must be skipped.
///
/// Guards resumption: after a mid-replay reconnect (or when resuming from a
/// persisted cursor) the server may re-deliver events at or below our
/// cursor; re-applying them would corrupt state (double-counted UTXOs,
/// re-applied ledger events). `last_id` is only meaningful as an *applied*
/// cursor once we applied something this session (`applied_any`) or the
/// caller asked to start past the beginning (`start_id > 0`, where
/// `last_id` was initialized to `start_id - 1`). The remaining case —
/// fresh sync from id 0 — must not skip a genuine first event with id 0.
pub(crate) fn already_applied(msg_id: i64, last_id: i64, start_id: i64, applied_any: bool) -> bool {
    (applied_any || start_id > 0) && msg_id <= last_id
}

/// Per-connection event order check for the replay loops.
///
/// `conn_high` is the highest event id the *current* subscription
/// connection has delivered so far (`None` until its first event); the
/// loops reset it on every (re)connect. The indexer delivers events in
/// ascending id order within one subscription, so a fresh connection may
/// legally start at the cursor + 1 or re-deliver ids at or below the
/// cross-connection applied cursor (which [`already_applied`] then skips),
/// but once a connection has delivered an id, anything lower from the same
/// connection means the stream is corrupt or hostile, including an id at
/// or below the cursor arriving after the connection already advanced past
/// it. Forward gaps are not flagged: filtered streams (unshielded) have
/// inherent gaps, and a withholding indexer is undetectable here anyway
/// (see the crate-level trust model docs).
///
/// Returns the high-water id the message regressed below, for error
/// reporting.
pub(crate) fn order_regression(msg_id: i64, conn_high: Option<i64>) -> Option<i64> {
    conn_high.filter(|&high| msg_id < high)
}

/// The indexer's latest block.
///
/// The query retries on the replay loops' reconnect bound and backoff, so a
/// sync or a resync survives the same brief indexer outage as its event
/// streams.
pub(crate) async fn latest_block(
    indexer_url: &str,
) -> Result<midnight_indexer_client::Block, WalletError> {
    info!("fetching latest block from indexer");
    let indexer_client = midnight_indexer_client::IndexerClient::new(indexer_url)?;
    let mut retries = 0;
    loop {
        match indexer_client.get_block(None).await {
            Ok(block) => {
                return block
                    .ok_or_else(|| WalletError::Sync("no blocks available from indexer".into()));
            }
            Err(e) if e.is_retryable() && retries < RECONNECT_MAX_RETRIES => {
                retries += 1;
                warn!(retries, error = %with_causes(&e), "fetch latest block failed, retrying");
                tokio::time::sleep(reconnect_delay(retries)).await;
            }
            Err(e) => return Err(gave_up("latest block", e)),
        }
    }
}

/// Returns `error` as [`WalletError::Indexer`].
///
/// It first logs a warning that names `request` and carries the causes of
/// `error`.
pub(crate) fn gave_up(request: &'static str, error: IndexerError) -> WalletError {
    warn!(request, error = %with_causes(&error), "giving up on the indexer");
    WalletError::Indexer(error)
}

/// `error` with the causes its own message leaves out, such as the
/// "Connection refused" under a failed HTTP request.
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(c) = cause {
        let text = c.to_string();
        if !message.ends_with(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        cause = c.source();
    }
    message
}

pub(crate) async fn replay_unshielded_events(
    sub_client: &SubscriptionClient,
    address: &str,
    initial_utxos: Vec<TrackedUtxo>,
    start_tx_id: i64,
    resuming: bool,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<(Vec<TrackedUtxo>, i64, i64, Vec<SpentUtxoKey>), WalletError> {
    use midnight_indexer_client::subscription::queries::UNSHIELDED_TRANSACTIONS_SUBSCRIPTION;

    let mut utxos: Vec<TrackedUtxo> = initial_utxos;
    let mut last_height: i64 = 0;
    let mut last_seen_tx_id: i64 = last_applied_before(start_tx_id);
    // Keys of every spent UTXO observed during this replay, surfaced to the
    // caller so it can clear confirmed pending reservations.
    let mut spent_keys: Vec<SpentUtxoKey> = Vec::new();
    // The server merges two streams: transaction events and periodic progress
    // updates. The progress stream fires immediately (tokio interval), so the
    // first event is almost always a Progress before any transactions arrive.
    // We must wait until we've received all transactions up to the target
    // before returning. The target survives reconnects: it is a chain-side
    // high-water mark, not connection state.
    let mut target_tx_id: Option<i64> = None;
    let mut applied_txs: u64 = 0;
    let mut retries: u32 = 0;

    'reconnect: loop {
        // First attempt starts from the caller's cursor; reconnects resume
        // from the transaction after the last applied one.
        let resume_tx_id = if applied_txs > 0 {
            last_seen_tx_id + 1
        } else {
            start_tx_id
        };
        let variables = serde_json::json!({
            "address": address,
            "transactionId": resume_tx_id,
        });
        let mut subscription = match sub_client
            .subscribe::<UnshieldedTxEvent>(UNSHIELDED_TRANSACTIONS_SUBSCRIPTION, variables)
            .await
        {
            Ok(s) => s,
            Err(e) if e.is_retryable() && retries < RECONNECT_MAX_RETRIES => {
                retries += 1;
                warn!(retries, error = %e, "unshielded subscribe failed, retrying");
                tokio::time::sleep(reconnect_delay(retries)).await;
                continue 'reconnect;
            }
            Err(e) => return Err(gave_up("unshielded", e)),
        };
        // Highest tx id delivered on *this* connection; see
        // `order_regression`. Events without a transaction id cannot be
        // ordered and are exempt, like they are from dedupe.
        let mut conn_high: Option<i64> = None;

        loop {
            // Semantic timeout above the client's transport keepalive, which
            // errors a dead socket out on its own. Reaching this bound means
            // the socket is alive and the server has sent nothing.
            //
            // A resume rarely reaches it: the indexer answers the subscribe
            // with a progress frame carrying its highest transaction id, and
            // a cursor already at that id ends the replay there. Silence is a
            // server that skipped the frame, so read it as at tip on a resume
            // and keep the longer fatal bound for an initial sync, where
            // silence really is a stall.
            let event_timeout = if resuming {
                std::time::Duration::from_secs(10)
            } else {
                std::time::Duration::from_secs(30)
            };
            let event = tokio::time::timeout(event_timeout, subscription.next()).await;

            match event {
                Ok(Some(Ok(ev))) => {
                    match ev.unshielded_transactions {
                        UnshieldedTxPayload::UnshieldedTransaction(tx_data) => {
                            let created = tx_data.created_utxos.len();
                            let spent = tx_data.spent_utxos.len();
                            let tx_id = tx_data.transaction.as_ref().and_then(|t| t.id);
                            debug!(tx_id, created, spent, "unshielded tx event");
                            // Dedupe re-deliveries across resumption. Events
                            // without a transaction id cannot be deduped and
                            // are applied as-is.
                            if let Some(id) = tx_id {
                                if let Some(prev) = order_regression(id, conn_high) {
                                    return Err(WalletError::EventOrder {
                                        kind: "unshielded",
                                        id,
                                        prev,
                                    });
                                }
                                conn_high = Some(id);
                                if already_applied(
                                    id,
                                    last_seen_tx_id,
                                    start_tx_id,
                                    applied_txs > 0,
                                ) {
                                    debug!(
                                        tx_id = id,
                                        last_seen_tx_id, "skipping re-delivered unshielded tx"
                                    );
                                    continue;
                                }
                            }
                            // Attach the event's tx id so a malformed UTXO
                            // is identifiable from the error alone.
                            apply_unshielded_tx(&mut utxos, &tx_data).map_err(|e| match e {
                                WalletError::MalformedUtxo {
                                    field,
                                    value,
                                    reason,
                                    tx_id: None,
                                } => WalletError::MalformedUtxo {
                                    field,
                                    value,
                                    reason,
                                    tx_id,
                                },
                                other => other,
                            })?;
                            // Only an applied transaction counts as progress
                            // for the reconnect bound; deduped re-deliveries
                            // must not reset it.
                            retries = 0;
                            applied_txs += 1;
                            spent_keys.extend(spent_utxo_keys(&tx_data));
                            if let Some(id) = tx_id {
                                last_seen_tx_id = last_seen_tx_id.max(id);
                            }
                            if let Some(ref tx_ref) = tx_data.transaction
                                && let Some(ref block) = tx_ref.block
                            {
                                last_height = last_height.max(block.height);
                            }
                            if let Some(target) = target_tx_id
                                && last_seen_tx_id >= target
                            {
                                info!(
                                    last_seen_tx_id,
                                    utxos = utxos.len(),
                                    "unshielded sync caught up"
                                );
                                send_progress(
                                    &progress,
                                    SyncProgress::UnshieldedCaughtUp { utxos: utxos.len() },
                                );
                                return Ok((utxos, last_seen_tx_id, last_height, spent_keys));
                            }
                        }
                        UnshieldedTxPayload::UnshieldedTransactionsProgress(prog) => {
                            // Any progress update is genuine server liveness
                            // (even a re-send of an unchanged target), so it
                            // also resets the reconnect bound.
                            retries = 0;
                            let target = prog.highest_transaction_id;
                            debug!(target, last_seen_tx_id, "unshielded progress update");
                            if target == 0 || last_seen_tx_id >= target {
                                info!(
                                    target,
                                    last_seen_tx_id,
                                    utxos = utxos.len(),
                                    "unshielded sync caught up"
                                );
                                send_progress(
                                    &progress,
                                    SyncProgress::UnshieldedCaughtUp { utxos: utxos.len() },
                                );
                                return Ok((
                                    utxos,
                                    last_seen_tx_id.max(target),
                                    last_height,
                                    spent_keys,
                                ));
                            }
                            target_tx_id = Some(target);
                        }
                    }
                }
                Ok(Some(Err(e))) if e.is_retryable() && retries < RECONNECT_MAX_RETRIES => {
                    retries += 1;
                    warn!(retries, error = %e, "unshielded subscription dropped, reconnecting");
                    tokio::time::sleep(reconnect_delay(retries)).await;
                    continue 'reconnect;
                }
                Ok(Some(Err(e))) => return Err(gave_up("unshielded", e)),
                Ok(None) => {
                    // Mid-sync stream end: treat as a dropped connection and
                    // resume from the cursor.
                    if retries < RECONNECT_MAX_RETRIES {
                        retries += 1;
                        warn!(retries, "unshielded subscription ended early, reconnecting");
                        tokio::time::sleep(reconnect_delay(retries)).await;
                        continue 'reconnect;
                    }
                    return Err(WalletError::Sync(format!(
                        "unshielded subscription ended before sync completed \
                         (after {RECONNECT_MAX_RETRIES} reconnect attempts)"
                    )));
                }
                Err(_) => {
                    if resuming {
                        info!(last_seen_tx_id, "unshielded already at tip");
                        return Ok((utxos, last_seen_tx_id, last_height, spent_keys));
                    }
                    return Err(WalletError::Sync(
                        "timeout waiting for unshielded sync".into(),
                    ));
                }
            }
        }
    }
}

/// Composite key for matching unshielded UTXOs during spend removal.
type UtxoKey = (String, String, u128, Option<String>, Option<i64>);

fn utxo_key(u: &TrackedUtxo) -> UtxoKey {
    (
        u.owner.clone(),
        u.token_type.clone(),
        u.value,
        u.intent_hash.clone(),
        u.output_index,
    )
}

fn parse_utxo(u: &SubscriptionUtxo) -> Result<TrackedUtxo, WalletError> {
    // The closure's parameter type can't be inferred through the `?`
    // conversion, so it stays annotated.
    let value: u128 =
        u.value
            .parse()
            .map_err(|e: std::num::ParseIntError| WalletError::MalformedUtxo {
                field: "value",
                value: u.value.clone(),
                reason: e.to_string(),
                tx_id: None,
            })?;
    Ok(TrackedUtxo {
        owner: u.owner.clone(),
        token_type: u.token_type.clone(),
        value,
        intent_hash: u.intent_hash.clone(),
        output_index: u.output_index,
        ctime: u.ctime,
        registered_for_dust_generation: u.registered_for_dust_generation,
    })
}

/// Extract the `(intent_hash, output_index)` keys of every spent UTXO in an
/// unshielded transaction event. UTXOs missing either identity field (or
/// with an out-of-range index) can't match a reservation — reservations
/// always carry both — and are skipped. Used to clear matching
/// `PendingReservations` entries once the chain confirms the spends.
fn spent_utxo_keys(tx_data: &UnshieldedTxData) -> Vec<SpentUtxoKey> {
    tx_data
        .spent_utxos
        .iter()
        .filter_map(|u| {
            let intent_hash = u.intent_hash.clone()?;
            let output_index = u32::try_from(u.output_index?).ok()?;
            Some(SpentUtxoKey {
                intent_hash,
                output_index,
            })
        })
        .collect()
}

/// Apply one unshielded transaction event to the tracked UTXO set,
/// all-or-nothing: every spent and created UTXO is parsed upfront, and the
/// first malformed field rejects the whole event with a typed error before
/// any mutation. An event therefore either fully applies or leaves `utxos`
/// untouched, and since the replay loops propagate the error and the sync
/// paths only commit a fully successful replay (`sync_inner` builds the
/// wallet at the end; `ResyncPlan::run` only then yields a `ResyncCommit`),
/// a malformed event never leaves partial state behind.
fn apply_unshielded_tx(
    utxos: &mut Vec<TrackedUtxo>,
    tx_data: &UnshieldedTxData,
) -> Result<(), WalletError> {
    // Parse everything upfront. If any field fails to parse the UTXO vec is
    // left untouched so retries cannot produce duplicates.
    let spent: Vec<TrackedUtxo> = tx_data
        .spent_utxos
        .iter()
        .map(parse_utxo)
        .collect::<Result<_, _>>()?;
    let created: Vec<TrackedUtxo> = tx_data
        .created_utxos
        .iter()
        .map(parse_utxo)
        .collect::<Result<_, _>>()?;

    let mut to_remove: std::collections::HashMap<UtxoKey, usize> = std::collections::HashMap::new();
    for u in &spent {
        *to_remove.entry(utxo_key(u)).or_insert(0) += 1;
    }
    if !to_remove.is_empty() {
        utxos.retain(|u| match to_remove.get_mut(&utxo_key(u)) {
            Some(count) if *count > 0 => {
                *count -= 1;
                false
            }
            _ => true,
        });
    }
    utxos.extend(created);

    Ok(())
}

/// Forward a progress event to the optional progress channel.
///
/// Progress is lossy by design: on a **full** channel the message is dropped
/// (a slow consumer only needs a recent sample, not every tick) and the
/// return value is `true`. A **closed** channel — the receiver was dropped —
/// is different: nobody will ever consume progress again, which on the
/// streaming sync path means the consumer abandoned the sync. Returns
/// `false` so replay loops can stop early instead of feeding a dead channel;
/// the two `try_send` failure modes must never be conflated.
pub(crate) fn send_progress(tx: &Option<mpsc::Sender<SyncProgress>>, msg: SyncProgress) -> bool {
    let Some(tx) = tx else { return true };
    match tx.try_send(msg) {
        Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    }
}

/// The error a replay loop returns when [`send_progress`] reports a dropped
/// receiver mid-replay.
pub(crate) fn progress_cancelled(kind: &str) -> WalletError {
    WalletError::Sync(format!(
        "{kind} replay cancelled: progress receiver dropped"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_indexer_client::testutil::{bind, read_http_request, write_json_response};

    #[tokio::test]
    async fn the_latest_block_survives_a_dropped_connection() {
        let (listener, url) = bind().await;
        tokio::spawn(async move {
            drop(listener.accept().await.unwrap());
            let (mut stream, _) = listener.accept().await.unwrap();
            if read_http_request(&mut stream).await {
                let body = r#"{"data":{"block":{"hash":"00","height":7}}}"#;
                write_json_response(&mut stream, "200 OK", body).await;
            }
        });
        let block = latest_block(&url)
            .await
            .expect("the second connection answers");
        assert_eq!(block.height, 7);
    }

    #[tokio::test]
    async fn the_latest_block_stops_at_an_answer_or_at_the_bound() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // `None` drops every connection unanswered.
        let cases = [
            (
                Some(r#"{"errors":[{"message":"no such field"}]}"#),
                1,
                false,
            ),
            (None, 1 + RECONNECT_MAX_RETRIES as usize, true),
        ];
        for (answer, expected, retryable) in cases {
            let (listener, url) = bind().await;
            let connections = Arc::new(AtomicUsize::new(0));
            let server_connections = Arc::clone(&connections);
            let server = tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    server_connections.fetch_add(1, Ordering::SeqCst);
                    if let Some(body) = answer
                        && read_http_request(&mut stream).await
                    {
                        write_json_response(&mut stream, "200 OK", body).await;
                    }
                }
            });
            let fetch =
                tokio::time::timeout(std::time::Duration::from_secs(30), latest_block(&url));
            let err = fetch
                .await
                .expect("the fetch gives up")
                .expect_err("no connection serves a block");
            server.abort();
            assert!(
                matches!(&err, WalletError::Indexer(e) if e.is_retryable() == retryable),
                "answer: {answer:?}, got: {err:?}"
            );
            assert_eq!(
                connections.load(Ordering::SeqCst),
                expected,
                "answer: {answer:?}"
            );
        }
    }

    fn sub_utxo(intent_hash: Option<&str>, output_index: Option<i64>) -> SubscriptionUtxo {
        SubscriptionUtxo {
            owner: "owner".into(),
            token_type: "00".repeat(32),
            value: "1".into(),
            intent_hash: intent_hash.map(str::to_string),
            output_index,
            ctime: None,
            registered_for_dust_generation: None,
        }
    }

    #[test]
    fn spent_utxo_keys_extracts_only_fully_identified_utxos() {
        let tx_data = UnshieldedTxData {
            transaction: None,
            created_utxos: vec![sub_utxo(Some("created"), Some(0))],
            spent_utxos: vec![
                sub_utxo(Some("abcd"), Some(2)),
                sub_utxo(None, Some(1)),
                sub_utxo(Some("ffff"), None),
                sub_utxo(Some("eeee"), Some(-1)),
            ],
        };

        // Only spent UTXOs carrying both identity fields (with an in-range
        // index) produce keys; created UTXOs never do.
        assert_eq!(
            spent_utxo_keys(&tx_data),
            vec![SpentUtxoKey {
                intent_hash: "abcd".into(),
                output_index: 2,
            }]
        );
    }

    #[test]
    fn send_progress_is_lossy_on_full_but_reports_closed() {
        let (tx, mut rx) = mpsc::channel(1);
        let tx = Some(tx);

        // Fills the buffer.
        assert!(send_progress(
            &tx,
            SyncProgress::ZswapComplete { events: 1 }
        ));
        // Full channel: message dropped, but the receiver is alive.
        assert!(send_progress(
            &tx,
            SyncProgress::ZswapComplete { events: 2 }
        ));
        assert!(rx.try_recv().is_ok());
        assert!(
            rx.try_recv().is_err(),
            "second message must have been dropped"
        );

        // Closed channel: must be reported so replay loops can stop.
        drop(rx);
        assert!(!send_progress(
            &tx,
            SyncProgress::ZswapComplete { events: 3 }
        ));

        // No channel at all: nothing to report.
        assert!(send_progress(
            &None,
            SyncProgress::ZswapComplete { events: 4 }
        ));
    }

    #[test]
    fn reconnect_delay_doubles_from_base() {
        assert_eq!(reconnect_delay(1).as_millis(), 250);
        assert_eq!(reconnect_delay(2).as_millis(), 500);
        assert_eq!(reconnect_delay(3).as_millis(), 1000);
        assert_eq!(reconnect_delay(4).as_millis(), 2000);
    }

    #[test]
    fn order_regression_truth_table() {
        // Fresh connection (initial start, resume from a persisted cursor,
        // or a mid-replay reconnect): no high-water yet, so any first id is
        // in order, including re-deliveries at or below the applied cursor
        // (those are `already_applied`'s job to skip, not a violation).
        assert_eq!(order_regression(0, None), None);
        assert_eq!(order_regression(7, None), None);
        // Within one connection ids must be non-decreasing.
        assert_eq!(order_regression(5, Some(5)), None); // duplicate: dedupe handles it
        assert_eq!(order_regression(6, Some(5)), None); // strictly forward
        assert_eq!(order_regression(9, Some(5)), None); // forward gaps: legal on filtered streams
        assert_eq!(order_regression(4, Some(5)), Some(5)); // intra-connection regression
        // Post-progress regression: the connection advanced past the
        // cross-connection cursor (say cursor 5, connection high-water 8);
        // an id at or below the cursor arriving now is a violation, not a
        // legitimate reconnect re-delivery.
        assert_eq!(order_regression(3, Some(8)), Some(8));
    }

    #[test]
    fn apply_unshielded_tx_is_all_or_nothing_on_malformed_field() {
        let tracked = TrackedUtxo {
            owner: "owner".into(),
            token_type: "00".repeat(32),
            value: 1,
            intent_hash: Some("aaaa".into()),
            output_index: Some(0),
            ctime: None,
            registered_for_dust_generation: None,
        };
        let mut utxos = vec![tracked];

        // One parseable created UTXO, then a malformed one, and a spent
        // entry matching the tracked UTXO. The malformed field must reject
        // the whole event: no removal, no insertion.
        let mut malformed = sub_utxo(Some("cccc"), Some(0));
        malformed.value = "not-a-number".into();
        let tx_data = UnshieldedTxData {
            transaction: None,
            created_utxos: vec![sub_utxo(Some("bbbb"), Some(0)), malformed],
            spent_utxos: vec![sub_utxo(Some("aaaa"), Some(0))],
        };

        let err = apply_unshielded_tx(&mut utxos, &tx_data)
            .expect_err("malformed value must reject the event");
        assert!(
            matches!(
                &err,
                WalletError::MalformedUtxo { field: "value", value, .. }
                    if value == "not-a-number"
            ),
            "got: {err:?}"
        );
        assert_eq!(utxos.len(), 1, "event must not be partially applied");
        assert_eq!(utxos[0].intent_hash.as_deref(), Some("aaaa"));
        assert_eq!(utxos[0].value, 1);
    }

    #[test]
    fn already_applied_guards_resumption_only() {
        // Fresh sync from the beginning: nothing is skipped, even an event
        // with id 0.
        assert!(!already_applied(0, 0, 0, false));
        assert!(!already_applied(1, 0, 0, false));
        // Once events were applied this session, anything at or below the
        // cursor is a re-delivered duplicate.
        assert!(already_applied(2, 2, 0, true));
        assert!(already_applied(1, 2, 0, true));
        assert!(!already_applied(3, 2, 0, true));
        // Resuming from a persisted cursor: re-deliveries below the
        // requested start are skipped even before anything was applied this
        // session (`last_id` was initialized to `start_id - 1`).
        assert!(already_applied(4, 4, 5, false));
        assert!(!already_applied(5, 4, 5, false));
    }

    /// Mock-WebSocket-server tests for the replay loops' reconnect, resume,
    /// and dedupe behavior, driven through `replay_unshielded_events` (the
    /// one replay loop that needs no ledger state). The zswap/dust loops
    /// share the same retry/dedupe structure and helpers. The mock server
    /// itself lives in `midnight_indexer_client::testutil` (behind the
    /// `test-util` feature) and is shared with the indexer-client and
    /// provider test suites.
    mod reconnect_ws {
        use midnight_indexer_client::testutil::{accept_subscriber, bind, next_json, send_next};
        use serde_json::json;

        use super::*;

        fn tx_event(id: i64, value: u64) -> serde_json::Value {
            json!({
                "unshieldedTransactions": {
                    "__typename": "UnshieldedTransaction",
                    "transaction": {"id": id, "block": {"height": id * 10}},
                    "createdUtxos": [{
                        "owner": "addr",
                        "tokenType": "00",
                        "value": value.to_string(),
                        "intentHash": format!("{id:02x}"),
                        "outputIndex": 0,
                    }],
                    "spentUtxos": [],
                }
            })
        }

        fn progress_event(target: i64) -> serde_json::Value {
            json!({
                "unshieldedTransactions": {
                    "__typename": "UnshieldedTransactionsProgress",
                    "highestTransactionId": target,
                }
            })
        }

        fn requested_tx_id(sub: &serde_json::Value) -> i64 {
            sub["payload"]["variables"]["transactionId"]
                .as_i64()
                .expect("transactionId variable")
        }

        #[tokio::test]
        async fn unshielded_replay_resumes_after_drop_and_dedupes() {
            let (listener, url) = bind().await;
            let server = tokio::spawn(async move {
                // Connection 1: announce target 3, deliver txs 1 and 2, then
                // drop the socket without a close handshake.
                let (mut ws, sub) = accept_subscriber(&listener).await;
                assert_eq!(requested_tx_id(&sub), 0);
                assert_eq!(sub["payload"]["variables"]["address"], "addr");
                send_next(&mut ws, &sub, progress_event(3)).await;
                send_next(&mut ws, &sub, tx_event(1, 100)).await;
                send_next(&mut ws, &sub, tx_event(2, 200)).await;
                drop(ws);

                // Connection 2: the client must resume from the cursor.
                // Re-deliver tx 2 (a duplicate the client must skip), then
                // deliver tx 3 to complete the sync.
                let (mut ws, sub) = accept_subscriber(&listener).await;
                assert_eq!(requested_tx_id(&sub), 3, "resume from last_id + 1");
                send_next(&mut ws, &sub, tx_event(2, 200)).await;
                send_next(&mut ws, &sub, tx_event(3, 300)).await;
                while next_json(&mut ws).await.is_some() {}
            });

            let sub_client = SubscriptionClient::new(&url);
            let (utxos, last_tx_id, last_height, _spent) =
                replay_unshielded_events(&sub_client, "addr", Vec::new(), 0, false, None)
                    .await
                    .expect("sync must succeed across the reconnect");

            let values: Vec<u128> = utxos.iter().map(|u| u.value).collect();
            assert_eq!(values, vec![100, 200, 300], "duplicate tx 2 re-applied?");
            assert_eq!(last_tx_id, 3);
            assert_eq!(last_height, 30);

            server.await.unwrap();
        }

        #[tokio::test]
        async fn unshielded_replay_fails_after_max_consecutive_failures() {
            let (listener, url) = bind().await;
            let attempts = 1 + RECONNECT_MAX_RETRIES as usize;
            let server = tokio::spawn(async move {
                // Complete the subscribe handshake, then drop, for every
                // allowed attempt. The client must give up afterwards.
                let mut connections = 0usize;
                for _ in 0..attempts {
                    let (ws, _sub) = accept_subscriber(&listener).await;
                    connections += 1;
                    drop(ws);
                }
                connections
            });

            let sub_client = SubscriptionClient::new(&url);
            let err = replay_unshielded_events(&sub_client, "addr", Vec::new(), 0, false, None)
                .await
                .expect_err("must fail after exhausting reconnect attempts");
            assert!(
                matches!(&err, WalletError::Indexer(e) if e.is_retryable()),
                "got: {err:?}"
            );

            assert_eq!(server.await.unwrap(), attempts);
        }

        #[tokio::test]
        async fn unshielded_replay_duplicate_only_redeliveries_exhaust_the_bound() {
            use std::sync::Arc;
            use std::sync::atomic::{AtomicUsize, Ordering};

            let (listener, url) = bind().await;
            let connections = Arc::new(AtomicUsize::new(0));
            let server_connections = Arc::clone(&connections);
            let server = tokio::spawn(async move {
                // Connection 1: announce target 3, deliver txs 1 and 2 (real
                // progress), then drop.
                let (mut ws, sub) = accept_subscriber(&listener).await;
                server_connections.fetch_add(1, Ordering::SeqCst);
                assert_eq!(requested_tx_id(&sub), 0);
                send_next(&mut ws, &sub, progress_event(3)).await;
                send_next(&mut ws, &sub, tx_event(1, 100)).await;
                send_next(&mut ws, &sub, tx_event(2, 200)).await;
                drop(ws);

                // Every reconnect: re-deliver only the already-applied tx 2,
                // then drop. A deduped re-delivery is not progress, so the
                // client must exhaust the reconnect bound instead of looping
                // forever. Keep accepting so a regression (resetting the
                // counter on deduped events) shows up as extra connections.
                loop {
                    let (mut ws, sub) = accept_subscriber(&listener).await;
                    server_connections.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(requested_tx_id(&sub), 3, "resume from last applied + 1");
                    send_next(&mut ws, &sub, tx_event(2, 200)).await;
                    drop(ws);
                }
            });

            let sub_client = SubscriptionClient::new(&url);
            let err = replay_unshielded_events(&sub_client, "addr", Vec::new(), 0, false, None)
                .await
                .expect_err("duplicate-only re-deliveries must not reset the bound");
            assert!(
                matches!(&err, WalletError::Indexer(e) if e.is_retryable()),
                "got: {err:?}"
            );

            server.abort();
            assert_eq!(
                connections.load(Ordering::SeqCst),
                1 + RECONNECT_MAX_RETRIES as usize,
                "client must give up after the bounded number of connections"
            );
        }

        #[tokio::test]
        async fn unshielded_replay_rejects_intra_connection_id_regression() {
            let (listener, url) = bind().await;
            let server = tokio::spawn(async move {
                let (mut ws, sub) = accept_subscriber(&listener).await;
                assert_eq!(requested_tx_id(&sub), 0);
                send_next(&mut ws, &sub, progress_event(5)).await;
                send_next(&mut ws, &sub, tx_event(2, 200)).await;
                send_next(&mut ws, &sub, tx_event(3, 300)).await;
                // Hostile / corrupt stream: id 1 after id 3 on the same
                // connection. Without the order check this would be
                // silently deduped; it must error instead.
                send_next(&mut ws, &sub, tx_event(1, 100)).await;
                while next_json(&mut ws).await.is_some() {}
            });

            let sub_client = SubscriptionClient::new(&url);
            let err = replay_unshielded_events(&sub_client, "addr", Vec::new(), 0, false, None)
                .await
                .expect_err("an id regression within one connection must error");
            assert!(
                matches!(
                    err,
                    WalletError::EventOrder {
                        kind: "unshielded",
                        id: 1,
                        prev: 3,
                    }
                ),
                "got: {err:?}"
            );

            server.await.unwrap();
        }
    }
}
