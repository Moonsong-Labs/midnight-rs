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
//! Serializing the sync methods against each other is the caller's job.
//! [`WalletFacade::resync`], [`WalletFacade::rescan_shielded`],
//! [`WalletFacade::watch_for_coins`] and [`WalletFacade::forget_coins`] each
//! release the wallet between what they read and what they commit, so two that
//! interleave can lose one's work. `MidnightProvider` holds a mutex across
//! them.

use std::sync::Arc;

use async_trait::async_trait;
use midnight_types::chain_pin::ChainView;
use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, LedgerVersion, Network,
    SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, WalletBalance, WalletError,
    WalletSeed,
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

    /// The seed this wallet signs and derives with.
    ///
    /// A build needs it, which is why it is here. It is also the one thing an
    /// external signer will not hand over, so a facade over one cannot serve
    /// this.
    async fn seed(&self) -> WalletSeed;

    /// The public keys a coin addressed to this wallet commits to.
    async fn shielded_public_keys(&self) -> (CoinPublicKey, EncryptionPublicKey);

    /// What this wallet can spend, across shielded coins, unshielded UTXOs and
    /// Dust.
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
    /// This drops only the entry `spent` describes. A build that releases late
    /// cannot take back an input a later build has since reserved, because the
    /// two reservations carry different `reserved_at` stamps.
    async fn release(&self, spent: &SpentInputs);

    /// Whether the confirmed state shows every input in `spent` as spent.
    ///
    /// The confirmed state is what the last sync or resync committed. The
    /// answer reads the confirmed state of each leg, never a reservation. A
    /// reservation can end with no spend on chain, by a release or by its
    /// TTL. A shielded reservation does not end when the spend confirms.
    /// An unshielded or Dust UTXO counts as spent when the state does not
    /// hold it. A shielded coin counts as spent when the coin set does not
    /// hold its nullifier. A `spent` that names no input reads as observed.
    ///
    /// The answer is valid only after a `Success` verdict. After a
    /// `PartialSuccess` or a `Failure`, an input that the chain did not spend
    /// keeps this `false`. `MidnightProvider::wait_observed` resyncs until
    /// this holds.
    async fn has_observed(&self, spent: &[SpentInputs]) -> bool;

    /// Resume the event streams from this wallet's cursors and apply what they
    /// deliver.
    ///
    /// The replay runs without the wallet's lock, so reads keep completing
    /// while it is in flight. Which indexer it resumes against is the
    /// wallet's own business: the cursors are event counts, so only the
    /// server that produced them can continue them.
    ///
    /// `chain` answers the two questions a chain pin asks. The cursors say
    /// nothing about which chain produced them, so a wallet that stays
    /// attached while the chain is replaced would otherwise replay onto the
    /// fresh chain and keep serving the old one's balance. An implementation
    /// that pins its snapshot checks it here and moves it forward afterwards.
    async fn resync(&self, chain: &dyn ChainView) -> Result<(), WalletError>;

    /// Replay the shielded event stream from its first event and rebuild the
    /// shielded state from it. Dust state, unshielded state, and their cursors
    /// are left alone.
    async fn rescan_shielded(&self) -> Result<(), WalletError>;

    /// Register coins this wallet owns but cannot discover, so the next replay
    /// claims them. See `Wallet::watch_for_coin` on the implementing wallet.
    async fn watch_for_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError>;

    /// Drop registrations that matched no on-chain output.
    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError>;
}

/// A shared wallet is a wallet: every call forwards to the one inside.
///
/// This lets a caller keep a handle on the wallet it attaches instead of
/// giving ownership away, and lets one implementation stand behind several
/// readers.
///
/// One wallet behind two `MidnightProvider`s is not among the uses. Nothing
/// here serializes the sync methods against each other, and the provider does
/// that with a mutex of its own, so two of them hold two different ones. Their
/// resyncs would snapshot the same cursors and race their commits, and the
/// slower replay would land last and walk the cursors backwards.
#[async_trait]
impl<T: WalletFacade + ?Sized> WalletFacade for Arc<T> {
    async fn network(&self) -> Network {
        (**self).network().await
    }

    async fn ledger_version(&self) -> LedgerVersion {
        (**self).ledger_version().await
    }

    async fn seed(&self) -> WalletSeed {
        (**self).seed().await
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

    async fn has_observed(&self, spent: &[SpentInputs]) -> bool {
        (**self).has_observed(spent).await
    }

    async fn resync(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        (**self).resync(chain).await
    }

    async fn rescan_shielded(&self) -> Result<(), WalletError> {
        (**self).rescan_shielded().await
    }

    async fn watch_for_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        (**self).watch_for_coins(coins).await
    }

    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        (**self).forget_coins(coins).await
    }
}
