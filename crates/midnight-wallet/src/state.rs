use std::sync::Arc;

use midnight_indexer_client::{SubscriptionClient, UnshieldedUtxo};
use midnight_node_ledger_helpers::{DefaultDB, LedgerContext, WalletSeed};
use midnight_node_toolkit::tx_generator::builder::build_fork_aware_context;
use midnight_node_toolkit::tx_generator::source::{FetchCacheConfig, GetTxs, GetTxsFromUrl};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::WalletError;

#[derive(Debug, Clone)]
pub struct SyncResult {
    pub blocks_processed: usize,
    pub height: i64,
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

impl From<UnshieldedUtxo> for TrackedUtxo {
    fn from(utxo: UnshieldedUtxo) -> Self {
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

/// Wallet state backed by the Midnight indexer for balance tracking.
///
/// Balance queries use the indexer (via subscription or HTTP). Transaction
/// building still requires a `LedgerContext` from the node, fetched on-demand
/// via [`sync_context`].
pub struct WalletState {
    seed: WalletSeed,
    node_url: String,
    indexer_url: String,

    // Indexer-tracked state
    unshielded_utxos: Vec<TrackedUtxo>,
    last_block_height: i64,
    last_tx_id: Option<i64>,

    // Node context for transaction building (lazy)
    node_block_height: i64,

    // Cached node context for transaction building (lazy)
    cached_context: Option<Arc<LedgerContext<DefaultDB>>>,
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

/// Response type for block subscription events.
#[derive(Debug, Clone, Deserialize)]
pub struct BlockEvent {
    pub blocks: BlockEventData,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockEventData {
    pub hash: String,
    pub height: i64,
    #[serde(default)]
    pub protocol_version: Option<i64>,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

impl WalletState {
    /// Create a new wallet state that uses the indexer for balance tracking.
    ///
    /// This performs an initial sync by subscribing to `unshieldedTransactions`
    /// from the beginning and replaying all events until caught up.
    pub async fn sync_from_indexer(
        node_url: &str,
        indexer_url: &str,
        seed: WalletSeed,
        address: &str,
    ) -> Result<Self, WalletError> {
        let sub_client = SubscriptionClient::new(indexer_url);

        let variables = serde_json::json!({
            "address": address,
            "transactionId": 0,
        });

        let mut subscription = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            sub_client.subscribe::<UnshieldedTxEvent>(
                midnight_indexer_client::subscription::queries::UNSHIELDED_TRANSACTIONS_SUBSCRIPTION,
                variables,
            ),
        )
        .await
        .map_err(|_| WalletError::Sync("timeout connecting to indexer".into()))?
        .map_err(|e| WalletError::Sync(format!("subscribe unshieldedTransactions: {e}")))?;

        let mut utxos: Vec<TrackedUtxo> = Vec::new();
        let mut last_height: i64 = 0;

        // Replay until we get a Progress event (which marks "caught up").
        // The indexer sends UnshieldedTransactionsProgress once all historical
        // events have been delivered. On a fresh chain with no transactions for
        // this address, the Progress event arrives immediately.
        // The Progress event's transaction_id is the authoritative resume cursor.
        let last_tx_id: i64;
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
                        last_tx_id = progress.highest_transaction_id;
                        debug!(
                            tx_id = progress.highest_transaction_id,
                            "indexer sync caught up"
                        );
                        break;
                    }
                },
                Ok(Some(Err(e))) => {
                    return Err(WalletError::Sync(format!(
                        "subscription error during initial sync: {e}"
                    )));
                }
                Ok(None) => {
                    return Err(WalletError::Sync(
                        "subscription ended before sync completed".into(),
                    ));
                }
                Err(_) => {
                    return Err(WalletError::Sync(
                        "timeout waiting for indexer sync to complete (no progress event received)"
                            .into(),
                    ));
                }
            }
        }

        info!(
            utxos = utxos.len(),
            height = last_height,
            "wallet synced from indexer"
        );

        Ok(Self {
            seed,
            node_url: node_url.to_string(),
            indexer_url: indexer_url.to_string(),
            unshielded_utxos: utxos,
            last_block_height: last_height,
            last_tx_id: Some(last_tx_id),
            node_block_height: 0,
            cached_context: None,
        })
    }

    /// Apply a single unshielded transaction event from the subscription.
    pub fn apply_event(&mut self, event: &UnshieldedTxEvent) {
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
                // Invalidate cached context since state changed
                self.cached_context = None;
            }
            UnshieldedTxPayload::UnshieldedTransactionsProgress(progress) => {
                self.last_tx_id = Some(progress.highest_transaction_id);
            }
        }
    }

    /// Fetch a `LedgerContext` from the node for transaction building.
    ///
    /// Caches the result so repeated calls within the same "session" don't
    /// re-fetch. The cache is invalidated when new indexer events arrive.
    /// Returns `(context, blocks_processed)` where `blocks_processed` is 0
    /// when the cached context was used.
    pub async fn sync_context(
        &mut self,
    ) -> Result<(Arc<LedgerContext<DefaultDB>>, usize), WalletError> {
        if let Some(ref ctx) = self.cached_context {
            return Ok((ctx.clone(), 0));
        }

        let (context, block_count) = fetch_context_with_height(&self.node_url, self.seed).await?;
        self.node_block_height = block_count as i64;
        let ctx = Arc::new(context);
        self.cached_context = Some(ctx.clone());
        Ok((ctx, block_count))
    }

    /// Get the cached context if available, without triggering a sync.
    pub fn context(&self) -> Option<&Arc<LedgerContext<DefaultDB>>> {
        self.cached_context.as_ref()
    }

    /// Block height from the indexer (tracks unshielded transaction events).
    pub fn last_synced_height(&self) -> i64 {
        self.last_block_height
    }

    /// Block height from the node (set after `sync_context` completes).
    pub fn node_block_height(&self) -> i64 {
        self.node_block_height
    }

    pub fn last_tx_id(&self) -> Option<i64> {
        self.last_tx_id
    }

    pub fn seed(&self) -> &WalletSeed {
        &self.seed
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

    /// Invalidate the cached node context so the next `sync_context` will
    /// re-fetch from the node.
    pub fn invalidate_context(&mut self) {
        self.cached_context = None;
    }

    /// Create a subscription client for the configured indexer URL.
    ///
    /// Returns `None` if no indexer URL was configured.
    pub fn subscription_client(&self) -> Option<SubscriptionClient> {
        if self.indexer_url.is_empty() {
            return None;
        }
        Some(SubscriptionClient::new(&self.indexer_url))
    }
}

fn apply_unshielded_tx(utxos: &mut Vec<TrackedUtxo>, tx_data: &UnshieldedTxData) {
    // Remove spent UTXOs (only the first match per spent entry to avoid
    // removing multiple UTXOs when optional fields like intent_hash are None)
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
    // Add created UTXOs
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

async fn fetch_context_with_height(
    node_url: &str,
    seed: WalletSeed,
) -> Result<(LedgerContext<DefaultDB>, usize), WalletError> {
    let fetcher = GetTxsFromUrl::new(node_url, 4, 4, true, false, FetchCacheConfig::InMemory);
    let source_txs = GetTxs::get_txs(&fetcher)
        .await
        .map_err(|e| WalletError::Sync(format!("fetch blocks: {e}")))?;

    let block_count = source_txs.blocks.len();
    let context = build_fork_aware_context(&source_txs, &[seed])
        .map_err(|e| WalletError::Sync(format!("build context: {e}")))?;

    Ok((context, block_count))
}
