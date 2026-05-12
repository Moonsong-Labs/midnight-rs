use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
use subxt::rpcs::client::{RpcClient, RpcParams};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::{Health, Provider, ProviderError, StateQuery, StateQueryResult};
use midnight_indexer_client::{
    BlockOffset, ContractAction, ContractActionOffset, IndexerClient, TransactionOffset,
};
use midnight_rpc_api::MidnightApiClient;
use midnight_wallet::Wallet;

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
    node_url: String,
    wallet: Option<Wallet>,
    conn: Arc<RwLock<Option<NodeConnection>>>,
    /// Timeout for establishing the WebSocket RPC connection (default: 10s).
    rpc_timeout: Duration,
}

impl MidnightProvider {
    /// Create a provider from node WebSocket URL and indexer HTTP URL.
    ///
    /// The node connection is **not** established here; it is deferred to
    /// the first call that requires it.
    ///
    /// For a fluent builder with wallet support, use:
    /// ```rust,ignore
    /// let wallet = Wallet::from_seed_hex(WALLET_SEED, "undeployed")?;
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?
    ///     .with_wallet(wallet);
    /// ```
    pub fn new(node_url: &str, indexer_url: &str) -> Result<Self, ProviderError> {
        let indexer = IndexerClient::new(indexer_url)?;
        Ok(Self {
            indexer,
            node_url: node_url.to_string(),
            wallet: None,
            conn: Arc::new(RwLock::new(None)),
            rpc_timeout: DEFAULT_RPC_TIMEOUT,
        })
    }

    /// Attach a [`Wallet`] for transaction signing and fee payment.
    pub fn with_wallet(mut self, wallet: Wallet) -> Self {
        self.wallet = Some(wallet);
        self
    }

    /// Set the RPC WebSocket connection timeout (default: 10s).
    pub fn with_rpc_timeout(mut self, timeout: Duration) -> Self {
        self.rpc_timeout = timeout;
        self
    }

    /// The node WebSocket URL.
    pub fn node_url(&self) -> &str {
        &self.node_url
    }

    /// The configured wallet, if any.
    pub fn wallet(&self) -> Option<&Wallet> {
        self.wallet.as_ref()
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
}
