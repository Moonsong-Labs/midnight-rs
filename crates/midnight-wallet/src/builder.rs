use std::sync::Arc;

use midnight_node_ledger_helpers::{DefaultDB, ProofProvider};
use tokio::sync::{mpsc, RwLock};

use crate::background::WalletSync;
use crate::balance::WalletBalance;
use crate::state::{SyncProgress, WalletState};
use crate::transfer::TransferBuilder;
use crate::{Wallet, WalletError};

pub struct WalletBuilder {
    wallet: Wallet,
    node_url: String,
    indexer_url: String,
}

impl WalletBuilder {
    pub fn new(
        wallet: Wallet,
        node_url: impl Into<String>,
        indexer_url: impl Into<String>,
    ) -> Self {
        Self {
            wallet,
            node_url: node_url.into(),
            indexer_url: indexer_url.into(),
        }
    }

    pub async fn build(self) -> Result<LiveWallet, WalletError> {
        let address = self.wallet.unshielded_address();

        let state = WalletState::sync_from_indexer(
            &self.node_url,
            &self.indexer_url,
            *self.wallet.seed(),
            &address,
            self.wallet.network(),
        )
        .await?;

        let state = Arc::new(RwLock::new(state));
        let sync = WalletSync::spawn(state.clone(), address);

        Ok(LiveWallet {
            wallet: self.wallet,
            state,
            sync: Some(sync),
        })
    }

    /// Like [`build`](Self::build), but returns a progress receiver alongside
    /// the build handle. The receiver emits [`SyncProgress`] updates during
    /// the initial sync. Once the sync completes, call `.await` on the handle
    /// to get the `LiveWallet`.
    pub async fn build_with_progress(
        self,
    ) -> (
        mpsc::Receiver<SyncProgress>,
        tokio::task::JoinHandle<Result<LiveWallet, WalletError>>,
    ) {
        let address = self.wallet.unshielded_address();
        let network = self.wallet.network().to_string();
        let seed = *self.wallet.seed();
        let (rx, sync_handle) = WalletState::sync_with_progress(
            &self.node_url,
            &self.indexer_url,
            seed,
            &address,
            &network,
            None,
        )
        .await;

        let wallet = self.wallet;
        let handle = tokio::spawn(async move {
            let state = sync_handle
                .await
                .map_err(|e| WalletError::Sync(format!("sync task panicked: {e}")))??;
            let state = Arc::new(RwLock::new(state));
            let sync = WalletSync::spawn(state.clone(), address);
            Ok(LiveWallet {
                wallet,
                state,
                sync: Some(sync),
            })
        });

        (rx, handle)
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

    /// Create a [`TransferBuilder`] for building transfer transactions.
    ///
    /// Refreshes the block context and builds a `LedgerContext` from the
    /// wallet's indexed state without requiring a full-chain-replay from
    /// the node.
    pub async fn transfer(
        &self,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<TransferGuard<'_>, WalletError> {
        let context = self.state.write().await.build_context().await?;
        let guard = self.state.read().await;

        Ok(TransferGuard {
            guard,
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
        if let Some(sync) = &self.sync {
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
