//! The API a consumer programs a Midnight wallet against.
//!
//! [`WalletFacade`] names the role. Every reading returns an owned value and
//! every mutation is one call, so nothing a caller holds is a lock and no
//! implementation is committed to a particular way of sharing its state. This
//! crate carries the trait and its whole vocabulary (requests, results,
//! errors), and depends on no wallet implementation; `midnight-wallet`
//! implements it with `LocalWallet`, over a `Wallet` that process owns.
//!
//! Serializing the sync methods against each other is the caller's job.
//! [`WalletFacade::resync`], [`WalletFacade::rescan_shielded`],
//! [`WalletFacade::watch_for_coins`] and [`WalletFacade::forget_coins`] each
//! release the wallet between what they read and what they commit, so two that
//! interleave can lose one's work. `MidnightProvider` holds a mutex across
//! them.

pub mod balance;
pub mod chain_pin;
pub mod network;
pub mod transfer;

mod error;
mod sync;

pub use balance::{
    DustBalance, ShieldedBalance, ShieldedCoinBalance, SpendableShieldedCoin, UnshieldedUtxoInfo,
    WalletBalance,
};
pub use error::WalletError;
pub use network::Network;
pub use sync::{SyncCursors, TrackedUtxo};
pub use transfer::{
    DustSpendBatch, PreparedTransfer, SpentInputs, SpentUtxoKey, TransferKind, TransferRequest,
    TransferResult, panic_message,
};

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chain_pin::ChainPin;
use midnight_helpers::{
    CoinInfo, CoinPublicKey, DefaultDB, EncryptionPublicKey, LedgerContext, LedgerParameters,
    ProofProvider, Timestamp, WalletSeed,
};

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
    /// claims them. See `Wallet::watch_for_coin` on the implementing wallet.
    async fn watch_for_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError>;

    /// Drop registrations that matched no on-chain output.
    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError>;
}
