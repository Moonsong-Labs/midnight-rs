use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
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
use midnight_rpc_api::MidnightApiClient;
use midnight_wallet::{
    Network, SyncProgress, TransferBuilder, TransferResult, Wallet, WalletBalance, WalletSeed,
};

/// Connection timeout for the node WebSocket RPC.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Cached node connection: a single jsonrpsee `WsClient` shared between
/// the subxt `RpcClient` (for standard Substrate RPCs) and the typed
/// `MidnightApiClient` (for custom midnight RPCs).
struct NodeConnection {
    /// jsonrpsee client — used directly for typed midnight RPC calls.
    ws: Arc<WsClient>,
    /// subxt wrapper around the same client — used for standard Substrate RPCs.
    rpc: RpcClient,
}

/// A [`Provider`] backed by an [`IndexerClient`] (GraphQL) and a node
/// WebSocket connection for direct RPC communication.
///
/// The node connection is established lazily on first use and cached for
/// subsequent calls. A single jsonrpsee `WsClient` is shared between
/// subxt (for Substrate RPCs like `chain_getHeader`) and the typed
/// `MidnightApiClient` (for `midnight_queryContractState`).
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
    /// Defaults to a fresh [`LocalProofServer`] if unset.
    pub fn with_proof_provider(
        mut self,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Self {
        self.proof_provider = Some(proof_provider);
        self
    }

    fn proof_provider(&self) -> Arc<dyn ProofProvider<DefaultDB>> {
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

    pub(crate) async fn build_shielded_transfer(
        &self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> Result<TransferResult, ProviderError> {
        let mut guard = self.open_transfer_guard().await?;
        let transfer = TransferBuilder::new(
            &guard.wallet,
            guard.context.clone(),
            guard.proof_provider.clone(),
        );
        let result = transfer.shielded(token_type, amount, recipient).await?;
        guard.reserve(&result);
        Ok(result)
    }

    pub(crate) async fn build_unshielded_transfer(
        &self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> Result<TransferResult, ProviderError> {
        let mut guard = self.open_transfer_guard().await?;
        let transfer = TransferBuilder::new(
            &guard.wallet,
            guard.context.clone(),
            guard.proof_provider.clone(),
        );
        let result = transfer.unshielded(token_type, amount, recipient).await?;
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
        submit::submit_bytes(&self.node_url, tx_bytes).await
    }

    /// Build and validate proven transaction bytes against the node without
    /// submitting them, returning a [`PreparedTx`] whose extrinsic hash is
    /// already known. Submit it with [`PreparedTx::submit`]. Lets a caller
    /// durably record state keyed by the extrinsic hash *before* the
    /// transaction reaches the mempool.
    pub async fn prepare(&self, tx_bytes: &[u8]) -> Result<submit::PreparedTx, ProviderError> {
        submit::prepare_bytes(&self.node_url, tx_bytes).await
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
    /// Creates a single jsonrpsee `WsClient` and wraps it in both an `Arc`
    /// (for direct typed RPC calls) and a subxt `RpcClient` (for standard
    /// Substrate RPCs). Both share the same underlying WebSocket connection.
    async fn get_or_connect(&self) -> Result<NodeConnection, ProviderError> {
        {
            let guard = self.conn.read().await;
            if let Some(ref conn) = *guard {
                return Ok(NodeConnection {
                    ws: Arc::clone(&conn.ws),
                    rpc: conn.rpc.clone(),
                });
            }
        }

        info!(url = %self.node_url, "Connecting to Midnight node");
        let ws = Arc::new(
            WsClientBuilder::default()
                .connection_timeout(RPC_TIMEOUT)
                .build(&self.node_url)
                .await
                .map_err(|e| ProviderError::Rpc(e.to_string()))?,
        );
        // Wrap the same jsonrpsee client for subxt's RpcClient interface
        let rpc = RpcClient::new(ws.clone());

        let mut guard = self.conn.write().await;
        if guard.is_none() {
            *guard = Some(NodeConnection {
                ws: Arc::clone(&ws),
                rpc: rpc.clone(),
            });
        }
        let conn = guard.as_ref().unwrap();
        Ok(NodeConnection {
            ws: Arc::clone(&conn.ws),
            rpc: conn.rpc.clone(),
        })
    }

    /// Clear the cached connection so the next call will reconnect.
    async fn clear_connection(&self) {
        let mut guard = self.conn.write().await;
        *guard = None;
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

impl MidnightProvider {
    /// Get the current block number from the node (`chain_getHeader.number`).
    pub async fn get_block_number(&self) -> Result<i64, ProviderError> {
        let conn = self.get_or_connect().await?;

        let header: serde_json::Value =
            match conn.rpc.request("chain_getHeader", RpcParams::new()).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "chain_getHeader failed, clearing cached connection");
                    self.clear_connection().await;
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

        Ok(block_number as i64)
    }

    /// Get the finalized head's block hash from the node
    /// (`chain_getFinalizedHead`). Returned 0x-prefixed, as the node emits
    /// it, so it can be passed straight back to hash-pinned RPCs like
    /// `midnight_contractState`.
    pub async fn get_finalized_block_hash(&self) -> Result<String, ProviderError> {
        let conn = self.get_or_connect().await?;

        let hash: String = match conn
            .rpc
            .request("chain_getFinalizedHead", RpcParams::new())
            .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "chain_getFinalizedHead failed, clearing cached connection");
                self.clear_connection().await;
                return Err(ProviderError::Rpc(e.to_string()));
            }
        };

        debug!(block_hash = %hash, "chain_getFinalizedHead response");

        Ok(hash)
    }

    /// Get the chain's network ID (`system_chain`).
    pub async fn get_network_id(&self) -> Result<String, ProviderError> {
        let conn = self.get_or_connect().await?;

        let network: String = match conn.rpc.request("system_chain", RpcParams::new()).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "system_chain failed, clearing cached connection");
                self.clear_connection().await;
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
                            self.clear_connection().await;
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
                            self.clear_connection().await;
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
                    })
                    .map(|n| n as i64);

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
        at_block_hash: Option<&str>,
    ) -> Result<Option<String>, ProviderError> {
        let conn = self.get_or_connect().await?;
        let block_hash = at_block_hash.map(|h| h.to_string());
        match conn.ws.get_state(address.to_string(), block_hash).await {
            Ok(hex_state) => {
                if hex_state.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(hex_state))
                }
            }
            Err(e) => {
                warn!(error = %e, "midnight_contractState failed, clearing cached connection");
                self.clear_connection().await;
                Err(ProviderError::Rpc(e.to_string()))
            }
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
        at_block_hash: Option<&str>,
    ) -> Result<Vec<StateQueryResult>, ProviderError> {
        let conn = self.get_or_connect().await?;
        let results = match conn
            .ws
            .query_contract_state(
                address.to_string(),
                queries,
                at_block_hash.map(|h| h.to_string()),
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "midnight_queryContractState failed, clearing cached connection");
                self.clear_connection().await;
                return Err(ProviderError::Rpc(e.to_string()));
            }
        };
        Ok(results)
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
