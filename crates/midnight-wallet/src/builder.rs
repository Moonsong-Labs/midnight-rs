use std::sync::Arc;

use midnight_node_ledger_helpers::{DefaultDB, ProofProvider};
use tokio::sync::RwLock;

use crate::background::WalletSync;
use crate::balance::WalletBalance;
use crate::state::{SyncResult, WalletState};
use crate::transfer::TransferBuilder;
use crate::{Wallet, WalletError};

pub struct WalletBuilder {
    wallet: Wallet,
    node_url: String,
    indexer_url: Option<String>,
}

impl WalletBuilder {
    pub fn new(wallet: Wallet, node_url: impl Into<String>) -> Self {
        Self {
            wallet,
            node_url: node_url.into(),
            indexer_url: None,
        }
    }

    /// Set the indexer URL for subscription-based balance tracking.
    ///
    /// When set, the wallet uses the indexer for real-time balance updates
    /// instead of periodic full-chain replay from the node.
    pub fn indexer_url(mut self, url: impl Into<String>) -> Self {
        self.indexer_url = Some(url.into());
        self
    }

    pub async fn build(self) -> Result<LiveWallet, WalletError> {
        let address = self.wallet.unshielded_address();

        let state = if let Some(ref indexer_url) = self.indexer_url {
            WalletState::sync_from_indexer(
                &self.node_url,
                indexer_url,
                *self.wallet.seed(),
                &address,
            )
            .await?
        } else {
            WalletState::sync_from_node(&self.node_url, *self.wallet.seed()).await?
        };

        let state = Arc::new(RwLock::new(state));

        let sync = if self.indexer_url.is_some() {
            Some(WalletSync::spawn(state.clone(), address))
        } else {
            None
        };

        Ok(LiveWallet {
            wallet: self.wallet,
            state,
            sync,
        })
    }
}

pub struct LiveWallet {
    wallet: Wallet,
    state: Arc<RwLock<WalletState>>,
    sync: Option<WalletSync>,
}

impl LiveWallet {
    pub fn wallet(&self) -> &Wallet {
        &self.wallet
    }

    pub fn state(&self) -> &Arc<RwLock<WalletState>> {
        &self.state
    }

    pub async fn balance(&self) -> WalletBalance {
        self.state.read().await.balance()
    }

    /// Sync a LedgerContext from the node for transaction building.
    ///
    /// This is separate from balance tracking (which uses the indexer).
    /// Only needed before building/signing transactions.
    pub async fn sync_context(&self) -> Result<SyncResult, WalletError> {
        let mut guard = self.state.write().await;
        guard.sync_context().await?;
        Ok(SyncResult {
            blocks_processed: 0,
            height: guard.last_synced_height(),
        })
    }

    /// Create a [`TransferBuilder`] for building transfer transactions.
    ///
    /// Syncs a `LedgerContext` from the node if not already cached, then
    /// returns a builder that can construct and prove transfers.
    pub async fn transfer(
        &self,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<TransferGuard<'_>, WalletError> {
        let context = {
            let mut guard = self.state.write().await;
            guard.sync_context().await?
        };

        Ok(TransferGuard {
            guard: self.state.read().await,
            context,
            proof_provider,
        })
    }

    pub async fn shutdown(mut self) {
        if let Some(sync) = self.sync.take() {
            sync.shutdown().await;
        }
    }
}

impl Drop for LiveWallet {
    fn drop(&mut self) {
        if let Some(sync) = self.sync.take() {
            sync.cancel();
        }
    }
}

/// Holds a read-lock on the wallet state and provides a [`TransferBuilder`].
pub struct TransferGuard<'a> {
    guard: tokio::sync::RwLockReadGuard<'a, WalletState>,
    context: Arc<midnight_node_ledger_helpers::LedgerContext<DefaultDB>>,
    proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
}

impl<'a> TransferGuard<'a> {
    pub fn builder(&'a self) -> TransferBuilder<'a> {
        TransferBuilder::new(
            &self.guard,
            self.context.clone(),
            self.proof_provider.clone(),
        )
    }
}
