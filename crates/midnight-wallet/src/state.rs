use std::path::{Path, PathBuf};
use std::sync::Arc;

use midnight_indexer_client::SubscriptionClient;
use midnight_node_ledger_helpers::midnight_serialize::tagged_deserialize;
use midnight_node_ledger_helpers::mn_ledger::dust::DustState;
use midnight_node_ledger_helpers::mn_ledger::events::EventDetails;
use midnight_node_ledger_helpers::mn_ledger::semantics::ZswapLocalStateExt;
use midnight_node_ledger_helpers::mn_ledger::structure::{Utxo as LedgerUtxo, UtxoMeta};
use midnight_node_ledger_helpers::{
    BlockContext, DefaultDB, DustWallet, Event, HashOutput, IntentHash, LedgerContext,
    LedgerParameters, LedgerState, MAX_SUPPLY, NIGHT, SecretKeys, ShieldedWallet, Sp, Timestamp,
    UnshieldedTokenType, UnshieldedWallet, Wallet as ContextWallet, WalletSeed,
    WalletState as ZswapLocalState,
};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::WalletError;

/// Progress updates emitted during wallet sync.
#[derive(Debug, Clone)]
pub enum SyncProgress {
    Resuming {
        zswap_event_id: i64,
        dust_event_id: i64,
    },
    ZswapEvents { current: i64, max: i64 },
    ZswapComplete { events: u64 },
    DustEvents { current: i64, max: i64 },
    DustComplete { events: u64 },
    UnshieldedCaughtUp { utxos: usize },
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

/// Wallet state backed by the Midnight indexer for both balance tracking
/// and transaction building.
///
/// Maintains three streams of state from the indexer:
/// - `zswapLedgerEvents` → shielded coin tracking + Merkle tree
/// - `dustLedgerEvents` → dust/fee UTXO tracking
/// - `unshieldedTransactions` → unshielded UTXO balance
///
/// Transaction building uses the local state directly (no full-chain-replay).
pub struct WalletState {
    seed: WalletSeed,
    secret_keys: SecretKeys,
    node_url: String,
    indexer_url: String,
    network_id: String,
    unshielded_address: String,

    // Shielded state (from zswapLedgerEvents)
    zswap_state: ZswapLocalState<DefaultDB>,
    zswap_event_id: i64,

    // Dust state (from dustLedgerEvents)
    dust_wallet: DustWallet<DefaultDB>,
    dust_event_id: i64,
    // Global dust state for transaction validation (tracks Merkle roots)
    dust_global_state: DustState<DefaultDB>,

    // Unshielded UTXOs (from unshieldedTransactions)
    unshielded_utxos: Vec<TrackedUtxo>,
    last_block_height: i64,
    last_tx_id: Option<i64>,

    // Chain parameters (from latest block via indexer HTTP)
    parameters: LedgerParameters,
    block_context: Option<BlockContext>,
}

// ---------------------------------------------------------------------------
// Subscription event types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEventMessage {
    pub id: i64,
    pub raw: String,
    pub max_id: i64,
}

/// Response type for zswapLedgerEvents subscription.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZswapEventEnvelope {
    pub zswap_ledger_events: LedgerEventMessage,
}

/// Response type for dustLedgerEvents subscription.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DustEventEnvelope {
    pub dust_ledger_events: LedgerEventMessage,
}

/// Response type for unshielded transaction subscription events.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnshieldedTxEvent {
    pub unshielded_transactions: UnshieldedTxPayload,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "__typename")]
pub enum UnshieldedTxPayload {
    UnshieldedTransaction(UnshieldedTxData),
    UnshieldedTransactionsProgress(UnshieldedTxProgress),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnshieldedTxData {
    pub transaction: Option<UnshieldedTxRef>,
    #[serde(default)]
    pub created_utxos: Vec<SubscriptionUtxo>,
    #[serde(default)]
    pub spent_utxos: Vec<SubscriptionUtxo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnshieldedTxRef {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub block: Option<SubscriptionBlock>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionBlock {
    pub height: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUtxo {
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
pub struct UnshieldedTxProgress {
    pub highest_transaction_id: i64,
}

// ---------------------------------------------------------------------------
// WalletState implementation
// ---------------------------------------------------------------------------

/// Number of dust events between checkpoint saves during initial sync.
const DUST_CHECKPOINT_INTERVAL: u64 = 50_000;

type DustCheckpointFn = dyn Fn(&DustWallet<DefaultDB>, &DustState<DefaultDB>, i64) + Send;

#[derive(Debug)]
enum DustReplayError {
    Sync(WalletError),
    CachedState { event_id: i64, reason: String },
}

impl DustReplayError {
    fn into_wallet_error(self) -> WalletError {
        match self {
            Self::Sync(err) => err,
            Self::CachedState { event_id, reason } => {
                WalletError::Sync(format!("apply dust event id={event_id}: {reason}"))
            }
        }
    }
}

impl From<WalletError> for DustReplayError {
    fn from(err: WalletError) -> Self {
        Self::Sync(err)
    }
}

#[allow(clippy::too_many_arguments)]
fn make_dust_checkpoint(
    storage_dir: Option<&Path>,
    network_id: &str,
    seed: WalletSeed,
    zswap_state: ZswapLocalState<DefaultDB>,
    parameters: LedgerParameters,
    zswap_event_id: i64,
    last_block_height: i64,
    last_tx_id: Option<i64>,
    unshielded_utxos: Vec<TrackedUtxo>,
) -> Option<Box<DustCheckpointFn>> {
    storage_dir.map(|dir| {
        let dir = dir.to_path_buf();
        let net = network_id.to_string();
        Box::new(
            move |dw: &DustWallet<DefaultDB>, dg: &DustState<DefaultDB>, dust_eid: i64| {
                if let Err(err) = crate::storage::save(
                    &dir,
                    &net,
                    &seed,
                    &zswap_state,
                    dw,
                    dg,
                    &parameters,
                    &None,
                    zswap_event_id,
                    dust_eid,
                    last_block_height,
                    last_tx_id,
                    &unshielded_utxos,
                ) {
                    warn!(error = %err, "failed to checkpoint dust state");
                }
            },
        ) as Box<DustCheckpointFn>
    })
}

fn last_applied_before(start_id: i64) -> i64 {
    start_id.saturating_sub(1).max(0)
}

impl WalletState {
    /// Default storage directory: `~/.midnight/wallets/`
    pub fn default_storage_dir() -> Option<PathBuf> {
        home_dir().map(|h| h.join(".midnight").join("wallets"))
    }

    /// Sync wallet state from the indexer, resuming from disk if available.
    ///
    /// Runs all three subscriptions concurrently:
    /// 1. `zswapLedgerEvents` (seconds)
    /// 2. `unshieldedTransactions` (seconds)
    /// 3. `dustLedgerEvents` (slow, ~30 min from genesis on preprod)
    ///
    /// Returns once all three are caught up. Checkpoints dust progress to
    /// disk periodically so interrupted syncs resume where they left off.
    pub async fn sync(
        node_url: &str,
        indexer_url: &str,
        seed: WalletSeed,
        address: &str,
        network_id: &str,
        storage_dir: Option<&Path>,
    ) -> Result<Self, WalletError> {
        Self::sync_inner(
            node_url,
            indexer_url,
            seed,
            address,
            network_id,
            storage_dir,
            None,
        )
        .await
    }

    /// Like [`sync`](Self::sync), but returns a channel receiver that emits
    /// [`SyncProgress`] updates as each subscription replays events.
    ///
    /// The channel has a bounded buffer of 64 messages. If the receiver falls
    /// behind, progress updates are dropped (sync continues unaffected).
    pub async fn sync_with_progress(
        node_url: &str,
        indexer_url: &str,
        seed: WalletSeed,
        address: &str,
        network_id: &str,
        storage_dir: Option<&Path>,
    ) -> (
        mpsc::Receiver<SyncProgress>,
        tokio::task::JoinHandle<Result<Self, WalletError>>,
    ) {
        let (tx, rx) = mpsc::channel(64);
        let node_url = node_url.to_string();
        let indexer_url = indexer_url.to_string();
        let address = address.to_string();
        let network_id = network_id.to_string();
        let storage_dir = storage_dir.map(|p| p.to_path_buf());
        let handle = tokio::spawn(async move {
            Self::sync_inner(
                &node_url,
                &indexer_url,
                seed,
                &address,
                &network_id,
                storage_dir.as_deref(),
                Some(tx),
            )
            .await
        });
        (rx, handle)
    }

    async fn sync_inner(
        node_url: &str,
        indexer_url: &str,
        seed: WalletSeed,
        address: &str,
        network_id: &str,
        storage_dir: Option<&Path>,
        progress: Option<mpsc::Sender<SyncProgress>>,
    ) -> Result<Self, WalletError> {
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

        let shielded = ShieldedWallet::<DefaultDB>::default(seed);
        let secret_keys = shielded.secret_keys().clone();

        info!("fetching latest block from indexer");
        let indexer_client = midnight_indexer_client::IndexerClient::new(indexer_url)
            .map_err(|e| WalletError::Sync(format!("indexer client: {e}")))?;
        let block = indexer_client
            .get_block(None)
            .await
            .map_err(|e| WalletError::Sync(format!("fetch latest block: {e}")))?
            .ok_or_else(|| WalletError::Sync("no blocks available from indexer".into()))?;

        let params_hex = block
            .ledger_parameters
            .as_deref()
            .ok_or_else(|| WalletError::Sync("latest block has no ledger_parameters".into()))?;
        let params_bytes = hex::decode(params_hex)
            .map_err(|e| WalletError::Sync(format!("decode ledger params hex: {e}")))?;
        let parameters: LedgerParameters = tagged_deserialize(&params_bytes[..])
            .map_err(|e| WalletError::Sync(format!("deserialize ledger params: {e}")))?;

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

        // Dust strategy (matches JS SDK / Lace wallet):
        // - Fresh sync (no cache): replay all dust events from genesis to build
        //   the DustWallet (local/collapsed).
        // - Resume (cache present): load DustWallet from cache and replay only
        //   new events since the cached cursor. This keeps DustWallet's
        //   spent_utxos and dust_utxos current so we never try to spend a UTXO
        //   the chain has already consumed.
        //
        // We do NOT maintain a full global DustState (which would be 55MB+ and
        // slow to load). The chain validates transactions against its own
        // root_history; we just provide the DustLocalState's current roots in
        // the proof and let the chain check them. The client-side `validate()`
        // call is bypassed in `build_no_validate()` for this reason.
        let (dust_wallet, start_dust_id) = if let Some(ref c) = cached {
            let cached_dust_id = c.dust_event_id;
            let dw = c.dust_wallet.clone();
            info!(
                dust_event_id = cached_dust_id,
                "resuming dust subscription"
            );
            (dw, cached_dust_id + 1)
        } else {
            let dw = DustWallet::default(seed, Some(&parameters));
            (dw, 0_i64)
        };
        // dust_global_state is no longer maintained as a persisted full tree.
        // Construct an empty DustState; client-side validate() is bypassed.
        let dust_global_state = DustState::<DefaultDB>::default();

        info!(
            start_zswap_id,
            start_tx_id,
            start_dust_id,
            "starting subscriptions"
        );

        // Run zswap + unshielded in parallel. Only run dust if we need a fresh replay.
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
        let (unshielded_utxos, last_tx_id, last_block_height) = unshielded_result?;

        let dust_checkpoint = make_dust_checkpoint(
            storage_dir,
            &network_id,
            seed,
            zswap_state.clone(),
            parameters.clone(),
            zswap_event_id,
            last_block_height,
            Some(last_tx_id),
            unshielded_utxos.clone(),
        );
        let dust_resuming = start_dust_id > 0;
        let (dust_wallet, dust_global_state, dust_event_id, last_dust_block_time) =
            match replay_dust_events(
                &sub_client,
                dust_wallet,
                dust_global_state,
                start_dust_id,
                dust_resuming,
                dust_checkpoint,
                progress.clone(),
            )
            .await
            {
                Ok(dust) => dust,
                Err(err) => return Err(err.into_wallet_error()),
            };

        // Use the block_time of the LAST dust event we processed as the timestamp
        // for `block_context.tblock`. The transaction's DustActions.ctime is set
        // from this; the chain validates that ctime is within the validity window
        // (ctime <= chain_tblock <= ctime + grace_period) and checks proof roots
        // against its own root_history.get(ctime). Since we processed all events
        // up to this block_time, the chain's root_history at that exact timestamp
        // matches our DustLocalState's current root.
        //
        // If there were no new dust events, use the cached timestamp; on first
        // sync with no events, fall back to the latest block timestamp.
        let cached_dust_timestamp = cached.as_ref().and_then(|c| c.dust_roots.as_ref()).map(|r| r.timestamp);
        let root_history_timestamp = last_dust_block_time
            .or(cached_dust_timestamp)
            .unwrap_or(block_timestamp);

        let block_context = Some(BlockContext {
            tblock: root_history_timestamp,
            tblock_err: 30,
            parent_block_hash: Default::default(),
            last_block_time: root_history_timestamp,
        });

        info!(
            zswap_event_id,
            dust_event_id,
            unshielded_utxos = unshielded_utxos.len(),
            height = last_block_height,
            resuming,
            "wallet synced"
        );

        let state = Self {
            seed,
            secret_keys,
            node_url: node_url.to_string(),
            indexer_url: indexer_url.to_string(),
            network_id,
            unshielded_address: address.to_string(),
            zswap_state,
            zswap_event_id,
            dust_wallet,
            dust_event_id,
            dust_global_state,
            unshielded_utxos,
            last_block_height,
            last_tx_id: Some(last_tx_id),
            parameters,
            block_context,
        };

        if let Some(dir) = storage_dir {
            state.save(dir)?;
        }

        Ok(state)
    }

    /// Whether the dust state has been synced (required for transaction building).
    pub fn dust_synced(&self) -> bool {
        self.dust_event_id > 0
    }

    /// Extract the mutated DustWallet from a LedgerContext back into this state.
    ///
    /// After the helpers' `StandardTrasactionInfo::prove()` builds a transaction,
    /// it calls `confirm_dust_spends()` which mutates the DustWallet inside the
    /// context's wallets map (adds spent nullifiers to `spent_utxos`). That
    /// mutation is on a CLONE of our `dust_wallet` (build_context_inner clones
    /// when inserting into the context). To prevent picking the same dust UTXO
    /// in subsequent transactions, we copy the context's mutated DustWallet
    /// back into this state.
    pub fn sync_dust_from_context(&mut self, context: &LedgerContext<DefaultDB>) {
        if let Ok(wallets) = context.wallets.lock() {
            if let Some(wallet) = wallets.get(&self.seed) {
                self.dust_wallet = wallet.dust.clone();
            }
        }
    }

    /// Remove unshielded UTXOs that were spent by a recently-built transaction.
    ///
    /// The indexer typically takes a few seconds to publish events confirming a
    /// transaction's UTXO consumption. Without this call, the next transfer
    /// would pick the same (now-spent) UTXOs and fail at the chain with
    /// `InputNotInUtxos`. Matches the JS SDK's pending-coin tracking pattern.
    pub fn remove_unshielded_spent(&mut self, spent: &[crate::transfer::SpentUtxoKey]) {
        self.unshielded_utxos.retain(|utxo| {
            let key_matches = |k: &crate::transfer::SpentUtxoKey| {
                utxo.intent_hash.as_deref() == Some(k.intent_hash.as_str())
                    && utxo.output_index == Some(k.output_index as i64)
            };
            !spent.iter().any(key_matches)
        });
    }

    /// Save the current wallet state to disk.
    pub fn save(&self, base: &Path) -> Result<(), WalletError> {
        crate::storage::save(
            base,
            &self.network_id,
            &self.seed,
            &self.zswap_state,
            &self.dust_wallet,
            &self.dust_global_state,
            &self.parameters,
            &self.block_context,
            self.zswap_event_id,
            self.dust_event_id,
            self.last_block_height,
            self.last_tx_id,
            &self.unshielded_utxos,
        )
    }

    /// Perform initial sync by replaying all indexer events from the beginning.
    /// Does not persist state to disk. Use [`sync`] for persistence.
    pub async fn sync_from_indexer(
        node_url: &str,
        indexer_url: &str,
        seed: WalletSeed,
        address: &str,
        network_id: &str,
    ) -> Result<Self, WalletError> {
        Self::sync(node_url, indexer_url, seed, address, network_id, None).await
    }

    /// Apply a zswap ledger event to the shielded state.
    pub fn apply_zswap_event(&mut self, msg: &LedgerEventMessage) -> Result<(), WalletError> {
        let raw_bytes = hex::decode(&msg.raw)
            .map_err(|e| WalletError::Sync(format!("decode zswap event hex: {e}")))?;
        let event: Event<DefaultDB> = tagged_deserialize(&raw_bytes[..])
            .map_err(|e| WalletError::Sync(format!("deserialize zswap event: {e}")))?;
        self.zswap_state = self
            .zswap_state
            .replay_events(&self.secret_keys, [&event])
            .map_err(|e| WalletError::Sync(format!("replay zswap event: {e}")))?;
        self.zswap_event_id = msg.id;
        Ok(())
    }

    /// Apply a dust ledger event to the dust wallet and global dust state.
    pub fn apply_dust_event(&mut self, msg: &LedgerEventMessage) -> Result<(), WalletError> {
        let raw_bytes = hex::decode(&msg.raw)
            .map_err(|e| WalletError::Sync(format!("decode dust event hex: {e}")))?;
        let event: Event<DefaultDB> = tagged_deserialize(&raw_bytes[..])
            .map_err(|e| WalletError::Sync(format!("deserialize dust event: {e}")))?;
        let mut dust_wallet = self.dust_wallet.clone();
        let mut dust_global_state = self.dust_global_state.clone();
        dust_wallet
            .replay_events([&event])
            .map_err(|e| WalletError::Sync(format!("replay dust event: {e}")))?;
        apply_dust_event_to_global(&mut dust_global_state, &event).map_err(|e| {
            WalletError::Sync(format!("apply dust global event id={}: {e}", msg.id))
        })?;
        self.dust_wallet = dust_wallet;
        self.dust_global_state = dust_global_state;
        self.dust_event_id = msg.id;
        Ok(())
    }

    /// Apply a single unshielded transaction event from the subscription.
    pub fn apply_unshielded_event(&mut self, event: &UnshieldedTxEvent) -> Result<(), WalletError> {
        match &event.unshielded_transactions {
            UnshieldedTxPayload::UnshieldedTransaction(tx_data) => {
                apply_unshielded_tx(&mut self.unshielded_utxos, tx_data)?;
                if let Some(ref tx_ref) = tx_data.transaction {
                    if let Some(id) = tx_ref.id {
                        self.last_tx_id = Some(id);
                    }
                    if let Some(ref block) = tx_ref.block {
                        self.last_block_height = self.last_block_height.max(block.height);
                    }
                }
            }
            UnshieldedTxPayload::UnshieldedTransactionsProgress(progress) => {
                self.last_tx_id = Some(progress.highest_transaction_id);
            }
        }
        Ok(())
    }

    /// Build a `LedgerContext` from the wallet's indexed state.
    ///
    /// Uses the block_context populated during sync, whose `tblock` is the
    /// block_time of the last dust event we processed. This timestamp is
    /// consistent with both our local root_history entry AND the chain's
    /// root_history entry at that timestamp. Using the chain's "current"
    /// block timestamp instead would be wrong: the chain has processed new
    /// dust events since we synced, so its root at the latest timestamp
    /// differs from our prover's root.
    pub async fn build_context(&mut self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
        self.build_context_inner()
    }

    pub(crate) fn build_context_inner(&self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
        // Create a LedgerState with correct parameters. The reserve_pool must equal
        // MAX_SUPPLY to satisfy the NIGHT balance invariant (total supply conservation).
        let mut ledger_state = LedgerState::with_genesis_settings(
            &self.network_id,
            self.parameters.clone(),
            0,
            MAX_SUPPLY,
            0,
        )
        .map_err(|e| WalletError::Sync(format!("construct ledger state: {e:?}")))?;

        // Populate UTXO state so the transaction builder can find our UTXOs.
        let unshielded = UnshieldedWallet::default(self.seed);
        let owner = unshielded.user_address;
        let utxo_ctime = self
            .block_context
            .as_ref()
            .map(|bc| Timestamp::from_secs(bc.tblock.to_secs().saturating_sub(3600)))
            .unwrap_or_else(|| Timestamp::from_secs(0));

        let mut utxo_state = (*ledger_state.utxo).clone();
        for tracked in &self.unshielded_utxos {
            let token_type = parse_token_type_hex(&tracked.token_type).unwrap_or(NIGHT);
            let intent_hash = tracked
                .intent_hash
                .as_deref()
                .and_then(parse_intent_hash_hex)
                .unwrap_or(IntentHash(HashOutput([0u8; 32])));
            let output_no = tracked.output_index.unwrap_or(0) as u32;

            let utxo = LedgerUtxo {
                value: tracked.value,
                owner,
                type_: token_type,
                intent_hash,
                output_no,
            };
            utxo_state = utxo_state.insert(utxo, UtxoMeta { ctime: utxo_ctime });
        }
        ledger_state.utxo = Sp::new(utxo_state);
        ledger_state.dust = Sp::new(self.dust_global_state.clone());

        let tblock = self.block_context.as_ref().map(|bc| bc.tblock);
        let commit_history =
            tblock.and_then(|t| self.dust_global_state.utxo.root_history.get(t).map(|r| r.0));
        let gen_history = tblock.and_then(|t| {
            self.dust_global_state
                .generation
                .root_history
                .get(t)
                .map(|r| r.0)
        });
        info!(
            ?tblock,
            ?commit_history,
            ?gen_history,
            commitments_first_free = self.dust_global_state.utxo.commitments_first_free,
            generating_tree_first_free = self.dust_global_state.generation.generating_tree_first_free,
            "dust root_history for context build"
        );

        let ctx = LedgerContext {
            ledger_state: std::sync::Mutex::new(Sp::new(ledger_state)),
            wallets: std::sync::Mutex::new(std::collections::HashMap::new()),
            resolver: tokio::sync::Mutex::new(
                midnight_node_ledger_helpers::context::DEFAULT_RESOLVER.clone(),
            ),
            latest_block_context: std::sync::Mutex::new(self.block_context.clone()),
        };

        // Insert wallet with our synced state
        {
            let mut shielded = ShieldedWallet::<DefaultDB>::default(self.seed);
            shielded.state = self.zswap_state.clone();

            let wallet = ContextWallet {
                root_seed: Some(self.seed),
                shielded,
                unshielded: midnight_node_ledger_helpers::UnshieldedWallet::default(self.seed),
                dust: self.dust_wallet.clone(),
            };

            ctx.wallets
                .lock()
                .map_err(|_| WalletError::Sync("wallets lock poisoned".into()))?
                .insert(self.seed, wallet);
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

    pub fn node_url(&self) -> &str {
        &self.node_url
    }

    pub fn indexer_url(&self) -> &str {
        &self.indexer_url
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

    /// Create a subscription client for the configured indexer URL.
    pub fn subscription_client(&self) -> Option<SubscriptionClient> {
        if self.indexer_url.is_empty() {
            return None;
        }
        Some(SubscriptionClient::new(&self.indexer_url))
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
            self.dust_wallet = DustWallet::default(self.seed, Some(&params));
        }
        self.parameters = params;
    }

    /// Re-sync the wallet state from the indexer, resuming from current cursors.
    ///
    /// Call this after a transaction is finalized to pick up the on-chain
    /// effects (spent dust UTXOs, new coins, etc.) before building the
    /// next transaction.
    pub async fn resync(&mut self) -> Result<(), WalletError> {
        let fresh = Self::sync(
            &self.node_url,
            &self.indexer_url,
            self.seed,
            &self.unshielded_address,
            &self.network_id,
            None,
        )
        .await?;
        *self = fresh;
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

                let raw_bytes = hex::decode(&msg.raw)
                    .map_err(|e| WalletError::Sync(format!("decode zswap event hex: {e}")))?;
                let ev: Event<DefaultDB> = tagged_deserialize(&raw_bytes[..])
                    .map_err(|e| WalletError::Sync(format!("deserialize zswap event: {e}")))?;
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
    mut dust_global: DustState<DefaultDB>,
    start_id: i64,
    resuming: bool,
    checkpoint: Option<impl Fn(&DustWallet<DefaultDB>, &DustState<DefaultDB>, i64)>,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<(DustWallet<DefaultDB>, DustState<DefaultDB>, i64, Option<Timestamp>), DustReplayError> {
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

                let raw_bytes = hex::decode(&msg.raw)
                    .map_err(|e| WalletError::Sync(format!("decode dust event hex: {e}")))?;
                let ev: Event<DefaultDB> = tagged_deserialize(&raw_bytes[..])
                    .map_err(|e| WalletError::Sync(format!("deserialize dust event: {e}")))?;

                dust_wallet
                    .replay_events([&ev])
                    .map_err(|e| DustReplayError::CachedState {
                        event_id: msg.id,
                        reason: format!("replay dust wallet event: {e}"),
                    })?;
                // Do NOT maintain a parallel `DustState` (`dust_global`). The
                // chain validates transactions against its own root_history;
                // we bypass client-side validate() in `build_no_validate()`.
                // The DustWallet's internal collapsed tree is sufficient for
                // generating proofs.
                let _ = &mut dust_global;

                if let Some(t) = event_block_time(&ev) {
                    last_block_time = Some(t);
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
                        save(&dust_wallet, &dust_global, last_id);
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
                ))
                .into());
            }
            Ok(None) => {
                if resuming && count == 0 {
                    info!(last_id, "dust already at tip");
                    send_progress(&progress, SyncProgress::DustComplete { events: 0 });
                    break;
                }
                return Err(WalletError::Sync(
                    "dust subscription ended before replay completed".into(),
                )
                .into());
            }
            Err(_) => {
                if resuming && count == 0 {
                    info!(last_id, "dust already at tip");
                    send_progress(&progress, SyncProgress::DustComplete { events: 0 });
                    break;
                }
                return Err(WalletError::Sync("timeout waiting for dust events".into()).into());
            }
        }
    }

    Ok((dust_wallet, dust_global, last_id, last_block_time))
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

/// Apply a dust event to the global DustState's Merkle trees.
///
/// IMPORTANT: This does NOT rehash the trees. After updating leaves via
/// `try_update_hash`, the parent node hashes are invalidated. The trees
/// remain in a "dirty" state with `root()` returning `None` until
/// `rehash()` is called explicitly.
///
/// This is critical for performance: rehashing is O(n) over the entire
/// tree, so doing it per-event during sync would be O(n²) total work
/// (~90 billion hash operations for 300k events on preprod). Instead,
/// we batch rehashes by calling `update_root_history` only once at the
/// end of sync (and again when building a transaction context).
fn apply_dust_event_to_global(
    state: &mut DustState<DefaultDB>,
    event: &Event<DefaultDB>,
) -> Result<(), String> {
    match &event.content {
        EventDetails::DustInitialUtxo {
            output,
            generation,
            generation_index,
            ..
        } => {
            let commitments = state
                .utxo
                .commitments
                .try_update_hash(output.mt_index, output.commitment().into(), ())
                .map_err(|e| {
                    format!(
                        "apply DustInitialUtxo commitment index {}: {e:?}",
                        output.mt_index
                    )
                })?;
            let generating_tree = state
                .generation
                .generating_tree
                .try_update_hash(*generation_index, generation.merkle_hash(), *generation)
                .map_err(|e| {
                    format!(
                        "apply DustInitialUtxo generation index {}: {e:?}",
                        generation_index
                    )
                })?;
            state.utxo.commitments = commitments;
            state.utxo.commitments_first_free = output.mt_index + 1;
            state.generation.generating_tree = generating_tree;
            state.generation.generating_tree_first_free = generation_index + 1;
        }
        EventDetails::DustSpendProcessed {
            commitment,
            commitment_index,
            ..
        } => {
            let commitments = state
                .utxo
                .commitments
                .try_update_hash(*commitment_index, (*commitment).into(), ())
                .map_err(|e| {
                    format!(
                        "apply DustSpendProcessed commitment index {}: {e:?}",
                        commitment_index
                    )
                })?;
            state.utxo.commitments = commitments;
            state.utxo.commitments_first_free = commitment_index + 1;
        }
        EventDetails::DustGenerationDtimeUpdate { update, .. } => {
            let generating_tree = state
                .generation
                .generating_tree
                .update_from_evidence(update.clone())
                .map_err(|e| format!("apply DustGenerationDtimeUpdate evidence: {e:?}"))?;
            state.generation.generating_tree = generating_tree;
        }
        _ => {}
    }
    Ok(())
}

#[allow(dead_code)]
fn update_root_history(state: &mut DustState<DefaultDB>, block_time: Timestamp) {
    state.utxo.commitments = state.utxo.commitments.rehash();
    if let Some(commitment_root) = state.utxo.commitments.root() {
        state.utxo.root_history = state.utxo.root_history.insert(block_time, commitment_root);
    }

    state.generation.generating_tree = state.generation.generating_tree.rehash();
    if let Some(generation_root) = state.generation.generating_tree.root() {
        state.generation.root_history = state
            .generation
            .root_history
            .insert(block_time, generation_root);
    }
}

async fn replay_unshielded_events(
    sub_client: &SubscriptionClient,
    address: &str,
    initial_utxos: Vec<TrackedUtxo>,
    start_tx_id: i64,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<(Vec<TrackedUtxo>, i64, i64), WalletError> {
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
                            return Ok((utxos, last_seen_tx_id, last_height));
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
                        return Ok((utxos, last_seen_tx_id.max(target), last_height));
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
#[derive(Hash, Eq, PartialEq)]
struct UtxoKey {
    owner: String,
    token_type: String,
    value: u128,
    intent_hash: Option<String>,
    output_index: Option<i64>,
}

impl TrackedUtxo {
    fn key(&self) -> UtxoKey {
        UtxoKey {
            owner: self.owner.clone(),
            token_type: self.token_type.clone(),
            value: self.value,
            intent_hash: self.intent_hash.clone(),
            output_index: self.output_index,
        }
    }
}

fn apply_unshielded_tx(
    utxos: &mut Vec<TrackedUtxo>,
    tx_data: &UnshieldedTxData,
) -> Result<(), WalletError> {
    // Parse all values upfront before mutating state. If any value fails to
    // parse, we return an error without having touched the UTXO vec, so
    // retries cannot produce duplicates.
    let mut to_remove: std::collections::HashMap<UtxoKey, usize> = std::collections::HashMap::new();
    for spent in &tx_data.spent_utxos {
        let value: u128 = spent.value.parse().map_err(|e| {
            WalletError::Sync(format!(
                "failed to parse spent UTXO value '{}': {e}",
                spent.value
            ))
        })?;
        let key = UtxoKey {
            owner: spent.owner.clone(),
            token_type: spent.token_type.clone(),
            value,
            intent_hash: spent.intent_hash.clone(),
            output_index: spent.output_index,
        };
        *to_remove.entry(key).or_insert(0) += 1;
    }

    let mut new_utxos = Vec::with_capacity(tx_data.created_utxos.len());
    for created in &tx_data.created_utxos {
        let value: u128 = created.value.parse().map_err(|e| {
            WalletError::Sync(format!(
                "failed to parse UTXO value '{}': {e}",
                created.value
            ))
        })?;
        new_utxos.push(TrackedUtxo {
            owner: created.owner.clone(),
            token_type: created.token_type.clone(),
            value,
            intent_hash: created.intent_hash.clone(),
            output_index: created.output_index,
        });
    }

    // All parsing succeeded, apply mutations.
    if !to_remove.is_empty() {
        utxos.retain(|u| match to_remove.get_mut(&u.key()) {
            Some(count) if *count > 0 => {
                *count -= 1;
                false
            }
            _ => true,
        });
    }
    utxos.extend(new_utxos);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::last_applied_before;

    #[test]
    fn last_applied_before_does_not_advance_to_unapplied_event() {
        assert_eq!(last_applied_before(0), 0);
        assert_eq!(last_applied_before(1), 0);
        assert_eq!(last_applied_before(42), 41);
        assert_eq!(last_applied_before(-1), 0);
    }
}

fn send_progress(tx: &Option<mpsc::Sender<SyncProgress>>, msg: SyncProgress) {
    if let Some(tx) = tx {
        let _ = tx.try_send(msg);
    }
}

fn parse_intent_hash_hex(hex: &str) -> Option<IntentHash> {
    let bytes = hex::decode(hex).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    Some(IntentHash(HashOutput(arr)))
}

fn parse_token_type_hex(hex: &str) -> Option<UnshieldedTokenType> {
    let bytes = hex::decode(hex).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    Some(UnshieldedTokenType(HashOutput(arr)))
}
