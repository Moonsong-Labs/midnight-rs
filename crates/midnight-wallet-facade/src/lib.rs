//! The API a consumer programs a Midnight wallet against.
//!
//! [`WalletFacade`] names the role. Every reading returns an owned value and
//! every mutation is one call, so nothing a caller holds is a lock and no
//! implementation is committed to a particular way of sharing its state. This
//! crate carries the trait alone and speaks in `midnight-types`'s
//! vocabulary, so it depends on no wallet implementation; `midnight-wallet`
//! implements it with `LocalWallet`, over a `Wallet` that process owns.
//!
//! Serializing the sync methods against each other is the caller's job.
//! [`WalletFacade::resync`], [`WalletFacade::rescan_shielded`],
//! [`WalletFacade::watch_for_coins`] and [`WalletFacade::forget_coins`] each
//! release the wallet between what they read and what they commit, so two that
//! interleave can lose one's work. `MidnightProvider` holds a mutex across
//! them.

use std::sync::Arc;

use async_trait::async_trait;
use midnight_helpers::{
    CoinPublicKey, DefaultDB, EncryptionPublicKey, LedgerContext, LedgerParameters, ProofProvider,
    StandardTrasactionInfo,
};
use midnight_types::chain_pin::ChainView;
use midnight_types::{
    CoinInfo, Network, PreparedTransfer, SpendableShieldedCoin, SpentInputs, SyncCursors,
    TrackedUtxo, TransferRequest, WalletBalance, WalletError, WalletSeed,
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

    /// Fund a transaction the caller assembled, and reserve what it drew, as
    /// one transition.
    ///
    /// [`Self::prepare_transfer`] is the same shape for a transfer this wallet
    /// selects itself. This one is for a caller that built its own
    /// transaction, a contract deploy or a maintenance update, and needs this
    /// wallet to pay its fee.
    ///
    /// Balancing the fee reads the funding view, and the reservation writes
    /// the reserved set that view subtracts, so one hold covers both. Proving
    /// is not part of it.
    ///
    /// `tx_info` must already ask for mock fee proofs. Balancing without them
    /// proves on every round, which is the cost preparing exists to avoid, and
    /// a transaction carrying a user circuit cannot mock-prove at all.
    async fn prepare_funded(
        &self,
        tx_info: StandardTrasactionInfo<DefaultDB>,
    ) -> Result<ReservedBuild, WalletError>;

    /// Record what a build spends, so a later build does not re-select the
    /// same inputs before the indexer surfaces the spend. `spent` carries the
    /// chain time to stamp the reservation with.
    ///
    /// This is the primitive, not the way in. Call it against the same read
    /// whose result the reservation protects, with no await in between, or a
    /// second build reads that view first and draws the same inputs.
    ///
    /// [`Self::prepare_transfer`] and [`Self::prepare_funded`] pair the two
    /// inside one hold and are what a caller should reach for. A build that
    /// cannot know its spend until it has proved, a contract call, reserves at
    /// the read it can make early and hands the inputs back if it never
    /// reaches the chain. `MidnightProvider::reserve_guarded` is that shape.
    async fn reserve(&self, spent: SpentInputs);

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

    async fn parameters(&self) -> LedgerParameters {
        (**self).parameters().await
    }

    async fn sync_cursors(&self) -> SyncCursors {
        (**self).sync_cursors().await
    }

    async fn dust_synced(&self) -> bool {
        (**self).dust_synced().await
    }

    async fn execution_context(&self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
        (**self).execution_context().await
    }

    async fn add_funding(&self, context: &LedgerContext<DefaultDB>) -> Result<(), WalletError> {
        (**self).add_funding(context).await
    }

    async fn prepare_transfer(
        &self,
        request: TransferRequest,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<ReservedBuild, WalletError> {
        (**self).prepare_transfer(request, proof_provider).await
    }

    async fn prepare_funded(
        &self,
        tx_info: StandardTrasactionInfo<DefaultDB>,
    ) -> Result<ReservedBuild, WalletError> {
        (**self).prepare_funded(tx_info).await
    }

    async fn reserve(&self, spent: SpentInputs) {
        (**self).reserve(spent).await
    }

    async fn release(&self, spent: &SpentInputs) {
        (**self).release(spent).await
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
