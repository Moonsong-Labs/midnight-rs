use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
use subxt::rpcs::client::{RpcClient, RpcParams};
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::{Health, PendingTx, Provider, ProviderError, StateQuery, StateQueryResult, submit};
use midnight_helpers::{
    DefaultDB, LedgerContext, LocalProofServer, ProofProvider, ShieldedTokenType,
    UnshieldedTokenType,
};
use midnight_indexer_client::{
    BlockOffset, ContractAction, ContractActionOffset, IndexerClient, TransactionOffset,
};
use midnight_rpc_api::MidnightApiClient;
use midnight_wallet::{
    SyncProgress, TransferBuilder, TransferResult, Wallet, WalletBalance, WalletSeed,
};

/// Default RPC connection timeout: 10 seconds.
pub const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(10);

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
    conn: Arc<RwLock<Option<NodeConnection>>>,
    /// Timeout for establishing the WebSocket RPC connection (default: 10s).
    rpc_timeout: Duration,
}

/// Handle to the background task spawned by
/// [`MidnightProvider::sync_wallet_with_progress`].
///
/// Awaiting it yields the synced [`MidnightProvider`] — a single `?` is
/// enough. A panic or cancellation of the spawned task surfaces as
/// [`ProviderError::SyncTaskJoin`]; the inner sync error path surfaces as
/// the matching `ProviderError` variant.
///
/// Dropping the handle does not cancel the task: the spawned sync is
/// detached and runs to completion in the background.
pub struct SyncHandle {
    inner: JoinHandle<Result<MidnightProvider, ProviderError>>,
}

impl SyncHandle {
    pub(crate) fn from_handle(inner: JoinHandle<Result<MidnightProvider, ProviderError>>) -> Self {
        Self { inner }
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
    ///     .sync_wallet(seed, "undeployed", None)
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
            conn: Arc::new(RwLock::new(None)),
            rpc_timeout: DEFAULT_RPC_TIMEOUT,
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
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?
    ///     .sync_wallet(seed, "undeployed", None)
    ///     .await?;
    /// ```
    ///
    /// Replays the initial sync (zswap + dust + unshielded events) and persists
    /// progress to `storage_dir` when supplied. For a streamed view, use
    /// [`Self::sync_wallet_with_progress`]. To incrementally refresh an
    /// already-attached wallet without a full resync, use
    /// [`Self::resync_wallet`].
    ///
    /// If a wallet is already attached (via [`Self::with_wallet`] or a previous
    /// `sync_wallet` call), it is replaced by the newly synced wallet.
    pub async fn sync_wallet(
        mut self,
        seed: WalletSeed,
        network: &str,
        storage_dir: Option<&Path>,
    ) -> Result<Self, ProviderError> {
        let address = midnight_wallet::address::derive_unshielded(&seed, network);
        let wallet = Wallet::sync_inner(
            &self.indexer_url,
            seed,
            &address,
            network,
            storage_dir,
            None,
        )
        .await
        .map_err(|e| ProviderError::Wallet(e.to_string()))?;
        self.wallet = Some(Arc::new(RwLock::new(wallet)));
        Ok(self)
    }

    /// Like [`Self::sync_wallet`] but returns a channel receiver that emits
    /// [`SyncProgress`] updates as each subscription replays events.
    ///
    /// ```rust,ignore
    /// let (mut rx, handle) = MidnightProvider::new(NODE_URL, INDEXER_URL)?
    ///     .sync_wallet_with_progress(seed, "preprod", None);
    /// while let Some(p) = rx.recv().await { /* render */ }
    /// let provider = handle.await?;
    /// ```
    ///
    /// The spawned sync task is detached: if the caller drops both `rx` and
    /// the returned [`SyncHandle`] without awaiting, the task continues
    /// running in the background until completion (it doesn't hold any
    /// user-visible state once the provider is dropped, so this is
    /// effectively a fire-and-forget drain rather than a leak).
    ///
    /// If a wallet is already attached, it is replaced when the sync finishes.
    pub fn sync_wallet_with_progress(
        mut self,
        seed: WalletSeed,
        network: &str,
        storage_dir: Option<&Path>,
    ) -> (mpsc::Receiver<SyncProgress>, SyncHandle) {
        let (tx, rx) = mpsc::channel(64);
        let indexer_url = self.indexer_url.clone();
        let network = network.to_string();
        let storage_dir = storage_dir.map(|p| p.to_path_buf());
        let handle = tokio::spawn(async move {
            let address = midnight_wallet::address::derive_unshielded(&seed, &network);
            let wallet = Wallet::sync_inner(
                &indexer_url,
                seed,
                &address,
                &network,
                storage_dir.as_deref(),
                Some(tx),
            )
            .await
            .map_err(|e| ProviderError::Wallet(e.to_string()))?;
            self.wallet = Some(Arc::new(RwLock::new(wallet)));
            Ok(self)
        });
        (rx, SyncHandle::from_handle(handle))
    }

    /// Set the WebSocket connection timeout for the node RPC (default: 10s).
    ///
    /// This only affects the node connection used by RPC calls (block headers,
    /// state queries, transaction submission). Indexer GraphQL calls use the
    /// indexer client's own timeout configuration.
    pub fn with_rpc_timeout(mut self, timeout: Duration) -> Self {
        self.rpc_timeout = timeout;
        self
    }

    /// The node WebSocket URL.
    pub fn node_url(&self) -> &str {
        &self.node_url
    }

    /// The indexer base URL (HTTP), used to derive subscription clients.
    pub fn indexer_url(&self) -> &str {
        &self.indexer_url
    }

    /// Acquire a read guard on the attached wallet. The guard is held for as
    /// long as the returned value is alive; release it promptly so background
    /// sync can mutate the wallet.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached. Use
    /// [`Self::wallet_handle`] if you need an `Option<Arc<RwLock<…>>>` to
    /// check presence without erroring or to share the wallet across tasks.
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

    /// The raw `Arc<RwLock<Wallet>>` handle, cheap to clone and safe to share
    /// across tasks.
    ///
    /// Returns `None` when no wallet was attached via [`Self::with_wallet`].
    /// Most callers want [`Self::wallet`] (read guard) or [`Self::wallet_mut`]
    /// (write guard) instead — this is the lower-level escape hatch for code
    /// that needs to own the handle (e.g. spawning a task that acquires its
    /// own locks as it runs).
    pub fn wallet_handle(&self) -> Option<Arc<RwLock<Wallet>>> {
        self.wallet.clone()
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
    /// zswap/dust/unshielded events, refreshes the latest block context, and
    /// commits the result. Fails if no wallet is attached.
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
    /// Holds a write lock on the wallet for the duration; callers that don't
    /// need to mutate should not hold a read lock concurrently or this will
    /// deadlock.
    pub async fn resync_wallet(&self) -> Result<(), ProviderError> {
        self.wait_for_chain_ready().await?;
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let mut wallet = arc.write().await;
        wallet
            .resync(&self.indexer_url)
            .await
            .map_err(|e| ProviderError::Wallet(e.to_string()))
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
    pub async fn wait_for_chain_ready(&self) -> Result<(), ProviderError> {
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
        wallet
            .build_context_inner()
            .map_err(|e| ProviderError::Wallet(e.to_string()))
    }

    /// Build a shielded (Zswap) transfer transaction.
    ///
    /// Holds a write lock on the wallet across the build so the
    /// [`LedgerContext`] snapshot and the wallet state stay consistent, and
    /// records the resulting dust + unshielded reservations in the wallet's
    /// pending list before returning. Caller submits via
    /// [`Self::submit`].
    pub async fn transfer_shielded(
        &self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> Result<TransferResult, ProviderError> {
        self.resync_wallet().await?;
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let mut wallet = arc.write().await;
        let context = wallet
            .build_context_inner()
            .map_err(|e| ProviderError::Wallet(e.to_string()))?;
        let reserved_at = context.latest_block_context().tblock;
        let transfer = TransferBuilder::new(&wallet, context, self.proof_provider());
        let result = transfer
            .shielded(token_type, amount, recipient)
            .await
            .map_err(|e| ProviderError::Wallet(e.to_string()))?;
        wallet.reserve_pending(
            result.dust_batches.clone(),
            result.spent_unshielded_inputs.clone(),
            reserved_at,
        );
        Ok(result)
    }

    /// Build an unshielded (UTXO) transfer transaction. See
    /// [`Self::transfer_shielded`] for lock + reservation semantics.
    pub async fn transfer_unshielded(
        &self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> Result<TransferResult, ProviderError> {
        self.resync_wallet().await?;
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let mut wallet = arc.write().await;
        let context = wallet
            .build_context_inner()
            .map_err(|e| ProviderError::Wallet(e.to_string()))?;
        let reserved_at = context.latest_block_context().tblock;
        let transfer = TransferBuilder::new(&wallet, context, self.proof_provider());
        let result = transfer
            .unshielded(token_type, amount, recipient)
            .await
            .map_err(|e| ProviderError::Wallet(e.to_string()))?;
        wallet.reserve_pending(
            result.dust_batches.clone(),
            result.spent_unshielded_inputs.clone(),
            reserved_at,
        );
        Ok(result)
    }

    /// Build a dust-address registration transaction. See
    /// [`Self::transfer_shielded`] for lock + reservation semantics.
    pub async fn register_dust(
        &self,
        utxo_ctime: Option<u64>,
    ) -> Result<TransferResult, ProviderError> {
        self.resync_wallet().await?;
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        let mut wallet = arc.write().await;
        let context = wallet
            .build_context_inner()
            .map_err(|e| ProviderError::Wallet(e.to_string()))?;
        let reserved_at = context.latest_block_context().tblock;
        let transfer = TransferBuilder::new(&wallet, context, self.proof_provider());
        let result = transfer
            .register_dust(utxo_ctime)
            .await
            .map_err(|e| ProviderError::Wallet(e.to_string()))?;
        wallet.reserve_pending(
            result.dust_batches.clone(),
            result.spent_unshielded_inputs.clone(),
            reserved_at,
        );
        Ok(result)
    }

    /// Submit proven transaction bytes to the node over the WebSocket RPC.
    ///
    /// Returns a [`PendingTx`] handle that lets the caller await inclusion
    /// (`wait_best`) and finalization (`wait_finalized`). The provider's
    /// `node_url` is used as the connection target — callers don't repeat it.
    pub async fn submit(&self, tx_bytes: &[u8]) -> Result<PendingTx, ProviderError> {
        submit::submit_bytes(&self.node_url, tx_bytes).await
    }

    /// The attached wallet's seed.
    ///
    /// Cloned under a short read lock so callers don't have to scope a guard.
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn seed(&self) -> Result<WalletSeed, ProviderError> {
        let arc = self.wallet.as_ref().ok_or(ProviderError::NoWallet)?;
        Ok(arc.read().await.seed().clone())
    }

    /// Access the underlying indexer client directly.
    pub fn indexer(&self) -> &IndexerClient {
        &self.indexer
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
                .connection_timeout(self.rpc_timeout)
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
    async fn get_block_number(&self) -> Result<i64, ProviderError> {
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

    async fn get_network_id(&self) -> Result<String, ProviderError> {
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

    async fn get_block(
        &self,
        offset: Option<BlockOffset>,
    ) -> Result<Option<midnight_indexer_client::Block>, ProviderError> {
        Ok(self.indexer.get_block(offset).await?)
    }

    async fn get_block_with_transactions(
        &self,
        offset: Option<BlockOffset>,
    ) -> Result<Option<midnight_indexer_client::Block>, ProviderError> {
        Ok(self.indexer.get_block_with_transactions(offset).await?)
    }

    async fn get_contract_state(
        &self,
        address: &str,
        offset: Option<ContractActionOffset>,
    ) -> Result<Option<String>, ProviderError> {
        Ok(self.indexer.get_contract_state(address, offset).await?)
    }

    async fn get_contract_action(
        &self,
        address: &str,
        offset: Option<ContractActionOffset>,
    ) -> Result<Option<ContractAction>, ProviderError> {
        Ok(self.indexer.get_contract_action(address, offset).await?)
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

    async fn get_transactions(
        &self,
        offset: TransactionOffset,
    ) -> Result<Vec<midnight_indexer_client::Transaction>, ProviderError> {
        Ok(self.indexer.get_transactions(offset).await?)
    }

    /// Returns the best-effort health status of both the node and indexer.
    ///
    /// This method never returns `Err`. All failures are reflected in the
    /// returned [`Health`] fields.
    async fn health(&self) -> Result<Health, ProviderError> {
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

    async fn query_contract_state(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
    ) -> Result<Vec<StateQueryResult>, ProviderError> {
        self.query_contract_state_at(address, queries, None).await
    }
}

impl MidnightProvider {
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
    pub async fn query_contract_state_at(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_provider() {
        let provider =
            MidnightProvider::new("ws://localhost:9944", "http://localhost:8088").unwrap();
        assert_eq!(
            provider.indexer().url(),
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
