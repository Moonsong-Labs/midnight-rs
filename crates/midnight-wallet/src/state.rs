use std::sync::Arc;

use midnight_node_ledger_helpers::{
    DefaultDB, LedgerContext, ShieldedTokenType, Utxo, Wallet as InternalWallet, WalletSeed,
};
use midnight_node_toolkit::tx_generator::builder::build_fork_aware_context;
use midnight_node_toolkit::tx_generator::source::{FetchCacheConfig, GetTxs, GetTxsFromUrl};
use tracing::{debug, info};

use crate::WalletError;

#[derive(Debug, Clone)]
pub struct SyncResult {
    pub blocks_processed: usize,
    pub height: i64,
}

pub struct WalletState {
    context: Arc<LedgerContext<DefaultDB>>,
    last_synced_height: i64,
    seed: WalletSeed,
    node_url: String,
}

impl WalletState {
    pub async fn sync_from_node(
        node_url: &str,
        seed: WalletSeed,
    ) -> Result<Self, WalletError> {
        let fetcher = GetTxsFromUrl::new(node_url, 4, 4, true, false, FetchCacheConfig::InMemory);
        let source_txs = GetTxs::get_txs(&fetcher)
            .await
            .map_err(|e| WalletError::Sync(format!("fetch blocks: {e}")))?;

        let block_count = source_txs.blocks.len();
        let context = build_fork_aware_context(&source_txs, &[seed])
            .map_err(|e| WalletError::Sync(format!("build context: {e}")))?;

        let height = block_count as i64;

        info!(blocks = block_count, "wallet synced");

        Ok(Self {
            context: Arc::new(context),
            last_synced_height: height,
            seed,
            node_url: node_url.to_string(),
        })
    }

    pub async fn resync(&mut self) -> Result<SyncResult, WalletError> {
        let fetcher = GetTxsFromUrl::new(
            &self.node_url,
            4,
            4,
            true,
            false,
            FetchCacheConfig::InMemory,
        );
        let source_txs = GetTxs::get_txs(&fetcher)
            .await
            .map_err(|e| WalletError::Sync(format!("fetch blocks: {e}")))?;

        let block_count = source_txs.blocks.len();
        let context = build_fork_aware_context(&source_txs, &[self.seed])
            .map_err(|e| WalletError::Sync(format!("build context: {e}")))?;

        let height = block_count as i64;

        debug!(blocks = block_count, "wallet resynced");

        self.context = Arc::new(context);
        let blocks_since_last = (height - self.last_synced_height).max(0) as usize;
        self.last_synced_height = height;

        Ok(SyncResult {
            blocks_processed: blocks_since_last,
            height,
        })
    }

    pub fn last_synced_height(&self) -> i64 {
        self.last_synced_height
    }

    pub fn seed(&self) -> &WalletSeed {
        &self.seed
    }

    pub fn node_url(&self) -> &str {
        &self.node_url
    }

    pub fn context(&self) -> &Arc<LedgerContext<DefaultDB>> {
        &self.context
    }

    pub(crate) fn wallet(&self) -> Option<InternalWallet<DefaultDB>> {
        self.context
            .wallets
            .lock()
            .expect("lock wallets")
            .get(&self.seed)
            .cloned()
    }

    pub fn dust_utxo_count(&self) -> usize {
        self.wallet()
            .and_then(|w| w.dust.dust_local_state.as_ref().map(|s| s.utxos().count()))
            .unwrap_or(0)
    }

    pub fn unshielded_utxos(&self) -> Vec<Utxo> {
        let Some(wallet) = self.wallet() else {
            return vec![];
        };
        let ledger_state = self.context.ledger_state.lock().expect("lock ledger_state");
        wallet.unshielded_utxos(&ledger_state)
    }

    pub fn shielded_coins(&self) -> Vec<ShieldedCoinInfo> {
        let Some(wallet) = self.wallet() else {
            return vec![];
        };
        wallet
            .shielded
            .state
            .coins
            .iter()
            .map(|(_nullifier, coin)| ShieldedCoinInfo {
                token_type: coin.type_,
                value: coin.value,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct ShieldedCoinInfo {
    pub token_type: ShieldedTokenType,
    pub value: u128,
}
