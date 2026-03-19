use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use subxt::rpcs::client::{RpcClient, RpcParams};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::{Health, Provider, ProviderError};
use midnight_indexer_client::{Block, ContractAction, IndexerClient, Transaction};

const RPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A [`Provider`] backed by an [`IndexerClient`] (GraphQL) and a [`subxt`] RPC
/// client for direct node communication.
///
/// The RPC connection is established lazily on first use and cached for
/// subsequent calls. If an RPC call fails, the cached connection is cleared so
/// the next call will reconnect.
pub struct MidnightProvider {
    indexer: IndexerClient,
    node_url: String,
    rpc: Arc<RwLock<Option<RpcClient>>>,
}

impl MidnightProvider {
    /// Create a provider from node WebSocket URL and indexer HTTP URL.
    ///
    /// The node RPC connection is **not** established here; it is deferred to
    /// the first call that requires it.
    pub fn new(node_url: &str, indexer_url: &str) -> Result<Self, ProviderError> {
        let indexer = IndexerClient::new(indexer_url)?;
        Ok(Self {
            indexer,
            node_url: node_url.to_string(),
            rpc: Arc::new(RwLock::new(None)),
        })
    }

    /// Access the underlying indexer client directly.
    pub fn indexer(&self) -> &IndexerClient {
        &self.indexer
    }

    /// Get or create the RPC client, reconnecting if the cache is empty.
    ///
    /// Uses a connect-then-swap pattern: the network call happens outside the
    /// write lock to avoid holding it across the `await` point.
    async fn get_or_connect(&self) -> Result<RpcClient, ProviderError> {
        // Fast path: return the cached client if available.
        {
            let guard = self.rpc.read().await;
            if let Some(ref client) = *guard {
                return Ok(client.clone());
            }
        }

        // Connect outside the lock so we don't hold it during the network call.
        info!(url = %self.node_url, "Connecting to Midnight node");
        let client = tokio::time::timeout(
            RPC_CONNECT_TIMEOUT,
            RpcClient::from_insecure_url(&self.node_url),
        )
        .await
        .map_err(|_| ProviderError::RpcTimeout)?
        .map_err(|e| ProviderError::Rpc(e.to_string()))?;

        // Acquire the write lock and store the client. Another task may have
        // connected in the meantime; that is fine — we just keep whichever
        // landed first.
        let mut guard = self.rpc.write().await;
        if guard.is_none() {
            *guard = Some(client.clone());
        }
        Ok(guard.as_ref().unwrap().clone())
    }

    /// Clear the cached RPC client so the next call will reconnect.
    async fn clear_connection(&self) {
        let mut guard = self.rpc.write().await;
        *guard = None;
    }
}

#[async_trait]
impl Provider for MidnightProvider {
    async fn get_block_number(&self) -> Result<i64, ProviderError> {
        let rpc = self.get_or_connect().await?;

        let header: serde_json::Value = rpc
            .request("chain_getHeader", RpcParams::new())
            .await
            .map_err(|e| {
                warn!(error = %e, "chain_getHeader failed, clearing cached connection");
                ProviderError::Rpc(e.to_string())
            })?;

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
        let rpc = self.get_or_connect().await?;

        let network: String = rpc
            .request("system_chain", RpcParams::new())
            .await
            .map_err(|e| {
                warn!(error = %e, "system_chain failed, clearing cached connection");
                ProviderError::Rpc(e.to_string())
            })?;

        debug!(network_id = %network, "system_chain response");

        Ok(network)
    }

    async fn get_block(&self) -> Result<Option<Block>, ProviderError> {
        Ok(self.indexer.get_latest_block().await?)
    }

    async fn get_block_by_height(&self, height: i64) -> Result<Option<Block>, ProviderError> {
        Ok(self.indexer.get_block_by_height(height).await?)
    }

    async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Block>, ProviderError> {
        Ok(self.indexer.get_block_by_hash(hash).await?)
    }

    async fn get_block_with_transactions(
        &self,
        height: i64,
    ) -> Result<Option<Block>, ProviderError> {
        Ok(self.indexer.get_block_with_transactions(height).await?)
    }

    async fn get_contract_state(&self, address: &str) -> Result<Option<String>, ProviderError> {
        Ok(self.indexer.get_contract_state(address).await?)
    }

    async fn get_contract_state_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<String>, ProviderError> {
        Ok(self
            .indexer
            .get_contract_state_at_height(address, height)
            .await?)
    }

    async fn get_contract_state_at_block_hash(
        &self,
        address: &str,
        hash: &str,
    ) -> Result<Option<String>, ProviderError> {
        Ok(self
            .indexer
            .get_contract_state_at_block_hash(address, hash)
            .await?)
    }

    async fn get_contract_state_at_tx_hash(
        &self,
        address: &str,
        tx_hash: &str,
    ) -> Result<Option<String>, ProviderError> {
        Ok(self
            .indexer
            .get_contract_state_at_tx_hash(address, tx_hash)
            .await?)
    }

    async fn get_contract_action(
        &self,
        address: &str,
    ) -> Result<Option<ContractAction>, ProviderError> {
        Ok(self.indexer.get_contract_action(address).await?)
    }

    async fn get_contract_action_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<ContractAction>, ProviderError> {
        Ok(self
            .indexer
            .get_contract_action_at_height(address, height)
            .await?)
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

    async fn get_transactions_by_hash(
        &self,
        hash: &str,
    ) -> Result<Vec<Transaction>, ProviderError> {
        Ok(self.indexer.get_transactions_by_hash(hash).await?)
    }

    async fn get_transactions_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Vec<Transaction>, ProviderError> {
        Ok(self
            .indexer
            .get_transactions_by_identifier(identifier)
            .await?)
    }

    /// Returns the best-effort health status of both the node and indexer.
    ///
    /// This method never returns `Err`. All failures are reflected in the
    /// returned [`Health`] fields.
    async fn health(&self) -> Result<Health, ProviderError> {
        // --- Node health via RPC ---
        let (node_connected, block_height, peers, is_syncing) =
            match self.get_or_connect().await {
                Err(err) => {
                    warn!(url = %self.node_url, error = %err, "Failed to connect to Midnight node");
                    (false, None, None, None)
                }
                Ok(rpc) => {
                    // system_health: peer count and sync status
                    let sys_health: Option<serde_json::Value> =
                        match rpc.request("system_health", RpcParams::new()).await {
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

                    // chain_getHeader: current block height
                    let header: Option<serde_json::Value> =
                        match rpc.request("chain_getHeader", RpcParams::new()).await {
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
        let provider =
            MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1").unwrap();
        let health = provider.health().await.unwrap();
        assert!(!health.node_connected);
        assert!(!health.indexer_connected);
    }
}
