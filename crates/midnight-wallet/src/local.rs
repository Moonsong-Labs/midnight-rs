//! [`Wallet`]: a synced wallet that locks its own state. Its builds are in
//! `crate::builds`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use midnight_types::chain_pin::{ChainView, current_pin};
use midnight_types::{
    CoinInfo, Network, ShieldedRecipient, SpendableShieldedCoin, SpentInputs, UnshieldedRecipient,
    WalletBalance, WalletError,
};
use midnight_wallet_facade::WalletFacade;
use tokio::sync::{Mutex, RwLock, mpsc};

use crate::SyncProgress;
use crate::background::{self, BackgroundSync};
use crate::sync::{SyncPlan, require_same_chain};
use crate::wallet::WalletState;

/// A synced Midnight wallet.
///
/// [`Wallet::from_seed`] starts one, and the sync of its
/// [`WalletBuilder`](crate::WalletBuilder) makes it. It is not `Clone`. It
/// locks its own state, so a `&Wallet` serves every call, and an
/// `Arc<Wallet>` shares it with a background sync and a provider.
///
/// The network, the addresses and the recipients are fixed when the sync
/// makes the wallet, so their readers take no lock. The synced state sits
/// behind a lock that no method hands out, so a caller cannot block a sync by
/// keeping a guard alive. Reads and builds take that lock alone.
///
/// The wallet serializes its syncs and coin watches against each other.
/// [`Self::sync_in_background`] syncs it in a background task.
pub struct Wallet {
    state: RwLock<WalletState>,
    /// Held across each sync and coin watch. Take it before `state`. Reads
    /// and builds take `state` alone, so the order cannot invert.
    sync: Mutex<()>,
    network: Network,
    unshielded_address: String,
    shielded_address: String,
    dust_address: String,
    shielded_recipient: ShieldedRecipient,
    unshielded_recipient: UnshieldedRecipient,
}

impl Wallet {
    /// Run the sync of `plan`, and make the wallet of its seed.
    pub(crate) async fn synced(
        plan: SyncPlan,
        progress: Option<mpsc::Sender<SyncProgress>>,
    ) -> Result<Self, WalletError> {
        let state = WalletState::sync_inner(plan, progress).await?;
        let seed = state.seed();
        Ok(Self {
            network: state.network().clone(),
            unshielded_address: state.unshielded_address(),
            shielded_address: seed.shielded_address(state.network()),
            dust_address: seed.dust_address(state.network()),
            shielded_recipient: state.shielded_recipient(),
            unshielded_recipient: seed.unshielded_recipient(),
            state: RwLock::new(state),
            sync: Mutex::new(()),
        })
    }

    pub(crate) fn state(&self) -> &RwLock<WalletState> {
        &self.state
    }

    /// The default storage directory: `~/.midnight/wallets/`. Pass it to
    /// [`WalletBuilder::with_storage`](crate::WalletBuilder::with_storage).
    pub fn default_storage_dir() -> Option<PathBuf> {
        std::env::home_dir().map(|h| h.join(".midnight").join("wallets"))
    }

    /// The network this wallet derives addresses for. [`Network::as_str`]
    /// gives its literal name in the bech32 HRP.
    pub fn network(&self) -> &Network {
        &self.network
    }

    /// The wallet's unshielded receiving address, e.g. `mn_addr_undeployed1...`.
    pub fn unshielded_address(&self) -> &str {
        &self.unshielded_address
    }

    /// The wallet's shielded receiving address, e.g. `mn_shield-addr_undeployed1...`.
    pub fn shielded_address(&self) -> &str {
        &self.shielded_address
    }

    /// The wallet's Dust address, e.g. `mn_dust_undeployed1...`. Another
    /// wallet can register its tNIGHT to it, so that this wallet gets the
    /// Dust that tNIGHT generates. See [`Seed::dust_address`](crate::Seed::dust_address).
    pub fn dust_address(&self) -> &str {
        &self.dust_address
    }

    /// The wallet's shielded public keys: the coin public key an output
    /// commits to, and the encryption key its discovery ciphertext is sealed
    /// to.
    ///
    /// Public material, so anything that only needs to address a coin to this
    /// wallet can take these instead of the seed.
    pub fn shielded_recipient(&self) -> ShieldedRecipient {
        self.shielded_recipient
    }

    /// The wallet's user address, the recipient of an unshielded transfer to
    /// this wallet. See [`Seed::unshielded_recipient`](crate::Seed::unshielded_recipient).
    pub fn unshielded_recipient(&self) -> UnshieldedRecipient {
        self.unshielded_recipient
    }

    /// What this wallet holds, across shielded coins, unshielded UTXOs and
    /// Dust, as of its last sync.
    ///
    /// The balance includes the inputs that a pending build reserves.
    /// [`DustBalance::spendable_speck`](crate::DustBalance::spendable_speck)
    /// and [`DustBalance::spendable_utxos`](crate::DustBalance::spendable_utxos)
    /// leave out the Dust that a pending build reserves. Each coin and UTXO
    /// that a pending build reserves reads `spendable: false`.
    ///
    /// A read does not sync. A build through the provider syncs first, and
    /// [`Self::sync_in_background`] syncs on a timer. For one fresh read, call
    /// [`Self::sync`] before it.
    pub async fn balance(&self) -> WalletBalance {
        self.state.read().await.balance()
    }

    /// The wallet's spendable shielded coins, each with its full coin info
    /// (nonce, token type, value) plus the nullifier that pins it.
    ///
    /// Use this to address a specific coin for a circuit that spends it (e.g.
    /// `receiveShielded`): build the `ShieldedCoinInfo` argument from the
    /// coin's `nonce`/`token_type`/`value`, then hand the same coin back to the
    /// call builder so the SDK spends that exact coin as the shielded input.
    /// Coins a still-pending build already spent are left out.
    pub async fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        self.state.read().await.spendable_shielded_coins()
    }

    /// Resume the event streams from this wallet's cursors and apply what
    /// they deliver.
    ///
    /// The replay runs against the indexer that the wallet synced from. It
    /// holds no lock that reads take, so reads keep completing while it is in
    /// flight. The wallet serializes it with its other sync methods, so a
    /// build through the provider waits for a sync in flight.
    ///
    /// `chain` answers the two queries of the chain pin. Pass the provider.
    /// The cursors say nothing about which chain produced them, so when the
    /// wallet holds a chain pin, the sync checks it against `chain` before
    /// the replay and moves it forward after the commit. A chain that the pin
    /// no longer matches fails the sync with [`WalletError::ChainMismatch`].
    ///
    /// # Errors
    ///
    /// [`WalletError::ChainMismatch`] as above, and the error of a replay that
    /// fails. A failed sync commits nothing.
    pub async fn sync(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        let _sync = self.sync.lock().await;
        self.resync_held(chain).await
    }

    /// Sync this wallet in a background task until the returned handle drops.
    ///
    /// Each pass is one [`Self::sync`] against `chain`. It resumes the replay
    /// of the wallet's indexer from its cursors. When the wallet holds a chain
    /// pin, the pass checks it against `chain`. The first pass starts `pause`
    /// after this call, because a [`Wallet`] is synced when it is made. Each
    /// later pass starts `pause` after the one before it ends, so passes never
    /// overlap.
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
    /// use midnight_wallet::{BackgroundSync, Seed, Wallet};
    ///
    /// # const INDEXER_URL: &str = "http://localhost:8088";
    /// # async fn example(provider: MidnightProvider, seed: Seed) -> Result<BackgroundSync, Box<dyn std::error::Error>> {
    /// let wallet = Arc::new(Wallet::from_seed(seed).sync(&provider, INDEXER_URL).await?);
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
    ///     eprintln!("the last background sync failed: {error}");
    /// }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// When `pause` is zero, because the loop then syncs with no rest. Also
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

    /// Register coins this wallet owns but cannot discover, persist the
    /// registrations, and replay the shielded stream from its first event, so
    /// the replay claims them. It decrypts nothing.
    ///
    /// A shielded coin normally reaches its owner through the discovery
    /// ciphertext on its output. That channel fails when the party that built
    /// the transaction attached no ciphertext, or sealed it to a key this
    /// wallet does not hold. The coin is still owned by this wallet's coin
    /// public key and still spendable, as long as its owner can rebuild the
    /// `CoinInfo` (nonce, token type, value) from somewhere else. A contract
    /// that evolves one public nonce per mint is such a case.
    ///
    /// The replay covers the whole shielded stream, because a replay that
    /// meets an output it cannot claim collapses that Merkle leaf, and a sync
    /// resumes from its cursor instead of revisiting it. It costs more than a
    /// sync, so register every coin you know about in one call. Coins already
    /// recovered keep their place. To check the outcome, look for the coin in
    /// [`Self::spendable_shielded_coins`]. The replay rebuilds the shielded
    /// state and leaves the Dust state, the unshielded state and their cursors
    /// alone.
    ///
    /// An empty list does nothing: no write and no replay. A registration is
    /// part of the wallet state, so a storage directory persists it, and a
    /// coin registered before it lands on chain survives a restart.
    ///
    /// `chain` answers the queries of the chain pin, as for [`Self::sync`].
    /// The wallet runs the registration and the replay under one hold of its
    /// sync lock, so no sync runs between them.
    ///
    /// # Errors
    ///
    /// [`WalletError::Storage`] when the save of the registrations fails,
    /// [`WalletError::ChainMismatch`] as for [`Self::sync`], and the error of
    /// a replay that fails. A replay that fails keeps the registrations, so
    /// call this again with the same coins.
    pub async fn watch_for_coins(
        &self,
        coins: Vec<CoinInfo>,
        chain: &dyn ChainView,
    ) -> Result<(), WalletError> {
        if coins.is_empty() {
            return Ok(());
        }
        let _sync = self.sync.lock().await;
        self.state.write().await.watch_for_coins(coins)?;

        let (pin, snapshot) = {
            let wallet = self.state.read().await;
            (wallet.chain_pin().cloned(), wallet.snapshot_dir())
        };
        if let Some(pin) = &pin {
            require_same_chain(chain, pin, snapshot.as_deref()).await?;
        }

        let plan = self.state.read().await.shielded_rescan_plan();
        let commit = plan.run().await?;
        self.state.write().await.commit_shielded_rescan(commit)
    }

    /// Hand back the inputs a build reserved, because that build will never
    /// reach the chain.
    ///
    /// Reserving on build stops a later build re-selecting the same inputs, so
    /// a transaction that is rejected at submit, or built and then abandoned,
    /// holds its coins until the TTL window elapses. Releasing returns them at
    /// once. Pass what the build reported spending.
    ///
    /// Only call this for a transaction that cannot land. Releasing one that is
    /// still in flight lets a later build re-select the same inputs, and the
    /// loser is rejected on chain.
    ///
    /// The release drops an entry only when `spent` names its input with its
    /// stamp ([`ReservedInput::reserved_at`](crate::ReservedInput::reserved_at)),
    /// so a late release cannot take back an input that a later build has
    /// reserved since. Persistence is best-effort, since the in-memory release
    /// already frees the inputs for this process.
    pub async fn release(&self, spent: &SpentInputs) {
        self.state.write().await.release(spent);
    }

    /// [`Self::sync`], for a caller that holds `self.sync`.
    async fn resync_held(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        let (pin, snapshot) = {
            let wallet = self.state.read().await;
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
        let plan = self.state.read().await.resync_plan();
        let commit = plan.run().await?;
        self.state.write().await.commit_resync(commit)?;

        // Move the pin forward with the commit, so a wallet that runs for a
        // long time keeps a recent mark rather than one an archive has since
        // pruned, which would leave every later check inconclusive.
        if let Some(fresh) = replacement {
            self.state.write().await.set_chain_pin(fresh);
        }
        Ok(())
    }
}

impl std::fmt::Debug for Wallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wallet").finish_non_exhaustive()
    }
}

#[async_trait]
impl WalletFacade for Wallet {
    async fn shielded_recipient(&self) -> ShieldedRecipient {
        self.shielded_recipient
    }

    async fn balance(&self) -> WalletBalance {
        Wallet::balance(self).await
    }

    async fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        Wallet::spendable_shielded_coins(self).await
    }

    async fn release(&self, spent: &SpentInputs) {
        Wallet::release(self, spent).await
    }

    async fn persist_reservation(&self, spent: &SpentInputs) {
        self.state.write().await.persist_reservation(spent);
    }

    async fn resync(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        self.sync(chain).await
    }

    async fn watch_for_coins(
        &self,
        coins: Vec<CoinInfo>,
        chain: &dyn ChainView,
    ) -> Result<(), WalletError> {
        Wallet::watch_for_coins(self, coins, chain).await
    }
}
