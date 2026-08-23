//! The wallet behind a trait, so a consumer depends on the role rather than on
//! one implementation of it.
//!
//! [`WalletFacade`] is that role. Every reading returns an owned value and
//! every mutation is one call, so nothing a caller holds is a lock and no
//! implementation is committed to a particular way of sharing its state.
//! [`LocalWallet`] is the implementation for a [`Wallet`] this process owns; it
//! keeps the lock as a private field.
//!
//! Serializing the sync methods against each other is the caller's job.
//! [`WalletFacade::resync`], [`WalletFacade::rescan_shielded`],
//! [`WalletFacade::watch_for_coins`] and [`WalletFacade::forget_coins`] each
//! release the wallet between what they read and what they commit, so two that
//! interleave can lose one's work. `MidnightProvider` holds a mutex across
//! them.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use midnight_helpers::{
    CoinInfo, CoinPublicKey, DefaultDB, EncryptionPublicKey, LedgerContext, LedgerParameters,
    ProofProvider, Timestamp, WalletSeed,
};
use tokio::sync::RwLock;

use crate::WalletError;
use crate::balance::{SpendableShieldedCoin, WalletBalance};
use crate::chain_pin::ChainPin;
use crate::network::Network;
use crate::state::{SyncCursors, TrackedUtxo, Wallet};
use crate::transfer::{PreparedTransfer, SpentInputs, TransferBuilder, TransferRequest};

/// A prepared build whose inputs the wallet already holds.
///
/// [`WalletFacade::prepare_transfer`] is what makes one, and proving is what
/// consumes one, so a build cannot reach the prover before the reservation
/// that protects its inputs.
pub struct ReservedBuild(PreparedTransfer);

impl ReservedBuild {
    /// Wrap a build whose inputs are reserved.
    ///
    /// Calling this is the implementation's statement that it has recorded the
    /// reservation.
    pub fn reserved(prepared: PreparedTransfer) -> Self {
        Self(prepared)
    }

    /// The build, out of the reservation's custody. Whoever takes it owns
    /// handing the inputs back if the build never reaches the chain.
    pub fn into_prepared(self) -> PreparedTransfer {
        self.0
    }
}

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

    /// The ledger parameters this wallet computes fees and Dust generation
    /// from.
    async fn parameters(&self) -> LedgerParameters;

    /// How far this wallet's sync has reached.
    async fn sync_cursors(&self) -> SyncCursors;

    /// Whether this wallet has completed its Dust sync.
    async fn dust_synced(&self) -> bool;

    /// Where this wallet's snapshot lives, when it persists one.
    async fn snapshot_dir(&self) -> Option<PathBuf>;

    /// The finalized block this wallet is pinned to.
    async fn chain_pin(&self) -> Option<ChainPin>;

    /// Move the pin to the block a fresh check saw.
    async fn set_chain_pin(&self, pin: ChainPin);

    /// The half of a [`LedgerContext`] a transaction executes against. It
    /// carries no key material and no coin state.
    async fn execution_context(&self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError>;

    /// Put this wallet's spendable view into `context`, so a build can fund
    /// itself from it.
    async fn add_funding(&self, context: &LedgerContext<DefaultDB>) -> Result<(), WalletError>;

    /// Select the inputs a request draws on and reserve them, as one
    /// transition.
    ///
    /// Selection reads the reserved set and the reservation writes it, so an
    /// implementation that shares its state must cover both with one hold.
    /// Proving is not part of it: it is the slowest step in a build and reads
    /// only the build context.
    async fn prepare_transfer(
        &self,
        request: TransferRequest,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<ReservedBuild, WalletError>;

    /// Reserve what a build spends, so a later build does not re-select the
    /// same inputs before the indexer surfaces the spend.
    ///
    /// [`Self::prepare_transfer`] does this for the builds it runs. This is
    /// for a caller that assembled its own transaction and has to reserve
    /// afterwards.
    async fn reserve(&self, spent: SpentInputs, reserved_at: Timestamp);

    /// Hand back what a build reserved, because that build will never reach
    /// the chain.
    ///
    /// Only for a transaction that cannot land. Releasing one still in flight
    /// lets a later build re-select the same inputs, and the loser is rejected
    /// on chain.
    async fn release(&self, spent: &SpentInputs);

    /// Resume the event streams from this wallet's cursors and apply what they
    /// deliver.
    ///
    /// The replay runs without the wallet's lock, so reads keep completing
    /// while it is in flight.
    async fn resync(&self, indexer_url: &str) -> Result<(), WalletError>;

    /// Replay the shielded event stream from its first event and rebuild the
    /// shielded state from it. Dust state, unshielded state, and their cursors
    /// are left alone.
    async fn rescan_shielded(&self, indexer_url: &str) -> Result<(), WalletError>;

    /// Register coins this wallet owns but cannot discover, so the next replay
    /// claims them. See `Wallet::watch_for_coin`.
    async fn watch_for_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError>;

    /// Drop registrations that matched no on-chain output.
    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError>;
}

/// A [`Wallet`] this process owns, shared behind its own lock.
///
/// The lock is a private field and no method hands it out, so a consumer that
/// holds a `LocalWallet` cannot block a sync by keeping a guard alive.
pub struct LocalWallet {
    inner: RwLock<Wallet>,
}

impl LocalWallet {
    pub fn new(wallet: Wallet) -> Self {
        Self {
            inner: RwLock::new(wallet),
        }
    }
}

impl From<Wallet> for LocalWallet {
    fn from(wallet: Wallet) -> Self {
        Self::new(wallet)
    }
}

#[async_trait]
impl WalletFacade for LocalWallet {
    async fn network(&self) -> Network {
        Network::from(self.inner.read().await.network())
    }

    async fn seed(&self) -> WalletSeed {
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

    async fn parameters(&self) -> LedgerParameters {
        self.inner.read().await.parameters().clone()
    }

    async fn sync_cursors(&self) -> SyncCursors {
        self.inner.read().await.sync_cursors()
    }

    async fn dust_synced(&self) -> bool {
        self.inner.read().await.dust_synced()
    }

    async fn snapshot_dir(&self) -> Option<PathBuf> {
        self.inner.read().await.snapshot_dir()
    }

    async fn chain_pin(&self) -> Option<ChainPin> {
        self.inner.read().await.chain_pin().cloned()
    }

    async fn set_chain_pin(&self, pin: ChainPin) {
        self.inner.write().await.set_chain_pin(pin);
    }

    async fn execution_context(&self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
        self.inner.read().await.execution_context()
    }

    async fn add_funding(&self, context: &LedgerContext<DefaultDB>) -> Result<(), WalletError> {
        self.inner.write().await.add_funding(context)
    }

    async fn prepare_transfer(
        &self,
        request: TransferRequest,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<ReservedBuild, WalletError> {
        let mut wallet = self.inner.write().await;
        let context = wallet.build_context_inner()?;
        let reserved_at = context.latest_block_context().tblock;
        let prepared = TransferBuilder::new(&*wallet, context, proof_provider)
            .prepare(request)
            .await?;
        let spent = prepared.spent_inputs();
        wallet.reserve_pending(
            spent.dust_batches,
            spent.unshielded,
            spent.shielded,
            reserved_at,
        );
        Ok(ReservedBuild::reserved(prepared))
    }

    async fn reserve(&self, spent: SpentInputs, reserved_at: Timestamp) {
        self.inner.write().await.reserve_pending(
            spent.dust_batches,
            spent.unshielded,
            spent.shielded,
            reserved_at,
        );
    }

    async fn release(&self, spent: &SpentInputs) {
        self.inner.write().await.release_pending(
            &spent.dust_nullifiers(),
            &spent.unshielded,
            &spent.shielded,
        );
    }

    async fn resync(&self, indexer_url: &str) -> Result<(), WalletError> {
        // The plan is snapshotted under a read lock and the commit applied
        // under a write one, so the replay in between runs with the wallet
        // free.
        let plan = self.inner.read().await.resync_plan();
        let commit = plan.run(indexer_url).await?;
        self.inner.write().await.commit_resync(commit)
    }

    async fn rescan_shielded(&self, indexer_url: &str) -> Result<(), WalletError> {
        let plan = self.inner.read().await.shielded_rescan_plan();
        let commit = plan.run(indexer_url).await?;
        self.inner.write().await.commit_shielded_rescan(commit)
    }

    async fn watch_for_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        self.inner.write().await.watch_for_coins(coins)
    }

    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        self.inner.write().await.forget_coins(coins)
    }
}
