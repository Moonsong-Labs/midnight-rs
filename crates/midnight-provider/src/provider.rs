use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use subxt::OnlineClient;
use subxt::config::RpcConfigFor;
use subxt::rpcs::ChainHeadRpcMethods;
use subxt::rpcs::client::reconnecting_rpc_client::RpcClient as ReconnectingRpcClient;
use subxt::rpcs::client::{RpcClient, RpcParams};
use tokio::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::transfer::{DustRegistration, ShieldedTransfer, UnshieldedTransfer};
use crate::{
    Health, PendingTx, Provider, ProviderError, StateQuery, StateQueryResult, TxResultWait, submit,
};
use midnight_helpers::{
    DefaultDB, LedgerContext, LocalProofServer, ProofProvider, ShieldedTokenType, Timestamp,
    UnshieldedTokenType,
};
use midnight_indexer_client::{
    BlockOffset, ContractAction, ContractActionOffset, IndexerClient, TransactionOffset,
};
use midnight_private_state::PrivateStateProvider;
use midnight_wallet::{
    Network, SpendableShieldedCoin, SyncProgress, TransferBuilder, TransferResult, Wallet,
    WalletBalance, WalletSeed,
};

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
    /// The wallet, owned by the provider behind interior mutability.
    ///
    /// The `Arc<RwLock<_>>` is the single source of truth for the wallet's
    /// synced state. Background sync, resync, and tx-building all lock this
    /// to read or mutate. Cloning the `Arc` is cheap and safe.
    wallet: Option<Arc<RwLock<Wallet>>>,
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

/// Handle to the background task spawned by
/// [`SyncWalletBuilder::stream`].
///
/// Awaiting it yields the synced [`MidnightProvider`] — a single `?` is
/// enough. A panic or cancellation of the spawned task surfaces as
/// [`ProviderError::SyncTaskJoin`]; the inner sync error path surfaces as
/// the matching `ProviderError` variant.
///
/// **Dropping the handle cancels the sync.** The handle is the only way to
/// obtain the synced provider, so once it is dropped the sync's result is
/// unobservable and letting it run would only keep three indexer WebSocket
/// subscriptions alive for nothing. To run a sync without holding a
/// `SyncHandle`, spawn the one-shot path yourself:
/// `tokio::spawn(provider.sync_wallet(seed, network).into_future())`.
pub struct SyncHandle {
    inner: JoinHandle<Result<MidnightProvider, ProviderError>>,
}

impl SyncHandle {
    pub(crate) fn from_handle(inner: JoinHandle<Result<MidnightProvider, ProviderError>>) -> Self {
        Self { inner }
    }
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        // Cancel-on-drop (see the struct docs). Aborting the task drops its
        // in-flight `Subscription` handles, which tear down their WebSocket
        // reader tasks — no orphaned subscriptions survive the handle. The
        // sync task holds no locks at any await point (the wallet is only
        // wrapped in its `RwLock` after the sync completes), so an abort
        // cannot strand a lock. No-op if the task already finished, e.g.
        // after the handle was awaited to completion.
        self.inner.abort();
    }
}

impl std::future::Future for SyncHandle {
    type Output = Result<MidnightProvider, ProviderError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.inner)
            .poll(cx)
            .map(|outer| match outer {
                Ok(inner) => inner,
                Err(join_err) => Err(join_err.into()),
            })
    }
}

/// Builder returned by [`MidnightProvider::sync_wallet`].
///
/// Holds the configuration (seed, network, optional storage dir) until the
/// caller selects a sync path:
///
/// - `.await` — runs the sync in the current task, returns the synced
///   [`MidnightProvider`]. No progress events.
/// - [`stream()`](Self::stream) — spawns the sync in a background task and
///   returns `(receiver, handle)`. The receiver emits [`SyncProgress`] events;
///   the [`SyncHandle`] resolves to the synced provider when sync completes.
pub struct SyncWalletBuilder {
    provider: MidnightProvider,
    seed: WalletSeed,
    network: Network,
    storage_dir: Option<std::path::PathBuf>,
}

impl SyncWalletBuilder {
    /// Persist sync progress + recovered state under `dir`. Without this call,
    /// the wallet runs in-memory only. The directory is retained: every
    /// successful resync re-saves the wallet and each transfer build
    /// persists its pending reservation.
    ///
    /// See [`docs/wallet.md`](https://github.com/RomarQ/midnight-rs/blob/main/docs/wallet.md#persistence)
    /// for the on-disk layout.
    pub fn with_storage(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.storage_dir = Some(dir.into());
        self
    }

    /// Run the sync in a background task and stream progress events.
    ///
    /// Returns `(receiver, handle)`. The receiver emits [`SyncProgress`]
    /// events as each subscription replays. The [`SyncHandle`] resolves to
    /// the synced [`MidnightProvider`] when all three subscriptions finish.
    ///
    /// **Cancellation:** the spawned task lives exactly as long as both
    /// returned ends do. Dropping the progress receiver mid-sync cancels the
    /// task (the handle then resolves to [`ProviderError::SyncCancelled`]),
    /// and dropping the [`SyncHandle`] aborts it — either way the three
    /// indexer WebSocket subscriptions are torn down promptly instead of
    /// running on with no consumer. Keep the receiver alive until you are
    /// done with the sync (the usual `while rx.recv().await` loop does this
    /// naturally: it only ends when the sync itself finishes). For a sync
    /// without progress events, use the plain `.await` path instead of
    /// `stream()`.
    pub fn stream(self) -> (mpsc::Receiver<SyncProgress>, SyncHandle) {
        let (tx, rx) = mpsc::channel(64);
        let SyncWalletBuilder {
            mut provider,
            seed,
            network,
            storage_dir,
        } = self;
        let indexer_url = provider.indexer_url.clone();
        let handle = tokio::spawn(async move {
            let address = midnight_wallet::address::derive_unshielded(&seed, network.clone());
            // A clone of the progress sender watches for receiver drop; the
            // original is consumed by the sync itself.
            let receiver_gone = tx.clone();
            let sync = Wallet::sync_inner(
                &indexer_url,
                seed,
                &address,
                network,
                storage_dir.as_deref(),
                Some(tx),
            );
            tokio::select! {
                // Biased with the cancellation arm first: when the receiver
                // drop and a sync-side "receiver dropped" error become ready
                // in the same poll, the documented `SyncCancelled` must win.
                biased;
                // Receiver dropped mid-sync: the consumer abandoned the
                // stream. Dropping the sync future here tears down its
                // subscriptions and their WebSocket connections.
                _ = receiver_gone.closed() => Err(ProviderError::SyncCancelled),
                result = sync => {
                    provider.wallet = Some(Arc::new(RwLock::new(result?)));
                    Ok(provider)
                }
            }
        });
        (rx, SyncHandle::from_handle(handle))
    }
}

impl std::future::IntoFuture for SyncWalletBuilder {
    type Output = Result<MidnightProvider, ProviderError>;
    type IntoFuture =
        std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send + 'static>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let SyncWalletBuilder {
                mut provider,
                seed,
                network,
                storage_dir,
            } = self;
            let address = midnight_wallet::address::derive_unshielded(&seed, network.clone());
            let wallet = Wallet::sync_inner(
                &provider.indexer_url,
                seed,
                &address,
                network,
                storage_dir.as_deref(),
                None,
            )
            .await?;
            provider.wallet = Some(Arc::new(RwLock::new(wallet)));
            Ok(provider)
        })
    }
}

impl MidnightProvider {
    /// Create a provider from node WebSocket URL and indexer HTTP URL.
    ///
    /// The node connection is **not** established here; it is deferred to
    /// the first call that requires it.
    ///
    /// For the common case where you want the provider to drive sync end-to-end
    /// (URLs only appear in `new`), use [`Self::sync_wallet`]:
    /// ```rust,ignore
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?
    ///     .sync_wallet(seed, Network::Undeployed, None)
    ///     .await?;
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

    /// Attach a synced [`Wallet`]. The provider takes ownership of the wallet
    /// (behind `Arc<RwLock<_>>`) and becomes the single entry point for
    /// resync, transaction-context construction, and background sync.
    pub fn with_wallet(mut self, wallet: Wallet) -> Self {
        self.wallet = Some(Arc::new(RwLock::new(wallet)));
        self
    }

    /// Sync a wallet from the indexer and attach it to this provider.
    ///
    /// Convenience builder around the wallet's sync logic that uses this
    /// provider's indexer URL — callers don't repeat URLs that already live on
    /// the provider:
    ///
    /// ```rust,ignore
    /// // Simple one-shot sync.
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?
    ///     .sync_wallet(seed, Network::Undeployed)
    ///     .await?;
    ///
    /// // With persistence + streamed progress.
    /// let (mut rx, handle) = MidnightProvider::new(NODE_URL, INDEXER_URL)?
    ///     .sync_wallet(seed, Network::Preprod)
    ///     .with_storage(storage_dir)
    ///     .stream();
    /// while let Some(p) = rx.recv().await { /* render */ }
    /// let provider = handle.await?;
    /// ```
    ///
    /// Returns a [`SyncWalletBuilder`] that defers the actual work. Configure
    /// optional persistence with [`SyncWalletBuilder::with_storage`], then
    /// either `.await` for the one-shot path or `.stream()` for streamed
    /// progress events. The two paths share their entire body — they only
    /// differ in whether a progress sender is attached and whether the sync
    /// runs in the current task or a spawned one.
    ///
    /// If a wallet is already attached (via [`Self::with_wallet`] or a previous
    /// `sync_wallet` call), it is replaced by the newly synced wallet.
    ///
    /// To incrementally refresh an already-attached wallet without a full
    /// resync, use [`Self::resync_wallet`].
    pub fn sync_wallet(
        self,
        seed: impl Into<WalletSeed>,
        network: impl Into<Network>,
    ) -> SyncWalletBuilder {
        SyncWalletBuilder {
            provider: self,
            seed: seed.into(),
            network: network.into(),
            storage_dir: None,
        }
    }

    /// Acquire a read guard on the attached wallet. The guard is held for as
    /// long as the returned value is alive; release it promptly so background
    /// sync can mutate the wallet.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached. Use
    /// [`Self::wallet_mut`] for write access.
    pub async fn wallet(&self) -> Result<RwLockReadGuard<'_, Wallet>, ProviderError> {
        match &self.wallet {
            Some(arc) => Ok(arc.read().await),
            None => Err(ProviderError::NoWallet),
        }
    }

    /// Acquire a write guard on the attached wallet. The guard is held for
    /// as long as the returned value is alive; release it promptly because
    /// other readers and the background sync are blocked while it lives.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn wallet_mut(&self) -> Result<RwLockWriteGuard<'_, Wallet>, ProviderError> {
        match &self.wallet {
            Some(arc) => Ok(arc.write().await),
            None => Err(ProviderError::NoWallet),
        }
    }

    /// Return the current wallet balance.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn balance(&self) -> Result<WalletBalance, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.read().await.balance())
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
        Ok(arc.read().await.spendable_shielded_coins())
    }

    /// Whether the attached wallet has completed dust sync.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn dust_synced(&self) -> Result<bool, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.read().await.dust_synced())
    }

    /// Re-sync the wallet against the indexer.
    ///
    /// Resumes from the wallet's current event cursors, applies any new
    /// zswap/dust/unshielded events, refreshes the latest block context and
    /// ledger parameters, and commits the result (re-persisting it when the
    /// wallet was synced with a storage directory). Fails if no wallet is
    /// attached.
    ///
    /// Also waits — once, idempotently — for the chain to advance past
    /// genesis before resyncing. Necessary because dev devnets ship a
    /// hardcoded genesis `tblock` from months before wall clock: building
    /// a transaction while only genesis exists computes an `intent.ttl`
    /// that's already in the past by the time the chain produces its
    /// first real-time block, causing rejection with chain custom error
    /// 182 at submission. On any chain with block height ≥ 1 (mainnet,
    /// preprod, or a local devnet older than ~6s) the wait returns
    /// immediately. See [`wait_for_chain_ready`](Self::wait_for_chain_ready).
    ///
    /// Locking: the slow replay I/O runs **without** the wallet lock, so
    /// concurrent reads ([`Self::balance`], [`Self::dust_synced`], ...) keep
    /// completing while a resync is in flight; the wallet lock is only taken
    /// briefly to snapshot the replay inputs (read) and to commit the result
    /// (write). Concurrent `resync_wallet` calls are serialized on an
    /// internal mutex. Do not hold a guard from [`Self::wallet`] /
    /// [`Self::wallet_mut`] across this call: the commit's write lock would
    /// deadlock against your own guard.
    pub async fn resync_wallet(&self) -> Result<(), ProviderError> {
        self.wait_for_chain_ready().await?;
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;

        // Serialize resyncs across plan → replay → commit: the replay below
        // runs without the wallet lock, so without this guard two concurrent
        // resyncs would replay from the same cursors and race their commits.
        let _resync_guard = self.resync_lock.lock().await;

        // Brief read lock: snapshot the cursors and replay state.
        let plan = arc.read().await.resync_plan();

        // Long I/O, lock-free: the three subscription replays plus the
        // latest-block fetch. Reads (and even transfer builds, which will
        // block on the resync mutex via their own resync, not on the wallet
        // lock) proceed against the pre-resync state meanwhile.
        let commit = plan.run(&self.indexer_url).await?;

        // Brief write lock: apply and persist. `commit_resync` merges with
        // commit-time pending state (see its docs), so wallet mutations that
        // interleaved with the replay are preserved.
        arc.write().await.commit_resync(commit)?;
        Ok(())
    }

    /// Wait until the chain has produced at least one post-genesis block.
    ///
    /// Returns immediately on any chain with block height ≥ 1. On a fresh
    /// dev devnet (only the genesis block exists), polls the indexer every
    /// 2s for up to 60s, returning [`ProviderError::ChainNotReady`] if the
    /// chain hasn't advanced by then. This is called automatically by
    /// [`Self::resync_wallet`] (and therefore by [`Self::build_context`]
    /// and every transfer / contract path that goes through resync); it is
    /// also exposed as a public hook for callers that want to gate their
    /// own logic on chain readiness.
    pub(crate) async fn wait_for_chain_ready(&self) -> Result<(), ProviderError> {
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
        const MAX_WAIT_SECS: u64 = 60;
        let max_attempts = MAX_WAIT_SECS / POLL_INTERVAL.as_secs();
        for _ in 0..max_attempts {
            if let Some(b) = self.indexer.get_block(None).await? {
                if b.height >= 1 {
                    return Ok(());
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        Err(ProviderError::ChainNotReady(MAX_WAIT_SECS))
    }

    /// Build a [`LedgerContext`] for the attached wallet.
    ///
    /// Drives a [`Self::resync_wallet`] first so the proof root and TTL anchor
    /// match the chain's current view, then constructs the context from the
    /// wallet's local state. Takes a write lock on the wallet because
    /// [`Wallet::build_context_inner`] evicts TTL-expired pending entries
    /// against the just-refreshed `block_context`.
    pub async fn build_context(&self) -> Result<Arc<LedgerContext<DefaultDB>>, ProviderError> {
        self.resync_wallet().await?;
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let mut wallet = arc.write().await;
        Ok(wallet.build_context_inner()?)
    }

    /// Build a shielded (Zswap) transfer transaction.
    ///
    /// Returns a pending builder. `.await?` builds + submits and returns the
    /// resulting [`PendingTx`]; `.build().await?` returns the raw
    /// [`TransferResult`] without submitting (e.g. for inspection or custom
    /// routing). Either path holds a wallet write lock across the build so the
    /// [`LedgerContext`] snapshot and wallet state stay consistent, and
    /// records the resulting dust + unshielded reservations in the wallet's
    /// pending list.
    pub fn transfer_shielded<'a>(
        &'a self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> ShieldedTransfer<'a> {
        ShieldedTransfer::new(self, token_type, amount, recipient)
    }

    /// Build an unshielded (UTXO) transfer transaction. See
    /// [`Self::transfer_shielded`] for lock + reservation semantics and the
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
    /// [`Self::transfer_shielded`] for lock + reservation semantics and the
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
    ) -> Result<TransferResult, ProviderError> {
        let mut guard = self.open_transfer_guard().await?;
        let transfer = TransferBuilder::new(
            &guard.wallet,
            guard.context.clone(),
            guard.proof_provider.clone(),
        );
        let result = transfer
            .shielded(token_type, amount, recipient, pay_fees)
            .await?;
        guard.reserve(&result);
        Ok(result)
    }

    /// Build an unshielded transfer. See [`Self::build_shielded_transfer`] for
    /// the `pay_fees` flag.
    pub(crate) async fn build_unshielded_transfer(
        &self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
        pay_fees: bool,
    ) -> Result<TransferResult, ProviderError> {
        let mut guard = self.open_transfer_guard().await?;
        let transfer = TransferBuilder::new(
            &guard.wallet,
            guard.context.clone(),
            guard.proof_provider.clone(),
        );
        let result = transfer
            .unshielded(token_type, amount, recipient, pay_fees)
            .await?;
        guard.reserve(&result);
        Ok(result)
    }

    pub(crate) async fn build_register_dust(
        &self,
        utxo_ctime: Option<u64>,
    ) -> Result<TransferResult, ProviderError> {
        let mut guard = self.open_transfer_guard().await?;
        let transfer = TransferBuilder::new(
            &guard.wallet,
            guard.context.clone(),
            guard.proof_provider.clone(),
        );
        let result = transfer.register_dust(utxo_ctime).await?;
        guard.reserve(&result);
        Ok(result)
    }

    /// Acquire a write lock + build a `LedgerContext` snapshot in one step,
    /// for the three transfer/registration build paths. Resyncs first so the
    /// proof root and TTL anchor match the chain's current view.
    async fn open_transfer_guard(&self) -> Result<TransferGuard<'_>, ProviderError> {
        self.resync_wallet().await?;
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let mut wallet = arc.write().await;
        let context = wallet.build_context_inner()?;
        let reserved_at = context.latest_block_context().tblock;
        Ok(TransferGuard {
            wallet,
            context,
            reserved_at,
            proof_provider: self.proof_provider(),
        })
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
    /// submitting them, returning a [`PreparedTx`] whose extrinsic hash is
    /// already known. Submit it with [`PreparedTx::submit`]. Lets a caller
    /// durably record state keyed by the extrinsic hash *before* the
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
    /// ([`DustlessBuilder::without_dust`]) and a single payer covers the fees
    /// with [`Self::balance_transaction`] (whose fee intent rides a distinct,
    /// non-colliding segment). A Dustless *shielded* transfer carries no intent
    /// at all (pure Zswap), so it always merges cleanly.
    ///
    /// Errors ([`ProviderError::Transaction`]) when given no transactions, when
    /// a byte string fails to deserialize, or when two transactions cannot be
    /// merged (colliding intent segments or mismatched network ids). Purely
    /// local; nothing is sent to the node.
    pub fn merge_transactions(&self, txs: &[Vec<u8>]) -> Result<Vec<u8>, ProviderError> {
        use midnight_helpers::FinalizedTransaction;
        use midnight_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};

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
    pub async fn balance_transaction(&self, tx_bytes: &[u8]) -> Result<Vec<u8>, ProviderError> {
        use midnight_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
        use midnight_helpers::{
            Array, DustActions, FinalizedTransaction, HashMapStorage, Intent, PedersenRandomness,
            ProofPreimageMarker, SeedableRng, Segment, Signature, Sp, SplittableRng, StdRng,
            TokenType, Transaction,
        };
        use midnight_wallet::transfer::DustSpendBatch;

        // A high, unlikely-to-collide segment for the fee-only intent we add,
        // so `merge` doesn't hit an intent-segment collision with the external
        // transaction. Dust balance is aggregated across segments, so the exact
        // id doesn't matter for fee accounting.
        const DUST_FEE_SEGMENT: u16 = 0xFEED;
        // Adding Dust grows the fee, so we re-draw and re-check; bounded like the
        // wallet's own `pay_fees` loop (`MAX_FEE_BALANCE_ITERATIONS`).
        const MAX_FEE_ITERATIONS: usize = 10;

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

        // Context supplies the resolver, ledger parameters, network id, and the
        // current block time; the proof provider proves the fee transaction.
        let context = self.build_context().await?;
        let now = context.latest_block_context().tblock;
        let ttl = now + context.with_ledger_state(|ls| ls.parameters.global_ttl);
        let (params, network_id) =
            context.with_ledger_state(|ls| (ls.parameters.clone(), ls.network_id.clone()));
        let resolver = context.resolver().await;
        let cost_model = params.cost_model.runtime_cost_model.clone();
        let proof_provider = self.proof_provider();
        let funding_seed = self.seed().await?;
        let mut rng = StdRng::from_entropy();

        // Adding Dust grows the transaction (and its fee), so draw Dust for the
        // current fee estimate, merge it, recompute, and repeat until the drawn
        // Dust covers the recomputed fee.
        let mut target = external
            .fees_with_margin(&params, 3)
            .map_err(|e| ProviderError::Transaction(format!("fee estimate: {e:?}")))?;
        for _ in 0..MAX_FEE_ITERATIONS {
            // Draw Dust from the context wallet, which has this process's
            // still-pending (unconfirmed) spends already applied via
            // `mark_spent`. Selecting from the live `self.dust_wallet()` (which
            // reflects only indexer-confirmed events) would let back-to-back or
            // concurrent sponsors re-select Dust another in-flight build already
            // reserved, producing a double-spend the node rejects.
            let (spends, updated_state) = {
                let wallets = context.wallets.lock().map_err(|_| {
                    ProviderError::Transaction("context wallets lock poisoned".into())
                })?;
                let funding_wallet = wallets.get(&funding_seed).ok_or_else(|| {
                    ProviderError::Transaction("funding wallet missing from context".into())
                })?;
                funding_wallet
                    .dust
                    .speculative_spend(target, now, &params.dust)
                    .map_err(|e| ProviderError::Transaction(format!("draw dust for fees: {e}")))?
            };
            // `speculative_spend` silently caps at the available balance rather
            // than erroring, so a short draw means the wallet is out of Dust.
            let drawn: u128 = spends.iter().map(|s| s.v_fee).sum();

            let mut intent: Intent<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB> =
                Intent::empty(&mut rng, ttl);
            intent.dust_actions = Some(Sp::new(DustActions {
                spends: spends.iter().cloned().collect(),
                registrations: Array::new(),
                ctime: now,
            }));
            let intents = HashMapStorage::new().insert(DUST_FEE_SEGMENT, intent);
            let dust_tx: Transaction<
                Signature,
                ProofPreimageMarker,
                PedersenRandomness,
                DefaultDB,
            > = Transaction::from_intents(network_id.as_str(), intents);
            // Probe the fee with a mock proof: it serializes to the same size as
            // a real proof, so the fee is accurate, but it skips the expensive ZK
            // work. Only the converged iteration pays for a real proof (this is
            // how the production `pay_fees` loop estimates fees). Valid here
            // because the fee-only intent has no unproven contract calls.
            let mock_proven = dust_tx
                .mock_prove()
                .map_err(|e| ProviderError::Transaction(format!("mock-prove fee: {e:?}")))?;
            let merged = external
                .merge(&mock_proven)
                .map_err(|e| ProviderError::Transaction(format!("merge fee payment: {e:?}")))?;

            let fee = merged
                .fees_with_margin(&params, 3)
                .map_err(|e| ProviderError::Transaction(format!("fee: {e:?}")))?;
            // Dust imbalance is aggregated under the Guaranteed segment
            // regardless of which segment the Dust spends live in (the wallet's
            // own `compute_missing_dust` reads the same key).
            let dust_delta = merged
                .balance(Some(fee))
                .map_err(|e| ProviderError::Transaction(format!("balance: {e:?}")))?
                .get(&(TokenType::Dust, Segment::Guaranteed.into()))
                .copied()
                .unwrap_or(0);
            if dust_delta >= 0 {
                // Converged: pay for the real proof once. `prove` yields a
                // `PedersenRandomness`-bound tx; `seal` converts it to the
                // `PureGeneratorPedersen` binding of a `FinalizedTransaction`
                // (matching `external`), the same finishing step the build path
                // does.
                let dust_proven = proof_provider
                    .prove(dust_tx, rng.split(), &resolver, &cost_model)
                    .await
                    .seal(rng.split());
                let merged = external
                    .merge(&dust_proven)
                    .map_err(|e| ProviderError::Transaction(format!("merge fee payment: {e:?}")))?;
                // Reserve the drawn Dust so a later in-process build (including a
                // subsequent `balance_transaction`) skips it until the spend
                // confirms on-chain. This mirrors the context read above.
                self.wallet_mut().await?.reserve_pending(
                    vec![DustSpendBatch {
                        seed: funding_seed.clone(),
                        spends,
                        updated_state,
                    }],
                    Vec::new(),
                    now,
                );
                let mut out = Vec::new();
                tagged_serialize(&merged, &mut out).map_err(|e| {
                    ProviderError::Transaction(format!("serialize balanced transaction: {e}"))
                })?;
                return Ok(out);
            }
            // Not converged. If we already drew every speck the wallet has, no
            // further iteration can close the gap, so fail clearly rather than
            // spinning to the iteration cap.
            let shortfall = (-dust_delta) as u128;
            if drawn < target {
                return Err(ProviderError::Transaction(format!(
                    "insufficient dust to cover fees: drew {drawn} specks, still short {shortfall}"
                )));
            }
            target += shortfall;
        }
        Err(ProviderError::Transaction(format!(
            "could not balance transaction fees within {MAX_FEE_ITERATIONS} iterations"
        )))
    }

    /// Wait for the indexer to surface a transaction's chain-side
    /// [`TransactionResult`] by extrinsic hash.
    ///
    /// `wait_best` / `wait_finalized` only confirm that the transaction landed
    /// in a block — they say nothing about whether the *fallible* phase of the
    /// transaction succeeded. A contract call can be in a finalized block and
    /// have done nothing useful (`PartialSuccess`). Use this after `wait_best`
    /// to distinguish, via the status inside [`TxResultWait::Found`]:
    ///
    /// - [`TransactionResultStatus::Success`] — guaranteed + all fallible
    ///   segments succeeded; state mutations applied.
    /// - [`TransactionResultStatus::PartialSuccess`] — guaranteed phase OK
    ///   and the tx is recorded on-chain, but at least one fallible segment
    ///   failed. Inspect [`TransactionResult::segments`] for which.
    /// - [`TransactionResultStatus::Failure`] — whole tx rolled back. Rare
    ///   because guaranteed-phase failures normally aren't included at all.
    ///
    /// Polls the indexer every `poll_interval` until the tx is found *with*
    /// a non-null `transaction_result`, or `timeout` elapses. A timeout
    /// surfaces as [`TxResultWait::TimedOut`], which is **not** evidence the
    /// tx didn't land: the indexer cannot positively report "never landed"
    /// (absence is always provisional — see the [`TxResultWait`] docs), so a
    /// lagging indexer and a tx that was never included look identical
    /// within the deadline. See
    /// [`docs/midnight-js-comparison.md`](https://github.com/RomarQ/midnight-rs/blob/main/docs/midnight-js-comparison.md#guaranteed-vs-fallible-transaction-phases)
    /// for the guaranteed/fallible phase model.
    ///
    /// [`TransactionResultStatus::Success`]: midnight_indexer_client::TransactionResultStatus::Success
    /// [`TransactionResultStatus::PartialSuccess`]: midnight_indexer_client::TransactionResultStatus::PartialSuccess
    /// [`TransactionResultStatus::Failure`]: midnight_indexer_client::TransactionResultStatus::Failure
    /// [`TransactionResult::segments`]: midnight_indexer_client::TransactionResult::segments
    pub async fn wait_transaction_result(
        &self,
        extrinsic_hash: &[u8; 32],
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<TxResultWait, ProviderError> {
        let hash_hex = hex::encode(extrinsic_hash);
        let start = std::time::Instant::now();
        loop {
            let txs = self
                .indexer
                .get_transactions(TransactionOffset::hash(hash_hex.clone()))
                .await?;
            if let Some(result) = txs.iter().find_map(|t| t.transaction_result().cloned()) {
                return Ok(TxResultWait::Found(result));
            }
            if start.elapsed() >= timeout {
                return Ok(TxResultWait::TimedOut);
            }
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// The attached wallet's seed.
    ///
    /// Cloned under a short read lock so callers don't have to scope a guard.
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn seed(&self) -> Result<WalletSeed, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.read().await.seed().clone())
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

    /// Get the chain's network ID (`system_chain`).
    pub async fn get_network_id(&self) -> Result<String, ProviderError> {
        let conn = self.get_or_connect().await?;

        let network: String = match conn.rpc.request("system_chain", RpcParams::new()).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "system_chain failed");
                return Err(ProviderError::Rpc(e.to_string()));
            }
        };

        debug!(network_id = %network, "system_chain response");

        Ok(network)
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

/// Internal: write-locked wallet plus the snapshot inputs the three transfer
/// build paths share. Holding the lock keeps the [`LedgerContext`] snapshot
/// consistent with the wallet state across the build, and `reserved_at` is
/// the `tblock` recorded against any pending reservations the build emits.
struct TransferGuard<'a> {
    wallet: RwLockWriteGuard<'a, Wallet>,
    context: Arc<LedgerContext<DefaultDB>>,
    reserved_at: Timestamp,
    proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
}

impl TransferGuard<'_> {
    fn reserve(&mut self, result: &TransferResult) {
        self.wallet.reserve_pending(
            result.dust_batches.clone(),
            result.spent_unshielded_inputs.clone(),
            self.reserved_at,
        );
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
            "http://localhost:8088/api/v3/graphql"
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

    #[tokio::test]
    async fn sync_handle_maps_join_error_to_provider_error() {
        let handle: JoinHandle<Result<MidnightProvider, ProviderError>> = tokio::spawn(async {
            std::future::pending::<()>().await;
            unreachable!()
        });
        handle.abort();
        let sync = SyncHandle::from_handle(handle);
        let Err(err) = sync.await else {
            panic!("aborted task should surface as a ProviderError");
        };
        assert!(
            matches!(err, ProviderError::SyncTaskJoin(_)),
            "expected SyncTaskJoin, got {err:?}"
        );
    }

    #[tokio::test]
    async fn sync_handle_passes_through_inner_error() {
        let handle: JoinHandle<Result<MidnightProvider, ProviderError>> =
            tokio::spawn(async { Err(ProviderError::NoWallet) });
        let sync = SyncHandle::from_handle(handle);
        let Err(err) = sync.await else {
            panic!("inner Err should propagate");
        };
        assert!(
            matches!(err, ProviderError::NoWallet),
            "expected NoWallet, got {err:?}"
        );
    }
}
