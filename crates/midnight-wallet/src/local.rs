//! [`LocalWallet`]: the [`WalletFacade`] implementation for a [`Wallet`] this
//! process owns. Its builds for each ledger generation are in the generation
//! modules.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use midnight_types::chain_pin::{ChainView, current_pin};
use midnight_types::hd::Seed;
use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, LedgerVersion, Network,
    SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, WalletBalance, WalletError,
};
use midnight_wallet_facade::{IntoWalletFacade, WalletFacade};
use tokio::sync::{Mutex, RwLock};

use crate::Wallet;
use crate::background::{self, BackgroundSync};
use crate::sync::require_same_chain;

/// A [`Wallet`] this process owns, shared behind its own lock.
///
/// The lock is a private field and no method hands it out, so a consumer that
/// holds a `LocalWallet` cannot block a sync by keeping a guard alive.
///
/// `LocalWallet` serializes its resyncs, rescans, coin watches and forgets
/// against each other. [`Self::sync_in_background`] resyncs it in a
/// background task.
pub struct LocalWallet {
    inner: RwLock<Wallet>,
    /// Held across each resync, rescan, coin watch and forget. Take it before
    /// `inner`. Reads and builds take `inner` alone, so the order cannot
    /// invert.
    sync: Mutex<()>,
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
            sync: Mutex::new(()),
        }
    }

    /// Resync this wallet in a background task until the returned handle
    /// drops.
    ///
    /// Each pass is one [`WalletFacade::resync`] against `chain`. It resumes
    /// the replay of the wallet's indexer from its cursors. When the wallet
    /// holds a chain pin, the pass checks it against `chain`. The first pass
    /// starts `pause` after this call, because a [`Wallet`] is synced when it
    /// is made. Each later pass starts `pause` after the one before it ends,
    /// so passes never overlap.
    ///
    /// A failed pass does not stop the sync. The task logs the error, and
    /// [`BackgroundSync::last_error`] holds it until a pass commits. The next
    /// pass runs after `pause`.
    ///
    /// The wallet serializes each pass with its other sync methods. So a build
    /// through the provider waits for a pass in flight.
    ///
    /// Pass the `Arc<MidnightProvider>` that the wallet is attached to as
    /// `chain`. The task holds it until the handle drops. Start one background
    /// sync for each wallet.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use std::sync::Arc;
    /// use std::time::Duration;
    ///
    /// use midnight_provider::MidnightProvider;
    /// use midnight_wallet::{BackgroundSync, LocalWallet, Wallet};
    ///
    /// # async fn example(provider: MidnightProvider) -> Result<BackgroundSync, Box<dyn std::error::Error>> {
    /// let wallet = Wallet::from_seed("0000000000000000000000000000000000000000000000000000000000000001")?
    ///     .sync(&provider)
    ///     .await?;
    /// let wallet = Arc::new(LocalWallet::new(wallet));
    /// let provider = Arc::new(provider.with_wallet(Arc::clone(&wallet)));
    ///
    /// // Keep the handle for as long as the wallet must stay fresh. Dropping
    /// // it stops the background sync.
    /// let sync = wallet.sync_in_background(provider.clone(), Duration::from_secs(5));
    /// # Ok(sync)
    /// # }
    /// ```
    ///
    /// Read [`BackgroundSync::last_error`] later, for example on each tick of
    /// a UI:
    ///
    /// ```rust,no_run
    /// # fn tick(sync: &midnight_wallet::BackgroundSync) {
    /// if let Some(error) = sync.last_error() {
    ///     eprintln!("the last background resync failed: {error}");
    /// }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// When `pause` is zero, because the loop then resyncs with no rest. Also
    /// when called outside a Tokio runtime.
    pub fn sync_in_background(
        self: &Arc<Self>,
        chain: Arc<dyn ChainView>,
        pause: Duration,
    ) -> BackgroundSync {
        assert!(
            !pause.is_zero(),
            "the pause of a background sync must be above zero"
        );
        background::spawn(Arc::clone(self), chain, pause)
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
        self.inner.read().await.network().clone()
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

    async fn resync(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        let _sync = self.sync.lock().await;
        let (pin, snapshot) = {
            let wallet = self.inner.read().await;
            (wallet.chain_pin().cloned(), wallet.snapshot_dir())
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
        let commit = plan.run().await?;
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
        let _sync = self.sync.lock().await;
        let plan = self.inner.read().await.shielded_rescan_plan();
        let commit = plan.run().await?;
        self.inner.write().await.commit_shielded_rescan(commit)
    }

    async fn watch_for_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        let _sync = self.sync.lock().await;
        self.inner.write().await.watch_for_coins(coins)
    }

    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        // Wait for a resync in flight, although this is one write. That resync
        // commits the state its plan read, so a registration dropped here would
        // come back.
        let _sync = self.sync.lock().await;
        self.inner.write().await.forget_coins(coins)
    }
}
