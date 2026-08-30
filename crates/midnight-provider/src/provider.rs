use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use subxt::OnlineClient;
use subxt::config::RpcConfigFor;
use subxt::rpcs::ChainHeadRpcMethods;
use subxt::rpcs::client::reconnecting_rpc_client::RpcClient as ReconnectingRpcClient;
use subxt::rpcs::client::{RpcClient, RpcParams};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::transfer::{DustRegistration, ShieldedSwap, ShieldedTransfer, UnshieldedTransfer};
use crate::{Health, PendingTx, Provider, ProviderError, StateQuery, StateQueryResult, submit};
use midnight_indexer_client::{
    BlockOffset, ContractAction, ContractActionOffset, IndexerClient, TransactionOffset,
};
use midnight_private_state::PrivateStateProvider;
use midnight_types::{
    BuilderCtx, CoinInfo, DefaultDB, LedgerContext, LedgerParameters, LocalProofServer,
    ProofProvider, ShieldedTokenType, UnshieldedTokenType,
};
use midnight_types::{
    Network, SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, TransferKind,
    TransferRequest, TransferResult, WalletBalance,
};
use midnight_wallet_facade::{ReservedBuild, WalletFacade};

/// Connection timeout for the node WebSocket RPC.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Cached node connection over a single auto-reconnecting websocket: the
/// subxt `RpcClient` carries every raw RPC (standard Substrate and custom
/// `midnight_*` methods alike), and the `OnlineClient` on top of it serves
/// the runtime-aware submission path.
#[derive(Clone)]
struct NodeConnection {
    rpc: RpcClient,
    client: OnlineClient<subxt::SubstrateConfig>,
}

/// A [`Provider`] backed by an [`IndexerClient`] (GraphQL) and a node
/// WebSocket connection for direct RPC communication.
///
/// The node connection is established lazily on first use, cached for the
/// provider's lifetime, and auto-reconnects with backoff on network drops.
/// One websocket carries everything: raw Substrate and `midnight_*` RPCs
/// through the subxt `RpcClient`, and transaction submission through the
/// `OnlineClient` built on the same transport.
pub struct MidnightProvider {
    indexer: IndexerClient,
    indexer_url: String,
    node_url: String,
    /// The wallet, reached only through its API.
    ///
    /// How its state is shared is the implementation's business: every method
    /// on [`WalletFacade`] returns an owned value or covers one transition, so
    /// nothing here holds a lock. Cloning the `Arc` is cheap and safe.
    wallet: Option<Arc<dyn WalletFacade>>,
    /// Proof backend for transaction building. Defaults to a fresh
    /// [`LocalProofServer`] on first use; override with
    /// [`Self::with_proof_provider`] to use a remote prover or a custom
    /// implementation.
    proof_provider: Option<Arc<dyn ProofProvider<DefaultDB>>>,
    /// Optional store for per-contract private state and maintenance signing
    /// keys. Set with [`Self::with_private_state`]; absent for contracts whose
    /// witnesses are stateless.
    private_state: Option<Arc<dyn PrivateStateProvider>>,
    conn: Arc<RwLock<Option<NodeConnection>>>,
    /// Serializes [`Self::resync_wallet`] runs. The resync's replay phase
    /// runs without the wallet lock (so reads keep flowing); this mutex is
    /// what keeps two concurrent resyncs from replaying the same cursors
    /// and racing their commits. Held across plan → replay → commit.
    resync_lock: Mutex<()>,
}

impl MidnightProvider {
    /// Create a provider from node WebSocket URL and indexer HTTP URL.
    ///
    /// The node connection is **not** established here; it is deferred to
    /// the first call that requires it.
    ///
    /// A wallet is built on its own and attached with
    /// [`Self::with_wallet`]. With the local implementation from
    /// `midnight-wallet`:
    /// ```rust,ignore
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?;
    /// let wallet = Wallet::sync(provider.indexer_url(), seed, Network::Undeployed).await?;
    /// let provider = provider.with_wallet(LocalWallet::new(wallet));
    /// ```
    pub fn new(node_url: &str, indexer_url: &str) -> Result<Self, ProviderError> {
        let indexer = IndexerClient::new(indexer_url)?;
        Ok(Self {
            indexer,
            indexer_url: indexer_url.to_string(),
            node_url: node_url.to_string(),
            wallet: None,
            proof_provider: None,
            private_state: None,
            conn: Arc::new(RwLock::new(None)),
            resync_lock: Mutex::new(()),
        })
    }

    /// Override the proof backend used by [`Self::transfer_shielded`],
    /// [`Self::transfer_unshielded`], and [`Self::register_dust`].
    ///
    /// Defaults to a fresh [`LocalProofServer`] if unset. Pass a
    /// [`RemoteProofServer`](crate::RemoteProofServer) to offload proving to an
    /// HTTP proof server, or any custom [`ProofProvider`] implementation:
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use midnight_provider::{MidnightProvider, RemoteProofServer};
    ///
    /// let prover = Arc::new(RemoteProofServer::new("http://localhost:6300".to_string()));
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?.with_proof_provider(prover);
    /// ```
    pub fn with_proof_provider(
        mut self,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Self {
        self.proof_provider = Some(proof_provider);
        self
    }

    /// The proof backend used to prove transactions built through this
    /// provider (transfers, dust registration, and every contract deploy /
    /// call / maintenance op driven by a `Contract` built on it).
    ///
    /// Returns the backend set via [`Self::with_proof_provider`], or a fresh
    /// [`LocalProofServer`] when none was configured. Cheap to clone (`Arc`).
    pub fn proof_provider(&self) -> Arc<dyn ProofProvider<DefaultDB>> {
        self.proof_provider
            .clone()
            .unwrap_or_else(|| Arc::new(LocalProofServer::new()))
    }

    /// Attach a [`PrivateStateProvider`] for per-contract private state (and an
    /// optional per-contract signing-key slot; contract governance signs
    /// externally and does not use it).
    ///
    /// Optional: contracts whose witnesses are stateless never need it. When
    /// attached, a circuit call loads the contract's private state before
    /// execution, threads it through the witnesses via `WitnessContext`, and
    /// persists the updated state after the transaction lands (see
    /// `docs/private-state.md`).
    ///
    /// The load-execute-submit-persist window is not locked: concurrent calls to
    /// the same contract start from the same baseline and the last to persist
    /// wins. Serialize calls to one contract if you fan them out.
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use midnight_provider::FsPrivateStateProvider;
    ///
    /// let store = Arc::new(FsPrivateStateProvider::with_default_dir().unwrap());
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?.with_private_state(store);
    /// ```
    pub fn with_private_state(mut self, store: Arc<dyn PrivateStateProvider>) -> Self {
        self.private_state = Some(store);
        self
    }

    /// The attached [`PrivateStateProvider`], or `None` if none was set via
    /// [`Self::with_private_state`]. Cheap to clone (`Arc`) and safe to share
    /// across tasks.
    pub fn private_state(&self) -> Option<Arc<dyn PrivateStateProvider>> {
        self.private_state.clone()
    }

    /// The indexer URL this provider was built with, for a caller syncing a
    /// wallet against the same indexer.
    pub fn indexer_url(&self) -> &str {
        &self.indexer_url
    }

    /// Attach a wallet, and become the single entry point for its resync,
    /// transaction-context construction, and background sync.
    ///
    /// A synced `Wallet` this process owns goes in as
    /// `LocalWallet::new(wallet)`; anything else that implements
    /// [`WalletFacade`] goes in as itself.
    pub fn with_wallet(mut self, wallet: impl WalletFacade + 'static) -> Self {
        self.wallet = Some(Arc::new(wallet));
        self
    }

    /// Return the current wallet balance.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn balance(&self) -> Result<WalletBalance, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.balance().await)
    }

    /// Enumerate the wallet's spendable shielded coins with their full coin
    /// info (nonce, token type, value) and pinning nullifier.
    ///
    /// Use this to address a specific coin for a circuit that spends it (e.g.
    /// `receiveShielded`), then hand the coin to the contract call builder's
    /// `with_shielded_inputs`. See [`SpendableShieldedCoin`].
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn spendable_shielded_coins(
        &self,
    ) -> Result<Vec<SpendableShieldedCoin>, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.spendable_shielded_coins().await)
    }

    /// Whether the attached wallet has completed dust sync.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn dust_synced(&self) -> Result<bool, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.dust_synced().await)
    }

    /// The unshielded UTXOs the attached wallet tracks.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn unshielded_utxos(&self) -> Result<Vec<TrackedUtxo>, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.unshielded_utxos().await)
    }

    /// The ledger parameters the attached wallet computes fees and dust
    /// generation from.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn parameters(&self) -> Result<LedgerParameters, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.parameters().await)
    }

    /// How far the attached wallet's sync has reached. See
    /// `Wallet::sync_cursors` on the implementing wallet.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn sync_cursors(&self) -> Result<SyncCursors, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.sync_cursors().await)
    }

    /// Re-sync the wallet against the indexer.
    ///
    /// Resumes from the wallet's current event cursors, applies any new
    /// zswap/dust/unshielded events, refreshes the latest block context and
    /// ledger parameters, and commits the result (re-persisting it when the
    /// wallet was synced with a storage directory). Fails if no wallet is
    /// attached.
    ///
    /// Locking: the slow replay I/O runs **without** the wallet lock, so
    /// concurrent reads ([`Self::balance`], [`Self::dust_synced`], ...) keep
    /// completing while a resync is in flight; the wallet lock is only taken
    /// briefly to snapshot the replay inputs (read) and to commit the result
    /// (write). Concurrent `resync_wallet` calls are serialized on an
    /// internal mutex.
    pub async fn resync_wallet(&self) -> Result<(), ProviderError> {
        // Boxed so the replay stays off the caller's frame. A debug build
        // makes this future tens of kilobytes, and an inlined one is carried
        // by every future that awaits it, up to the test or task that owns
        // the stack. Every build path awaits this one.
        Box::pin(self.resync_wallet_inner()).await
    }

    async fn resync_wallet_inner(&self) -> Result<(), ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;

        // Serialize resyncs across plan → replay → commit: the replay below
        // runs without the wallet lock, so without this guard two concurrent
        // resyncs would replay from the same cursors and race their commits.
        let _resync_guard = self.resync_lock.lock().await;

        // The replay is the long part, and it runs with the wallet free:
        // reads (and even transfer builds, which block on the resync mutex via
        // their own resync) proceed against the pre-resync state meanwhile.
        //
        // The wallet resumes against its own indexer and checks its own chain
        // pin; this provider is only the node view those checks ask.
        arc.resync(self).await?;
        Ok(())
    }

    /// Register a coin the wallet owns but cannot discover, then replay the
    /// shielded stream so the registration is honoured.
    ///
    /// A shielded coin normally reaches its owner through the discovery
    /// ciphertext on its output. When that ciphertext is missing, or is
    /// sealed to a key this wallet does not hold, the coin is still owned by
    /// the wallet's coin public key and still spendable, as long as the caller
    /// can rebuild the [`CoinInfo`] (nonce, token type, value) from somewhere
    /// else, such as a contract that evolves one public nonce per mint. Pass
    /// that rebuilt coin here and the wallet claims it from the chain's own
    /// output, decrypting nothing:
    ///
    /// ```rust,ignore
    /// use midnight_provider::{CoinInfo, HashOutput, Nonce, ShieldedTokenType};
    ///
    /// provider
    ///     .watch_for_coin(CoinInfo {
    ///         nonce: Nonce(HashOutput(nonce)),
    ///         type_: ShieldedTokenType(HashOutput(token_type)),
    ///         value,
    ///     })
    ///     .await?;
    ///
    /// // Claimed coins now appear in the wallet's spendable set.
    /// let coins = provider.spendable_shielded_coins().await?;
    /// ```
    ///
    /// The replay covers the whole shielded stream, because a registration
    /// only takes effect while the coin's output is still ahead of the sync
    /// cursor. It costs more than a resync, so register every coin you know
    /// about in one call rather than calling this per coin, and treat it as a
    /// recovery step, not a routine one. Coins already recovered keep their
    /// place: each replay re-registers what the wallet holds. To check the
    /// outcome, look for the coin in
    /// [`Self::spendable_shielded_coins`]; a coin the replay did not claim is
    /// still listed by `Wallet::watched_coins`, and `Wallet::forget_coin`
    /// drops it if the rebuilt coin was wrong.
    ///
    /// The registration is recorded (and persisted) before the replay starts,
    /// so a replay that fails leaves it in place: retry with
    /// [`Self::rescan_shielded`] rather than registering again.
    ///
    /// Locking matches [`Self::resync_wallet`]: the replay runs without the
    /// wallet lock, and the same internal mutex serializes this against
    /// resyncs. Recording the registration holds the wallet's write lock
    /// across a full state save, which blocks readers for as long as that
    /// write takes.
    pub async fn watch_for_coin(&self, coin: CoinInfo) -> Result<(), ProviderError> {
        self.watch_for_coins([coin]).await
    }

    /// [`Self::watch_for_coin`] for several coins, with one replay.
    ///
    /// Registering nothing is a no-op: no write, no replay.
    pub async fn watch_for_coins(
        &self,
        coins: impl IntoIterator<Item = CoinInfo> + Send,
    ) -> Result<(), ProviderError> {
        let coins: Vec<CoinInfo> = coins.into_iter().collect();
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        if coins.is_empty() {
            return Ok(());
        }
        let _resync_guard = self.resync_lock.lock().await;
        arc.watch_for_coins(coins).await?;
        self.rescan_shielded_serialized(arc).await
    }

    /// Drop a registration [`Self::watch_for_coin`] made, for a coin that
    /// turned out to be wrong.
    ///
    /// A registration whose rebuilt `CoinInfo` matches no on-chain output
    /// stays in `Wallet::watched_coins` and rides along on every later
    /// replay; this is how a caller removes one. A coin the wallet already
    /// claimed is untouched, and no replay runs.
    pub async fn forget_coin(&self, coin: CoinInfo) -> Result<(), ProviderError> {
        self.forget_coins([coin]).await
    }

    /// [`Self::forget_coin`] for several coins, with one write.
    pub async fn forget_coins(
        &self,
        coins: impl IntoIterator<Item = CoinInfo> + Send,
    ) -> Result<(), ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        // Serialized against resyncs: a resync commits the state its plan
        // snapshotted, which still carries a registration dropped after that
        // snapshot, so an unserialized forget would come back.
        let _resync_guard = self.resync_lock.lock().await;
        arc.forget_coins(coins.into_iter().collect()).await?;
        Ok(())
    }

    /// Replay the shielded event stream from its first event and rebuild the
    /// wallet's shielded state from it.
    ///
    /// [`Self::watch_for_coin`] already does this for the coins it registers.
    /// Call this directly to rebuild shielded state that a resync cannot
    /// repair because its cursor has moved past the events in question.
    /// Dust state, unshielded state, and their cursors are left alone.
    pub async fn rescan_shielded(&self) -> Result<(), ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let _resync_guard = self.resync_lock.lock().await;
        self.rescan_shielded_serialized(arc).await
    }

    /// The rescan's plan → run → commit sequence. The caller must already
    /// hold `resync_lock`: a resync interleaving here would commit its own
    /// cursor over the rebuilt state.
    async fn rescan_shielded_serialized(
        &self,
        wallet: &Arc<dyn WalletFacade>,
    ) -> Result<(), ProviderError> {
        wallet.rescan_shielded().await?;
        Ok(())
    }

    /// Build a [`LedgerContext`] the attached wallet both executes against and
    /// pays from.
    ///
    /// [`Self::execution_context`] followed by [`Self::add_funding`], for the
    /// builds that fund from the wallet that builds them. Use the two
    /// separately when a circuit has to run before the payer is known.
    pub async fn build_context(&self) -> Result<Arc<LedgerContext<DefaultDB>>, ProviderError> {
        let context = self.execution_context().await?;
        self.add_funding(&context).await?;
        Ok(context)
    }

    /// Build the half of a [`LedgerContext`] a transaction executes against:
    /// chain parameters, genesis settings, the resolver, and the latest block
    /// context.
    ///
    /// Resyncs first so the proof root and TTL anchor match the chain's
    /// current view, then reads the wallet. The result holds no key material
    /// and no coin state, so a caller can run a circuit
    /// against it and only then decide who pays. Add a payer with
    /// [`Self::add_funding`]; a context that never gets that call funds
    /// nothing.
    pub async fn execution_context(&self) -> Result<Arc<LedgerContext<DefaultDB>>, ProviderError> {
        self.resync_wallet().await?;
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.execution_context().await?)
    }

    /// Put the attached wallet's spendable view into `context`, so a build can
    /// fund itself from it.
    ///
    /// Mutates the wallet: its `add_funding` evicts TTL-expired pending
    /// entries against the refreshed `block_context`.
    pub async fn add_funding(
        &self,
        context: &LedgerContext<DefaultDB>,
    ) -> Result<(), ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.add_funding(context).await?)
    }

    /// Build a shielded (Zswap) transfer transaction.
    ///
    /// Returns a pending builder. `.await?` builds + submits and returns the
    /// resulting [`PendingTx`]; `.build().await?` returns the raw
    /// [`TransferResult`] without submitting (e.g. for inspection or custom
    /// routing). Either path selects the inputs and records them in the
    /// wallet's pending list as one transition, so a second build in this
    /// process cannot draw the same input. Proving runs after that, with the
    /// wallet free.
    pub fn transfer_shielded<'a>(
        &'a self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> ShieldedTransfer<'a> {
        ShieldedTransfer::new(self, token_type, amount, recipient)
    }

    /// Build one half of a native two-party shielded token swap.
    ///
    /// The half spends `give_amount` of `give_token` and creates an output for
    /// `receive_amount` of `receive_token` payable to this wallet. It is net
    /// unbalanced (`+give_token`, `-receive_token`) and therefore fee-less by
    /// construction, so awaiting the returned [`ShieldedSwap`] yields a
    /// [`DustlessTransaction`](crate::DustlessTransaction) directly rather than
    /// submitting.
    ///
    /// The counterparty builds the exact mirror
    /// (`shielded_swap(receive_token, receive_amount, give_token, give_amount)`).
    /// A sponsor (either party or a third party) then combines the two halves
    /// with [`Self::merge_transactions`] into a balanced, fee-less transaction,
    /// funds its Dust with [`Self::balance_transaction`], and submits:
    ///
    /// ```rust,ignore
    /// let a_half = alice.shielded_swap(token_x, dx, token_y, dy).await?;
    /// let b_half = bob.shielded_swap(token_y, dy, token_x, dx).await?;
    /// let merged = sponsor.merge_transactions(&[a_half.into_bytes(), b_half.into_bytes()])?;
    /// let funded = sponsor.balance_transaction(&merged).await?;
    /// sponsor.submit(&funded).await?;
    /// ```
    ///
    /// The two halves must carry exactly mirrored `(token, amount)` pairs or the
    /// merge won't balance; the builder can't enforce the counterparty's side.
    /// See [`Self::transfer_shielded`] for reservation semantics.
    pub fn shielded_swap<'a>(
        &'a self,
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
    ) -> ShieldedSwap<'a> {
        ShieldedSwap::new(self, give_token, give_amount, receive_token, receive_amount)
    }

    /// Build an unshielded (UTXO) transfer transaction. See
    /// [`Self::transfer_shielded`] for reservation semantics and the
    /// `.await` vs `.build()` distinction.
    pub fn transfer_unshielded<'a>(
        &'a self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> UnshieldedTransfer<'a> {
        UnshieldedTransfer::new(self, token_type, amount, recipient)
    }

    /// Build a dust-address registration transaction. See
    /// [`Self::transfer_shielded`] for reservation semantics and the
    /// `.await` vs `.build()` distinction.
    pub fn register_dust(&self, utxo_ctime: Option<u64>) -> DustRegistration<'_> {
        DustRegistration::new(self, utxo_ctime)
    }

    // -- Internal build paths driven by the transfer/register builders. --

    /// Build a shielded transfer. `pay_fees` false produces a Dustless
    /// (fee-less) transaction for another wallet to sponsor via
    /// [`Self::balance_transaction`]; the builder's `.without_dust()` path
    /// passes false, every other path passes true.
    pub(crate) async fn build_shielded_transfer(
        &self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
        pay_fees: bool,
        coin_selection: midnight_types::CoinSelectionStrategy,
    ) -> Result<TransferResult, ProviderError> {
        self.build_then_prove(
            TransferRequest::new(TransferKind::Shielded {
                token_type,
                amount,
                recipient: recipient.to_string(),
                pay_fees,
            })
            .with_coin_selection(coin_selection),
        )
        .await
    }

    /// Build a shielded swap half. Always fee-less (an unbalanced half can't
    /// self-fund), so there is no `pay_fees` flag. Reserves the spent give-side
    /// coins so a later in-process build doesn't re-select them.
    pub(crate) async fn build_shielded_swap(
        &self,
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
        coin_selection: midnight_types::CoinSelectionStrategy,
    ) -> Result<TransferResult, ProviderError> {
        self.build_then_prove(
            TransferRequest::new(TransferKind::ShieldedSwap {
                give_token,
                give_amount,
                receive_token,
                receive_amount,
            })
            .with_coin_selection(coin_selection),
        )
        .await
    }

    /// Build an unshielded transfer. See [`Self::build_shielded_transfer`] for
    /// the `pay_fees` flag.
    pub(crate) async fn build_unshielded_transfer(
        &self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
        pay_fees: bool,
        coin_selection: midnight_types::CoinSelectionStrategy,
    ) -> Result<TransferResult, ProviderError> {
        self.build_then_prove(
            TransferRequest::new(TransferKind::Unshielded {
                token_type,
                amount,
                recipient: recipient.to_string(),
                pay_fees,
            })
            .with_coin_selection(coin_selection),
        )
        .await
    }

    pub(crate) async fn build_register_dust(
        &self,
        utxo_ctime: Option<u64>,
    ) -> Result<TransferResult, ProviderError> {
        self.build_then_prove(TransferRequest::new(TransferKind::DustRegistration {
            utxo_ctime,
        }))
        .await
    }

    /// Prepare a build under the attached wallet, then prove without it.
    ///
    /// Proving is the slowest step in a build and reads only the build
    /// context, so leaving it inside the wallet's hold would make every other
    /// consumer wait on work that never needed the wallet.
    async fn build_then_prove(
        &self,
        request: TransferRequest,
    ) -> Result<TransferResult, ProviderError> {
        let reserved = self.prepare_transfer(request).await?;
        self.prove_reserved(reserved).await
    }

    /// Resync, then ask the wallet to select and reserve. See
    /// [`WalletFacade::prepare_transfer`].
    ///
    /// The resync goes first so the proof root and the TTL anchor match the
    /// chain's current view.
    async fn prepare_transfer(
        &self,
        request: TransferRequest,
    ) -> Result<ReservedBuild, ProviderError> {
        self.resync_wallet().await?;
        let proof_provider = self.proof_provider();
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.prepare_transfer(request, proof_provider).await?)
    }

    /// Prove a build whose inputs the wallet already holds, with the wallet
    /// released.
    ///
    /// Takes a [`ReservedBuild`] rather than a bare prepared build, so a build
    /// reaches the prover only through a wallet that says it reserved the
    /// inputs first. A proof that fails hands them back, because the
    /// reservation outlives the decision that made it and would otherwise
    /// strand them until their TTL elapses.
    async fn prove_reserved(
        &self,
        reserved: ReservedBuild,
    ) -> Result<TransferResult, ProviderError> {
        let prepared = reserved.into_prepared();
        let mut held = HeldInputs::of(prepared.spent_inputs(), self.wallet.clone());

        match prepared.prove().await {
            Ok(result) => {
                held.keep();
                Ok(result)
            }
            Err(err) => {
                // Release here rather than leaving it to `held`, so a caller
                // that observes the error also observes the inputs back.
                held.keep();
                if let Some(arc) = self.wallet.as_ref() {
                    arc.release(&held.spent).await;
                }
                Err(err.into())
            }
        }
    }

    /// Submit proven transaction bytes to the node over the WebSocket RPC.
    ///
    /// Returns a [`PendingTx`] handle that lets the caller await inclusion
    /// (`wait_best`) and finalization (`wait_finalized`). The provider's
    /// `node_url` is used as the connection target — callers don't repeat it.
    pub async fn submit(&self, tx_bytes: &[u8]) -> Result<PendingTx, ProviderError> {
        let conn = self.get_or_connect().await?;
        submit::submit_bytes(&conn.client, tx_bytes).await
    }

    /// Build and validate proven transaction bytes against the node without
    /// submitting them, returning a [`crate::PreparedTx`] whose extrinsic hash
    /// is already known. Submit it with [`crate::PreparedTx::submit`]. Lets a
    /// caller durably record state keyed by the extrinsic hash *before* the
    /// transaction reaches the mempool.
    pub async fn prepare(&self, tx_bytes: &[u8]) -> Result<submit::PreparedTx, ProviderError> {
        let conn = self.get_or_connect().await?;
        submit::prepare_bytes(&conn.client, tx_bytes).await
    }

    /// Merge proven transactions into one, for multi-party flows: e.g. combining
    /// a contract call built without submitting (`Contract::build_call_with`, or
    /// the generated `circuits().<circuit>().build().await`) with a
    /// counterparty's already-proven transaction before submitting.
    ///
    /// Each input is a tagged-serialized proven transaction (the byte output of
    /// any build path). The result is the merged transaction, ready for
    /// [`Self::submit`] / [`Self::prepare`]. Merging combines the transactions'
    /// intents and Zswap offers and sums their binding randomness; it does NOT
    /// rebalance, so every input must already balance its own tokens.
    ///
    /// **Intent segments must not collide.** The ledger rejects a merge where
    /// two inputs both carry an intent at the same segment. A self-funded build
    /// attaches its Dust-fee intent at the fallible segment (1); a contract call
    /// and an unshielded (UTXO) transfer place their action there too. So at
    /// most one merged input may carry a segment-1 intent, and two self-funded
    /// transactions cannot be merged directly. The supported multi-party shape
    /// is "one party pays": the contributors build fee-less
    /// ([`crate::DustlessBuilder::without_dust`]) and a single payer covers the
    /// fees with [`Self::balance_transaction`] (whose fee intent rides a distinct,
    /// non-colliding segment). A Dustless *shielded* transfer carries no intent
    /// at all (pure Zswap), so it always merges cleanly.
    ///
    /// Errors ([`ProviderError::Transaction`]) when given no transactions, when
    /// a byte string fails to deserialize, or when two transactions cannot be
    /// merged (colliding intent segments or mismatched network ids). Purely
    /// local; nothing is sent to the node.
    pub fn merge_transactions(&self, txs: &[Vec<u8>]) -> Result<Vec<u8>, ProviderError> {
        use midnight_types::FinalizedTransaction;
        use midnight_types::midnight_serialize::{tagged_deserialize, tagged_serialize};

        let deserialize = |bytes: &[u8]| -> Result<FinalizedTransaction<DefaultDB>, ProviderError> {
            tagged_deserialize(&mut &bytes[..])
                .map_err(|e| ProviderError::Transaction(format!("deserialize transaction: {e}")))
        };

        let mut iter = txs.iter();
        let first = iter.next().ok_or_else(|| {
            ProviderError::Transaction(
                "merge_transactions requires at least one transaction".into(),
            )
        })?;
        let mut merged = deserialize(first)?;
        for bytes in iter {
            let other = deserialize(bytes)?;
            merged = merged
                .merge(&other)
                .map_err(|e| ProviderError::Transaction(format!("merge transactions: {e:?}")))?;
        }

        let mut out = Vec::new();
        tagged_serialize(&merged, &mut out).map_err(|e| {
            ProviderError::Transaction(format!("serialize merged transaction: {e}"))
        })?;
        Ok(out)
    }

    /// Pay the fees for an external party's proven, fee-less transaction from
    /// this provider's wallet, returning the completed transaction ready to
    /// submit.
    ///
    /// This is the "one party pays fees" flow (midnight-js `balanceTransaction`):
    /// the caller (fee payer) covers the Dust fees for a transaction someone
    /// else already built and proved, without holding their keys. Purely
    /// additive, the external transaction's proofs are untouched; a separately
    /// proven Dust-paying transaction is combined in via `Transaction::merge`.
    ///
    /// Covers **fees only**: the transaction must already be balanced on every
    /// non-fee token. A token deficit (e.g. an unfunded swap side) is rejected;
    /// covering it from the funding wallet is a planned follow-up.
    ///
    /// The wallet draws the Dust and reserves it as one transition, so two
    /// calls on one provider cannot draw the same Dust. Proving runs
    /// afterwards, with the wallet free, and hands the Dust back if it fails.
    pub async fn balance_transaction(&self, tx_bytes: &[u8]) -> Result<Vec<u8>, ProviderError> {
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        Box::pin(self.balance_transaction_inner(tx_bytes)).await
    }

    async fn balance_transaction_inner(&self, tx_bytes: &[u8]) -> Result<Vec<u8>, ProviderError> {
        use midnight_types::midnight_serialize::tagged_deserialize;
        use midnight_types::{
            FinalizedTransaction, FromContext, StandardTrasactionInfo, TokenType,
        };

        let external: FinalizedTransaction<DefaultDB> = tagged_deserialize(&mut &tx_bytes[..])
            .map_err(|e| ProviderError::Transaction(format!("deserialize transaction: {e}")))?;

        // Refuse any non-fee token deficit: this path only adds Dust, so a
        // shortfall in any other token (an unfunded swap side) would just fail
        // at submit. Dust itself is what we are here to supply, so skip it.
        let imbalance = external
            .balance(None)
            .map_err(|e| ProviderError::Transaction(format!("compute balance: {e:?}")))?;
        if imbalance
            .iter()
            .any(|((tt, _seg), val)| !matches!(tt, TokenType::Dust) && *val < 0)
        {
            return Err(ProviderError::Transaction(
                "balance_transaction covers fees only; the transaction has a token deficit \
                 (swap balancing is not supported yet)"
                    .into(),
            ));
        }

        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let context = self.execution_context().await?;
        let tx_info =
            StandardTrasactionInfo::new_from_context(context, self.proof_provider(), None);
        let Some(reserved) = arc.prepare_fees(tx_info, &external).await? else {
            return Ok(tx_bytes.to_vec());
        };
        let fee = self.prove_reserved(reserved).await?;
        self.merge_transactions(&[tx_bytes.to_vec(), fee.tx_bytes])
    }

    /// Fund a transaction the caller assembled, and prove it.
    ///
    /// The wallet balances the fee and records what it drew as one transition,
    /// then proving runs with the wallet free. A proof that fails hands the
    /// Dust back.
    ///
    /// One call rather than a reserve step and a prove step, so a build cannot
    /// reach a different provider between them. The reservation belongs to
    /// this provider's wallet, and only this provider can hand it back.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn build_funded(
        &self,
        tx_info: midnight_types::StandardTrasactionInfo<DefaultDB, BuilderCtx>,
    ) -> Result<TransferResult, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let reserved = arc.prepare_funded(tx_info).await?;
        self.prove_reserved(reserved).await
    }

    /// Hand back the inputs a build reserved, because that build will never
    /// reach the chain. See [`WalletFacade::release`].
    ///
    /// A build reserves its inputs so a later one does not re-select them, so
    /// a transaction that is rejected at submit, or built and then abandoned,
    /// keeps its coins out of circulation until the TTL window elapses.
    /// Releasing frees them at once.
    ///
    /// Only for a transaction that cannot land. Releasing one still in flight
    /// lets a later build re-select the same inputs, and the loser is rejected
    /// on chain.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn release(&self, spent: &SpentInputs) -> Result<(), ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        arc.release(spent).await;
        Ok(())
    }

    /// Submit a built transaction and keep its reservation alive on the
    /// returned handle.
    ///
    /// A node's definitive rejection arrives as a terminal status while
    /// awaiting inclusion, after this has already returned, so the inputs a
    /// build reserved have to travel with the handle for anything to hand them
    /// back.
    ///
    /// Failing here frees them only when the transaction provably never left
    /// this process. A failed RPC call may still have delivered it, so those
    /// inputs stay reserved and wait out their TTL.
    pub(crate) async fn submit_reserved(
        &self,
        result: &TransferResult,
    ) -> Result<PendingTx, ProviderError> {
        match self.submit(&result.tx_bytes).await {
            Ok(pending) => Ok(match self.wallet.as_ref() {
                Some(wallet) => pending.with_reservation(crate::submit::Reservation::new(
                    wallet.clone(),
                    SpentInputs::from(result),
                )),
                None => pending,
            }),
            Err(err) => {
                if crate::submit::never_reached_the_node(&err)
                    && let Err(release_err) = self.release(&SpentInputs::from(result)).await
                {
                    tracing::warn!(
                        error = %release_err,
                        "could not release the inputs of a transaction that never reached the \
                         node; they stay reserved until their TTL elapses"
                    );
                }
                Err(err)
            }
        }
    }

    /// Spend the given coins, returning inputs an offer builder can hold.
    ///
    /// The wallet performs the spends, so what comes back carries no key
    /// material and the builder never sees a seed. `context` must be the one
    /// the caller is building against: the spends roll its wallet state
    /// forward, which is what stops a coin being spent twice in one offer.
    ///
    /// Each coin is named by a nullifier the wallet already knows, so this
    /// selects nothing; the caller decides what to spend. Returns
    /// [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn prepare_shielded_inputs(
        &self,
        context: &Arc<midnight_types::LedgerContext<DefaultDB>>,
        coins: &[midnight_types::SpendableShieldedCoin],
        rng: &mut midnight_types::StdRng,
    ) -> Result<(Vec<midnight_types::PreparedInput>, HeldInputs), ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let nullifiers: Vec<_> = coins.iter().map(|c| c.nullifier).collect();
        let (prepared, spent) = arc.spend_shielded(context, nullifiers, rng).await?;
        Ok((prepared, HeldInputs::of(spent, self.wallet.clone())))
    }

    /// The attached wallet's shielded public keys. See
    /// `Wallet::shielded_public_keys` on the implementing wallet.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn shielded_public_keys(
        &self,
    ) -> Result<
        (
            midnight_types::CoinPublicKey,
            midnight_types::EncryptionPublicKey,
        ),
        ProviderError,
    > {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.shielded_public_keys().await)
    }

    /// Get or create the node connection.
    ///
    /// Built once and cached for the provider's lifetime: the underlying
    /// websocket auto-reconnects with backoff, so a network drop needs no
    /// cache invalidation. The initial dial is bounded by [`RPC_TIMEOUT`]
    /// (the reconnecting client would otherwise retry a misconfigured URL
    /// forever instead of failing fast).
    async fn get_or_connect(&self) -> Result<NodeConnection, ProviderError> {
        {
            let guard = self.conn.read().await;
            if let Some(ref conn) = *guard {
                return Ok(conn.clone());
            }
        }

        info!(url = %self.node_url, "Connecting to Midnight node");
        let reconnecting = tokio::time::timeout(
            RPC_TIMEOUT,
            ReconnectingRpcClient::builder().build(&self.node_url),
        )
        .await
        .map_err(|_| {
            ProviderError::Rpc(format!(
                "connecting to the node at {} timed out after {RPC_TIMEOUT:?}",
                self.node_url
            ))
        })?
        .map_err(|e| ProviderError::Rpc(e.to_string()))?;
        let rpc = RpcClient::new(reconnecting);
        // The runtime-aware client shares the same auto-reconnecting
        // transport; building it fetches metadata, so it is part of the
        // one-time connection cost.
        let client = OnlineClient::<subxt::SubstrateConfig>::from_rpc_client(rpc.clone())
            .await
            .map_err(|e| ProviderError::Rpc(format!("building the runtime client: {e}")))?;

        let mut guard = self.conn.write().await;
        if guard.is_none() {
            *guard = Some(NodeConnection { rpc, client });
        }
        Ok(guard.as_ref().unwrap().clone())
    }
}

#[async_trait]
impl Provider for MidnightProvider {
    async fn get_contract_state(
        &self,
        address: &str,
        offset: Option<ContractActionOffset>,
    ) -> Result<Option<String>, ProviderError> {
        Ok(self.indexer.get_contract_state(address, offset).await?)
    }

    async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, ProviderError> {
        Ok(self
            .indexer
            .get_latest_contract_block_height(address)
            .await?)
    }

    async fn query_contract_state(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
    ) -> Result<Vec<StateQueryResult>, ProviderError> {
        self.query_contract_state_at(address, queries, None).await
    }
}

/// The node's block-hash type under the chain's Substrate config.
pub type NodeBlockHash = subxt::config::HashFor<subxt::SubstrateConfig>;

/// A node block hash as lower-case `0x` hex.
///
/// Through `LowerHex`, the trait that promises hex, and not through `Debug`.
/// They agree today only because `fixed-hash` writes `Debug` as `{:#x}`, while
/// `Display` on the same type truncates to `0x1234…5678`. A stored hash has to
/// survive a dependency bump.
fn hash_hex(hash: &NodeBlockHash) -> String {
    format!("{hash:#x}")
}

#[cfg(test)]
mod hash_hex_tests {
    use super::*;

    /// The rendering is persisted in a wallet snapshot and compared on the
    /// next resume, so a dependency bump that changed it would make every
    /// stored pin stop matching and every persisted wallet refuse to sync.
    #[test]
    fn a_hash_renders_as_full_lowercase_hex() {
        let hash = NodeBlockHash::from([0xab; 32]);
        let rendered = hash_hex(&hash);
        assert_eq!(rendered, format!("0x{}", "ab".repeat(32)));
        assert_eq!(rendered.len(), 66, "0x plus 64 hex digits");
        assert!(
            !rendered.contains('\u{2026}'),
            "an abbreviated hash would still look like one and never match"
        );
    }
}
/// The node's block-header type under the chain's Substrate config; `number`
/// is the block height.
pub type NodeHeader = <subxt::SubstrateConfig as subxt::Config>::Header;

impl MidnightProvider {
    /// Get the current block number from the node (`chain_getHeader.number`).
    pub async fn get_block_number(&self) -> Result<u64, ProviderError> {
        let conn = self.get_or_connect().await?;

        let header: serde_json::Value =
            match conn.rpc.request("chain_getHeader", RpcParams::new()).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "chain_getHeader failed");
                    return Err(ProviderError::Rpc(e.to_string()));
                }
            };

        debug!(header = %header, "chain_getHeader response");

        let block_number = header
            .get("number")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::Rpc("missing 'number' field in header".to_string()))
            .and_then(|hex| {
                let hex = hex.strip_prefix("0x").unwrap_or(hex);
                u64::from_str_radix(hex, 16)
                    .map_err(|e| ProviderError::Rpc(format!("invalid block number hex: {e}")))
            })?;

        Ok(block_number)
    }

    /// Get the latest finalized block height (`archive_v1_finalizedHeight`).
    ///
    /// Finalized blocks cannot reorg (GRANDPA), so heights at or below this
    /// are safe for observers that must never see a block twice. Part of the
    /// `archive_v1` spec, the replacement for the legacy `chain_*` RPCs.
    pub async fn get_finalized_block_height(&self) -> Result<u64, ProviderError> {
        let conn = self.get_or_connect().await?;

        match archive_rpc(&conn).archive_v1_finalized_height().await {
            Ok(height) => Ok(height as u64),
            Err(e) => {
                warn!(error = %e, "archive_v1_finalizedHeight failed");
                Err(ProviderError::Rpc(e.to_string()))
            }
        }
    }

    /// Get the hashes of the blocks at `height` (`archive_v1_hashByHeight`):
    /// exactly one for a height at or below the finalized height, empty when
    /// the chain has not reached `height`, and possibly several while
    /// unfinalized forks exist at it.
    ///
    /// A finalized height's hash pins historical reads such as
    /// [`get_state_from_node`](Self::get_state_from_node).
    pub async fn get_block_hashes_by_height(
        &self,
        height: u64,
    ) -> Result<Vec<NodeBlockHash>, ProviderError> {
        let conn = self.get_or_connect().await?;

        let height = usize::try_from(height)
            .map_err(|_| ProviderError::Rpc(format!("block height {height} overflows usize")))?;
        match archive_rpc(&conn).archive_v1_hash_by_height(height).await {
            Ok(hashes) => Ok(hashes),
            Err(e) => {
                warn!(error = %e, "archive_v1_hashByHeight failed");
                Err(ProviderError::Rpc(e.to_string()))
            }
        }
    }

    /// Get the header of the block with `hash` (`archive_v1_header`), or
    /// `None` when the node does not know the hash. The SCALE-encoded
    /// response decodes into the config-derived [`NodeHeader`].
    pub async fn get_block_header(
        &self,
        hash: NodeBlockHash,
    ) -> Result<Option<NodeHeader>, ProviderError> {
        let conn = self.get_or_connect().await?;

        match archive_rpc(&conn).archive_v1_header(hash).await {
            Ok(header) => Ok(header),
            Err(e) => {
                warn!(error = %e, "archive_v1_header failed");
                Err(ProviderError::Rpc(e.to_string()))
            }
        }
    }

    /// The node's chain-spec display name (substrate `system_chain`), e.g.
    /// `"Midnight Devnet"`.
    ///
    /// This is a human-readable label, **not** the ledger network id. It is not
    /// interchangeable with [`Network`]: feeding it to
    /// a wallet sync would yield `Network::Other(<label>)`
    /// and therefore wrong bech32 address prefixes. For the value that governs
    /// address encoding and transaction binding, use
    /// [`MidnightProvider::ledger_network_id`] or [`MidnightProvider::network`].
    pub async fn system_chain(&self) -> Result<String, ProviderError> {
        let conn = self.get_or_connect().await?;

        let chain: String = match conn.rpc.request("system_chain", RpcParams::new()).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "system_chain failed");
                return Err(ProviderError::Rpc(e.to_string()));
            }
        };

        debug!(chain = %chain, "system_chain response");

        Ok(chain)
    }

    /// The ledger's network id, read from current ledger state.
    ///
    /// This is the authoritative value: it is what binds a transaction
    /// (`Transaction::from_intents`) and what a wallet's bech32 address prefix
    /// must agree with. Compare it against [`MidnightProvider::network`] to
    /// detect a wallet synced against the wrong chain.
    ///
    /// Ledger state reaches this SDK only through a build context, so this
    /// requires an attached wallet (otherwise [`ProviderError::NoWallet`]) and
    /// resyncs it as a side effect. It reads no coin state, so it builds only
    /// the execution half and leaves the pending reservations alone. The
    /// resync still takes the wallet's write lock to commit.
    pub async fn ledger_network_id(&self) -> Result<String, ProviderError> {
        let context = self.execution_context().await?;
        Ok(context.with_ledger_state(|ls| ls.network_id.clone()))
    }

    /// The [`Network`] this provider's wallet derives addresses for.
    ///
    /// Errors if no wallet is attached.
    pub async fn network(&self) -> Result<Network, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.network().await)
    }

    /// Get a block by optional offset. Returns the latest block when
    /// `offset` is `None`. Forwards to the indexer's `IndexerClient::get_block`.
    pub async fn get_block(
        &self,
        offset: Option<BlockOffset>,
    ) -> Result<Option<midnight_indexer_client::Block>, ProviderError> {
        Ok(self.indexer.get_block(offset).await?)
    }

    /// Get a block plus its transactions by optional offset. Returns the
    /// latest block when `offset` is `None`. Forwards to the indexer's
    /// `IndexerClient::get_block_with_transactions`.
    pub async fn get_block_with_transactions(
        &self,
        offset: Option<BlockOffset>,
    ) -> Result<Option<midnight_indexer_client::Block>, ProviderError> {
        Ok(self.indexer.get_block_with_transactions(offset).await?)
    }

    /// Fetch a contract action (state + metadata) at an optional offset.
    /// Returns the latest action when `offset` is `None`. Forwards to the
    /// indexer's `IndexerClient::get_contract_action`.
    pub async fn get_contract_action(
        &self,
        address: &str,
        offset: Option<ContractActionOffset>,
    ) -> Result<Option<ContractAction>, ProviderError> {
        Ok(self.indexer.get_contract_action(address, offset).await?)
    }

    /// Fetch transactions by offset (hash or identifier). Forwards to the
    /// indexer's `IndexerClient::get_transactions`.
    ///
    /// A hash offset means the Midnight transaction hash that
    /// [`TxInBlock`](crate::TxInBlock) carries, never the substrate extrinsic
    /// hash.
    ///
    /// The SDK reads a transaction's fate from the node, not from here: a
    /// completed [`PendingTx::wait_best`] / [`PendingTx::wait_finalized`]
    /// hands back a [`TxInBlock`](crate::TxInBlock) whose `verdict` is the
    /// chain's own. Reach for this when you need what the node's events do
    /// not carry, which is the per-segment breakdown in
    /// `TransactionResult::segments` for a transaction holding more than one
    /// fallible segment (a merged multi-party transaction).
    pub async fn get_transactions(
        &self,
        offset: TransactionOffset,
    ) -> Result<Vec<midnight_indexer_client::Transaction>, ProviderError> {
        Ok(self.indexer.get_transactions(offset).await?)
    }

    /// Best-effort health status of both the node and indexer.
    ///
    /// Never returns `Err`; failures surface in the returned [`Health`] fields.
    pub async fn health(&self) -> Result<Health, ProviderError> {
        // --- Node health via RPC ---
        let (node_connected, block_height, peers, is_syncing) = match self.get_or_connect().await {
            Err(err) => {
                warn!(url = %self.node_url, error = %err, "Failed to connect to Midnight node");
                (false, None, None, None)
            }
            Ok(conn) => {
                let sys_health: Option<serde_json::Value> =
                    match conn.rpc.request("system_health", RpcParams::new()).await {
                        Ok(v) => Some(v),
                        Err(e) => {
                            warn!(error = %e, "system_health RPC call failed");
                            None
                        }
                    };

                let peers = sys_health
                    .as_ref()
                    .and_then(|v| v.get("peers"))
                    .and_then(|v| v.as_u64());
                let is_syncing = sys_health
                    .as_ref()
                    .and_then(|v| v.get("isSyncing"))
                    .and_then(|v| v.as_bool());

                debug!(health = ?sys_health, "system_health response");

                let header: Option<serde_json::Value> =
                    match conn.rpc.request("chain_getHeader", RpcParams::new()).await {
                        Ok(v) => Some(v),
                        Err(e) => {
                            warn!(error = %e, "chain_getHeader RPC call failed");
                            None
                        }
                    };

                debug!(header = ?header, "chain_getHeader response");

                let block_height = header
                    .as_ref()
                    .and_then(|v| v.get("number"))
                    .and_then(|v| v.as_str())
                    .and_then(|hex| {
                        let hex = hex.strip_prefix("0x").unwrap_or(hex);
                        u64::from_str_radix(hex, 16).ok()
                    });

                let node_connected = sys_health.is_some() || header.is_some();
                (node_connected, block_height, peers, is_syncing)
            }
        };

        // --- Indexer health ---
        let indexer_connected = self.indexer.health_check().await;

        Ok(Health {
            node_connected,
            indexer_connected,
            block_height,
            peers,
            is_syncing,
        })
    }

    /// Fetch full contract state via the node RPC (`midnight_contractState`).
    ///
    /// Returns the hex-encoded serialized contract state, or `None` if the
    /// contract is not deployed. This uses the standard node RPC that is
    /// available on all devnet nodes (unlike `midnight_queryContractState`
    /// which requires a custom node build).
    pub async fn get_state_from_node(
        &self,
        address: &str,
        at_block_hash: Option<NodeBlockHash>,
    ) -> Result<Option<String>, ProviderError> {
        let conn = self.get_or_connect().await?;
        let mut params = RpcParams::new();
        params
            .push(address)
            .map_err(|e| ProviderError::Rpc(e.to_string()))?;
        params
            .push(at_block_hash.map(|hash| format!("{hash:#x}")))
            .map_err(|e| ProviderError::Rpc(e.to_string()))?;
        let hex_state: String = conn
            .rpc
            .request("midnight_contractState", params)
            .await
            .map_err(|e| ProviderError::Rpc(e.to_string()))?;
        if hex_state.is_empty() {
            Ok(None)
        } else {
            Ok(Some(hex_state))
        }
    }

    /// Query contract state with an optional block hash pin.
    ///
    /// When `at_block_hash` is `None`, the node returns state at the latest
    /// block. When set, the node returns state as of that specific block hash.
    pub(crate) async fn query_contract_state_at(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
        at_block_hash: Option<NodeBlockHash>,
    ) -> Result<Vec<StateQueryResult>, ProviderError> {
        let conn = self.get_or_connect().await?;
        let mut params = RpcParams::new();
        params
            .push(address)
            .map_err(|e| ProviderError::Rpc(e.to_string()))?;
        params
            .push(queries)
            .map_err(|e| ProviderError::Rpc(e.to_string()))?;
        params
            .push(at_block_hash.map(|hash| format!("{hash:#x}")))
            .map_err(|e| ProviderError::Rpc(e.to_string()))?;
        conn.rpc
            .request("midnight_queryContractState", params)
            .await
            .map_err(|e| ProviderError::Rpc(e.to_string()))
    }
}

/// The inputs a build has reserved, released if the build does not finish.
///
/// The reservation is recorded before proving, so anything that ends a build
/// early has to hand the inputs back or they stay unusable until their TTL
/// elapses. An error path can await the release itself; a caller that drops
/// the build future (a `timeout`, a `select!`, an aborted task) gives it no
/// chance to, and `Drop` cannot await, so that case hands the release to the
/// runtime. Call [`Self::keep`] on any path that deals with the inputs itself.
pub struct HeldInputs {
    wallet: Option<Arc<dyn WalletFacade>>,
    spent: SpentInputs,
}

impl HeldInputs {
    fn of(spent: SpentInputs, wallet: Option<Arc<dyn WalletFacade>>) -> Self {
        Self { wallet, spent }
    }

    /// Stop this from releasing anything, because the build reached the
    /// chain and the reservation must stand.
    pub fn keep(&mut self) {
        self.wallet = None;
    }
}

impl Drop for HeldInputs {
    fn drop(&mut self) {
        let Some(wallet) = self.wallet.take() else {
            return;
        };
        let spent = std::mem::take(&mut self.spent);
        if spent.is_empty() {
            return;
        }
        // No runtime means the process is going down, which frees the
        // in-memory reservation anyway; the persisted one is rebuilt on the
        // next sync.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { wallet.release(&spent).await });
        }
    }
}

/// The node facts a chain pin asks for. `Wallet::sync`'s `pinned_to` takes
/// this view, and the provider's own resync checks pins through the same
/// answers.
#[async_trait]
impl midnight_types::chain_pin::ChainView for MidnightProvider {
    async fn block_hashes_at(&self, height: u64) -> Option<Vec<String>> {
        self.get_block_hashes_by_height(height)
            .await
            .ok()
            .map(|hs| hs.iter().map(hash_hex).collect())
    }

    async fn finalized_height(&self) -> Option<u64> {
        self.get_finalized_block_height().await.ok()
    }
}

/// Typed view over a connection's raw client for the new JSON-RPC spec
/// family (`chainHead_v1` / `archive_v1`), with hash and header types
/// derived from the chain's Substrate config.
fn archive_rpc(conn: &NodeConnection) -> ChainHeadRpcMethods<RpcConfigFor<subxt::SubstrateConfig>> {
    ChainHeadRpcMethods::new(conn.rpc.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_provider() {
        let provider =
            MidnightProvider::new("ws://localhost:9944", "http://localhost:8088").unwrap();
        assert_eq!(
            provider.indexer.url(),
            "http://localhost:8088/api/v4/graphql"
        );
    }

    fn test_provider() -> MidnightProvider {
        MidnightProvider::new("ws://test", "http://test").unwrap()
    }

    /// Merging nothing is a caller error, not an empty transaction.
    #[test]
    fn merge_transactions_rejects_empty() {
        let err = test_provider().merge_transactions(&[]).unwrap_err();
        assert!(
            matches!(err, ProviderError::Transaction(ref m) if m.contains("at least one")),
            "got {err:?}"
        );
    }

    /// Undecodable bytes surface as a typed `Transaction` error naming the
    /// failing step, not a panic inside the ledger deserializer.
    #[test]
    fn merge_transactions_rejects_invalid_bytes() {
        let err = test_provider()
            .merge_transactions(&[vec![0xFF; 8]])
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::Transaction(ref m) if m.contains("deserialize")),
            "got {err:?}"
        );
    }

    /// `balance_transaction` deserializes before touching the network, so
    /// garbage bytes fail fast with a typed error and no node access.
    #[tokio::test]
    async fn balance_transaction_rejects_invalid_bytes() {
        let err = test_provider()
            .balance_transaction(&[0xFF; 8])
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::Transaction(ref m) if m.contains("deserialize")),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn health_returns_disconnected_on_bad_urls() {
        let provider = MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1").unwrap();
        let health = provider.health().await.unwrap();
        assert!(!health.node_connected);
        assert!(!health.indexer_connected);
    }

    /// Both entry points to coin recovery need a wallet to register against;
    /// neither may reach the indexer without one.
    ///
    /// The coin is built through this crate's own re-exports, the paths a
    /// caller who depends only on `midnight-provider` has to write.
    #[tokio::test]
    async fn coin_recovery_without_a_wallet_is_a_typed_error() {
        use crate::{CoinInfo, HashOutput, Nonce, ShieldedTokenType};

        let coin = CoinInfo {
            nonce: Nonce(HashOutput([9u8; 32])),
            type_: ShieldedTokenType(HashOutput([3u8; 32])),
            value: 42,
        };
        assert!(matches!(
            test_provider().watch_for_coin(coin).await.unwrap_err(),
            ProviderError::NoWallet
        ));
        assert!(matches!(
            test_provider().rescan_shielded().await.unwrap_err(),
            ProviderError::NoWallet
        ));
        assert!(matches!(
            test_provider().forget_coin(coin).await.unwrap_err(),
            ProviderError::NoWallet
        ));
    }
}
