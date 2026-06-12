use std::path::{Path, PathBuf};
use std::sync::Arc;

use midnight_helpers::midnight_serialize::tagged_deserialize;
use midnight_helpers::mn_ledger::events::EventDetails;
use midnight_helpers::mn_ledger::semantics::ZswapLocalStateExt;
use midnight_helpers::mn_ledger::structure::{Utxo as LedgerUtxo, UtxoMeta};
use midnight_helpers::{
    BlockContext, DefaultDB, DustNullifier, DustWallet, Event, HashOutput, IntentHash,
    LedgerContext, LedgerParameters, LedgerState, MAX_SUPPLY, SecretKeys, ShieldedWallet, Sp,
    Timestamp, UnshieldedTokenType, UnshieldedWallet, Wallet as ContextWallet, WalletSeed,
    WalletState as ZswapLocalState,
};
use midnight_indexer_client::SubscriptionClient;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::pending::PendingReservations;
use crate::{SpentUtxoKey, WalletError};

/// Progress updates emitted during wallet sync.
#[derive(Debug, Clone)]
pub enum SyncProgress {
    Resuming {
        zswap_event_id: i64,
        dust_event_id: i64,
    },
    ZswapEvents {
        current: i64,
        max: i64,
    },
    ZswapComplete {
        events: u64,
    },
    DustEvents {
        current: i64,
        max: i64,
    },
    DustComplete {
        events: u64,
    },
    UnshieldedCaughtUp {
        utxos: usize,
    },
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// A tracked unshielded UTXO from the indexer.
#[derive(Debug, Clone)]
pub struct TrackedUtxo {
    pub owner: String,
    pub token_type: String,
    pub value: u128,
    pub intent_hash: Option<String>,
    pub output_index: Option<i64>,
}

impl TryFrom<midnight_indexer_client::UnshieldedUtxo> for TrackedUtxo {
    type Error = WalletError;

    fn try_from(utxo: midnight_indexer_client::UnshieldedUtxo) -> Result<Self, Self::Error> {
        let value: u128 = utxo.value.parse().map_err(|e| {
            WalletError::Sync(format!("failed to parse UTXO value '{}': {e}", utxo.value))
        })?;
        Ok(Self {
            owner: utxo.owner,
            token_type: utxo.token_type,
            value,
            intent_hash: utxo.intent_hash,
            output_index: utxo.output_index,
        })
    }
}

/// A Midnight wallet: identity (seed, addresses) and synced ledger state.
///
/// Maintains three streams of state from the indexer:
/// - `zswapLedgerEvents` → shielded coin tracking + Merkle tree
/// - `dustLedgerEvents` → dust/fee UTXO tracking
/// - `unshieldedTransactions` → unshielded UTXO balance
///
/// Transaction building uses the local state directly (no full-chain-replay).
/// `Wallet` owns the synced state and exposes mutation methods
/// (`set_block_context`, `set_parameters`, `reserve_pending`). All I/O —
/// initial sync, resync, subscriptions, building a [`LedgerContext`] —
/// is driven by [`midnight_provider::MidnightProvider`], which owns the wallet
/// behind an `Arc<RwLock<_>>`.
pub struct Wallet {
    seed: WalletSeed,
    secret_keys: SecretKeys,
    network_id: String,
    unshielded_address: String,

    // Shielded state (from zswapLedgerEvents)
    zswap_state: ZswapLocalState<DefaultDB>,
    zswap_event_id: i64,

    // Dust state (from dustLedgerEvents)
    dust_wallet: DustWallet<DefaultDB>,
    dust_event_id: i64,

    // Unshielded UTXOs (from unshieldedTransactions)
    unshielded_utxos: Vec<TrackedUtxo>,
    last_block_height: i64,
    last_tx_id: Option<i64>,

    // Chain parameters (from latest block via indexer HTTP)
    parameters: LedgerParameters,
    block_context: Option<BlockContext>,

    /// In-flight reservations: spends built locally but not yet observed
    /// as confirmed on-chain. Applied at [`Wallet::build_context_inner`]
    /// time to prevent local double-builds, cleared when corresponding
    /// events arrive or when the TTL window elapses. Never written to the
    /// confirmed-state files; persisted separately via `pending.json`.
    pending: PendingReservations,

    /// Where this wallet persists its state, when [`Wallet::sync_inner`] was
    /// given a storage directory. Retained so [`Wallet::resync`] can re-save
    /// the moved cursors and [`Wallet::reserve_pending`] can persist
    /// `pending.json` without the caller re-supplying the path.
    storage_dir: Option<PathBuf>,
}

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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnshieldedTxProgress {
    pub highest_transaction_id: i64,
}

// ---------------------------------------------------------------------------
// Wallet implementation
// ---------------------------------------------------------------------------

/// Number of dust events between checkpoint saves during initial sync.
const DUST_CHECKPOINT_INTERVAL: u64 = 50_000;

type DustCheckpointFn = dyn Fn(&DustWallet<DefaultDB>, i64) + Send;

#[allow(clippy::too_many_arguments)]
fn make_dust_checkpoint(
    storage_dir: Option<&Path>,
    network_id: &str,
    seed: WalletSeed,
    zswap_state: ZswapLocalState<DefaultDB>,
    zswap_event_id: i64,
    last_block_height: i64,
    last_tx_id: Option<i64>,
    unshielded_utxos: Vec<TrackedUtxo>,
) -> Option<Box<DustCheckpointFn>> {
    storage_dir.map(|dir| {
        let dir = dir.to_path_buf();
        let net = network_id.to_string();
        Box::new(move |dw: &DustWallet<DefaultDB>, dust_eid: i64| {
            if let Err(err) = crate::storage::save(
                &dir,
                &net,
                &seed,
                &zswap_state,
                dw,
                zswap_event_id,
                dust_eid,
                last_block_height,
                last_tx_id,
                &unshielded_utxos,
            ) {
                warn!(error = %err, "failed to checkpoint dust state");
            }
        }) as Box<DustCheckpointFn>
    })
}

fn last_applied_before(start_id: i64) -> i64 {
    start_id.saturating_sub(1).max(0)
}

/// Construct a `BlockContext` anchored at the given `tblock`.
fn block_context_at(tblock: Timestamp) -> BlockContext {
    BlockContext {
        tblock,
        tblock_err: 30,
        parent_block_hash: Default::default(),
        last_block_time: tblock,
    }
}

/// Hex-decode a `LedgerEventMessage` and tagged-deserialize the inner `Event`.
fn decode_event(msg: &LedgerEventMessage, kind: &str) -> Result<Event<DefaultDB>, WalletError> {
    let raw_bytes = hex::decode(&msg.raw)
        .map_err(|e| WalletError::Sync(format!("decode {kind} event hex: {e}")))?;
    tagged_deserialize(&raw_bytes[..])
        .map_err(|e| WalletError::Sync(format!("deserialize {kind} event: {e}")))
}

/// Hex-decode and tagged-deserialize the `ledger_parameters` carried on an
/// indexer block. Both initial sync and resync read parameters from the
/// latest block so governance changes (fees, TTL, dust rates) take effect.
fn decode_ledger_parameters(
    block: &midnight_indexer_client::Block,
) -> Result<LedgerParameters, WalletError> {
    let params_hex = block
        .ledger_parameters
        .as_deref()
        .ok_or_else(|| WalletError::Sync("latest block has no ledger_parameters".into()))?;
    let params_bytes = hex::decode(params_hex)
        .map_err(|e| WalletError::Sync(format!("decode ledger params hex: {e}")))?;
    tagged_deserialize(&params_bytes[..])
        .map_err(|e| WalletError::Sync(format!("deserialize ledger params: {e}")))
}

/// Validated results of a resync's replay tasks and latest-block fetch,
/// ready to be committed into a [`Wallet`]. Groups the inputs of
/// [`Wallet::commit_resync`] so the commit-and-persist sequence is
/// unit-testable without a live indexer.
struct ResyncCommit {
    dust_wallet: DustWallet<DefaultDB>,
    dust_event_id: i64,
    last_dust_block_time: Option<Timestamp>,
    dust_nullifiers: Vec<DustNullifier>,
    zswap_state: ZswapLocalState<DefaultDB>,
    zswap_event_id: i64,
    unshielded_utxos: Vec<TrackedUtxo>,
    last_tx_id: i64,
    last_block_height: i64,
    spent_unshielded: Vec<SpentUtxoKey>,
    chain_tblock: Timestamp,
    parameters: LedgerParameters,
}

impl Wallet {
    /// Default storage directory: `~/.midnight/wallets/`
    pub fn default_storage_dir() -> Option<PathBuf> {
        home_dir().map(|h| h.join(".midnight").join("wallets"))
    }

    /// Internal sync entry point — public so `midnight-provider` can call it
    /// across crates. Prefer [`midnight_provider::MidnightProvider::sync_wallet`]
    /// (which returns a `SyncWalletBuilder`; `.stream()` gives progress
    /// events). The provider supplies the indexer URL from its own
    /// configuration.
    ///
    /// Runs all three subscriptions concurrently:
    /// 1. `zswapLedgerEvents` (seconds)
    /// 2. `unshieldedTransactions` (seconds)
    /// 3. `dustLedgerEvents` (slow, ~30 min from genesis on preprod)
    ///
    /// Returns once all three are caught up. Checkpoints dust progress to
    /// disk periodically so interrupted syncs resume where they left off.
    #[doc(hidden)]
    pub async fn sync_inner(
        indexer_url: &str,
        seed: WalletSeed,
        address: &str,
        network: impl Into<crate::Network>,
        storage_dir: Option<&Path>,
        progress: Option<mpsc::Sender<SyncProgress>>,
    ) -> Result<Self, WalletError> {
        let network = network.into();
        let network_id: &str = network.as_str();
        info!("loading cached state from disk");
        let cached = match storage_dir {
            Some(dir) => crate::storage::load(dir, network_id, &seed)?,
            None => None,
        };
        let resuming = cached.is_some();

        if resuming {
            let c = cached.as_ref().unwrap();
            info!(
                zswap_event_id = c.zswap_event_id,
                dust_event_id = c.dust_event_id,
                "resuming from cached state"
            );
            send_progress(
                &progress,
                SyncProgress::Resuming {
                    zswap_event_id: c.zswap_event_id,
                    dust_event_id: c.dust_event_id,
                },
            );
        }

        let shielded = ShieldedWallet::<DefaultDB>::default(seed.clone());
        let secret_keys = shielded.secret_keys().clone();

        info!("fetching latest block from indexer");
        let indexer_client = midnight_indexer_client::IndexerClient::new(indexer_url)?;
        let block = indexer_client
            .get_block(None)
            .await
            .map_err(|e| WalletError::Sync(format!("fetch latest block: {e}")))?
            .ok_or_else(|| WalletError::Sync("no blocks available from indexer".into()))?;

        let parameters = decode_ledger_parameters(&block)?;

        let block_timestamp = block
            .timestamp
            .map(|ms| Timestamp::from_secs((ms / 1000) as u64))
            .ok_or_else(|| WalletError::Sync("latest block has no timestamp".into()))?;

        let network_id = network_id.to_string();
        let sub_client = SubscriptionClient::new(indexer_url);

        // Extract starting state from cache or defaults.
        // When resuming, start from the next event after the last applied one
        // (the subscription is inclusive, so start_id itself would be re-delivered).
        let (initial_zswap, start_zswap_id) = match &cached {
            Some(c) => (c.zswap_state.clone(), c.zswap_event_id + 1),
            None => (shielded.state.clone(), 0),
        };
        let (initial_utxos, start_tx_id) = match &cached {
            Some(c) => (
                c.unshielded_utxos.clone(),
                c.last_tx_id.map(|id| id + 1).unwrap_or(0),
            ),
            None => (Vec::new(), 0),
        };

        let (dust_wallet, start_dust_id) = if let Some(ref c) = cached {
            (c.dust_wallet.clone(), c.dust_event_id + 1)
        } else {
            (DustWallet::default(seed.clone(), Some(&parameters)), 0_i64)
        };

        info!(
            start_zswap_id,
            start_tx_id, start_dust_id, "starting subscriptions"
        );

        let (zswap_result, unshielded_result) = tokio::join!(
            replay_zswap_events(
                &sub_client,
                &secret_keys,
                initial_zswap,
                start_zswap_id,
                resuming,
                progress.clone(),
            ),
            replay_unshielded_events(
                &sub_client,
                address,
                initial_utxos,
                start_tx_id,
                progress.clone(),
            ),
        );
        let (zswap_state, zswap_event_id) = zswap_result?;
        let (unshielded_utxos, last_tx_id, replay_block_height, spent_unshielded) =
            unshielded_result?;
        // The unshielded subscription only updates `last_block_height` when a
        // transaction touches our address. On a resume with no new unshielded
        // txs, replay returns 0, so we keep the persisted value as a floor.
        let cached_block_height = cached.as_ref().map(|c| c.last_block_height).unwrap_or(0);
        let last_block_height = replay_block_height.max(cached_block_height);

        let dust_checkpoint = make_dust_checkpoint(
            storage_dir,
            &network_id,
            seed.clone(),
            zswap_state.clone(),
            zswap_event_id,
            last_block_height,
            Some(last_tx_id),
            unshielded_utxos.clone(),
        );
        let dust_resuming = start_dust_id > 0;
        let (dust_wallet, dust_event_id, last_dust_block_time, dust_nullifiers) =
            replay_dust_events(
                &sub_client,
                dust_wallet,
                start_dust_id,
                dust_resuming,
                dust_checkpoint,
                progress.clone(),
            )
            .await?;

        // See `resync` for the full discussion of the anchor selection. Prefer
        // `last_dust_block_time + 1s` (race-safe) while its TTL window still
        // covers the chain's current time, falling back to `block_timestamp`
        // for devnet's hardcoded-genesis case.
        let global_ttl = parameters.global_ttl;
        let candidate = last_dust_block_time.map(|t| t + midnight_helpers::Duration::from_secs(1));
        let block_tblock = match candidate {
            Some(t) if t + global_ttl >= block_timestamp => t,
            _ => block_timestamp,
        };
        let block_context = Some(block_context_at(block_tblock));

        info!(
            zswap_event_id,
            dust_event_id,
            unshielded_utxos = unshielded_utxos.len(),
            height = last_block_height,
            resuming,
            "wallet synced"
        );

        // Load any pre-existing pending reservations from disk so they
        // survive process restarts. Confirmed-state files never carry
        // pending entries; this is a separate file.
        let pending = match storage_dir {
            Some(dir) => crate::storage::load_pending(dir, &network_id, &seed)?.unwrap_or_default(),
            None => PendingReservations::default(),
        };

        let mut state = Self {
            seed,
            secret_keys,
            network_id,
            unshielded_address: address.to_string(),
            zswap_state,
            zswap_event_id,
            dust_wallet,
            dust_event_id,
            unshielded_utxos,
            last_block_height,
            last_tx_id: Some(last_tx_id),
            parameters,
            block_context,
            pending,
            storage_dir: storage_dir.map(Path::to_path_buf),
        };

        // Reservations made before a restart whose spends this replay just
        // observed confirmed are no longer in flight; drop them so the
        // underlying UTXOs become spendable again immediately.
        state
            .pending
            .clear_confirmed(&spent_unshielded, &dust_nullifiers);

        // Any pending entry whose TTL window has elapsed against the chain's
        // current view can no longer produce a valid transaction; drop them
        // so they don't pollute subsequent build contexts.
        if let Some(ref bc) = state.block_context {
            state
                .pending
                .evict_expired(bc.tblock, state.parameters.global_ttl);
        }

        if let Some(dir) = storage_dir {
            state.save(dir)?;
        }

        Ok(state)
    }

    /// Whether the dust state has been synced (required for transaction building).
    pub fn dust_synced(&self) -> bool {
        self.dust_event_id > 0
    }

    /// Record the dust + unshielded spends of a freshly-built (and typically
    /// about-to-be-submitted) transaction so subsequent in-process builds
    /// don't re-select the same inputs.
    ///
    /// Reservations live in `Wallet::pending` until either:
    /// - event replay ([`Wallet::sync_inner`] or [`Wallet::resync`]) observes
    ///   the corresponding confirmed spends and clears them,
    /// - or their TTL window elapses (evicted at [`Wallet::build_context_inner`]
    ///   time).
    ///
    /// `reserved_at` should be the chain time (typically the same anchor used
    /// to build the transaction); TTL eviction compares against the chain's
    /// `block_context.tblock`. Confirmed-state files never persist these
    /// reservations — they live in `pending.json` only and are dropped from
    /// disk once `Wallet::pending` becomes empty.
    ///
    /// When the wallet was synced with a storage directory, the updated
    /// pending set is persisted to `pending.json` immediately so a crash
    /// between build and confirmation does not lose the reservation. The
    /// write is best-effort: a failure is logged and the in-memory
    /// reservation stands, since the transaction was already built.
    pub fn reserve_pending(
        &mut self,
        dust_batches: Vec<crate::transfer::DustSpendBatch>,
        unshielded_spends: Vec<SpentUtxoKey>,
        reserved_at: Timestamp,
    ) {
        self.pending
            .reserve(dust_batches, unshielded_spends, reserved_at);

        // Persist only the pending file: a full `save` would rewrite the
        // multi-MB confirmed-state files on every transfer. The write is
        // best-effort because erroring here would strand a transaction that
        // was already built; the in-memory reservation still protects the
        // running process, and the same disk fault will fail loudly at the
        // next resync's hard `save`. Crash-safety is degraded until then,
        // hence the error-level log.
        if let Some(dir) = self.storage_dir.as_deref() {
            if let Err(err) =
                crate::storage::save_pending(dir, &self.network_id, &self.seed, &self.pending)
            {
                error!(error = %err, "failed to persist pending reservations; reservation held in memory only");
            }
        }
    }

    /// Save the current wallet state to disk.
    ///
    /// Writes the confirmed-state files (`metadata.json`, `zswap-N.bin`,
    /// `dust_wallet-N.bin`) and the in-flight reservations to a separate
    /// `pending.json`. Confirmed and pending live in distinct files so a
    /// failed save of one does not corrupt the other. Runs automatically at
    /// the end of initial sync and after every successful [`Wallet::resync`]
    /// when a storage directory is configured; calling it manually is only
    /// needed for extra checkpoints.
    pub fn save(&self, base: &Path) -> Result<(), WalletError> {
        crate::storage::save(
            base,
            &self.network_id,
            &self.seed,
            &self.zswap_state,
            &self.dust_wallet,
            self.zswap_event_id,
            self.dust_event_id,
            self.last_block_height,
            self.last_tx_id,
            &self.unshielded_utxos,
        )?;
        crate::storage::save_pending(base, &self.network_id, &self.seed, &self.pending)
    }

    /// Build a [`LedgerContext`] from the wallet's current local state.
    ///
    /// Performs no I/O. The only mutation is TTL eviction of expired
    /// `pending` entries against `block_context.tblock` — entries whose
    /// `reserved_at + global_ttl` window has elapsed cannot produce a valid
    /// transaction and would just block the underlying UTXOs forever
    /// otherwise. The caller is responsible for keeping the wallet synced
    /// (typically via `MidnightProvider::resync_wallet`) and for refreshing
    /// [`Self::block_context`] before calling this, since the embedded
    /// `block_context.tblock` drives proof root lookup and transaction TTL.
    pub fn build_context_inner(&mut self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
        // Evict any expired pending reservations against the latest known
        // chain time. Cheap (Vec::retain on a typically tiny list) and the
        // only place that doesn't require the caller to restart the process
        // to free up UTXOs reserved by transactions that never confirmed.
        if let Some(bc) = self.block_context.as_ref() {
            self.pending
                .evict_expired(bc.tblock, self.parameters.global_ttl);
        }

        // reserve_pool must equal MAX_SUPPLY to satisfy the NIGHT balance invariant.
        let mut ledger_state = LedgerState::with_genesis_settings(
            &self.network_id,
            self.parameters.clone(),
            0,
            MAX_SUPPLY,
            0,
        )
        .map_err(|e| WalletError::Sync(format!("construct ledger state: {e:?}")))?;

        // Populate UTXO state so the transaction builder can find our UTXOs.
        let unshielded = UnshieldedWallet::default(self.seed.clone());
        let owner = unshielded.user_address;
        let utxo_ctime = self
            .block_context
            .as_ref()
            .map(|bc| Timestamp::from_secs(bc.tblock.to_secs().saturating_sub(3600)))
            .unwrap_or_else(|| Timestamp::from_secs(0));

        // Filter out UTXOs reserved by recent (still-pending) builds so the
        // selector doesn't re-pick them before the indexer confirms the
        // spend.
        let pending_unshielded: std::collections::HashSet<(String, i64)> = self
            .pending
            .unshielded_keys()
            .map(|k| (k.intent_hash.clone(), k.output_index as i64))
            .collect();

        // intent_hash + output_no are part of a UTXO's identity; falling back
        // to default values silently creates collisions between distinct UTXOs
        // and synthesizes inputs the chain will reject.
        let mut utxo_state = (*ledger_state.utxo).clone();
        for tracked in &self.unshielded_utxos {
            let key = match (&tracked.intent_hash, tracked.output_index) {
                (Some(h), Some(idx)) => Some((h.clone(), idx)),
                _ => None,
            };
            if let Some(k) = key {
                if pending_unshielded.contains(&k) {
                    continue;
                }
            }
            let utxo = tracked_to_ledger_utxo(tracked, owner)?;
            utxo_state = utxo_state.insert(utxo, UtxoMeta { ctime: utxo_ctime });
        }
        ledger_state.utxo = Sp::new(utxo_state);

        let ctx = LedgerContext {
            ledger_state: std::sync::Mutex::new(Sp::new(ledger_state)),
            wallets: std::sync::Mutex::new(std::collections::HashMap::new()),
            resolver: tokio::sync::Mutex::new(midnight_helpers::context::DEFAULT_RESOLVER.clone()),
            latest_block_context: std::sync::Mutex::new(self.block_context.clone()),
        };

        // Insert wallet with our synced state. Pending dust reservations are
        // re-applied via `mark_spent` so the fee selector skips them; they
        // live only on this LedgerContext clone — `self.dust_wallet` itself
        // retains only events confirmed by the indexer. Each pending entry
        // carries its post-spend `DustLocalState`; applying them in
        // chronological order leaves the clone's `dust_local_state` at the
        // most recent post-pending value.
        {
            let mut shielded = ShieldedWallet::<DefaultDB>::default(self.seed.clone());
            shielded.state = self.zswap_state.clone();

            // Add pending-spend nullifiers to the dust wallet's `spent_utxos`
            // set so speculative_spend skips them — but DO NOT overwrite
            // `dust_local_state` with the prior tx's post-spend tree.
            //
            // The new `DustWallet::mark_spent(spends, updated_state)` API in
            // ledger-helpers 8.1.0-rc.1 took the previous single-arg form
            // `mark_spent(spends)` and bolted on a state overwrite. If we apply
            // the speculative `updated_state`, the dust commitment tree the
            // proof witnesses against is the wallet's projected post-spend
            // tree, which has no corresponding entry in the chain's
            // `root_history` until the prior tx has been processed at the
            // chain-block level — and even then it only matches if no other
            // dust events landed in the same block. Re-passing the current
            // state makes the overwrite a no-op while still adding the
            // nullifiers, keeping the witnessed root aligned with the chain's
            // `root_history.get(ctime)` lookup at the proof's declared
            // timestamp.
            let mut dust = self.dust_wallet.clone();
            match dust.dust_local_state.clone() {
                Some(state) => {
                    for batch in self.pending.dust_batches() {
                        dust.mark_spent(&batch.spends, state.clone());
                    }
                }
                // No construction path in this crate produces `None` here
                // alongside pending batches: every `DustWallet` is built
                // with `Some(&parameters)`, so even a pre-registration
                // wallet carries `Some(empty)` state. `None` is only
                // reachable via deserialized/legacy or manually-mutated
                // state, and with nothing pending there is nothing to
                // replay. The guard below is defensive: pending dust
                // reservations with no state to apply them to would
                // silently disable double-build prevention, so refuse and
                // let the caller sync first.
                None => {
                    let pending_dust = self.pending.dust_batches().count();
                    if pending_dust > 0 {
                        return Err(WalletError::Transfer(format!(
                            "wallet has {pending_dust} pending dust reservation(s) but no dust \
                             state; wait for dust sync before building"
                        )));
                    }
                }
            }

            let wallet = ContextWallet {
                root_seed: Some(self.seed.clone()),
                shielded,
                unshielded: midnight_helpers::UnshieldedWallet::default(self.seed.clone()),
                dust,
            };

            ctx.wallets
                .lock()
                .map_err(|_| WalletError::Sync("wallets lock poisoned".into()))?
                .insert(self.seed.clone(), wallet);
        }

        Ok(Arc::new(ctx))
    }

    // -------------------------------------------------------------------------
    // Accessors
    // -------------------------------------------------------------------------

    /// Height of the latest block seen in an unshielded transaction event.
    ///
    /// This is NOT a general chain-sync cursor. It only advances when the
    /// wallet's unshielded address appears in a transaction.
    pub fn last_block_height(&self) -> i64 {
        self.last_block_height
    }

    pub fn last_tx_id(&self) -> Option<i64> {
        self.last_tx_id
    }

    pub fn zswap_event_id(&self) -> i64 {
        self.zswap_event_id
    }

    pub fn dust_event_id(&self) -> i64 {
        self.dust_event_id
    }

    pub fn seed(&self) -> &WalletSeed {
        &self.seed
    }

    pub fn secret_keys(&self) -> &SecretKeys {
        &self.secret_keys
    }

    /// The network identifier this wallet derives addresses for
    /// (e.g. `"undeployed"`, `"testnet"`). Returned as `&str` because the
    /// wallet stores the literal name from the bech32 HRP; callers that want
    /// the typed form can use `Network::from(wallet.network())`.
    pub fn network(&self) -> &str {
        &self.network_id
    }

    /// The wallet's unshielded receiving address (cached at construction).
    pub fn unshielded_address(&self) -> String {
        self.unshielded_address.clone()
    }

    /// The wallet's shielded receiving address, e.g. `mn_shield-addr_undeployed1...`.
    pub fn shielded_address(&self) -> String {
        crate::address::derive_shielded(&self.seed, self.network_id.as_str())
    }

    pub fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        &self.unshielded_utxos
    }

    pub fn parameters(&self) -> &LedgerParameters {
        &self.parameters
    }

    pub fn zswap_state(&self) -> &ZswapLocalState<DefaultDB> {
        &self.zswap_state
    }

    pub fn dust_wallet(&self) -> &DustWallet<DefaultDB> {
        &self.dust_wallet
    }

    pub fn block_context(&self) -> Option<&BlockContext> {
        self.block_context.as_ref()
    }

    /// Update the block context (called when a new block is observed).
    pub fn set_block_context(&mut self, ctx: BlockContext) {
        self.block_context = Some(ctx);
    }

    /// Update ledger parameters (e.g., after a governance change).
    pub fn set_parameters(&mut self, params: LedgerParameters) {
        // Re-initialize dust wallet with new params if needed
        if self.dust_wallet.dust_local_state.is_none() {
            self.dust_wallet = DustWallet::default(self.seed.clone(), Some(&params));
        }
        self.parameters = params;
    }

    /// Re-sync the wallet state from the indexer, resuming from current cursors.
    ///
    /// Call this after a transaction is finalized to pick up the on-chain
    /// effects (spent dust UTXOs, new coins, etc.) before building the
    /// next transaction.
    ///
    /// On a replay or fetch error, `self` is left untouched: all results are
    /// awaited and validated before any field is mutated. The chain's current
    /// block_time is fetched as part of the same operation; failure to fetch
    /// it is also fatal because `block_context.tblock` drives TTL and proof
    /// root lookup. Ledger parameters are refreshed from the same latest
    /// block, so governance changes to fees/TTL/dust rates take effect on
    /// the next build.
    ///
    /// When the wallet was synced with a storage directory, the committed
    /// state is re-persisted before returning so a crash does not lose the
    /// moved cursors or resurrect cleared reservations. Persistence is
    /// skipped when the resync changed no durable state (no cursor moved, no
    /// reservation cleared, parameters unchanged), since resyncs run before
    /// every build and a no-op must not rewrite the generation files. A
    /// persistence failure surfaces as [`WalletError::Storage`] with the
    /// in-memory state already updated.
    ///
    /// `indexer_url` is passed in by the caller (typically
    /// [`midnight_provider::MidnightProvider::resync_wallet`]) so the wallet
    /// itself stays free of network-endpoint state.
    pub async fn resync(&mut self, indexer_url: &str) -> Result<(), WalletError> {
        let sub_client = SubscriptionClient::new(indexer_url);
        let indexer_client = midnight_indexer_client::IndexerClient::new(indexer_url)?;

        let start_tx_id = self.last_tx_id.map(|id| id + 1).unwrap_or(0);

        let (dust_res, zswap_res, unshielded_res, block_res) = tokio::join!(
            replay_dust_events(
                &sub_client,
                self.dust_wallet.clone(),
                self.dust_event_id + 1,
                true,
                None::<fn(&DustWallet<DefaultDB>, i64)>,
                None,
            ),
            replay_zswap_events(
                &sub_client,
                &self.secret_keys,
                self.zswap_state.clone(),
                self.zswap_event_id + 1,
                true,
                None,
            ),
            replay_unshielded_events(
                &sub_client,
                &self.unshielded_address,
                self.unshielded_utxos.clone(),
                start_tx_id,
                None,
            ),
            indexer_client.get_block(None),
        );

        // Await every result before mutating `self`. If any task failed the
        // wallet's state stays as it was on entry.
        let (dust_wallet, dust_event_id, last_dust_block_time, dust_nullifiers) = dust_res?;
        let (zswap_state, zswap_event_id) = zswap_res?;
        let (unshielded_utxos, last_tx_id, last_block_height, spent_unshielded) = unshielded_res?;
        let block = block_res
            .map_err(|e| WalletError::Sync(format!("fetch latest block: {e}")))?
            .ok_or_else(|| WalletError::Sync("no blocks available from indexer".into()))?;
        let tblock_ms = block
            .timestamp
            .ok_or_else(|| WalletError::Sync("latest block has no timestamp".into()))?;
        let chain_tblock = Timestamp::from_secs((tblock_ms / 1000) as u64);
        let parameters = decode_ledger_parameters(&block)?;

        self.commit_resync(ResyncCommit {
            dust_wallet,
            dust_event_id,
            last_dust_block_time,
            dust_nullifiers,
            zswap_state,
            zswap_event_id,
            unshielded_utxos,
            last_tx_id,
            last_block_height,
            spent_unshielded,
            chain_tblock,
            parameters,
        })
    }

    /// Apply validated resync results to `self` and persist when (and only
    /// when) durable state changed. Factored out of [`Wallet::resync`],
    /// which performs the I/O and validation, so this sequence is
    /// unit-testable without an indexer.
    fn commit_resync(&mut self, commit: ResyncCommit) -> Result<(), WalletError> {
        let ResyncCommit {
            dust_wallet,
            dust_event_id,
            last_dust_block_time,
            dust_nullifiers,
            zswap_state,
            zswap_event_id,
            unshielded_utxos,
            last_tx_id,
            last_block_height,
            spent_unshielded,
            chain_tblock,
            parameters,
        } = commit;

        // Dirty-check inputs, captured before the assignments below
        // overwrite them. Resync runs before every transfer/contract build
        // (`MidnightProvider::resync_wallet`) and on user polling, so the
        // `save` at the end must be skipped when nothing durable moved;
        // otherwise every no-op resync rewrites the multi-MB
        // `zswap-N.bin`/`dust_wallet-N.bin` generation files. Dirty means:
        // a sync cursor advanced, the pending set changed across
        // `clear_confirmed`, or the chain's ledger parameters changed (a
        // governance move). `block_context` is recomputed on every resync
        // and is not persisted state, so it deliberately does not count.
        let cursors_advanced = dust_event_id != self.dust_event_id
            || zswap_event_id != self.zswap_event_id
            || Some(last_tx_id) != self.last_tx_id
            || last_block_height > self.last_block_height;
        let parameters_changed = parameters != self.parameters;
        let pending_before =
            self.pending.dust_batches().count() + self.pending.unshielded_keys().count();

        self.dust_wallet = dust_wallet;
        self.dust_event_id = dust_event_id;
        self.zswap_state = zswap_state;
        self.zswap_event_id = zswap_event_id;
        self.unshielded_utxos = unshielded_utxos;
        self.last_tx_id = Some(last_tx_id);
        // Only advance last_block_height if the unshielded sync actually saw a
        // newer block. Without this guard, a resume with no new unshielded txs
        // would clobber the persisted height with 0 (the default returned by
        // `replay_unshielded_events` when no events arrive).
        if last_block_height > self.last_block_height {
            self.last_block_height = last_block_height;
        }
        // Refresh parameters from the latest block so governance changes to
        // fees/TTL/dust rates take effect. Assigned before `global_ttl` is
        // read below so the anchor math uses the fresh value.
        self.parameters = parameters;

        // `block_context.tblock` drives both the proof's `DustActions.ctime`
        // and the intent's `ttl = tblock + global_ttl`. The chain checks:
        //
        //   1. `root_history.get(ctime)` matches our DustLocalState root, and
        //   2. `ttl >= chain.current_tblock` at apply time.
        //
        // Constraint (1) wants the most recent block_time we know matches the
        // chain's root: `last_dust_block_time + 1s` (root_history only changes
        // on dust events, so any time in the gap returns the entry at our
        // last seen event).
        //
        // Constraint (2) wants `tblock` close to the chain's current time. On
        // devnet, where genesis is hardcoded months before wall clock but the
        // chain runs in real time, `last_dust_block_time` from a genesis event
        // is too old: `last_dust + global_ttl` is already in the past.
        //
        // Prefer the most recent race-safe candidate that still has a valid
        // TTL window: `last_dust_block_time + 1s` if we observed new events,
        // else the previous block_context anchor (still race-safe because our
        // state hasn't changed since then). Fall back to `chain_tblock` only
        // when neither has a TTL window that covers the chain's current time;
        // that fallback accepts a small race window (a dust event indexed
        // between our replay's tip and `get_block`) but is required when chain
        // time has advanced past `candidate + global_ttl` — e.g. on devnet
        // where genesis is hardcoded months before wall clock.
        let global_ttl = self.parameters.global_ttl;
        let candidate = last_dust_block_time
            .map(|t| t + midnight_helpers::Duration::from_secs(1))
            .or_else(|| self.block_context.as_ref().map(|bc| bc.tblock));
        let tblock = match candidate {
            Some(t) if t + global_ttl >= chain_tblock => t,
            _ => chain_tblock,
        };
        self.block_context = Some(block_context_at(tblock));

        // Reservations whose spends this replay just observed confirmed are
        // no longer in flight; drop them so the underlying UTXOs become
        // spendable again immediately instead of waiting for TTL eviction.
        self.pending
            .clear_confirmed(&spent_unshielded, &dust_nullifiers);
        let pending_changed = self.pending.dust_batches().count()
            + self.pending.unshielded_keys().count()
            != pending_before;

        // Re-persist the committed state (moved cursors, refreshed
        // parameters, cleared pending set) so a crash before the next sync
        // resumes from here. Must run after `clear_confirmed` above: `save`
        // rewrites (or removes) `pending.json` from the in-memory set.
        // Skipped entirely on no-op resyncs (see the dirty-check above):
        // pre-build resyncs are frequent and must not rewrite the
        // generation files when nothing moved.
        if cursors_advanced || parameters_changed || pending_changed {
            if let Some(dir) = self.storage_dir.as_deref() {
                self.save(dir)?;
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Replay helpers
// ---------------------------------------------------------------------------

async fn replay_zswap_events(
    sub_client: &SubscriptionClient,
    secret_keys: &SecretKeys,
    initial_state: ZswapLocalState<DefaultDB>,
    start_id: i64,
    resuming: bool,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<(ZswapLocalState<DefaultDB>, i64), WalletError> {
    use midnight_indexer_client::subscription::queries::ZSWAP_LEDGER_EVENTS_SUBSCRIPTION;

    let variables = serde_json::json!({ "id": start_id });

    let mut subscription = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        sub_client.subscribe::<ZswapEventEnvelope>(ZSWAP_LEDGER_EVENTS_SUBSCRIPTION, variables),
    )
    .await
    .map_err(|_| WalletError::Sync("timeout connecting to zswapLedgerEvents".into()))?
    .map_err(|e| WalletError::Sync(format!("subscribe zswapLedgerEvents: {e}")))?;

    let mut state = initial_state;
    let mut last_id: i64 = last_applied_before(start_id);
    let mut count: u64 = 0;
    let event_timeout = if resuming {
        std::time::Duration::from_secs(10)
    } else {
        std::time::Duration::from_secs(30)
    };

    loop {
        let event = tokio::time::timeout(event_timeout, subscription.next()).await;

        match event {
            Ok(Some(Ok(envelope))) => {
                let msg = &envelope.zswap_ledger_events;

                if msg.max_id == 0 {
                    debug!("no zswap events on this chain");
                    break;
                }

                let ev = decode_event(msg, "zswap")?;
                state = state.replay_events(secret_keys, [&ev]).map_err(|e| {
                    WalletError::Sync(format!("replay zswap event id={}: {e}", msg.id))
                })?;

                last_id = msg.id;
                count += 1;

                if count % 10_000 == 0 {
                    info!(
                        count,
                        id = msg.id,
                        max_id = msg.max_id,
                        "zswap replay progress"
                    );
                    send_progress(
                        &progress,
                        SyncProgress::ZswapEvents {
                            current: msg.id,
                            max: msg.max_id,
                        },
                    );
                }

                if msg.id >= msg.max_id {
                    info!(count, last_id, "zswap replay complete");
                    send_progress(&progress, SyncProgress::ZswapComplete { events: count });
                    break;
                }
            }
            Ok(Some(Err(e))) => {
                return Err(WalletError::Sync(format!(
                    "zswap subscription error during replay: {e}"
                )));
            }
            Ok(None) => {
                if resuming && count == 0 {
                    info!(last_id, "zswap already at tip");
                    send_progress(&progress, SyncProgress::ZswapComplete { events: 0 });
                    break;
                }
                return Err(WalletError::Sync(
                    "zswap subscription ended before replay completed".into(),
                ));
            }
            Err(_) => {
                if resuming && count == 0 {
                    info!(last_id, "zswap already at tip");
                    send_progress(&progress, SyncProgress::ZswapComplete { events: 0 });
                    break;
                }
                return Err(WalletError::Sync("timeout waiting for zswap events".into()));
            }
        }
    }

    Ok((state, last_id))
}

async fn replay_dust_events(
    sub_client: &SubscriptionClient,
    mut dust_wallet: DustWallet<DefaultDB>,
    start_id: i64,
    resuming: bool,
    checkpoint: Option<impl Fn(&DustWallet<DefaultDB>, i64)>,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<
    (
        DustWallet<DefaultDB>,
        i64,
        Option<Timestamp>,
        Vec<DustNullifier>,
    ),
    WalletError,
> {
    use midnight_indexer_client::subscription::queries::DUST_LEDGER_EVENTS_SUBSCRIPTION;

    let variables = serde_json::json!({ "id": start_id });

    let mut subscription = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        sub_client.subscribe::<DustEventEnvelope>(DUST_LEDGER_EVENTS_SUBSCRIPTION, variables),
    )
    .await
    .map_err(|_| WalletError::Sync("timeout connecting to dustLedgerEvents".into()))?
    .map_err(|e| WalletError::Sync(format!("subscribe dustLedgerEvents: {e}")))?;

    let mut last_id: i64 = last_applied_before(start_id);
    let mut last_block_time: Option<Timestamp> = None;
    // Nullifiers of every DustSpendProcessed event seen during this replay,
    // surfaced to the caller so it can clear confirmed pending reservations.
    let mut spend_nullifiers: Vec<DustNullifier> = Vec::new();
    let mut count: u64 = 0;
    let mut since_checkpoint: u64 = 0;
    let event_timeout = if resuming {
        std::time::Duration::from_secs(10)
    } else {
        std::time::Duration::from_secs(30)
    };

    loop {
        let event = tokio::time::timeout(event_timeout, subscription.next()).await;

        match event {
            Ok(Some(Ok(envelope))) => {
                let msg = &envelope.dust_ledger_events;

                if msg.max_id == 0 {
                    debug!("no dust events on this chain");
                    break;
                }

                let ev = decode_event(msg, "dust")?;
                dust_wallet.replay_events([&ev]).map_err(|e| {
                    WalletError::Sync(format!("apply dust event id={}: {e}", msg.id))
                })?;

                if let Some(t) = event_block_time(&ev) {
                    last_block_time = Some(t);
                }
                if let Some(n) = event_spend_nullifier(&ev) {
                    spend_nullifiers.push(n);
                }
                last_id = msg.id;
                count += 1;
                since_checkpoint += 1;

                if count % 10_000 == 0 {
                    info!(
                        count,
                        id = msg.id,
                        max_id = msg.max_id,
                        "dust replay progress"
                    );
                    send_progress(
                        &progress,
                        SyncProgress::DustEvents {
                            current: msg.id,
                            max: msg.max_id,
                        },
                    );
                }

                if since_checkpoint >= DUST_CHECKPOINT_INTERVAL {
                    if let Some(ref save) = checkpoint {
                        save(&dust_wallet, last_id);
                    }
                    since_checkpoint = 0;
                }

                if msg.id >= msg.max_id {
                    info!(count, last_id, "dust replay complete");
                    send_progress(&progress, SyncProgress::DustComplete { events: count });
                    break;
                }
            }
            Ok(Some(Err(e))) => {
                return Err(WalletError::Sync(format!(
                    "dust subscription error during replay: {e}"
                )));
            }
            Ok(None) => {
                if resuming && count == 0 {
                    info!(last_id, "dust already at tip");
                    send_progress(&progress, SyncProgress::DustComplete { events: 0 });
                    break;
                }
                return Err(WalletError::Sync(
                    "dust subscription ended before replay completed".into(),
                ));
            }
            Err(_) => {
                if resuming && count == 0 {
                    info!(last_id, "dust already at tip");
                    send_progress(&progress, SyncProgress::DustComplete { events: 0 });
                    break;
                }
                return Err(WalletError::Sync("timeout waiting for dust events".into()));
            }
        }
    }

    Ok((dust_wallet, last_id, last_block_time, spend_nullifiers))
}

/// Extract the block_time from a dust event, if present.
fn event_block_time(event: &Event<DefaultDB>) -> Option<Timestamp> {
    match &event.content {
        EventDetails::DustInitialUtxo { block_time, .. } => Some(*block_time),
        EventDetails::DustSpendProcessed { block_time, .. } => Some(*block_time),
        EventDetails::DustGenerationDtimeUpdate { block_time, .. } => Some(*block_time),
        _ => None,
    }
}

/// Extract the spend nullifier from a dust event, if it is a processed
/// spend. Used to clear matching `PendingReservations` dust batches once
/// the chain confirms them.
fn event_spend_nullifier(event: &Event<DefaultDB>) -> Option<DustNullifier> {
    match &event.content {
        EventDetails::DustSpendProcessed { nullifier, .. } => Some(*nullifier),
        _ => None,
    }
}

async fn replay_unshielded_events(
    sub_client: &SubscriptionClient,
    address: &str,
    initial_utxos: Vec<TrackedUtxo>,
    start_tx_id: i64,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<(Vec<TrackedUtxo>, i64, i64, Vec<SpentUtxoKey>), WalletError> {
    use midnight_indexer_client::subscription::queries::UNSHIELDED_TRANSACTIONS_SUBSCRIPTION;

    let variables = serde_json::json!({
        "address": address,
        "transactionId": start_tx_id,
    });

    let mut subscription = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        sub_client.subscribe::<UnshieldedTxEvent>(UNSHIELDED_TRANSACTIONS_SUBSCRIPTION, variables),
    )
    .await
    .map_err(|_| WalletError::Sync("timeout connecting to unshieldedTransactions".into()))?
    .map_err(|e| WalletError::Sync(format!("subscribe unshieldedTransactions: {e}")))?;

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
    // before returning.
    let mut target_tx_id: Option<i64> = None;

    loop {
        let event =
            tokio::time::timeout(std::time::Duration::from_secs(30), subscription.next()).await;

        match event {
            Ok(Some(Ok(ev))) => match ev.unshielded_transactions {
                UnshieldedTxPayload::UnshieldedTransaction(tx_data) => {
                    let created = tx_data.created_utxos.len();
                    let spent = tx_data.spent_utxos.len();
                    let tx_id = tx_data.transaction.as_ref().and_then(|t| t.id);
                    debug!(tx_id, created, spent, "unshielded tx event");
                    apply_unshielded_tx(&mut utxos, &tx_data)?;
                    spent_keys.extend(spent_utxo_keys(&tx_data));
                    if let Some(id) = tx_id {
                        last_seen_tx_id = last_seen_tx_id.max(id);
                    }
                    if let Some(ref tx_ref) = tx_data.transaction {
                        if let Some(ref block) = tx_ref.block {
                            last_height = last_height.max(block.height);
                        }
                    }
                    if let Some(target) = target_tx_id {
                        if last_seen_tx_id >= target {
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
                }
                UnshieldedTxPayload::UnshieldedTransactionsProgress(prog) => {
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
                        return Ok((utxos, last_seen_tx_id.max(target), last_height, spent_keys));
                    }
                    target_tx_id = Some(target);
                }
            },
            Ok(Some(Err(e))) => {
                return Err(WalletError::Sync(format!(
                    "unshielded subscription error during sync: {e}"
                )));
            }
            Ok(None) => {
                return Err(WalletError::Sync(
                    "unshielded subscription ended before sync completed".into(),
                ));
            }
            Err(_) => {
                return Err(WalletError::Sync(
                    "timeout waiting for unshielded sync".into(),
                ));
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
    let value: u128 = u
        .value
        .parse()
        .map_err(|e| WalletError::Sync(format!("failed to parse UTXO value '{}': {e}", u.value)))?;
    Ok(TrackedUtxo {
        owner: u.owner.clone(),
        token_type: u.token_type.clone(),
        value,
        intent_hash: u.intent_hash.clone(),
        output_index: u.output_index,
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

fn apply_unshielded_tx(
    utxos: &mut Vec<TrackedUtxo>,
    tx_data: &UnshieldedTxData,
) -> Result<(), WalletError> {
    // Parse everything upfront. If any value fails to parse the UTXO vec is
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

fn send_progress(tx: &Option<mpsc::Sender<SyncProgress>>, msg: SyncProgress) {
    if let Some(tx) = tx {
        let _ = tx.try_send(msg);
    }
}

/// Decode a hex string into a 32-byte array. Returns `None` on hex decode
/// error or wrong length. Used to build typed hash wrappers
/// (`IntentHash`, `UnshieldedTokenType`, ...).
fn parse_hex_32(hex: &str) -> Option<[u8; 32]> {
    hex::decode(hex).ok()?.try_into().ok()
}

pub(crate) fn parse_intent_hash_hex(hex: &str) -> Option<IntentHash> {
    parse_hex_32(hex).map(|arr| IntentHash(HashOutput(arr)))
}

fn parse_token_type_hex(hex: &str) -> Option<UnshieldedTokenType> {
    parse_hex_32(hex).map(|arr| UnshieldedTokenType(HashOutput(arr)))
}

fn tracked_to_ledger_utxo(
    tracked: &TrackedUtxo,
    owner: midnight_helpers::UserAddress,
) -> Result<LedgerUtxo, WalletError> {
    let type_ = parse_token_type_hex(&tracked.token_type).ok_or_else(|| {
        WalletError::Sync(format!(
            "tracked UTXO has malformed token_type {}",
            tracked.token_type
        ))
    })?;
    let intent_hash_hex = tracked
        .intent_hash
        .as_deref()
        .ok_or_else(|| WalletError::Sync("tracked UTXO has no intent_hash".into()))?;
    let intent_hash = parse_intent_hash_hex(intent_hash_hex).ok_or_else(|| {
        WalletError::Sync(format!(
            "tracked UTXO has malformed intent_hash {intent_hash_hex}"
        ))
    })?;
    let idx = tracked
        .output_index
        .ok_or_else(|| WalletError::Sync("tracked UTXO has no output_index".into()))?;
    let output_no = u32::try_from(idx)
        .map_err(|_| WalletError::Sync(format!("tracked UTXO output_index {idx} out of range")))?;
    Ok(LedgerUtxo {
        value: tracked.value,
        owner,
        type_,
        intent_hash,
        output_no,
    })
}

#[cfg(test)]
mod tests {
    use midnight_helpers::midnight_serialize::tagged_serialize;
    use midnight_helpers::mn_ledger::dust::DustCommitment;
    use midnight_helpers::mn_ledger::events::EventSource;
    use midnight_helpers::{
        DustLocalState, DustNullifier, DustSpend, Fr, INITIAL_PARAMETERS, KeyLocation, Nullifier,
        ProofPreimage, ProofPreimageMarker, TransactionHash,
    };

    use super::*;
    use crate::transfer::DustSpendBatch;

    #[test]
    fn last_applied_before_does_not_advance_to_unapplied_event() {
        assert_eq!(last_applied_before(0), 0);
        assert_eq!(last_applied_before(1), 0);
        assert_eq!(last_applied_before(42), 41);
        assert_eq!(last_applied_before(-1), 0);
    }

    fn sub_utxo(intent_hash: Option<&str>, output_index: Option<i64>) -> SubscriptionUtxo {
        SubscriptionUtxo {
            owner: "owner".into(),
            token_type: "00".repeat(32),
            value: "1".into(),
            intent_hash: intent_hash.map(str::to_string),
            output_index,
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

    fn dust_event(content: EventDetails<DefaultDB>) -> Event<DefaultDB> {
        Event {
            source: EventSource {
                transaction_hash: TransactionHash(HashOutput([0u8; 32])),
                logical_segment: 0,
                physical_segment: 0,
            },
            content,
        }
    }

    #[test]
    fn event_spend_nullifier_matches_dust_spend_processed_only() {
        let nullifier = DustNullifier(Fr::from(7u64));
        let spend = dust_event(EventDetails::DustSpendProcessed {
            commitment: DustCommitment(Fr::from(8u64)),
            commitment_index: 0,
            nullifier,
            v_fee: 1,
            declared_time: Timestamp::from_secs(0),
            block_time: Timestamp::from_secs(0),
        });
        assert_eq!(event_spend_nullifier(&spend), Some(nullifier));

        let other = dust_event(EventDetails::ZswapInput {
            nullifier: Nullifier(HashOutput([1u8; 32])),
            contract: None,
        });
        assert_eq!(event_spend_nullifier(&other), None);
    }

    /// Minimal offline wallet for unit tests: fresh state, no sync.
    fn test_wallet(storage_dir: Option<PathBuf>) -> Wallet {
        let seed = WalletSeed::try_from_hex_str(&"22".repeat(32)).unwrap();
        let shielded = ShieldedWallet::<DefaultDB>::default(seed.clone());
        let secret_keys = shielded.secret_keys().clone();
        Wallet {
            seed: seed.clone(),
            secret_keys,
            network_id: "undeployed".into(),
            unshielded_address: "mn_addr_undeployed1test".into(),
            zswap_state: shielded.state.clone(),
            zswap_event_id: 0,
            dust_wallet: DustWallet::default(seed, Some(&INITIAL_PARAMETERS)),
            dust_event_id: 0,
            unshielded_utxos: Vec::new(),
            last_block_height: 0,
            last_tx_id: None,
            parameters: INITIAL_PARAMETERS,
            block_context: None,
            pending: PendingReservations::default(),
            storage_dir,
        }
    }

    /// A structurally-valid `DustSpend` whose identity is `DustNullifier(n)`.
    /// The proof is a placeholder preimage — the pending-replay paths only
    /// look at `old_nullifier`.
    fn dust_spend(n: u64) -> DustSpend<ProofPreimageMarker, DefaultDB> {
        DustSpend {
            v_fee: 1,
            old_nullifier: DustNullifier(Fr::from(n)),
            new_commitment: DustCommitment(Fr::from(n + 1)),
            proof: ProofPreimage {
                inputs: Vec::new(),
                private_transcript: Vec::new(),
                public_transcript_inputs: Vec::new(),
                public_transcript_outputs: Vec::new(),
                binding_input: Fr::from(0u64),
                communications_commitment: None,
                key_location: KeyLocation(std::borrow::Cow::Borrowed("test")),
            },
        }
    }

    fn dust_batch(nullifiers: &[u64]) -> DustSpendBatch {
        DustSpendBatch {
            seed: WalletSeed::try_from_hex_str(&"22".repeat(32)).unwrap(),
            spends: nullifiers.iter().map(|&n| dust_spend(n)).collect(),
            updated_state: Sp::new(DustLocalState::new(INITIAL_PARAMETERS.dust)),
        }
    }

    fn block_with_params(ledger_parameters: Option<String>) -> midnight_indexer_client::Block {
        midnight_indexer_client::Block {
            hash: "00".repeat(32),
            height: 1,
            protocol_version: None,
            timestamp: Some(1_000),
            author: None,
            transactions: None,
            ledger_parameters,
        }
    }

    #[test]
    fn decode_ledger_parameters_round_trips_block_parameters() {
        let mut encoded = Vec::new();
        tagged_serialize(&INITIAL_PARAMETERS, &mut encoded).unwrap();
        let block = block_with_params(Some(hex::encode(&encoded)));

        let decoded = decode_ledger_parameters(&block).unwrap();

        let mut reencoded = Vec::new();
        tagged_serialize(&decoded, &mut reencoded).unwrap();
        assert_eq!(reencoded, encoded);
    }

    #[test]
    fn decode_ledger_parameters_rejects_missing_or_malformed() {
        assert!(matches!(
            decode_ledger_parameters(&block_with_params(None)),
            Err(WalletError::Sync(_))
        ));
        assert!(matches!(
            decode_ledger_parameters(&block_with_params(Some("zz".into()))),
            Err(WalletError::Sync(_))
        ));
    }

    #[test]
    fn build_context_refuses_pending_dust_without_dust_state() {
        let mut wallet = test_wallet(None);
        wallet.dust_wallet.dust_local_state = None;
        wallet.pending.reserve(
            vec![dust_batch(&[7])],
            Vec::new(),
            Timestamp::from_secs(100),
        );

        let err = match wallet.build_context_inner() {
            Err(e) => e,
            Ok(_) => panic!("expected build_context_inner to refuse"),
        };
        assert!(matches!(err, WalletError::Transfer(_)));
        assert!(err.to_string().contains("pending dust reservation"));
    }

    #[test]
    fn build_context_allows_missing_dust_state_with_no_pending_dust() {
        // The register-dust bootstrap: no dust state yet, nothing pending.
        let mut wallet = test_wallet(None);
        wallet.dust_wallet.dust_local_state = None;
        assert!(wallet.build_context_inner().is_ok());
    }

    #[test]
    fn build_context_replays_pending_dust_when_state_present() {
        let mut wallet = test_wallet(None);
        wallet.pending.reserve(
            vec![dust_batch(&[7])],
            Vec::new(),
            Timestamp::from_secs(100),
        );
        assert!(wallet.build_context_inner().is_ok());
    }

    #[test]
    fn reserve_pending_persists_pending_file_when_storage_dir_set() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.reserve_pending(
            Vec::new(),
            vec![SpentUtxoKey {
                intent_hash: "abcd".into(),
                output_index: 0,
            }],
            Timestamp::from_secs(100),
        );

        let loaded = crate::storage::load_pending(dir.path(), "undeployed", &wallet.seed)
            .unwrap()
            .expect("pending.json should exist after reserve_pending");
        assert_eq!(loaded.unshielded_keys().count(), 1);
    }

    #[test]
    fn save_after_clearance_removes_stale_pending_file() {
        // Seam for the resync commit path: reserve (file written), then
        // clear confirmed and `save` — the file must go away so disk stays
        // consistent with the cleared in-memory set.
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        let key = SpentUtxoKey {
            intent_hash: "abcd".into(),
            output_index: 0,
        };
        wallet.reserve_pending(Vec::new(), vec![key.clone()], Timestamp::from_secs(100));

        wallet.pending.clear_confirmed(&[key], &[]);
        wallet.save(dir.path()).unwrap();

        assert!(
            crate::storage::load_pending(dir.path(), "undeployed", &wallet.seed)
                .unwrap()
                .is_none()
        );
    }

    /// Storage generations present on disk, identified by the `zswap-N.bin`
    /// files under `base` (recursively, since the per-wallet directory name
    /// is a seed digest). A no-op resync must leave this unchanged; a dirty
    /// one bumps it.
    fn stored_generations(base: &Path) -> Vec<u64> {
        fn walk(dir: &Path, out: &mut Vec<u64>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if let Some(generation) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("zswap-"))
                    .and_then(|n| n.strip_suffix(".bin"))
                    .and_then(|n| n.parse().ok())
                {
                    out.push(generation);
                }
            }
        }
        let mut out = Vec::new();
        walk(base, &mut out);
        out.sort_unstable();
        out
    }

    /// A `ResyncCommit` carrying exactly the wallet's current durable state:
    /// the shape of a resync that found nothing new on chain.
    fn noop_commit(wallet: &Wallet) -> ResyncCommit {
        ResyncCommit {
            dust_wallet: wallet.dust_wallet.clone(),
            dust_event_id: wallet.dust_event_id,
            last_dust_block_time: None,
            dust_nullifiers: Vec::new(),
            zswap_state: wallet.zswap_state.clone(),
            zswap_event_id: wallet.zswap_event_id,
            unshielded_utxos: wallet.unshielded_utxos.clone(),
            last_tx_id: wallet.last_tx_id.unwrap_or(0),
            last_block_height: 0,
            spent_unshielded: Vec::new(),
            chain_tblock: Timestamp::from_secs(1_000),
            parameters: wallet.parameters.clone(),
        }
    }

    #[test]
    fn noop_resync_commit_skips_persistence() {
        // Seam for the resync commit path: resync runs before every build,
        // so a commit that changes no durable state must not rewrite the
        // generation files, even though it refreshes `block_context`.
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.last_tx_id = Some(3);
        wallet.save(dir.path()).unwrap();
        assert_eq!(stored_generations(dir.path()), vec![1]);

        let commit = noop_commit(&wallet);
        wallet.commit_resync(commit).unwrap();

        assert_eq!(stored_generations(dir.path()), vec![1]);
        // The non-durable block context was still refreshed.
        assert!(wallet.block_context.is_some());
    }

    #[test]
    fn resync_commit_persists_when_cursor_advances() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.last_tx_id = Some(3);
        wallet.save(dir.path()).unwrap();

        let mut commit = noop_commit(&wallet);
        commit.dust_event_id += 1;
        wallet.commit_resync(commit).unwrap();

        assert_eq!(wallet.dust_event_id, 1);
        assert_eq!(stored_generations(dir.path()), vec![2]);
    }

    #[test]
    fn resync_commit_persists_when_parameters_change() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.last_tx_id = Some(3);
        wallet.save(dir.path()).unwrap();

        let mut commit = noop_commit(&wallet);
        commit
            .parameters
            .cardano_to_midnight_bridge_fee_basis_points += 1;
        wallet.commit_resync(commit).unwrap();

        assert_eq!(stored_generations(dir.path()), vec![2]);
    }

    #[test]
    fn resync_commit_persists_when_reservation_cleared() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.last_tx_id = Some(3);
        wallet.save(dir.path()).unwrap();
        let key = SpentUtxoKey {
            intent_hash: "abcd".into(),
            output_index: 0,
        };
        wallet.reserve_pending(Vec::new(), vec![key.clone()], Timestamp::from_secs(100));

        let mut commit = noop_commit(&wallet);
        commit.spent_unshielded = vec![key];
        wallet.commit_resync(commit).unwrap();

        assert!(wallet.pending.is_empty());
        assert_eq!(stored_generations(dir.path()), vec![2]);
        assert!(
            crate::storage::load_pending(dir.path(), "undeployed", &wallet.seed)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn reserve_pending_keeps_reservation_when_persistence_fails() {
        // `storage_dir` points at a regular file, so `save_pending` cannot
        // create the wallet directory and the disk write fails. The write
        // is best-effort: no panic, and the in-memory reservation must
        // still gate `build_context_inner`.
        let dir = tempfile::TempDir::new().unwrap();
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"occupied").unwrap();

        let mut wallet = test_wallet(Some(blocker));
        wallet.dust_wallet.dust_local_state = None;
        wallet.reserve_pending(
            vec![dust_batch(&[7])],
            vec![SpentUtxoKey {
                intent_hash: "abcd".into(),
                output_index: 0,
            }],
            Timestamp::from_secs(100),
        );

        assert_eq!(wallet.pending.unshielded_keys().count(), 1);
        assert_eq!(wallet.pending.dust_batches().count(), 1);
        // With no dust state to replay the pending batch against, the
        // surviving reservation still refuses the build.
        assert!(matches!(
            wallet.build_context_inner(),
            Err(WalletError::Transfer(_))
        ));
    }

}
