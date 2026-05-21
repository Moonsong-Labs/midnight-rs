use std::path::{Path, PathBuf};
use std::sync::Arc;

use midnight_indexer_client::SubscriptionClient;
use midnight_node_ledger_helpers::midnight_serialize::tagged_deserialize;
use midnight_node_ledger_helpers::mn_ledger::events::EventDetails;
use midnight_node_ledger_helpers::mn_ledger::semantics::ZswapLocalStateExt;
use midnight_node_ledger_helpers::mn_ledger::structure::{Utxo as LedgerUtxo, UtxoMeta};
use midnight_node_ledger_helpers::{
    BlockContext, DefaultDB, DustWallet, Event, HashOutput, IntentHash, LedgerContext,
    LedgerParameters, LedgerState, MAX_SUPPLY, SecretKeys, ShieldedWallet, Sp, Timestamp,
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
/// `Wallet` is a pure state machine: it owns synced state and exposes pure
/// mutation methods (`apply_*_event`, `set_block_context`, `set_parameters`).
/// All I/O — initial sync, resync, subscriptions, building a [`LedgerContext`] —
/// is driven by [`midnight_provider::MidnightProvider`], which owns the wallet
/// behind an `Arc<RwLock<_>>`.
pub struct Wallet {
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

impl Wallet {
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

        let (dust_wallet, start_dust_id) = if let Some(ref c) = cached {
            (c.dust_wallet.clone(), c.dust_event_id + 1)
        } else {
            (DustWallet::default(seed, Some(&parameters)), 0_i64)
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
        let (unshielded_utxos, last_tx_id, replay_block_height) = unshielded_result?;
        // The unshielded subscription only updates `last_block_height` when a
        // transaction touches our address. On a resume with no new unshielded
        // txs, replay returns 0, so we keep the persisted value as a floor.
        let cached_block_height = cached.as_ref().map(|c| c.last_block_height).unwrap_or(0);
        let last_block_height = replay_block_height.max(cached_block_height);

        let dust_checkpoint = make_dust_checkpoint(
            storage_dir,
            &network_id,
            seed,
            zswap_state.clone(),
            zswap_event_id,
            last_block_height,
            Some(last_tx_id),
            unshielded_utxos.clone(),
        );
        let dust_resuming = start_dust_id > 0;
        let (dust_wallet, dust_event_id, last_dust_block_time) = replay_dust_events(
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
        let candidate =
            last_dust_block_time.map(|t| t + midnight_node_ledger_helpers::Duration::from_secs(1));
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

    /// Copy the mutated `DustWallet` back from a `LedgerContext` after build.
    ///
    /// `build_no_validate` calls `mark_spent` on the context's clone of our
    /// `DustWallet`. Without this call, the next transfer would pick the same
    /// dust UTXOs again.
    pub fn sync_dust_from_context(&mut self, context: &LedgerContext<DefaultDB>) {
        if let Ok(wallets) = context.wallets.lock() {
            if let Some(wallet) = wallets.get(&self.seed) {
                self.dust_wallet = wallet.dust.clone();
            }
        }
    }

    /// Remove unshielded UTXOs spent by a recently-built transaction so the
    /// next transfer doesn't pick them before the indexer confirms the spend.
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
        let event = decode_event(msg, "zswap")?;
        self.zswap_state = self
            .zswap_state
            .replay_events(&self.secret_keys, [&event])
            .map_err(|e| WalletError::Sync(format!("replay zswap event: {e}")))?;
        self.zswap_event_id = msg.id;
        Ok(())
    }

    /// Apply a dust ledger event to the DustWallet.
    pub fn apply_dust_event(&mut self, msg: &LedgerEventMessage) -> Result<(), WalletError> {
        let event = decode_event(msg, "dust")?;
        self.dust_wallet
            .replay_events([&event])
            .map_err(|e| WalletError::Sync(format!("replay dust event: {e}")))?;
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
    /// Replays any new dust events since the last cursor and refreshes the
    /// block timestamp so the transaction's TTL (`now + global_ttl`) is valid
    /// when the chain applies it and the proof roots match the chain's
    /// `root_history.get(ctime)`.
    pub async fn build_context(&mut self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
        self.resync().await?;
        self.build_context_inner()
    }

    pub(crate) fn build_context_inner(&self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
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
        let unshielded = UnshieldedWallet::default(self.seed);
        let owner = unshielded.user_address;
        let utxo_ctime = self
            .block_context
            .as_ref()
            .map(|bc| Timestamp::from_secs(bc.tblock.to_secs().saturating_sub(3600)))
            .unwrap_or_else(|| Timestamp::from_secs(0));

        // intent_hash + output_no are part of a UTXO's identity; falling back
        // to default values silently creates collisions between distinct UTXOs
        // and synthesizes inputs the chain will reject.
        let mut utxo_state = (*ledger_state.utxo).clone();
        for tracked in &self.unshielded_utxos {
            let utxo = tracked_to_ledger_utxo(tracked, owner)?;
            utxo_state = utxo_state.insert(utxo, UtxoMeta { ctime: utxo_ctime });
        }
        ledger_state.utxo = Sp::new(utxo_state);

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

    /// The network identifier this wallet derives addresses for
    /// (e.g. `"undeployed"`, `"testnet"`).
    pub fn network(&self) -> &str {
        &self.network_id
    }

    /// The wallet's unshielded receiving address (cached at construction).
    pub fn unshielded_address(&self) -> String {
        self.unshielded_address.clone()
    }

    /// The wallet's shielded receiving address, e.g. `mn_shield-addr_undeployed1...`.
    pub fn shielded_address(&self) -> String {
        crate::address::derive_shielded(&self.seed, &self.network_id)
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
    ///
    /// On error, `self` is left untouched: all replay results are awaited and
    /// validated before any field is mutated. The chain's current block_time
    /// is fetched as part of the same operation; failure to fetch it is also
    /// fatal because `block_context.tblock` drives TTL and proof root lookup.
    pub async fn resync(&mut self) -> Result<(), WalletError> {
        let sub_client = SubscriptionClient::new(&self.indexer_url);
        let indexer_client = midnight_indexer_client::IndexerClient::new(&self.indexer_url)
            .map_err(|e| WalletError::Sync(format!("indexer client: {e}")))?;

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
        let (dust_wallet, dust_event_id, last_dust_block_time) = dust_res?;
        let (zswap_state, zswap_event_id) = zswap_res?;
        let (unshielded_utxos, last_tx_id, last_block_height) = unshielded_res?;
        let block = block_res
            .map_err(|e| WalletError::Sync(format!("fetch latest block: {e}")))?
            .ok_or_else(|| WalletError::Sync("no blocks available from indexer".into()))?;
        let tblock_ms = block
            .timestamp
            .ok_or_else(|| WalletError::Sync("latest block has no timestamp".into()))?;
        let chain_tblock = Timestamp::from_secs((tblock_ms / 1000) as u64);

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
            .map(|t| t + midnight_node_ledger_helpers::Duration::from_secs(1))
            .or_else(|| self.block_context.as_ref().map(|bc| bc.tblock));
        let tblock = match candidate {
            Some(t) if t + global_ttl >= chain_tblock => t,
            _ => chain_tblock,
        };

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
        self.block_context = Some(block_context_at(tblock));

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
) -> Result<(DustWallet<DefaultDB>, i64, Option<Timestamp>), WalletError> {
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

                let ev = decode_event(msg, "dust")?;
                dust_wallet.replay_events([&ev]).map_err(|e| {
                    WalletError::Sync(format!("apply dust event id={}: {e}", msg.id))
                })?;

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

    Ok((dust_wallet, last_id, last_block_time))
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

fn tracked_to_ledger_utxo(
    tracked: &TrackedUtxo,
    owner: midnight_node_ledger_helpers::UserAddress,
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

/// Compat alias retained while the codebase migrates off the old `Wallet` /
/// `WalletState` split. Prefer [`Wallet`] in new code.
pub type WalletState = Wallet;
