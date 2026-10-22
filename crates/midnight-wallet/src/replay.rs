//! The generation-free half of a replay: the indexer's subscription
//! messages, the reconnect policy that every replay loop and the latest-block
//! query follow, the loop that reads the zswap and Dust streams for a
//! generation to apply, and the unshielded replay, whose events are JSON in
//! every ledger generation.

use hex::FromHex;
use midnight_indexer_client::{IndexerError, SubscriptionClient};
use midnight_types::{HashOutput, TrackedUtxo, UnshieldedTokenType};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{LedgerVersion, SpentUtxoKey, SyncProgress, WalletError};

// ---------------------------------------------------------------------------
// Subscription event types — internal to the sync loop.
//
// These shapes mirror the indexer's GraphQL subscription responses and exist
// to deserialize them. They are not part of the user-facing wallet API: sync
// is `MidnightProvider`'s job, and consumers see only its `SyncProgress`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LedgerEventMessage {
    id: i64,
    raw: String,
    max_id: i64,
}

/// Response envelope for the zswapLedgerEvents and dustLedgerEvents
/// subscriptions.
#[derive(Debug, Clone, Deserialize)]
struct LedgerEventEnvelope {
    // A stream's query asks for its own field only, so taking either name
    // lets no other stream's events in.
    #[serde(rename = "zswapLedgerEvents", alias = "dustLedgerEvents")]
    events: LedgerEventMessage,
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
    pub intent_hash: String,
    pub output_index: i64,
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
fn resume_id(applied_this_replay: u64, last_id: i64) -> i64 {
    if applied_this_replay > 0 {
        last_id + 1
    } else {
        last_id
    }
}

fn last_applied_before(start_id: i64) -> i64 {
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
const RECONNECT_MAX_RETRIES: u32 = 4;

/// Base delay of the reconnect backoff; doubles per consecutive retry:
/// 250ms, 500ms, 1s, 2s.
const RECONNECT_BASE_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

/// Backoff delay before retry number `retry` (1-based).
fn reconnect_delay(retry: u32) -> std::time::Duration {
    RECONNECT_BASE_DELAY * 2u32.saturating_pow(retry.saturating_sub(1))
}

/// How long a replay loop waits for its next message.
///
/// An initial sync reads silence as a fatal stall, so its bound sits above the
/// client's transport keepalive, which errors out a dead socket on its own
/// (see `midnight_indexer_client::subscription`). A loop that reaches it has a
/// live socket and a server that sends nothing. A resume reads silence as "at
/// the tip", so it waits less.
fn event_timeout(resuming: bool) -> std::time::Duration {
    if resuming {
        std::time::Duration::from_secs(10)
    } else {
        std::time::Duration::from_secs(30)
    }
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
fn already_applied(msg_id: i64, last_id: i64, start_id: i64, applied_any: bool) -> bool {
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
fn order_regression(msg_id: i64, conn_high: Option<i64>) -> Option<i64> {
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
    let indexer_client = midnight_indexer_client::IndexerClient::new(indexer_url)
        .map_err(|e| gave_up("latest block", e))?;
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

/// The generation that `block` ran, from the ledger parameters the indexer
/// serves on it.
pub(crate) fn ledger_of(
    block: &midnight_indexer_client::Block,
) -> Result<LedgerVersion, WalletError> {
    LedgerVersion::of_ledger_parameters(&encoded_ledger_parameters(block)?)
        .map_err(WalletError::from)
}

/// The encoded ledger parameters that the indexer serves on `block`.
pub(crate) fn encoded_ledger_parameters(
    block: &midnight_indexer_client::Block,
) -> Result<Vec<u8>, WalletError> {
    let params_hex = block
        .ledger_parameters
        .as_deref()
        .ok_or_else(|| WalletError::Sync("latest block has no ledger_parameters".into()))?;
    hex::decode(params_hex).map_err(|e| WalletError::Sync(format!("decode ledger params hex: {e}")))
}

fn gave_up(request: &'static str, error: IndexerError) -> WalletError {
    warn!(request, error = %with_causes(&error), "giving up on the indexer");
    WalletError::Indexer {
        retryable: error.is_retryable(),
        source: Box::new(error),
    }
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

/// One of the two ledger-event streams that [`replay_ledger_events`] reads.
#[derive(Debug, Clone, Copy)]
pub(crate) enum LedgerStream {
    Zswap,
    Dust,
}

impl LedgerStream {
    /// The name that logs and errors give the stream.
    fn kind(self) -> &'static str {
        match self {
            Self::Zswap => "zswap",
            Self::Dust => "dust",
        }
    }

    fn query(self) -> &'static str {
        use midnight_indexer_client::subscription::queries::{
            DUST_LEDGER_EVENTS_SUBSCRIPTION, ZSWAP_LEDGER_EVENTS_SUBSCRIPTION,
        };
        match self {
            Self::Zswap => ZSWAP_LEDGER_EVENTS_SUBSCRIPTION,
            Self::Dust => DUST_LEDGER_EVENTS_SUBSCRIPTION,
        }
    }

    fn events(self, current: i64, max: i64) -> SyncProgress {
        match self {
            Self::Zswap => SyncProgress::ZswapEvents { current, max },
            Self::Dust => SyncProgress::DustEvents { current, max },
        }
    }

    fn complete(self, events: u64) -> SyncProgress {
        match self {
            Self::Zswap => SyncProgress::ZswapComplete { events },
            Self::Dust => SyncProgress::DustComplete { events },
        }
    }
}

/// Events between two progress reports of a ledger-event replay.
const PROGRESS_INTERVAL: u64 = 10_000;

/// Where a ledger-event replay stopped.
pub(crate) struct LedgerReplay {
    /// The id of the last event consumed.
    pub last_id: i64,
    /// Where the stream moved to a later ledger generation: the id of that
    /// generation's first event, which the replay did not consume.
    pub later_ledger: Option<(i64, LedgerVersion)>,
}

/// Hex-decode the event a `LedgerEventMessage` carries.
fn event_bytes(msg: &LedgerEventMessage, kind: &str) -> Result<Vec<u8>, WalletError> {
    hex::decode(&msg.raw).map_err(|e| WalletError::Sync(format!("decode {kind} event hex: {e}")))
}

/// Replay `stream` from `start_id` for a wallet in the generation `ledger`.
///
/// The replay hands each new event to `apply` with its id, its bytes and its
/// generation, which is `ledger` or an earlier one, and consumes the event
/// when `apply` returns `Ok`. An error from `apply` ends the replay with that
/// error. The replay stops before the first event of a later generation and
/// reports it in [`LedgerReplay::later_ledger`].
pub(crate) async fn replay_ledger_events(
    sub_client: &SubscriptionClient,
    stream: LedgerStream,
    ledger: LedgerVersion,
    start_id: i64,
    resuming: bool,
    progress: Option<mpsc::Sender<SyncProgress>>,
    mut apply: impl FnMut(i64, &[u8], LedgerVersion) -> Result<(), WalletError>,
) -> Result<LedgerReplay, WalletError> {
    let kind = stream.kind();
    let mut last_id: i64 = last_applied_before(start_id);
    let mut count: u64 = 0;
    let mut retries: u32 = 0;
    let mut later_ledger = None;

    'reconnect: loop {
        let resume_id = resume_id(count, last_id);
        let variables = serde_json::json!({ "id": resume_id });
        let mut subscription = match sub_client
            .subscribe::<LedgerEventEnvelope>(stream.query(), variables)
            .await
        {
            Ok(s) => s,
            Err(e) if e.is_retryable() && retries < RECONNECT_MAX_RETRIES => {
                retries += 1;
                warn!(retries, error = %e, "{kind} subscribe failed, retrying");
                tokio::time::sleep(reconnect_delay(retries)).await;
                continue 'reconnect;
            }
            Err(e) => return Err(gave_up(kind, e)),
        };
        // Highest id delivered on *this* connection; see `order_regression`.
        let mut conn_high: Option<i64> = None;

        loop {
            let event = tokio::time::timeout(event_timeout(resuming), subscription.next()).await;

            match event {
                Ok(Some(Ok(envelope))) => {
                    let msg = &envelope.events;

                    if let Some(prev) = order_regression(msg.id, conn_high) {
                        return Err(WalletError::EventOrder {
                            kind,
                            id: msg.id,
                            prev,
                        });
                    }
                    conn_high = Some(msg.id);

                    if msg.max_id == 0 {
                        debug!("no {kind} events on this chain");
                        break 'reconnect;
                    }

                    if already_applied(msg.id, last_id, start_id, count > 0) {
                        debug!(id = msg.id, last_id, "skipping re-delivered {kind} event");
                        if msg.id >= msg.max_id {
                            info!(count, last_id, "{kind} replay complete");
                            send_progress(&progress, stream.complete(count));
                            break 'reconnect;
                        }
                        continue;
                    }

                    let raw = event_bytes(msg, kind)?;
                    let event_ledger = LedgerVersion::of_event(&raw)?;
                    if event_ledger > ledger {
                        info!(id = msg.id, ledger = %event_ledger, "{kind} events move to a later ledger");
                        later_ledger = Some((msg.id, event_ledger));
                        break 'reconnect;
                    }
                    apply(msg.id, &raw, event_ledger)?;

                    // Only a new event counts as progress for the reconnect
                    // bound; deduped re-deliveries must not reset it.
                    retries = 0;
                    last_id = msg.id;
                    count += 1;

                    if count.is_multiple_of(PROGRESS_INTERVAL) {
                        info!(
                            count,
                            id = msg.id,
                            max_id = msg.max_id,
                            "{kind} replay progress"
                        );
                        let alive = send_progress(&progress, stream.events(msg.id, msg.max_id));
                        if !alive {
                            return Err(progress_cancelled(kind));
                        }
                    }

                    if msg.id >= msg.max_id {
                        info!(count, last_id, "{kind} replay complete");
                        send_progress(&progress, stream.complete(count));
                        break 'reconnect;
                    }
                }
                Ok(Some(Err(e))) if e.is_retryable() && retries < RECONNECT_MAX_RETRIES => {
                    retries += 1;
                    warn!(retries, error = %e, "{kind} subscription dropped, reconnecting");
                    tokio::time::sleep(reconnect_delay(retries)).await;
                    continue 'reconnect;
                }
                Ok(Some(Err(e))) => return Err(gave_up(kind, e)),
                Ok(None) => {
                    if resuming && count == 0 {
                        info!(last_id, "{kind} already at tip");
                        send_progress(&progress, stream.complete(0));
                        break 'reconnect;
                    }
                    // Mid-replay stream end without a `complete`: treat as a
                    // dropped connection and resume from the cursor.
                    if retries < RECONNECT_MAX_RETRIES {
                        retries += 1;
                        warn!(retries, "{kind} subscription ended early, reconnecting");
                        tokio::time::sleep(reconnect_delay(retries)).await;
                        continue 'reconnect;
                    }
                    return Err(gave_up(
                        kind,
                        IndexerError::Transport(format!(
                            "{kind} subscription ended before replay completed \
                             (after {RECONNECT_MAX_RETRIES} reconnect attempts)"
                        )),
                    ));
                }
                Err(_) => {
                    if resuming && count == 0 {
                        info!(last_id, "{kind} already at tip");
                        send_progress(&progress, stream.complete(0));
                        break 'reconnect;
                    }
                    return Err(gave_up(
                        kind,
                        IndexerError::Transport(format!("timeout waiting for {kind} events")),
                    ));
                }
            }
        }
    }

    Ok(LedgerReplay {
        last_id,
        later_ledger,
    })
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
            let event = tokio::time::timeout(event_timeout(resuming), subscription.next()).await;

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
                            let confirmed =
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
                            spent_keys.extend(confirmed);
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
                    return Err(gave_up(
                        "unshielded",
                        IndexerError::Transport(format!(
                            "unshielded subscription ended before sync completed \
                             (after {RECONNECT_MAX_RETRIES} reconnect attempts)"
                        )),
                    ));
                }
                Err(_) => {
                    // A resume rarely gets here: the indexer answers the
                    // subscribe with a progress frame that carries its highest
                    // transaction id, and a cursor already at that id ends the
                    // replay there. Silence is a server that skipped the frame.
                    if resuming {
                        info!(last_seen_tx_id, "unshielded already at tip");
                        return Ok((utxos, last_seen_tx_id, last_height, spent_keys));
                    }
                    return Err(gave_up(
                        "unshielded",
                        IndexerError::Transport("timeout waiting for unshielded sync".into()),
                    ));
                }
            }
        }
    }
}

/// Composite key for matching unshielded UTXOs during spend removal.
type UtxoKey = (String, UnshieldedTokenType, u128, HashOutput, u32);

fn utxo_key(u: &TrackedUtxo) -> UtxoKey {
    (
        u.owner.clone(),
        u.token_type,
        u.value,
        u.intent_hash,
        u.output_index,
    )
}

fn parse_utxo(u: &SubscriptionUtxo) -> Result<TrackedUtxo, WalletError> {
    let malformed = |field, value, reason: &dyn std::fmt::Display| WalletError::MalformedUtxo {
        field,
        value,
        reason: reason.to_string(),
        tx_id: None,
    };
    let token_type = <[u8; 32]>::from_hex(&u.token_type)
        .map_err(|e| malformed("token_type", u.token_type.clone(), &e))?;
    let value = u
        .value
        .parse::<u128>()
        .map_err(|e| malformed("value", u.value.clone(), &e))?;
    let intent_hash = <[u8; 32]>::from_hex(&u.intent_hash)
        .map_err(|e| malformed("intent_hash", u.intent_hash.clone(), &e))?;
    let output_index = u32::try_from(u.output_index)
        .map_err(|e| malformed("output_index", u.output_index.to_string(), &e))?;
    Ok(TrackedUtxo {
        owner: u.owner.clone(),
        token_type: UnshieldedTokenType(HashOutput(token_type)),
        value,
        intent_hash: HashOutput(intent_hash),
        output_index,
        ctime: u.ctime,
        registered_for_dust_generation: u.registered_for_dust_generation,
    })
}

/// Apply one unshielded transaction event to the tracked UTXO set,
/// all-or-nothing: every spent and created UTXO is parsed upfront, and the
/// first malformed field rejects the whole event with a typed error before
/// any mutation. An event therefore either fully applies or leaves `utxos`
/// untouched, and since the replay loops propagate the error and the sync
/// paths only commit a fully successful replay (`sync_inner` builds the
/// wallet at the end; `ResyncPlan::run` only then yields a `ResyncCommit`),
/// a malformed event never leaves partial state behind.
///
/// Returns the [`TrackedUtxo::spent_key`] of each spent UTXO, so the caller
/// can clear the reservations the event confirms.
fn apply_unshielded_tx(
    utxos: &mut Vec<TrackedUtxo>,
    tx_data: &UnshieldedTxData,
) -> Result<Vec<SpentUtxoKey>, WalletError> {
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

    Ok(spent.iter().map(TrackedUtxo::spent_key).collect())
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
                matches!(&err, WalletError::Indexer { retryable: r, .. } if *r == retryable),
                "answer: {answer:?}, got: {err:?}"
            );
            assert_eq!(
                connections.load(Ordering::SeqCst),
                expected,
                "answer: {answer:?}"
            );
        }
    }

    /// A NIGHT UTXO of value 1 from a transaction whose intent hash repeats
    /// the byte `intent_hash`.
    fn sub_utxo(intent_hash: u8, output_index: i64) -> SubscriptionUtxo {
        SubscriptionUtxo {
            owner: "owner".into(),
            token_type: "00".repeat(32),
            value: "1".into(),
            intent_hash: hex::encode([intent_hash; 32]),
            output_index,
            ctime: None,
            registered_for_dust_generation: None,
        }
    }

    /// A reservation names an input, so only the spent side of an event can
    /// confirm one.
    #[test]
    fn apply_unshielded_tx_reports_the_spent_keys_only() {
        let tx_data = UnshieldedTxData {
            transaction: None,
            created_utxos: vec![sub_utxo(0xcc, 0)],
            spent_utxos: vec![sub_utxo(0xab, 2)],
        };

        assert_eq!(
            apply_unshielded_tx(&mut Vec::new(), &tx_data).unwrap(),
            vec![SpentUtxoKey {
                intent_hash: HashOutput([0xab; 32]),
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

    /// Every UTXO field is parsed before the event touches the tracked set.
    /// A field that falls back to a default, or an index cast with `as`,
    /// applies a UTXO that the balance and the builds then read wrong.
    #[test]
    fn apply_unshielded_tx_is_all_or_nothing_on_malformed_field() {
        type Corrupt = fn(&mut SubscriptionUtxo);
        let rows: [(&str, &str, Corrupt); 4] = [
            ("value", "not-a-number", |u| u.value = "not-a-number".into()),
            ("token_type", "zz", |u| u.token_type = "zz".into()),
            ("intent_hash", "abcd", |u| u.intent_hash = "abcd".into()),
            ("output_index", "-1", |u| u.output_index = -1),
        ];
        for (field, raw, corrupt) in rows {
            let mut utxos = vec![TrackedUtxo {
                owner: "owner".into(),
                token_type: midnight_types::NIGHT,
                value: 1,
                intent_hash: HashOutput([0xaa; 32]),
                output_index: 0,
                ctime: None,
                registered_for_dust_generation: None,
            }];

            // One parseable created UTXO, then a malformed one, and a spent
            // entry matching the tracked UTXO. The malformed field must
            // reject the whole event: no removal, no insertion.
            let mut malformed = sub_utxo(0xcc, 0);
            corrupt(&mut malformed);
            let tx_data = UnshieldedTxData {
                transaction: None,
                created_utxos: vec![sub_utxo(0xbb, 0), malformed],
                spent_utxos: vec![sub_utxo(0xaa, 0)],
            };

            let Err(err) = apply_unshielded_tx(&mut utxos, &tx_data) else {
                panic!("{field}: a malformed field must reject the event");
            };
            assert!(
                matches!(
                    &err,
                    WalletError::MalformedUtxo { field: f, value, .. }
                        if *f == field && value == raw
                ),
                "{field}: got {err:?}"
            );
            assert_eq!(
                utxos.len(),
                1,
                "{field}: event must not be partially applied"
            );
            assert_eq!(utxos[0].intent_hash, HashOutput([0xaa; 32]), "{field}");
        }
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
    /// and dedupe behavior, driven through `replay_unshielded_events`, whose
    /// events are plain JSON. `replay_ledger_events` shares the same
    /// retry and dedupe helpers. The mock server
    /// itself lives in `midnight_indexer_client::testutil` (behind the
    /// `test-util` feature) and is shared with the indexer-client and
    /// provider test suites.
    mod reconnect_ws {
        use midnight_indexer_client::testutil::{
            accept_subscriber, bind, next_json, send_json, send_next, subscription_variable,
            unshielded_progress,
        };
        use serde_json::json;

        use super::*;

        fn tx_event(id: i64, value: u64) -> serde_json::Value {
            json!({
                "unshieldedTransactions": {
                    "__typename": "UnshieldedTransaction",
                    "transaction": {"id": id, "block": {"height": id * 10}},
                    "createdUtxos": [{
                        "owner": "addr",
                        "tokenType": "00".repeat(32),
                        "value": value.to_string(),
                        "intentHash": format!("{id:064x}"),
                        "outputIndex": 0,
                    }],
                    "spentUtxos": [],
                }
            })
        }

        #[tokio::test]
        async fn unshielded_replay_resumes_after_drop_and_dedupes() {
            let (listener, url) = bind().await;
            let server = tokio::spawn(async move {
                // Connection 1: announce target 3, deliver txs 1 and 2, then
                // drop the socket without a close handshake.
                let (mut ws, sub) = accept_subscriber(&listener).await;
                assert_eq!(subscription_variable(&sub, "transactionId"), 0);
                assert_eq!(sub["payload"]["variables"]["address"], "addr");
                send_next(&mut ws, &sub, unshielded_progress(3)).await;
                send_next(&mut ws, &sub, tx_event(1, 100)).await;
                send_next(&mut ws, &sub, tx_event(2, 200)).await;
                drop(ws);

                // Connection 2: the client must resume from the cursor.
                // Re-deliver tx 2 (a duplicate the client must skip), then
                // deliver tx 3 to complete the sync.
                let (mut ws, sub) = accept_subscriber(&listener).await;
                assert_eq!(
                    subscription_variable(&sub, "transactionId"),
                    3,
                    "resume from last_id + 1"
                );
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
            // A dropped socket reaches the replay as a read error, and a
            // completed subscription as the end of the stream.
            for complete in [false, true] {
                let (listener, url) = bind().await;
                let attempts = 1 + RECONNECT_MAX_RETRIES as usize;
                let server = tokio::spawn(async move {
                    // Complete the subscribe handshake, then end the
                    // connection, for every allowed attempt. The client must
                    // give up afterwards.
                    let mut connections = 0usize;
                    for _ in 0..attempts {
                        let (mut ws, sub) = accept_subscriber(&listener).await;
                        connections += 1;
                        if complete {
                            send_json(&mut ws, &json!({"type": "complete", "id": sub["id"]})).await;
                            while next_json(&mut ws).await.is_some() {}
                        }
                    }
                    connections
                });

                let sub_client = SubscriptionClient::new(&url);
                let err = replay_unshielded_events(&sub_client, "addr", Vec::new(), 0, false, None)
                    .await
                    .expect_err("must fail after exhausting reconnect attempts");
                assert!(
                    matches!(&err, WalletError::Indexer { retryable, .. } if *retryable),
                    "complete: {complete}, got: {err:?}"
                );

                assert_eq!(server.await.unwrap(), attempts, "complete: {complete}");
            }
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
                assert_eq!(subscription_variable(&sub, "transactionId"), 0);
                send_next(&mut ws, &sub, unshielded_progress(3)).await;
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
                    assert_eq!(
                        subscription_variable(&sub, "transactionId"),
                        3,
                        "resume from last applied + 1"
                    );
                    send_next(&mut ws, &sub, tx_event(2, 200)).await;
                    drop(ws);
                }
            });

            let sub_client = SubscriptionClient::new(&url);
            let err = replay_unshielded_events(&sub_client, "addr", Vec::new(), 0, false, None)
                .await
                .expect_err("duplicate-only re-deliveries must not reset the bound");
            assert!(
                matches!(&err, WalletError::Indexer { retryable, .. } if *retryable),
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
                assert_eq!(subscription_variable(&sub, "transactionId"), 0);
                send_next(&mut ws, &sub, unshielded_progress(5)).await;
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
