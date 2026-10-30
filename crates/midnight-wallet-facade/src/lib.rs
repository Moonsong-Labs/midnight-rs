//! The API a consumer programs a Midnight wallet against.
//!
//! [`WalletFacade`] names the role. Every reading returns an owned value and
//! every mutation is one call, so nothing a caller holds is a lock and no
//! implementation is committed to a particular way of sharing its state. This
//! crate carries the traits alone and speaks in `midnight-types`'s
//! vocabulary, so it depends on no wallet implementation; `midnight-wallet`
//! implements them with `LocalWallet`, over a `Wallet` that process owns.
//!
//! [`WalletFacade`] is the same for every ledger generation. The builds are
//! not, because they name a generation's transaction types:
//! [`ledger_8::WalletBuilds`] and [`ledger_9::WalletBuilds`] carry them, one
//! source compiled for each generation. A wallet that serves both implements
//! both, and a build dispatches on [`WalletFacade::ledger_version`].
//!
//! An implementation must serialize [`WalletFacade::resync`],
//! [`WalletFacade::rescan_shielded`], [`WalletFacade::watch_for_coins`] and
//! [`WalletFacade::forget_coins`] against each other. A resync or a rescan
//! releases the wallet between what it reads and what it commits. A sync
//! method that runs in that window can lose its work, or the work of the
//! other one. No caller holds a lock across them: `MidnightProvider` and every
//! handle on a shared wallet call them directly. So when a rescan needs a
//! resync first, or a coin watch needs a rescan, the implementation runs both
//! steps under one hold.

use std::sync::Arc;

use async_trait::async_trait;
use midnight_types::chain_pin::ChainView;
use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, LedgerVersion, Network,
    SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, WalletBalance, WalletError,
};

pub mod ledger_8;
#[expect(
    clippy::duplicate_mod,
    reason = "`ledger_8` and `ledger_9` compile the same per-ledger source against each generation"
)]
pub mod ledger_9;

/// One wallet, as the API its consumers program against.
///
/// Readings return owned values rather than a guard, so the implementation
/// chooses how its state is shared. Selection and reservation are one call, so
/// the hold that keeps them consistent lives inside the implementation: a
/// second consumer that selected between them would draw the same input twice.
#[async_trait]
pub trait WalletFacade: Send + Sync {
    /// The network this wallet derives addresses for.
    async fn network(&self) -> Network;

    /// The ledger generation this wallet's state is in, which is the one its
    /// chain ran at the last sync or resync.
    ///
    /// A build dispatches on it to this wallet's `WalletBuilds` of that
    /// generation.
    async fn ledger_version(&self) -> LedgerVersion;

    /// The public keys a coin addressed to this wallet commits to.
    async fn shielded_public_keys(&self) -> (CoinPublicKey, EncryptionPublicKey);

    /// What this wallet holds, across shielded coins, unshielded UTXOs and
    /// Dust.
    ///
    /// The balance includes the inputs that a pending build reserves. An
    /// implementation fills [`DustBalance::spendable_speck`] and
    /// [`DustBalance::spendable_utxos`] with the Dust that a new build can draw
    /// on, which leaves out the Dust that its pending builds reserve. A field
    /// left at its `Default` reads as no spendable Dust. An implementation
    /// also sets [`ShieldedCoinBalance::spendable`] and
    /// [`UnshieldedUtxoInfo::spendable`] to `false` on each coin and UTXO that
    /// a pending build reserves. [`WalletBalance::spendable_shielded_value`]
    /// and [`WalletBalance::spendable_unshielded_value`] read these flags, so
    /// they leave out the inputs that a pending build reserves.
    ///
    /// [`DustBalance::spendable_speck`]: midnight_types::DustBalance::spendable_speck
    /// [`DustBalance::spendable_utxos`]: midnight_types::DustBalance::spendable_utxos
    /// [`ShieldedCoinBalance::spendable`]: midnight_types::ShieldedCoinBalance::spendable
    /// [`UnshieldedUtxoInfo::spendable`]: midnight_types::UnshieldedUtxoInfo::spendable
    async fn balance(&self) -> WalletBalance;

    /// The shielded coins this wallet can spend, each with the nullifier that
    /// pins it.
    async fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin>;

    /// The unshielded UTXOs this wallet tracks.
    async fn unshielded_utxos(&self) -> Vec<TrackedUtxo>;

    /// The chain's Dust and TTL parameters, as this wallet last synced them.
    async fn parameters(&self) -> ChainParameters;

    /// How far this wallet's sync has reached.
    async fn sync_cursors(&self) -> SyncCursors;

    /// Whether this wallet has completed its Dust sync.
    async fn dust_synced(&self) -> bool;

    /// Hand back what a build reserved, because that build will never reach
    /// the chain.
    ///
    /// Only for a transaction that cannot land. Releasing one still in flight
    /// lets a later build re-select the same inputs, and the loser is rejected
    /// on chain.
    ///
    /// This drops an entry only when `spent` names its input with its stamp
    /// ([`ReservedInput::reserved_at`](midnight_types::ReservedInput::reserved_at)).
    /// Each input carries its own stamp, so one release can hand back the
    /// entries of two reservations. A build that releases late cannot take
    /// back an input a later build has since reserved, because the later
    /// reservation carries another stamp.
    async fn release(&self, spent: &SpentInputs);

    /// Resume the event streams from this wallet's cursors and apply what they
    /// deliver.
    ///
    /// The replay holds no lock that reads take, so reads keep completing
    /// while it is in flight. Which indexer it resumes against is the
    /// wallet's own business: the cursors are event counts, so only the
    /// server that produced them can continue them.
    ///
    /// `chain` answers the two queries that a chain pin makes. The cursors say
    /// nothing about which chain produced them, so a wallet that stays
    /// attached while the chain is replaced would otherwise replay onto the
    /// fresh chain and keep serving the old one's balance. An implementation
    /// that pins its snapshot checks it here and moves it forward afterwards.
    async fn resync(&self, chain: &dyn ChainView) -> Result<(), WalletError>;

    /// Replay the shielded event stream from its first event and rebuild the
    /// shielded state from it. Dust state, unshielded state, and their cursors
    /// are left alone.
    ///
    /// `chain` answers the queries of a chain pin, as in [`Self::resync`]. An
    /// implementation that pins its snapshot checks the pin before the replay.
    /// When the state is in an earlier ledger generation than the chain, the
    /// implementation first crosses the fork with a resync against `chain`.
    /// A replay in the earlier generation cannot apply the events of the
    /// later one.
    async fn rescan_shielded(&self, chain: &dyn ChainView) -> Result<(), WalletError>;

    /// Register coins this wallet owns but cannot discover, persist the
    /// registrations, and then rescan as [`Self::rescan_shielded`] does, so
    /// the replay claims them. See `Wallet::watch_for_coin` on the
    /// implementing wallet.
    ///
    /// An empty list does nothing: no write and no replay.
    ///
    /// A replay that fails keeps the registration. Retry with
    /// [`Self::rescan_shielded`] rather than registering again.
    async fn watch_for_coins(
        &self,
        coins: Vec<CoinInfo>,
        chain: &dyn ChainView,
    ) -> Result<(), WalletError>;

    /// Drop registrations that matched no on-chain output, as
    /// [`Self::watched_coins`] lists them.
    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError>;

    /// The registrations ([`Self::watch_for_coins`]) that no replay has
    /// claimed yet.
    ///
    /// They are ordered by nonce, then token type, then value. See
    /// `Wallet::watched_coins` on the implementing wallet.
    async fn watched_coins(&self) -> Vec<CoinInfo>;
}

/// A value that `MidnightProvider::with_wallet` attaches as its wallet.
///
/// A wallet that serves the builds of both ledger generations goes in as
/// itself. `midnight-wallet` also implements it for its synced `Wallet`,
/// which goes in behind the lock of a `LocalWallet`.
pub trait IntoWalletFacade {
    /// The wallet the provider holds.
    type Facade: ledger_8::WalletBuilds + ledger_9::WalletBuilds + 'static;

    /// Convert into the wallet the provider holds.
    fn into_facade(self) -> Self::Facade;
}

impl<W> IntoWalletFacade for W
where
    W: ledger_8::WalletBuilds + ledger_9::WalletBuilds + 'static,
{
    type Facade = W;

    fn into_facade(self) -> W {
        self
    }
}

/// A shared wallet is a wallet: every call forwards to the one inside.
///
/// This lets a caller keep a handle on the wallet it attaches instead of
/// giving ownership away, and lets several callers share one implementation.
/// A handle can call every method, from any number of tasks or providers,
/// because the implementation serializes its sync methods.
#[async_trait]
impl<T: WalletFacade + ?Sized> WalletFacade for Arc<T> {
    async fn network(&self) -> Network {
        (**self).network().await
    }

    async fn ledger_version(&self) -> LedgerVersion {
        (**self).ledger_version().await
    }

    async fn shielded_public_keys(&self) -> (CoinPublicKey, EncryptionPublicKey) {
        (**self).shielded_public_keys().await
    }

    async fn balance(&self) -> WalletBalance {
        (**self).balance().await
    }

    async fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        (**self).spendable_shielded_coins().await
    }

    async fn unshielded_utxos(&self) -> Vec<TrackedUtxo> {
        (**self).unshielded_utxos().await
    }

    async fn parameters(&self) -> ChainParameters {
        (**self).parameters().await
    }

    async fn sync_cursors(&self) -> SyncCursors {
        (**self).sync_cursors().await
    }

    async fn dust_synced(&self) -> bool {
        (**self).dust_synced().await
    }

    async fn release(&self, spent: &SpentInputs) {
        (**self).release(spent).await
    }

    async fn resync(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        (**self).resync(chain).await
    }

    async fn rescan_shielded(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        (**self).rescan_shielded(chain).await
    }

    async fn watch_for_coins(
        &self,
        coins: Vec<CoinInfo>,
        chain: &dyn ChainView,
    ) -> Result<(), WalletError> {
        (**self).watch_for_coins(coins, chain).await
    }

    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        (**self).forget_coins(coins).await
    }

    async fn watched_coins(&self) -> Vec<CoinInfo> {
        (**self).watched_coins().await
    }
}
