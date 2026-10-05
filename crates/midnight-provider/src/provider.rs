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
use crate::{
    Health, PendingTx, ProofProviders, Provider, ProviderError, StateQuery, StateQueryResult,
    TransactionHash, ledger_8, ledger_9, submit,
};
use midnight_indexer_client::{
    BlockOffset, ContractAction, ContractActionOffset, IndexerClient, IndexerError,
    TransactionOffset,
};
use midnight_private_state::PrivateStateProvider;
use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, DustBalance, EncryptionPublicKey, LedgerVersion,
    Network, ShieldedTokenType, SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo,
    TransferKind, TransferRequest, TransferResult, UnshieldedTokenType, WalletBalance,
};
use midnight_wallet_facade::WalletFacade;

/// Connection timeout for the node WebSocket RPC.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// The pause between two rounds of an effect wait, such as
/// [`MidnightProvider::wait_observed`].
///
/// A wallet sees the zswap events of a transaction about 1.1 s after
/// `wait_finalized` returns, measured on indexer 4.0.0. The round after this
/// pause therefore usually sees them.
const EFFECT_POLL: Duration = Duration::from_secs(1);

/// Cached node connection over a single auto-reconnecting websocket: the
/// subxt `RpcClient` carries every raw RPC (standard Substrate and custom
/// `midnight_*` methods alike), and the `OnlineClient` on top of it serves
/// the calls that need runtime metadata, such as submission.
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
/// through the subxt `RpcClient`, and the calls that need runtime metadata,
/// such as transaction submission, through the `OnlineClient` built on the
/// same transport.
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
    /// The same wallet, as each generation's builds.
    builds: Option<WalletBuildsOf>,
    /// Proof backends for transaction building, one per ledger generation.
    /// Defaults to the local provers on first use; override with
    /// [`Self::with_proof_provider`] to use a remote prover or a custom
    /// implementation.
    proof_provider: Option<ProofProviders>,
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
    /// ```rust,no_run
    /// # use midnight_provider::{MidnightProvider, Network, WalletSeed};
    /// # use midnight_wallet::{LocalWallet, Wallet};
    /// # async fn f(seed: WalletSeed) -> anyhow::Result<()> {
    /// # const NODE_URL: &str = "ws://localhost:9944";
    /// # const INDEXER_URL: &str = "http://localhost:8088";
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?;
    /// let wallet = Wallet::sync(provider.indexer_url(), seed, Network::Undeployed).await?;
    /// let provider = provider.with_wallet(LocalWallet::new(wallet));
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(node_url: &str, indexer_url: &str) -> Result<Self, ProviderError> {
        let indexer = IndexerClient::new(indexer_url)?;
        Ok(Self {
            indexer,
            indexer_url: indexer_url.to_string(),
            node_url: node_url.to_string(),
            wallet: None,
            builds: None,
            proof_provider: None,
            private_state: None,
            conn: Arc::new(RwLock::new(None)),
            resync_lock: Mutex::new(()),
        })
    }

    /// Override the proof backend used by [`Self::transfer_shielded`],
    /// [`Self::transfer_unshielded`], and [`Self::register_dust`].
    ///
    /// Defaults to [`ProofProviders::local`] if unset. Pass a
    /// [`RemoteProofServer`](crate::RemoteProofServer) to offload proving to an
    /// HTTP proof server, or any custom prover for each generation:
    ///
    /// ```rust,no_run
    /// # fn f() -> anyhow::Result<()> {
    /// # const NODE_URL: &str = "ws://localhost:9944";
    /// # const INDEXER_URL: &str = "http://localhost:8088";
    /// use std::sync::Arc;
    /// use midnight_provider::{MidnightProvider, RemoteProofServer};
    ///
    /// let prover = Arc::new(RemoteProofServer::new("http://localhost:6300".to_string()));
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?.with_proof_provider(prover);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_proof_provider(mut self, proof_provider: impl Into<ProofProviders>) -> Self {
        self.proof_provider = Some(proof_provider.into());
        self
    }

    /// The proof backend used to prove transactions built through this
    /// provider (transfers, dust registration, and every contract deploy /
    /// call / maintenance op driven by a `Contract` built on it).
    ///
    /// Returns the backends set via [`Self::with_proof_provider`], or
    /// [`ProofProviders::local`] when none was configured. Cheap to clone
    /// (`Arc`).
    pub fn proof_provider(&self) -> ProofProviders {
        self.proof_provider
            .clone()
            .unwrap_or_else(ProofProviders::local)
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
    /// A store holds one wallet's private state, so open it for the unshielded
    /// address of the wallet that makes the calls.
    ///
    /// ```rust,no_run
    /// # use midnight_provider::MidnightProvider;
    /// # fn f(wallet_address: &str) -> anyhow::Result<()> {
    /// # const NODE_URL: &str = "ws://localhost:9944";
    /// # const INDEXER_URL: &str = "http://localhost:8088";
    /// use std::sync::Arc;
    /// use midnight_provider::FsPrivateStateProvider;
    ///
    /// let store = Arc::new(FsPrivateStateProvider::with_default_dir(wallet_address).unwrap());
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?.with_private_state(store);
    /// # Ok(())
    /// # }
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
    /// `LocalWallet::new(wallet)`. Anything else that implements
    /// [`WalletFacade`], [`ledger_8::WalletBuilds`] and
    /// [`ledger_9::WalletBuilds`] goes in as itself.
    pub fn with_wallet<W>(mut self, wallet: W) -> Self
    where
        W: midnight_wallet_facade::ledger_8::WalletBuilds
            + midnight_wallet_facade::ledger_9::WalletBuilds
            + 'static,
    {
        let wallet = Arc::new(wallet);
        self.builds = Some(WalletBuildsOf {
            ledger_8: wallet.clone(),
            ledger_9: wallet.clone(),
        });
        self.wallet = Some(wallet);
        self
    }

    /// The attached wallet, as the API it implements.
    pub(crate) fn facade(&self) -> Option<Arc<dyn WalletFacade>> {
        self.wallet.clone()
    }

    /// The ledger generation the chain runs, read from the ledger parameters
    /// of the indexer's latest block.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Indexer`] when the indexer has no block yet, or serves
    /// one this build cannot read.
    pub async fn ledger_version(&self) -> Result<LedgerVersion, ProviderError> {
        let block = self
            .indexer
            .get_block(None)
            .await?
            .ok_or(IndexerError::MissingData)?;
        LedgerVersion::of_block(&block)
            .map_err(|e| IndexerError::Deserialization(e.to_string()).into())
    }

    /// Resync the attached wallet, then hand out its builds for the generation
    /// its state is in.
    ///
    /// The resync lets the builds see the chain's current view: the proof
    /// root, the TTL anchor, and, after a hard fork, the new generation.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn builds(&self) -> Result<Builds<'_>, ProviderError> {
        self.resync_wallet().await?;
        let wallet = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let builds = self.builds.as_ref().ok_or(ProviderError::NoWallet)?;
        let provers = self.proof_provider();
        Ok(match wallet.ledger_version().await {
            LedgerVersion::V8 => Builds::Ledger8(ledger_8::Builds::new(
                self,
                builds.ledger_8.clone(),
                provers.ledger_8(),
            )),
            LedgerVersion::V9 => Builds::Ledger9(ledger_9::Builds::new(
                self,
                builds.ledger_9.clone(),
                provers.ledger_9(),
            )),
        })
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

    /// The chain's Dust and TTL parameters, as the attached wallet last synced
    /// them.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn parameters(&self) -> Result<ChainParameters, ProviderError> {
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

    /// Resync the attached wallet until it sees every input that `spent`
    /// names spent on chain.
    ///
    /// A finalized transaction reaches the wallet only through a resync. The
    /// indexer serves its events a moment after finality, so one resync can
    /// miss them. Each round resyncs, then asks
    /// [`WalletFacade::has_observed`], then pauses for about a second. Pass
    /// the [`PendingTx::spent_inputs`] of the transaction.
    ///
    /// On `Ok`, the confirmed state of the wallet holds none of those inputs.
    /// The outputs of the same transaction usually arrive in the same resync,
    /// but nothing promises it. For an output on a leg that the transaction
    /// spends nothing from, use [`Self::resync_until`]. An example is a coin
    /// that another wallet sends to this one. A `spent` that names no input
    /// gives `Ok` after one resync.
    ///
    /// Call this only after [`TxInBlock::ensure_applied`] returns `Ok` on the
    /// finalized verdict. After a `PartialSuccess` or `Failure` verdict, an
    /// input that the chain did not spend never reads as spent, and the wait
    /// runs until the timeout.
    ///
    /// ```rust,no_run
    /// # async fn f(
    /// #     provider: midnight_provider::MidnightProvider,
    /// #     recipient: String,
    /// # ) -> anyhow::Result<()> {
    /// use std::time::Duration;
    ///
    /// use midnight_provider::NIGHT;
    ///
    /// let pending = provider.transfer_unshielded(NIGHT, 1, &recipient).await?;
    /// let (finalized, pending) = pending.wait_finalized().await?;
    /// let applied = finalized.ensure_applied()?;
    /// provider
    ///     .wait_observed(
    ///         applied.transaction_hash,
    ///         pending.spent_inputs(),
    ///         Duration::from_secs(60),
    ///     )
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// - [`ProviderError::EffectTimeout`] with `transaction_hash` when the
    ///   timeout passes first. The wait checks the deadline between rounds,
    ///   so a zero timeout runs one round.
    /// - The error of a failed resync, such as an indexer that restarts. It
    ///   ends the wait, which does not retry.
    /// - [`ProviderError::NoWallet`] if no wallet is attached.
    ///
    /// [`TxInBlock::ensure_applied`]: crate::TxInBlock::ensure_applied
    pub async fn wait_observed(
        &self,
        transaction_hash: TransactionHash,
        spent: &[SpentInputs],
        timeout: Duration,
    ) -> Result<(), ProviderError> {
        self.wait_observed_within(transaction_hash, spent, &EffectWait::start(timeout))
            .await
    }

    /// [`Self::wait_observed`] against a deadline that several waits share.
    async fn wait_observed_within(
        &self,
        transaction_hash: TransactionHash,
        spent: &[SpentInputs],
        wait: &EffectWait,
    ) -> Result<(), ProviderError> {
        let wallet = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        loop {
            self.resync_wallet().await?;
            if wallet.has_observed(spent).await {
                return Ok(());
            }
            wait.next_round(Some(transaction_hash)).await?;
        }
    }

    /// Resync the attached wallet until `done` holds for its balance, and
    /// return that balance.
    ///
    /// It waits for an effect that [`Self::wait_observed`] cannot name, such
    /// as an output on a leg that the transaction spends nothing from. A coin
    /// that another wallet sends to this one is an example. Each round
    /// resyncs, reads [`Self::balance`], and calls `done`, then pauses for
    /// about a second.
    ///
    /// ```rust,no_run
    /// # async fn f(provider: midnight_provider::MidnightProvider) -> anyhow::Result<()> {
    /// use std::time::Duration;
    ///
    /// let balance = provider
    ///     .resync_until(Duration::from_secs(60), |balance| {
    ///         !balance.shielded.coins.is_empty()
    ///     })
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// - [`ProviderError::EffectTimeout`] with no transaction hash when the
    ///   timeout passes first. The wait checks the deadline between rounds,
    ///   so a zero timeout runs one round.
    /// - The error of a failed resync. It ends the wait, which does not retry.
    /// - [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn resync_until<F>(
        &self,
        timeout: Duration,
        done: F,
    ) -> Result<WalletBalance, ProviderError>
    where
        F: FnMut(&WalletBalance) -> bool + Send,
    {
        self.resync_until_within(&EffectWait::start(timeout), done)
            .await
    }

    /// [`Self::resync_until`] against a deadline that several waits share.
    async fn resync_until_within<F>(
        &self,
        wait: &EffectWait,
        mut done: F,
    ) -> Result<WalletBalance, ProviderError>
    where
        F: FnMut(&WalletBalance) -> bool + Send,
    {
        let wallet = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        loop {
            self.resync_wallet().await?;
            let balance = wallet.balance().await;
            if done(&balance) {
                return Ok(balance);
            }
            wait.next_round(None).await?;
        }
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
    /// ```rust,no_run
    /// # async fn f(
    /// #     provider: midnight_provider::MidnightProvider,
    /// #     nonce: [u8; 32],
    /// #     token_type: [u8; 32],
    /// #     value: u128,
    /// # ) -> anyhow::Result<()> {
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
    /// # Ok(())
    /// # }
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
        match wallet.rescan_shielded().await {
            // The chain crossed a fork the wallet has not. A resync crosses
            // it, and the rescan then replays across the fork.
            Err(midnight_types::WalletError::LedgerMismatch { expected, found })
                if found > expected =>
            {
                wallet.resync(self).await?;
                wallet.rescan_shielded().await?;
            }
            result => result?,
        }
        Ok(())
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
    /// ```rust,no_run
    /// # use midnight_provider::{MidnightProvider, ShieldedTokenType, SpentInputs};
    /// # async fn f(
    /// #     alice: MidnightProvider,
    /// #     bob: MidnightProvider,
    /// #     sponsor: MidnightProvider,
    /// #     token_x: ShieldedTokenType,
    /// #     dx: u128,
    /// #     token_y: ShieldedTokenType,
    /// #     dy: u128,
    /// # ) -> anyhow::Result<()> {
    /// let a_half = alice.shielded_swap(token_x, dx, token_y, dy).await?;
    /// let b_half = bob.shielded_swap(token_y, dy, token_x, dx).await?;
    /// let merged = sponsor.merge_transactions(&[a_half.into_bytes(), b_half.into_bytes()])?;
    /// let funded = sponsor.balance_transaction(&merged).await?;
    /// sponsor
    ///     .submit_reserved(&funded.tx_bytes, vec![SpentInputs::from(&funded)])
    ///     .await?;
    /// # Ok(())
    /// # }
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
    ///
    /// One call registers one tNIGHT UTXO and pays its own fee from that
    /// UTXO. Call it again until [`DustBalance::unregistered_night_utxos`]
    /// is 0, or call [`Self::register_all_night`], which does that and then
    /// waits for spendable Dust. [`TransferBuilder::prepare_register_dust`]
    /// gives the full rule and the meaning of `utxo_ctime`.
    ///
    /// [`DustBalance::unregistered_night_utxos`]: midnight_types::DustBalance::unregistered_night_utxos
    /// [`TransferBuilder::prepare_register_dust`]: midnight_types::ledger_9::TransferBuilder::prepare_register_dust
    pub fn register_dust(&self, utxo_ctime: Option<u64>) -> DustRegistration<'_> {
        DustRegistration::new(self, utxo_ctime)
    }

    /// Register every tNIGHT UTXO for Dust generation, then wait until the
    /// wallet can spend Dust.
    ///
    /// A registration covers one tNIGHT UTXO, so this submits one
    /// [`Self::register_dust`] transaction, with no `utxo_ctime`, for each
    /// UTXO that [`DustBalance::unregistered_night_utxos`] counts. It submits
    /// them one at a time. After each one it waits for finality, checks the
    /// verdict, and waits until the wallet sees the spend. Last, it waits
    /// until [`DustBalance::spendable_speck`] is above 0.
    ///
    /// One timeout covers the whole call. Dust accrues with time, so the
    /// last wait can take tens of seconds on a devnet: give a timeout of
    /// minutes.
    ///
    /// Returns how many registrations it submitted. A wallet with nothing
    /// left to register and spendable Dust gives `Ok(0)`, so a second call
    /// is safe. To register one UTXO at a time, or to give a `utxo_ctime`,
    /// call [`Self::register_dust`].
    ///
    /// ```rust,no_run
    /// # async fn f(provider: midnight_provider::MidnightProvider) -> anyhow::Result<()> {
    /// use std::time::Duration;
    ///
    /// let submitted = provider
    ///     .register_all_night(Duration::from_secs(600))
    ///     .await?;
    /// println!("registered {submitted} tNIGHT UTXOs");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// - [`ProviderError::EffectTimeout`] when the timeout passes first. Its
    ///   `transaction_hash` names the registration that the call waited for,
    ///   and that registration can still land. It is `None` when the timeout
    ///   passes before a submit, in the wait for Dust, and in the wait for a
    ///   UTXO that the indexer gave no key.
    /// - [`ProviderError::NotApplied`] when the chain did not apply a
    ///   registration.
    /// - [`ProviderError::Wallet`] with [`WalletError::Transfer`] when the
    ///   wallet holds no tNIGHT and no Dust, so nothing can generate Dust.
    ///   The wallet must receive tNIGHT first.
    /// - The errors of [`Self::register_dust`], of
    ///   [`PendingTx::wait_finalized`], and of a failed resync.
    /// - [`ProviderError::NoWallet`] if no wallet is attached.
    ///
    /// [`DustBalance::unregistered_night_utxos`]: midnight_types::DustBalance::unregistered_night_utxos
    /// [`DustBalance::spendable_speck`]: midnight_types::DustBalance::spendable_speck
    /// [`WalletError::Transfer`]: midnight_types::WalletError::Transfer
    pub async fn register_all_night(&self, timeout: Duration) -> Result<usize, ProviderError> {
        let wallet = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let wait = EffectWait::start(timeout);
        self.resync_wallet().await?;
        let mut dust = wallet.balance().await.dust;
        let mut submitted = 0;
        while needs_registration(&dust) {
            if wait.remaining().is_zero() {
                return Err(wait.timed_out(None));
            }
            let left = dust.unregistered_night_utxos;
            let pending = self.register_dust(None).await?;
            let transaction_hash = pending.transaction_hash();
            let (finalized, pending) =
                tokio::time::timeout(wait.remaining(), pending.wait_finalized())
                    .await
                    .map_err(|_| wait.timed_out(Some(transaction_hash)))??;
            finalized.ensure_applied()?;
            submitted += 1;
            dust = if pending.spent_inputs().iter().all(SpentInputs::is_empty) {
                // A UTXO with no key leaves no spend to observe, so wait on
                // the count.
                self.resync_until_within(&wait, |balance| {
                    balance.dust.unregistered_night_utxos < left
                })
                .await?
                .dust
            } else {
                self.wait_observed_within(transaction_hash, pending.spent_inputs(), &wait)
                    .await?;
                wallet.balance().await.dust
            };
        }
        self.resync_until_within(&wait, |balance| balance.dust.spendable_speck > 0)
            .await?;
        Ok(submitted)
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

    /// Prepare a build under the attached wallet, then prove without it, on
    /// the generation the wallet's state is in.
    async fn build_then_prove(
        &self,
        request: TransferRequest,
    ) -> Result<TransferResult, ProviderError> {
        // Each arm is boxed so the caller's frame holds one future, not both
        // generations' (see the frame-size note on `resync_wallet`).
        match self.builds().await? {
            Builds::Ledger8(builds) => Box::pin(builds.build_then_prove(request)).await,
            Builds::Ledger9(builds) => Box::pin(builds.build_then_prove(request)).await,
        }
    }

    /// Submit proven transaction bytes to the node over the WebSocket RPC.
    ///
    /// Returns a [`PendingTx`] handle that lets the caller await inclusion
    /// (`wait_best`) and finalization (`wait_finalized`). The provider's
    /// `node_url` is used as the connection target — callers don't repeat it.
    ///
    /// The handle carries no reservation, so a rejection hands nothing back.
    /// Submit the bytes of a build of this provider's wallet with
    /// [`Self::submit_reserved`].
    ///
    /// # Errors
    ///
    /// [`ProviderError::Submission`] with [`SubmitError::NotSubmitted`] when
    /// the node cannot be reached or the extrinsic cannot be built, and with
    /// [`SubmitError::SubmitRpc`] when the submit call fails.
    ///
    /// [`SubmitError::NotSubmitted`]: crate::SubmitError::NotSubmitted
    /// [`SubmitError::SubmitRpc`]: crate::SubmitError::SubmitRpc
    pub async fn submit(&self, tx_bytes: &[u8]) -> Result<PendingTx, ProviderError> {
        self.prepare(tx_bytes).await?.submit().await
    }

    /// Wrap proven transaction bytes in the unsigned `send_mn_transaction`
    /// extrinsic without submitting it, returning a [`crate::PreparedTx`]
    /// whose extrinsic and transaction hashes are already known. Submit it
    /// with [`crate::PreparedTx::submit`]. Lets a caller durably record state
    /// keyed by the extrinsic hash *before* the transaction reaches the
    /// mempool.
    ///
    /// This builds the extrinsic locally from the node's metadata, which
    /// checks only the call's shape. The node validates the transaction only
    /// at submit.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Submission`] with [`SubmitError::NotSubmitted`] when
    /// the node cannot be reached or the extrinsic cannot be built.
    ///
    /// [`SubmitError::NotSubmitted`]: crate::SubmitError::NotSubmitted
    pub async fn prepare(&self, tx_bytes: &[u8]) -> Result<submit::PreparedTx, ProviderError> {
        // Only the submit paths map this: the read paths share the dial.
        let conn = self
            .get_or_connect()
            .await
            .map_err(|e| submit::SubmitError::NotSubmitted {
                message: e.to_string(),
            })?;
        submit::prepare_bytes(&conn.client, tx_bytes).await
    }

    /// [`Self::prepare`] for the bytes of a build of this provider's wallet,
    /// with the inputs that build reserved.
    ///
    /// `reserved` holds one entry per reservation the build made, each with
    /// its own `reserved_at`.
    ///
    /// The returned [`crate::PreparedTx`] guards the inputs: dropped before
    /// submit, it hands them back. Its submit moves them to the
    /// [`PendingTx`], which hands them back on a definitive rejection. Like
    /// [`Self::prepare`], this builds the extrinsic from the node's metadata,
    /// and the node validates the transaction only at submit.
    ///
    /// # Errors
    ///
    /// As [`Self::prepare`]. The transaction never left this process then,
    /// so the inputs are handed back before this returns.
    pub async fn prepare_reserved(
        &self,
        tx_bytes: &[u8],
        reserved: Vec<SpentInputs>,
    ) -> Result<submit::PreparedTx, ProviderError> {
        // Guard before the dial, so a caller that drops this future hands the
        // inputs back too.
        let held: Vec<HeldInputs> = reserved
            .into_iter()
            .map(|spent| HeldInputs::of(spent, self.facade()))
            .collect();
        match self.prepare(tx_bytes).await {
            Ok(prepared) => Ok(prepared.holding(held)),
            Err(err) => {
                let release = crate::submit::never_reached_the_node(&err);
                for mut guard in held {
                    if release {
                        guard.release().await;
                    } else {
                        guard.keep();
                    }
                }
                Err(err)
            }
        }
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
    /// a byte string fails to deserialize, when the transactions are of
    /// different ledger generations, or when two transactions cannot be
    /// merged (colliding intent segments or mismatched network ids). Purely
    /// local; nothing is sent to the node.
    pub fn merge_transactions(&self, txs: &[Vec<u8>]) -> Result<Vec<u8>, ProviderError> {
        let first = txs.first().ok_or_else(|| {
            ProviderError::Transaction(
                "merge_transactions requires at least one transaction".into(),
            )
        })?;
        let ledger = transaction_ledger_version(first)?;
        for tx in &txs[1..] {
            let other = transaction_ledger_version(tx)?;
            if other != ledger {
                return Err(ProviderError::Transaction(format!(
                    "cannot merge a {ledger} transaction with a {other} transaction"
                )));
            }
        }
        match ledger {
            LedgerVersion::V8 => ledger_8::merge_transactions(txs),
            LedgerVersion::V9 => ledger_9::merge_transactions(txs),
        }
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
    ///
    /// The [`TransferResult`] holds the merged transaction, the fee the chain
    /// charges for it, and the Dust this wallet drew for it. Submit it with
    /// [`Self::submit_reserved`], so that a rejection hands the Dust back:
    ///
    /// ```rust,no_run
    /// # async fn f(
    /// #     sponsor: midnight_provider::MidnightProvider,
    /// #     fee_less: Vec<u8>,
    /// # ) -> anyhow::Result<()> {
    /// use midnight_provider::SpentInputs;
    ///
    /// let funded = sponsor.balance_transaction(&fee_less).await?;
    /// let pending = sponsor
    ///     .submit_reserved(&funded.tx_bytes, vec![SpentInputs::from(&funded)])
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// A transaction that already pays its fee comes back as it is, with
    /// nothing reserved.
    pub async fn balance_transaction(
        &self,
        tx_bytes: &[u8],
    ) -> Result<TransferResult, ProviderError> {
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        Box::pin(self.balance_transaction_inner(tx_bytes)).await
    }

    async fn balance_transaction_inner(
        &self,
        tx_bytes: &[u8],
    ) -> Result<TransferResult, ProviderError> {
        let ledger = transaction_ledger_version(tx_bytes)?;
        match self.builds().await? {
            Builds::Ledger8(builds) if ledger == LedgerVersion::V8 => {
                Box::pin(builds.balance_transaction(tx_bytes)).await
            }
            Builds::Ledger9(builds) if ledger == LedgerVersion::V9 => {
                Box::pin(builds.balance_transaction(tx_bytes)).await
            }
            builds => Err(midnight_types::WalletError::LedgerMismatch {
                expected: builds.ledger_version(),
                found: ledger,
            }
            .into()),
        }
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

    /// Submit the bytes of a build of this provider's wallet, and keep the
    /// inputs that build reserved alive on the returned handle.
    ///
    /// A node's definitive rejection arrives as a terminal status while
    /// awaiting inclusion, after this has already returned, so the inputs a
    /// build reserved have to travel with the handle for anything to hand them
    /// back. `reserved` is as for [`Self::prepare_reserved`].
    ///
    /// ```rust,no_run
    /// # async fn f(
    /// #     provider: midnight_provider::MidnightProvider,
    /// #     recipient: String,
    /// # ) -> anyhow::Result<()> {
    /// use midnight_provider::{NIGHT, SpentInputs};
    ///
    /// let built = provider.transfer_unshielded(NIGHT, 100, &recipient).build().await?;
    /// println!("fee: {} SPECK", built.fee_speck);
    /// let pending = provider
    ///     .submit_reserved(&built.tx_bytes, vec![SpentInputs::from(&built)])
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// As [`Self::submit`]. On [`SubmitError::NotSubmitted`] the transaction
    /// never left this process, so the inputs are handed back before this
    /// returns. A failed submit call may still have delivered it, so on
    /// [`SubmitError::SubmitRpc`] the inputs stay reserved until their TTL
    /// elapses.
    ///
    /// [`SubmitError::NotSubmitted`]: crate::SubmitError::NotSubmitted
    /// [`SubmitError::SubmitRpc`]: crate::SubmitError::SubmitRpc
    pub async fn submit_reserved(
        &self,
        tx_bytes: &[u8],
        reserved: Vec<SpentInputs>,
    ) -> Result<PendingTx, ProviderError> {
        self.prepare_reserved(tx_bytes, reserved)
            .await?
            .submit()
            .await
    }

    /// The attached wallet's shielded public keys. See
    /// `Wallet::shielded_public_keys` on the implementing wallet.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn shielded_public_keys(
        &self,
    ) -> Result<(CoinPublicKey, EncryptionPublicKey), ProviderError> {
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

    /// Get the timestamp of the block with `hash`, as time since the Unix
    /// epoch.
    ///
    /// Reads `Timestamp::Now` in that block's state. Genesis sets no
    /// timestamp, so it reads as zero. Errors when the node does not know
    /// `hash`, or no longer holds the state at it.
    pub async fn get_block_timestamp(
        &self,
        hash: NodeBlockHash,
    ) -> Result<Duration, ProviderError> {
        let conn = self.get_or_connect().await?;

        let millis = conn
            .client
            .at_block(hash)
            .await
            .map_err(|e| ProviderError::Rpc(format!("reading block {hash:#x}: {e}")))?
            .storage()
            .fetch(subxt::dynamic::storage::<(), u64>("Timestamp", "Now"), ())
            .await
            .map_err(|e| ProviderError::Rpc(format!("reading Timestamp::Now at {hash:#x}: {e}")))?
            .decode()
            .map_err(|e| {
                ProviderError::Rpc(format!("decoding Timestamp::Now at {hash:#x}: {e}"))
            })?;
        Ok(Duration::from_millis(millis))
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
        Ok(match self.builds().await? {
            Builds::Ledger8(builds) => builds
                .execution_context()
                .await?
                .with_ledger_state(|ls| ls.network_id.clone()),
            Builds::Ledger9(builds) => builds
                .execution_context()
                .await?
                .with_ledger_state(|ls| ls.network_id.clone()),
        })
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
    /// The SDK reads a transaction's fate from the node, not from here:
    /// [`PendingTx::wait_best`] / [`PendingTx::wait_finalized`] read the
    /// chain's own verdict, and fail with [`ProviderError::NotApplied`] when
    /// the transaction did not apply. Reach for this when you need what the
    /// node's events do not carry, which is the per-segment breakdown in
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

/// The attached wallet's builds, on the generation its state is in. See
/// [`MidnightProvider::builds`].
#[derive(Debug)]
pub enum Builds<'a> {
    /// The builds of a wallet whose state is on ledger 8.
    Ledger8(ledger_8::Builds<'a>),
    /// The builds of a wallet whose state is on ledger 9.
    Ledger9(ledger_9::Builds<'a>),
}

impl Builds<'_> {
    /// The generation these builds build for.
    pub fn ledger_version(&self) -> LedgerVersion {
        match self {
            Self::Ledger8(_) => LedgerVersion::V8,
            Self::Ledger9(_) => LedgerVersion::V9,
        }
    }
}

/// The deadline of an effect wait, and what its timeout reports. Several
/// waits in one call can share it, so that one timeout bounds them all.
struct EffectWait {
    started: tokio::time::Instant,
    /// `None` when the timeout is too large for an instant, so the wait has
    /// no deadline.
    deadline: Option<tokio::time::Instant>,
}

impl EffectWait {
    fn start(timeout: Duration) -> Self {
        let started = tokio::time::Instant::now();
        Self {
            started,
            deadline: started.checked_add(timeout),
        }
    }

    /// The time left before the deadline, zero once it has passed.
    fn remaining(&self) -> Duration {
        self.deadline.map_or(Duration::MAX, |deadline| {
            deadline.saturating_duration_since(tokio::time::Instant::now())
        })
    }

    /// The timeout of this wait, which names `transaction_hash` when the wait
    /// was for one transaction.
    fn timed_out(&self, transaction_hash: Option<TransactionHash>) -> ProviderError {
        ProviderError::EffectTimeout {
            waited: self.started.elapsed(),
            transaction_hash,
        }
    }

    /// Pause until the next round, or fail with [`Self::timed_out`] once the
    /// deadline has passed.
    ///
    /// The pause ends at the deadline at the latest, so the last round runs
    /// there rather than up to a full [`EFFECT_POLL`] after it.
    async fn next_round(
        &self,
        transaction_hash: Option<TransactionHash>,
    ) -> Result<(), ProviderError> {
        let now = tokio::time::Instant::now();
        let next = now + EFFECT_POLL;
        let wake = match self.deadline {
            Some(deadline) if now >= deadline => return Err(self.timed_out(transaction_hash)),
            Some(deadline) => next.min(deadline),
            None => next,
        };
        tokio::time::sleep_until(wake).await;
        Ok(())
    }
}

/// Whether [`MidnightProvider::register_all_night`] submits another
/// registration for a wallet with this Dust balance.
///
/// A wallet with no tNIGHT and no Dust, reserved or not, gets one, so that
/// the build fails with the reason. A wallet with no tNIGHT but some Dust gets
/// none: that Dust needs only a wait.
fn needs_registration(dust: &DustBalance) -> bool {
    dust.unregistered_night_utxos > 0 || (!dust.night_generates_dust && dust.balance_speck == 0)
}

/// One wallet, as the builds of each generation.
struct WalletBuildsOf {
    ledger_8: Arc<dyn midnight_wallet_facade::ledger_8::WalletBuilds>,
    ledger_9: Arc<dyn midnight_wallet_facade::ledger_9::WalletBuilds>,
}

/// The generation of a proven transaction, from its tag.
fn transaction_ledger_version(tx_bytes: &[u8]) -> Result<LedgerVersion, ProviderError> {
    LedgerVersion::of_transaction(tx_bytes)
        .map_err(|e| ProviderError::Transaction(format!("deserialize transaction: {e}")))
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
    pub(crate) fn of(spent: SpentInputs, wallet: Option<Arc<dyn WalletFacade>>) -> Self {
        Self { wallet, spent }
    }

    /// What the build reserved.
    pub fn spent(&self) -> &SpentInputs {
        &self.spent
    }

    /// Stop this from releasing anything, because the build reached the
    /// chain and the reservation must stand.
    pub fn keep(&mut self) {
        self.wallet = None;
    }

    /// Hand the inputs back now, not from a task that `Drop` spawns, so the
    /// caller sees them free when this returns.
    pub(crate) async fn release(mut self) {
        if let Some(wallet) = self.wallet.take() {
            wallet.release(&self.spent).await;
        }
    }

    /// Stop releasing, and hand over what this guards with the wallet that
    /// holds it. `None` for a guard with no wallet.
    pub(crate) fn disarm(mut self) -> Option<(Arc<dyn WalletFacade>, SpentInputs)> {
        let wallet = self.wallet.take()?;
        Some((wallet, std::mem::take(&mut self.spent)))
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

    /// A submit that cannot reach the node never sent the transaction, so it
    /// must say `NotSubmitted`: that is the only failure on which a reserved
    /// build hands its inputs back. Any other error keeps them until the TTL.
    #[tokio::test]
    async fn a_failed_dial_is_not_submitted() {
        let provider = MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1").unwrap();
        let not_submitted = |what: &str, err: ProviderError| {
            assert!(
                matches!(
                    err,
                    ProviderError::Submission(submit::SubmitError::NotSubmitted { .. })
                ),
                "{what}: a failed dial must be NotSubmitted, got {err:?}"
            );
        };
        // Concurrent, because each dial waits out `RPC_TIMEOUT`.
        let (submitted, prepared) =
            tokio::join!(provider.submit(&[0; 8]), provider.prepare(&[0; 8]));
        not_submitted("submit", submitted.err().expect("no node to submit to"));
        not_submitted(
            "prepare",
            prepared.err().expect("no node to prepare against"),
        );
    }

    /// With no tNIGHT left to register, `register_all_night` submits only
    /// for a wallet that holds nothing, so the build names the cause. Any
    /// Dust, reserved or not, or any tNIGHT that generates, needs only the
    /// wait for spendable Dust.
    #[test]
    fn a_wallet_with_no_unregistered_night_registers_only_when_it_holds_nothing() {
        let dust = |night_generates_dust, balance_speck| DustBalance {
            spendable_utxos: 0,
            balance_speck,
            spendable_speck: 0,
            night_generates_dust,
            unregistered_night_utxos: 0,
        };
        assert!(
            needs_registration(&dust(false, 0)),
            "a wallet with no tNIGHT and no Dust must reach the build that says why"
        );
        assert!(
            !needs_registration(&dust(false, 5)),
            "Dust that a build in flight reserves becomes spendable with no registration"
        );
        assert!(
            !needs_registration(&dust(true, 0)),
            "registered tNIGHT generates Dust with no further registration"
        );
    }
}
