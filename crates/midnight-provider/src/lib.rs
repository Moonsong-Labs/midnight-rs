mod error;
mod provider;
mod submit;
mod types;

pub use error::ProviderError;
pub use provider::{DEFAULT_RPC_TIMEOUT, MidnightProvider, SyncHandle};
pub use submit::{PendingTx, TxInBlock};
pub use types::{Health, StateQuery, StateQueryResult};

// Re-export the wallet types that appear in MidnightProvider's public surface
// so callers don't need a separate dep on midnight-wallet for them.
pub use midnight_wallet::{
    HashOutput, NIGHT, ShieldedTokenType, SyncProgress, UnshieldedTokenType, Wallet, WalletBalance,
    WalletError, WalletSeed,
};

// Re-export indexer types so consumers of midnight-provider don't need
// a separate dependency on midnight-indexer-client for response types.
pub use midnight_indexer_client::{
    self as indexer, Block, BlockOffset, ContractAction, ContractActionOffset, ContractBalance,
    ContractCall, ContractDeploy, ContractUpdate, GraphQLError, IndexerClient, IndexerError,
    RegularTransaction, Segment, SystemTransaction, Transaction, TransactionFees,
    TransactionOffset, TransactionResult, TransactionResultStatus, UnshieldedUtxo,
};

use std::sync::Arc;

/// Read-only interface to the Midnight network.
///
/// Analogous to alloy's `Provider` trait. Currently covers state reads
/// and chain queries. Transaction submission will be added in a future
/// version alongside proving infrastructure.
///
/// Automatically implemented for `&T`, `Arc<T>`, and `Box<T>` where `T: Provider`.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Get the current block number from the node.
    async fn get_block_number(&self) -> Result<i64, ProviderError>;

    /// Get the chain's network ID.
    async fn get_network_id(&self) -> Result<String, ProviderError>;

    /// Get a block by optional offset. Returns the latest block when
    /// `offset` is `None`.
    async fn get_block(&self, offset: Option<BlockOffset>) -> Result<Option<Block>, ProviderError>;

    /// Get a block with its transactions by optional offset.
    async fn get_block_with_transactions(
        &self,
        offset: Option<BlockOffset>,
    ) -> Result<Option<Block>, ProviderError>;

    /// Fetch hex-encoded contract state. Returns the latest state when
    /// `offset` is `None`.
    async fn get_contract_state(
        &self,
        address: &str,
        offset: Option<ContractActionOffset>,
    ) -> Result<Option<String>, ProviderError>;

    /// Fetch a contract action (state + metadata). Returns the latest
    /// action when `offset` is `None`.
    async fn get_contract_action(
        &self,
        address: &str,
        offset: Option<ContractActionOffset>,
    ) -> Result<Option<ContractAction>, ProviderError>;

    /// Fetch the block height of the latest transaction touching a contract.
    async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, ProviderError>;

    /// Fetch transactions by offset (hash or identifier).
    async fn get_transactions(
        &self,
        offset: TransactionOffset,
    ) -> Result<Vec<Transaction>, ProviderError>;

    /// Check connectivity to both the node and indexer.
    async fn health(&self) -> Result<Health, ProviderError>;

    /// Query specific fields/keys in a contract's state tree without
    /// downloading the entire state blob.
    async fn query_contract_state(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
    ) -> Result<Vec<StateQueryResult>, ProviderError>;
}

// ---------------------------------------------------------------------------
// Bridge: MidnightProvider → StateQueryProvider (from midnight-bindgen)
// ---------------------------------------------------------------------------

#[cfg(feature = "bindgen")]
mod lazy_bridge {
    use super::*;
    use midnight_bindgen::{hex, lazy};
    use sp_storage::StorageKey;

    /// Re-export so consumers can use `StateQueryProvider` without depending
    /// on `midnight-bindgen` directly.
    pub use lazy::StateQueryProvider;

    impl lazy::StateQueryProvider for MidnightProvider {
        type Error = ProviderError;

        async fn query_contract_state(
            &self,
            address: &str,
            queries: Vec<lazy::StateQuery>,
            at_block_hash: Option<&str>,
        ) -> Result<Vec<lazy::StateQueryResult>, ProviderError> {
            // Convert bindgen hex strings → StorageKey raw bytes
            let provider_queries: Vec<StateQuery> = queries
                .into_iter()
                .map(|q| StateQuery {
                    path: q
                        .path
                        .into_iter()
                        .map(|h| StorageKey(hex::decode(&h).unwrap_or_else(|_| h.into_bytes())))
                        .collect(),
                })
                .collect();

            let results = self
                .query_contract_state_at(address, provider_queries, at_block_hash)
                .await?;

            // Convert StorageKey raw bytes → bindgen hex strings
            Ok(results
                .into_iter()
                .map(|r| lazy::StateQueryResult {
                    query: lazy::StateQuery {
                        path: r.query.path.into_iter().map(|k| hex::encode(k.0)).collect(),
                    },
                    value: r.value,
                    error: r.error,
                })
                .collect())
        }
    }
}

#[cfg(feature = "bindgen")]
pub use lazy_bridge::StateQueryProvider;

// ---------------------------------------------------------------------------
// Blanket impls for &T, Arc<T>, Box<T>
// ---------------------------------------------------------------------------

macro_rules! delegate_provider {
    ($($wrapper:ty),+ $(,)?) => { $(
        #[async_trait::async_trait]
        impl<T: Provider + ?Sized> Provider for $wrapper {
            async fn get_block_number(&self) -> Result<i64, ProviderError> {
                (**self).get_block_number().await
            }
            async fn get_network_id(&self) -> Result<String, ProviderError> {
                (**self).get_network_id().await
            }
            async fn get_block(
                &self,
                offset: Option<BlockOffset>,
            ) -> Result<Option<Block>, ProviderError> {
                (**self).get_block(offset).await
            }
            async fn get_block_with_transactions(
                &self,
                offset: Option<BlockOffset>,
            ) -> Result<Option<Block>, ProviderError> {
                (**self).get_block_with_transactions(offset).await
            }
            async fn get_contract_state(
                &self,
                address: &str,
                offset: Option<ContractActionOffset>,
            ) -> Result<Option<String>, ProviderError> {
                (**self).get_contract_state(address, offset).await
            }
            async fn get_contract_action(
                &self,
                address: &str,
                offset: Option<ContractActionOffset>,
            ) -> Result<Option<ContractAction>, ProviderError> {
                (**self).get_contract_action(address, offset).await
            }
            async fn get_latest_contract_block_height(
                &self,
                address: &str,
            ) -> Result<Option<i64>, ProviderError> {
                (**self).get_latest_contract_block_height(address).await
            }
            async fn get_transactions(
                &self,
                offset: TransactionOffset,
            ) -> Result<Vec<Transaction>, ProviderError> {
                (**self).get_transactions(offset).await
            }
            async fn health(&self) -> Result<Health, ProviderError> {
                (**self).health().await
            }
            async fn query_contract_state(
                &self,
                address: &str,
                queries: Vec<StateQuery>,
            ) -> Result<Vec<StateQueryResult>, ProviderError> {
                (**self).query_contract_state(address, queries).await
            }
        }
    )+ };
}

delegate_provider!(&T, Arc<T>, Box<T>);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct DummyProvider;

    #[async_trait::async_trait]
    impl Provider for DummyProvider {
        async fn get_block_number(&self) -> Result<i64, ProviderError> {
            Ok(0)
        }
        async fn get_network_id(&self) -> Result<String, ProviderError> {
            Ok("test".into())
        }
        async fn get_block(
            &self,
            _offset: Option<BlockOffset>,
        ) -> Result<Option<Block>, ProviderError> {
            Ok(None)
        }
        async fn get_block_with_transactions(
            &self,
            _offset: Option<BlockOffset>,
        ) -> Result<Option<Block>, ProviderError> {
            Ok(None)
        }
        async fn get_contract_state(
            &self,
            _address: &str,
            _offset: Option<ContractActionOffset>,
        ) -> Result<Option<String>, ProviderError> {
            Ok(None)
        }
        async fn get_contract_action(
            &self,
            _address: &str,
            _offset: Option<ContractActionOffset>,
        ) -> Result<Option<ContractAction>, ProviderError> {
            Ok(None)
        }
        async fn get_latest_contract_block_height(
            &self,
            _address: &str,
        ) -> Result<Option<i64>, ProviderError> {
            Ok(None)
        }
        async fn get_transactions(
            &self,
            _offset: TransactionOffset,
        ) -> Result<Vec<Transaction>, ProviderError> {
            Ok(vec![])
        }
        async fn health(&self) -> Result<Health, ProviderError> {
            Ok(Health {
                node_connected: true,
                indexer_connected: true,
                block_height: None,
                peers: None,
                is_syncing: None,
            })
        }
        async fn query_contract_state(
            &self,
            _address: &str,
            _queries: Vec<StateQuery>,
        ) -> Result<Vec<StateQueryResult>, ProviderError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn blanket_impl_ref() {
        let p = DummyProvider;
        let r: &dyn Provider = &p;
        assert_eq!(r.get_block_number().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn blanket_impl_arc() {
        let p = Arc::new(DummyProvider);
        assert_eq!(p.get_block_number().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn blanket_impl_box() {
        let p: Box<dyn Provider> = Box::new(DummyProvider);
        assert_eq!(p.get_block_number().await.unwrap(), 0);
    }
}
