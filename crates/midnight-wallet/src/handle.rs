//! [`Wallet`]: the handle on a synced wallet, and its [`WalletFacade`]
//! implementation. Its builds for each ledger generation are in the
//! generation modules.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use midnight_types::chain_pin::{ChainView, current_pin};
use midnight_types::hd::Seed;
use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, LedgerVersion, Network,
    SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, WalletBalance, WalletError,
};
use midnight_wallet_facade::WalletFacade;
use tokio::sync::{Mutex, RwLock};

use crate::background::{self, BackgroundSync};
use crate::sync::require_same_chain;
use crate::wallet::WalletState;

/// A synced Midnight wallet: its identity (seed, addresses) and its ledger
/// state.
///
/// [`WalletBuilder::sync`](crate::WalletBuilder::sync) gives one. A `Wallet`
/// is a handle, and every clone of it is the same wallet: a change through
/// one clone shows through all of them. Attach a clone to a provider and keep
/// the original to read or sync the wallet.
///
/// The wallet keeps three streams of state from the indexer:
/// - `zswapLedgerEvents` → shielded coin tracking + Merkle tree
/// - `dustLedgerEvents` → dust/fee UTXO tracking
/// - `unshieldedTransactions` → unshielded UTXO balance
///
/// The state is in the ledger generation its chain runs
/// ([`WalletFacade::ledger_version`]). A sync reads the generation from the
/// chain. A sync or resync that meets a hard fork carries the wallet across
/// it. The shielded state continues in the new generation, and the Dust state
/// starts empty, because the fork emptied the chain's Dust state.
///
/// The identity does not change after the sync, so its readers are plain
/// methods: [`Self::network`], [`Self::seed`] and the three addresses. The
/// readers of the synced state, such as the balance, and the sync methods are
/// [`WalletFacade`] methods, so import the trait to call them. Each read
/// returns an owned value. The lock over the state is a private field, so a
/// caller cannot block a sync by keeping a guard alive.
///
/// The wallet serializes its resyncs, rescans, coin watches and forgets
/// against each other, across all its clones. [`Self::sync_in_background`]
/// resyncs it in a background task.
///
/// # Examples
///
/// ```rust,no_run
/// use midnight_provider::MidnightProvider;
/// use midnight_wallet::{Wallet, WalletFacade};
///
/// # const INDEXER_URL: &str = "http://localhost:8088";
/// # async fn example(provider: MidnightProvider, phrase: &str) -> Result<(), Box<dyn std::error::Error>> {
/// let wallet = Wallet::from_mnemonic(phrase)?.sync(&provider, INDEXER_URL).await?;
/// let provider = provider.with_wallet(wallet.clone());
///
/// let address = wallet.unshielded_address();
/// let balance = wallet.balance().await;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Wallet {
    shared: Arc<Shared>,
}

/// What every clone of a [`Wallet`] holds.
struct Shared {
    seed: Seed,
    network: Network,
    unshielded_address: String,
    inner: RwLock<WalletState>,
    /// Held across each resync, rescan, coin watch and forget. Take it before
    /// `inner`. Reads and builds take `inner` alone, so the order cannot
    /// invert.
    sync: Mutex<()>,
}

impl Wallet {
    /// Put a synced state behind the lock of a new handle.
    pub(crate) fn from_state(state: WalletState) -> Self {
        Self {
            shared: Arc::new(Shared {
                seed: state.seed().clone(),
                network: state.network().clone(),
                unshielded_address: state.unshielded_address(),
                inner: RwLock::new(state),
                sync: Mutex::new(()),
            }),
        }
    }

    /// Default storage directory: `~/.midnight/wallets/`
    pub fn default_storage_dir() -> Option<PathBuf> {
        std::env::home_dir().map(|h| h.join(".midnight").join("wallets"))
    }

    /// The seed this wallet signs and derives with.
    pub fn seed(&self) -> &Seed {
        &self.shared.seed
    }

    /// The network this wallet derives addresses for. [`Network::as_str`]
    /// gives its literal name in the bech32 HRP.
    pub fn network(&self) -> &Network {
        &self.shared.network
    }

    /// The wallet's unshielded receiving address.
    pub fn unshielded_address(&self) -> String {
        self.shared.unshielded_address.clone()
    }

    /// The wallet's shielded receiving address, e.g. `mn_shield-addr_undeployed1...`.
    pub fn shielded_address(&self) -> String {
        self.shared.seed.shielded_address(self.network())
    }

    /// The wallet's Dust address, e.g. `mn_dust_undeployed1...`. Another
    /// wallet can register its tNIGHT to it, so that this wallet gets the
    /// Dust that tNIGHT generates. See [`Seed::dust_address`].
    pub fn dust_address(&self) -> String {
        self.shared.seed.dust_address(self.network())
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
    /// `chain`. The task holds it and a clone of this wallet until the handle
    /// drops. Start one background sync for each wallet.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use std::sync::Arc;
    /// use std::time::Duration;
    ///
    /// use midnight_provider::MidnightProvider;
    /// use midnight_wallet::{BackgroundSync, Wallet};
    ///
    /// # const INDEXER_URL: &str = "http://localhost:8088";
    /// # async fn example(provider: MidnightProvider) -> Result<BackgroundSync, Box<dyn std::error::Error>> {
    /// let wallet = Wallet::from_seed("0000000000000000000000000000000000000000000000000000000000000001")?
    ///     .sync(&provider, INDEXER_URL)
    ///     .await?;
    /// let provider = Arc::new(provider.with_wallet(wallet.clone()));
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
    pub fn sync_in_background(&self, chain: Arc<dyn ChainView>, pause: Duration) -> BackgroundSync {
        assert!(
            !pause.is_zero(),
            "the pause of a background sync must be above zero"
        );
        background::spawn(self.clone(), chain, pause)
    }

    pub(crate) fn inner(&self) -> &RwLock<WalletState> {
        &self.shared.inner
    }

    /// [`WalletFacade::resync`], for a caller that holds the sync lock.
    async fn resync_held(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        let (pin, snapshot) = {
            let wallet = self.inner().read().await;
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
        let plan = self.inner().read().await.resync_plan();
        let commit = plan.run().await?;
        self.inner().write().await.commit_resync(commit)?;

        // Move the pin forward with the commit, so a wallet that runs for a
        // long time keeps a recent mark rather than one an archive has since
        // pruned, which would leave every later check inconclusive.
        if let Some(fresh) = replacement {
            self.inner().write().await.set_chain_pin(fresh);
        }
        Ok(())
    }

    /// [`WalletFacade::rescan_shielded`], for a caller that holds the sync
    /// lock.
    async fn rescan_held(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        let (pin, snapshot) = {
            let wallet = self.inner().read().await;
            (wallet.chain_pin().cloned(), wallet.snapshot_dir())
        };
        if let Some(pin) = &pin {
            require_same_chain(chain, pin, snapshot.as_deref()).await?;
        }

        match self.rescan_once().await {
            // A fork the state has not crossed. `resync` would wait for the
            // sync lock that the caller holds, so cross with `resync_held`.
            Err(WalletError::LedgerMismatch { expected, found }) if found > expected => {
                self.resync_held(chain).await?;
                self.rescan_once().await
            }
            result => result,
        }
    }

    async fn rescan_once(&self) -> Result<(), WalletError> {
        let plan = self.inner().read().await.shielded_rescan_plan();
        let commit = plan.run().await?;
        self.inner().write().await.commit_shielded_rescan(commit)
    }
}

impl std::fmt::Debug for Wallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wallet")
            .field("network", &self.shared.network)
            .field("unshielded_address", &self.shared.unshielded_address)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl WalletFacade for Wallet {
    async fn network(&self) -> Network {
        self.shared.network.clone()
    }

    async fn ledger_version(&self) -> LedgerVersion {
        self.inner().read().await.ledger_version()
    }

    async fn shielded_public_keys(&self) -> (CoinPublicKey, EncryptionPublicKey) {
        self.inner().read().await.shielded_public_keys()
    }

    async fn balance(&self) -> WalletBalance {
        self.inner().read().await.balance()
    }

    async fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        self.inner().read().await.spendable_shielded_coins()
    }

    async fn unshielded_utxos(&self) -> Vec<TrackedUtxo> {
        self.inner().read().await.unshielded_utxos().to_vec()
    }

    async fn parameters(&self) -> ChainParameters {
        self.inner().read().await.parameters()
    }

    async fn sync_cursors(&self) -> SyncCursors {
        self.inner().read().await.sync_cursors()
    }

    async fn dust_synced(&self) -> bool {
        self.inner().read().await.dust_synced()
    }

    async fn release(&self, spent: &SpentInputs) {
        self.inner().write().await.release(spent);
    }

    async fn resync(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        let _sync = self.shared.sync.lock().await;
        self.resync_held(chain).await
    }

    async fn rescan_shielded(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        let _sync = self.shared.sync.lock().await;
        self.rescan_held(chain).await
    }

    async fn watch_for_coins(
        &self,
        coins: Vec<CoinInfo>,
        chain: &dyn ChainView,
    ) -> Result<(), WalletError> {
        if coins.is_empty() {
            return Ok(());
        }
        let _sync = self.shared.sync.lock().await;
        self.inner().write().await.watch_for_coins(coins)?;
        self.rescan_held(chain).await
    }

    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        // Wait for a resync in flight, although this is one write. That resync
        // commits the state its plan read, so a registration dropped here would
        // come back.
        let _sync = self.shared.sync.lock().await;
        self.inner().write().await.forget_coins(coins)
    }
}
