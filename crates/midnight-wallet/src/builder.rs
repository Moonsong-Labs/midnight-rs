use std::sync::Arc;
use std::time::Duration;

use midnight_node_ledger_helpers::{DefaultDB, ProofProvider};
use tokio::sync::RwLock;

use crate::background::WalletSync;
use crate::balance::WalletBalance;
use crate::state::{SyncResult, WalletState};
use crate::transfer::TransferBuilder;
use crate::{Wallet, WalletError};

const DEFAULT_SYNC_INTERVAL: Duration = Duration::from_secs(30);

pub struct WalletBuilder {
    wallet: Wallet,
    node_url: String,
    sync_interval: Duration,
}

impl WalletBuilder {
    pub fn new(wallet: Wallet, node_url: impl Into<String>) -> Self {
        Self {
            wallet,
            node_url: node_url.into(),
            sync_interval: DEFAULT_SYNC_INTERVAL,
        }
    }

    pub fn sync_interval(mut self, interval: Duration) -> Self {
        self.sync_interval = interval;
        self
    }

    pub async fn build(self) -> Result<LiveWallet, WalletError> {
        let state = WalletState::sync_from_node(&self.node_url, *self.wallet.seed()).await?;
        let state = Arc::new(RwLock::new(state));
        let sync = WalletSync::spawn(state.clone(), self.sync_interval);

        Ok(LiveWallet {
            wallet: self.wallet,
            state,
            sync: Some(sync),
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

    pub async fn sync(&self) -> Result<SyncResult, WalletError> {
        self.state.write().await.resync().await
    }

    /// Create a [`TransferBuilder`] for building transfer transactions.
    ///
    /// The returned builder borrows the wallet state read-lock guard, so
    /// callers must hold the guard for the duration of the transfer build.
    /// For typical usage, prefer the async `transfer_with` helper or access
    /// the state directly.
    pub async fn transfer(
        &self,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> TransferGuard<'_> {
        TransferGuard {
            guard: self.state.read().await,
            proof_provider,
        }
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
    proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
}

impl<'a> TransferGuard<'a> {
    pub fn builder(&'a self) -> TransferBuilder<'a> {
        TransferBuilder::new(&self.guard, self.proof_provider.clone())
    }
}
