use std::sync::Arc;

use midnight_indexer_client::SubscriptionClient;
use midnight_node_ledger_helpers::midnight_serialize::tagged_deserialize;
use midnight_node_ledger_helpers::mn_ledger::dust::DustState;
use midnight_node_ledger_helpers::mn_ledger::events::EventDetails;
use midnight_node_ledger_helpers::mn_ledger::semantics::ZswapLocalStateExt;
use midnight_node_ledger_helpers::{
    BlockContext, DefaultDB, DustWallet, Event, LedgerContext, LedgerParameters, LedgerState,
    MAX_SUPPLY, SecretKeys, ShieldedWallet, Sp, Timestamp, Wallet as ContextWallet, WalletSeed,
    WalletState as ZswapLocalState,
};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::WalletError;

/// A tracked unshielded UTXO from the indexer.
#[derive(Debug, Clone)]
pub struct TrackedUtxo {
    pub owner: String,
    pub token_type: String,
    pub value: u128,
    pub intent_hash: Option<String>,
    pub output_index: Option<i64>,
}

impl From<midnight_indexer_client::UnshieldedUtxo> for TrackedUtxo {
    fn from(utxo: midnight_indexer_client::UnshieldedUtxo) -> Self {
        let value = utxo.value.parse().unwrap_or_else(|e| {
            warn!(value = %utxo.value, error = %e, "failed to parse UTXO value, defaulting to 0");
            0
        });
        Self {
            owner: utxo.owner,
            token_type: utxo.token_type.clone(),
            value,
            intent_hash: utxo.intent_hash,
            output_index: utxo.output_index,
        }
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

impl WalletState {
    /// Perform initial sync by replaying all indexer events from the beginning.
    ///
    /// Subscribes to three streams concurrently:
    /// 1. `zswapLedgerEvents` - replays until `id == maxId`
    /// 2. `dustLedgerEvents` - replays until `id == maxId`
    /// 3. `unshieldedTransactions` - replays until Progress event
    ///
    /// Also fetches `LedgerParameters` from the latest block.
    pub async fn sync_from_indexer(
        node_url: &str,
        indexer_url: &str,
        seed: WalletSeed,
        address: &str,
        network_id: &str,
    ) -> Result<Self, WalletError> {
        let shielded = ShieldedWallet::<DefaultDB>::default(seed);
        let secret_keys = shielded.secret_keys().clone();

        // Fetch ledger parameters from the latest block
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

        let network_id = network_id.to_string();

        let dust_wallet = DustWallet::default(seed, Some(&parameters));

        // Run all three subscriptions concurrently
        let sub_client = SubscriptionClient::new(indexer_url);

        let (zswap_result, dust_result, unshielded_result) = tokio::join!(
            replay_zswap_events(&sub_client, &secret_keys, shielded.state.clone()),
            replay_dust_events(&sub_client, dust_wallet),
            replay_unshielded_events(&sub_client, address),
        );

        let (zswap_state, zswap_event_id) = zswap_result?;
        let (dust_wallet, mut dust_global_state, dust_event_id) = dust_result?;
        let (unshielded_utxos, last_tx_id, last_block_height) = unshielded_result?;

        // Use the latest block timestamp for the block context. This serves two purposes:
        // 1. Recent enough for TTL checks and dust value accrual calculations
        // 2. We insert our current tree roots into dust_global_state.root_history at this
        //    timestamp, so dust spend proofs reference a ctime where our local root_history
        //    and the node's root_history agree (avoiding InvalidDustSpendProof errors
        //    from the node's tree advancing between our sync and transaction submission)
        let block_timestamp = block
            .timestamp
            .map(|ms| Timestamp::from_secs((ms / 1000) as u64))
            .ok_or_else(|| WalletError::Sync("latest block has no timestamp".into()))?;

        // Insert current tree roots at the block timestamp so our root_history
        // has an entry matching the node's at this point in time.
        update_root_history(&mut dust_global_state, block_timestamp);

        let block_context = Some(BlockContext {
            tblock: block_timestamp,
            tblock_err: 30,
            parent_block_hash: Default::default(),
            last_block_time: block_timestamp,
        });

        info!(
            zswap_event_id,
            dust_event_id,
            unshielded_utxos = unshielded_utxos.len(),
            height = last_block_height,
            "wallet synced from indexer"
        );

        Ok(Self {
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
        })
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

    /// Apply a dust ledger event to the dust wallet.
    pub fn apply_dust_event(&mut self, msg: &LedgerEventMessage) -> Result<(), WalletError> {
        let raw_bytes = hex::decode(&msg.raw)
            .map_err(|e| WalletError::Sync(format!("decode dust event hex: {e}")))?;
        let event: Event<DefaultDB> = tagged_deserialize(&raw_bytes[..])
            .map_err(|e| WalletError::Sync(format!("deserialize dust event: {e}")))?;
        self.dust_wallet
            .replay_events([&event])
            .map_err(|e| WalletError::Sync(format!("replay dust event: {e}")))?;
        apply_dust_event_to_global(&mut self.dust_global_state, &event);
        self.dust_event_id = msg.id;
        Ok(())
    }

    /// Apply a single unshielded transaction event from the subscription.
    pub fn apply_unshielded_event(&mut self, event: &UnshieldedTxEvent) {
        match &event.unshielded_transactions {
            UnshieldedTxPayload::UnshieldedTransaction(tx_data) => {
                apply_unshielded_tx(&mut self.unshielded_utxos, tx_data);
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
    }

    /// Build a `LedgerContext` from the wallet's indexed state.
    ///
    /// This replaces the expensive full-chain-replay. The context is constructed
    /// from local state (zswap + dust + parameters) and is suitable for
    /// transaction building via `StandardTrasactionInfo`.
    pub fn build_context(&self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
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

        // Populate dust state with the Merkle root history built during event replay.
        // This is required for client-side well_formed() validation of dust spend proofs.
        ledger_state.dust = Sp::new(self.dust_global_state.clone());

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

    pub fn last_synced_height(&self) -> i64 {
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

    /// Re-sync the wallet state from the indexer.
    ///
    /// Replays all indexer events from the beginning, replacing the current
    /// state. Call this after a transaction is finalized to pick up the
    /// on-chain effects (spent dust UTXOs, new coins, etc.) before building
    /// the next transaction.
    pub async fn resync(&mut self) -> Result<(), WalletError> {
        let fresh = Self::sync_from_indexer(
            &self.node_url,
            &self.indexer_url,
            self.seed,
            &self.unshielded_address,
            &self.network_id,
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
) -> Result<(ZswapLocalState<DefaultDB>, i64), WalletError> {
    use midnight_indexer_client::subscription::queries::ZSWAP_LEDGER_EVENTS_SUBSCRIPTION;

    let variables = serde_json::json!({ "id": 0 });

    let mut subscription = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        sub_client.subscribe::<ZswapEventEnvelope>(ZSWAP_LEDGER_EVENTS_SUBSCRIPTION, variables),
    )
    .await
    .map_err(|_| WalletError::Sync("timeout connecting to zswapLedgerEvents".into()))?
    .map_err(|e| WalletError::Sync(format!("subscribe zswapLedgerEvents: {e}")))?;

    let mut state = initial_state;
    let mut last_id: i64 = 0;
    let mut count: u64 = 0;

    loop {
        let event =
            tokio::time::timeout(std::time::Duration::from_secs(30), subscription.next()).await;

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

                if msg.id >= msg.max_id {
                    debug!(count, last_id, "zswap replay complete");
                    break;
                }
            }
            Ok(Some(Err(e))) => {
                return Err(WalletError::Sync(format!(
                    "zswap subscription error during replay: {e}"
                )));
            }
            Ok(None) => {
                return Err(WalletError::Sync(
                    "zswap subscription ended before replay completed".into(),
                ));
            }
            Err(_) => {
                return Err(WalletError::Sync("timeout waiting for zswap events".into()));
            }
        }
    }

    Ok((state, last_id))
}

async fn replay_dust_events(
    sub_client: &SubscriptionClient,
    mut dust_wallet: DustWallet<DefaultDB>,
) -> Result<(DustWallet<DefaultDB>, DustState<DefaultDB>, i64), WalletError> {
    use midnight_indexer_client::subscription::queries::DUST_LEDGER_EVENTS_SUBSCRIPTION;

    let variables = serde_json::json!({ "id": 0 });

    let mut subscription = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        sub_client.subscribe::<DustEventEnvelope>(DUST_LEDGER_EVENTS_SUBSCRIPTION, variables),
    )
    .await
    .map_err(|_| WalletError::Sync("timeout connecting to dustLedgerEvents".into()))?
    .map_err(|e| WalletError::Sync(format!("subscribe dustLedgerEvents: {e}")))?;

    let mut last_id: i64 = 0;
    let mut count: u64 = 0;
    let mut dust_global = DustState::<DefaultDB>::default();

    loop {
        let event =
            tokio::time::timeout(std::time::Duration::from_secs(30), subscription.next()).await;

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
                dust_wallet.replay_events([&ev]).map_err(|e| {
                    WalletError::Sync(format!("replay dust event id={}: {e}", msg.id))
                })?;

                apply_dust_event_to_global(&mut dust_global, &ev);

                last_id = msg.id;
                count += 1;

                if msg.id >= msg.max_id {
                    debug!(count, last_id, "dust replay complete");
                    break;
                }
            }
            Ok(Some(Err(e))) => {
                return Err(WalletError::Sync(format!(
                    "dust subscription error during replay: {e}"
                )));
            }
            Ok(None) => {
                return Err(WalletError::Sync(
                    "dust subscription ended before replay completed".into(),
                ));
            }
            Err(_) => {
                return Err(WalletError::Sync("timeout waiting for dust events".into()));
            }
        }
    }

    Ok((dust_wallet, dust_global, last_id))
}

fn apply_dust_event_to_global(state: &mut DustState<DefaultDB>, event: &Event<DefaultDB>) {
    match &event.content {
        EventDetails::DustInitialUtxo {
            output,
            generation,
            generation_index,
            block_time,
        } => {
            if let Ok(t) = state.utxo.commitments.try_update_hash(
                output.mt_index,
                output.commitment().into(),
                (),
            ) {
                state.utxo.commitments = t;
            }
            state.utxo.commitments_first_free = output.mt_index + 1;
            if let Ok(t) = state.generation.generating_tree.try_update_hash(
                *generation_index,
                generation.merkle_hash(),
                *generation,
            ) {
                state.generation.generating_tree = t;
            }
            state.generation.generating_tree_first_free = generation_index + 1;
            update_root_history(state, *block_time);
        }
        EventDetails::DustSpendProcessed {
            commitment,
            commitment_index,
            block_time,
            ..
        } => {
            if let Ok(t) =
                state
                    .utxo
                    .commitments
                    .try_update_hash(*commitment_index, (*commitment).into(), ())
            {
                state.utxo.commitments = t;
            }
            state.utxo.commitments_first_free = commitment_index + 1;
            update_root_history(state, *block_time);
        }
        EventDetails::DustGenerationDtimeUpdate { update, block_time } => {
            if let Ok(updated) = state
                .generation
                .generating_tree
                .update_from_evidence(update.clone())
            {
                state.generation.generating_tree = updated;
            }
            update_root_history(state, *block_time);
        }
        _ => {}
    }
}

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
) -> Result<(Vec<TrackedUtxo>, i64, i64), WalletError> {
    use midnight_indexer_client::subscription::queries::UNSHIELDED_TRANSACTIONS_SUBSCRIPTION;

    let variables = serde_json::json!({
        "address": address,
        "transactionId": 0,
    });

    let mut subscription = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        sub_client.subscribe::<UnshieldedTxEvent>(UNSHIELDED_TRANSACTIONS_SUBSCRIPTION, variables),
    )
    .await
    .map_err(|_| WalletError::Sync("timeout connecting to unshieldedTransactions".into()))?
    .map_err(|e| WalletError::Sync(format!("subscribe unshieldedTransactions: {e}")))?;

    let mut utxos: Vec<TrackedUtxo> = Vec::new();
    let mut last_height: i64 = 0;

    loop {
        let event =
            tokio::time::timeout(std::time::Duration::from_secs(30), subscription.next()).await;

        match event {
            Ok(Some(Ok(ev))) => match ev.unshielded_transactions {
                UnshieldedTxPayload::UnshieldedTransaction(tx_data) => {
                    apply_unshielded_tx(&mut utxos, &tx_data);
                    if let Some(ref tx_ref) = tx_data.transaction {
                        if let Some(ref block) = tx_ref.block {
                            last_height = last_height.max(block.height);
                        }
                    }
                }
                UnshieldedTxPayload::UnshieldedTransactionsProgress(progress) => {
                    debug!(
                        tx_id = progress.highest_transaction_id,
                        "unshielded sync caught up"
                    );
                    return Ok((utxos, progress.highest_transaction_id, last_height));
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

fn apply_unshielded_tx(utxos: &mut Vec<TrackedUtxo>, tx_data: &UnshieldedTxData) {
    for spent in &tx_data.spent_utxos {
        let spent_value: u128 = spent.value.parse().unwrap_or_else(|e| {
            warn!(value = %spent.value, error = %e, "failed to parse spent UTXO value, defaulting to 0");
            0
        });
        if let Some(pos) = utxos.iter().position(|u| {
            u.owner == spent.owner
                && u.token_type == spent.token_type
                && u.value == spent_value
                && u.intent_hash == spent.intent_hash
                && u.output_index == spent.output_index
        }) {
            utxos.swap_remove(pos);
        }
    }
    for created in &tx_data.created_utxos {
        let value = created.value.parse().unwrap_or_else(|e| {
            warn!(value = %created.value, error = %e, "failed to parse UTXO value, defaulting to 0");
            0
        });
        utxos.push(TrackedUtxo {
            owner: created.owner.clone(),
            token_type: created.token_type.clone(),
            value,
            intent_hash: created.intent_hash.clone(),
            output_index: created.output_index,
        });
    }
}
