//! [`LocalWallet`]: the [`WalletFacade`] implementation for a [`Wallet`] this
//! process owns. Its builds for each ledger generation are in the generation
//! modules.

use async_trait::async_trait;
use midnight_types::chain_pin::{ChainView, current_pin};
use midnight_types::hd::Seed;
use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, LedgerVersion, Network,
    SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, WalletBalance, WalletError,
};
use midnight_wallet_facade::{IntoWalletFacade, WalletFacade};
use tokio::sync::RwLock;

use crate::Wallet;
use crate::sync::require_same_chain;

/// A [`Wallet`] this process owns, shared behind its own lock.
///
/// The lock is a private field and no method hands it out, so a consumer that
/// holds a `LocalWallet` cannot block a sync by keeping a guard alive.
pub struct LocalWallet {
    inner: RwLock<Wallet>,
}

impl LocalWallet {
    /// Put a synced [`Wallet`] behind its own lock.
    ///
    /// `MidnightProvider::with_wallet(wallet)` does this for a `Wallet`. To
    /// keep a handle on the attached wallet, put it in
    /// `Arc::new(LocalWallet::new(wallet))`. Then attach a clone of the `Arc`.
    pub fn new(wallet: Wallet) -> Self {
        Self {
            inner: RwLock::new(wallet),
        }
    }

    pub(crate) fn inner(&self) -> &RwLock<Wallet> {
        &self.inner
    }
}

impl std::fmt::Debug for LocalWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalWallet").finish_non_exhaustive()
    }
}

/// A synced [`Wallet`] attaches to a provider as a `LocalWallet`.
impl IntoWalletFacade for Wallet {
    type Facade = LocalWallet;

    fn into_facade(self) -> LocalWallet {
        LocalWallet::new(self)
    }
}

#[async_trait]
impl WalletFacade for LocalWallet {
    async fn network(&self) -> Network {
        Network::from(self.inner.read().await.network())
    }

    async fn ledger_version(&self) -> LedgerVersion {
        self.inner.read().await.ledger_version()
    }

    async fn seed(&self) -> Seed {
        self.inner.read().await.seed().clone()
    }

    async fn shielded_public_keys(&self) -> (CoinPublicKey, EncryptionPublicKey) {
        self.inner.read().await.shielded_public_keys()
    }

    async fn balance(&self) -> WalletBalance {
        self.inner.read().await.balance()
    }

    async fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        self.inner.read().await.spendable_shielded_coins()
    }

    async fn unshielded_utxos(&self) -> Vec<TrackedUtxo> {
        self.inner.read().await.unshielded_utxos().to_vec()
    }

    async fn parameters(&self) -> ChainParameters {
        self.inner.read().await.parameters()
    }

    async fn sync_cursors(&self) -> SyncCursors {
        self.inner.read().await.sync_cursors()
    }

    async fn dust_synced(&self) -> bool {
        self.inner.read().await.dust_synced()
    }

    async fn release(&self, spent: &SpentInputs) {
        self.inner.write().await.release(spent);
    }

    async fn has_observed(&self, spent: &[SpentInputs]) -> bool {
        self.inner.read().await.has_observed(spent)
    }

    async fn resync(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        let (pin, snapshot, indexer_url) = {
            let wallet = self.inner.read().await;
            (
                wallet.chain_pin().cloned(),
                wallet.snapshot_dir(),
                wallet.indexer_url().to_string(),
            )
        };

        // Take the replacement pin before the check; see
        // `require_same_chain`.
        let replacement = if pin.is_some() {
            current_pin(chain).await
        } else {
            None
        };

        if let Some(pin) = &pin {
            require_same_chain(chain, pin, snapshot.as_deref()).await?;
        }

        // The plan is snapshotted under a read lock and the commit applied
        // under a write one, so the replay in between runs with the wallet
        // free.
        let plan = self.inner.read().await.resync_plan();
        let commit = plan.run(&indexer_url).await?;
        self.inner.write().await.commit_resync(commit)?;

        // Move the pin forward with the commit, so a wallet that runs for a
        // long time keeps a recent mark rather than one an archive has since
        // pruned, which would leave every later check inconclusive.
        if let Some(fresh) = replacement {
            self.inner.write().await.set_chain_pin(fresh);
        }
        Ok(())
    }

    async fn rescan_shielded(&self) -> Result<(), WalletError> {
        let (plan, indexer_url) = {
            let wallet = self.inner.read().await;
            (
                wallet.shielded_rescan_plan(),
                wallet.indexer_url().to_string(),
            )
        };
        let commit = plan.run(&indexer_url).await?;
        self.inner.write().await.commit_shielded_rescan(commit)
    }

    async fn watch_for_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        self.inner.write().await.watch_for_coins(coins)
    }

    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        self.inner.write().await.forget_coins(coins)
    }
}
