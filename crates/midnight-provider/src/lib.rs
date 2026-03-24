mod error;
mod provider;
mod types;

pub use error::ProviderError;
pub use provider::MidnightProvider;
pub use types::{Health, StateQuery, StateQueryResult};

// Re-export indexer types so consumers of midnight-provider don't need
// a separate dependency on midnight-indexer-client for response types.
pub use midnight_indexer_client::{
    self as indexer, Block, ContractAction, ContractBalance, ContractCall, ContractDeploy,
    ContractUpdate, GraphQLError, IndexerClient, IndexerError, RegularTransaction, Segment,
    SystemTransaction, Transaction, TransactionFees, TransactionResult, TransactionResultStatus,
    UnshieldedUtxo,
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

    /// Get the latest block from the indexer.
    async fn get_block(&self) -> Result<Option<Block>, ProviderError>;

    /// Get a block by height.
    async fn get_block_by_height(&self, height: i64) -> Result<Option<Block>, ProviderError>;

    /// Get a block by hash.
    async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Block>, ProviderError>;

    /// Get a block with its transactions.
    async fn get_block_with_transactions(
        &self,
        height: i64,
    ) -> Result<Option<Block>, ProviderError>;

    /// Fetch the latest hex-encoded contract state.
    async fn get_contract_state(&self, address: &str) -> Result<Option<String>, ProviderError>;

    /// Fetch hex-encoded contract state at a specific block height.
    async fn get_contract_state_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<String>, ProviderError>;

    /// Fetch hex-encoded contract state at a specific block hash.
    async fn get_contract_state_at_block_hash(
        &self,
        address: &str,
        hash: &str,
    ) -> Result<Option<String>, ProviderError>;

    /// Fetch hex-encoded contract state at a specific transaction hash.
    async fn get_contract_state_at_tx_hash(
        &self,
        address: &str,
        tx_hash: &str,
    ) -> Result<Option<String>, ProviderError>;

    /// Fetch the latest contract action.
    async fn get_contract_action(
        &self,
        address: &str,
    ) -> Result<Option<ContractAction>, ProviderError>;

    /// Fetch contract action at a specific block height.
    async fn get_contract_action_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<ContractAction>, ProviderError>;

    /// Fetch the block height of the latest transaction touching a contract.
    async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, ProviderError>;

    /// Fetch transactions by hash.
    async fn get_transactions_by_hash(
        &self,
        hash: &str,
    ) -> Result<Vec<Transaction>, ProviderError>;

    /// Fetch transactions by identifier.
    async fn get_transactions_by_identifier(
        &self,
        identifier: &str,
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
// Blanket impl for &T
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl<T: Provider + ?Sized> Provider for &T {
    async fn get_block_number(&self) -> Result<i64, ProviderError> {
        (**self).get_block_number().await
    }

    async fn get_network_id(&self) -> Result<String, ProviderError> {
        (**self).get_network_id().await
    }

    async fn get_block(&self) -> Result<Option<Block>, ProviderError> {
        (**self).get_block().await
    }

    async fn get_block_by_height(&self, height: i64) -> Result<Option<Block>, ProviderError> {
        (**self).get_block_by_height(height).await
    }

    async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Block>, ProviderError> {
        (**self).get_block_by_hash(hash).await
    }

    async fn get_block_with_transactions(
        &self,
        height: i64,
    ) -> Result<Option<Block>, ProviderError> {
        (**self).get_block_with_transactions(height).await
    }

    async fn get_contract_state(&self, address: &str) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state(address).await
    }

    async fn get_contract_state_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state_at_height(address, height).await
    }

    async fn get_contract_state_at_block_hash(
        &self,
        address: &str,
        hash: &str,
    ) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state_at_block_hash(address, hash).await
    }

    async fn get_contract_state_at_tx_hash(
        &self,
        address: &str,
        tx_hash: &str,
    ) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state_at_tx_hash(address, tx_hash).await
    }

    async fn get_contract_action(
        &self,
        address: &str,
    ) -> Result<Option<ContractAction>, ProviderError> {
        (**self).get_contract_action(address).await
    }

    async fn get_contract_action_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<ContractAction>, ProviderError> {
        (**self).get_contract_action_at_height(address, height).await
    }

    async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, ProviderError> {
        (**self).get_latest_contract_block_height(address).await
    }

    async fn get_transactions_by_hash(
        &self,
        hash: &str,
    ) -> Result<Vec<Transaction>, ProviderError> {
        (**self).get_transactions_by_hash(hash).await
    }

    async fn get_transactions_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Vec<Transaction>, ProviderError> {
        (**self).get_transactions_by_identifier(identifier).await
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

// ---------------------------------------------------------------------------
// Blanket impl for Arc<T>
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl<T: Provider + ?Sized> Provider for Arc<T> {
    async fn get_block_number(&self) -> Result<i64, ProviderError> {
        (**self).get_block_number().await
    }

    async fn get_network_id(&self) -> Result<String, ProviderError> {
        (**self).get_network_id().await
    }

    async fn get_block(&self) -> Result<Option<Block>, ProviderError> {
        (**self).get_block().await
    }

    async fn get_block_by_height(&self, height: i64) -> Result<Option<Block>, ProviderError> {
        (**self).get_block_by_height(height).await
    }

    async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Block>, ProviderError> {
        (**self).get_block_by_hash(hash).await
    }

    async fn get_block_with_transactions(
        &self,
        height: i64,
    ) -> Result<Option<Block>, ProviderError> {
        (**self).get_block_with_transactions(height).await
    }

    async fn get_contract_state(&self, address: &str) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state(address).await
    }

    async fn get_contract_state_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state_at_height(address, height).await
    }

    async fn get_contract_state_at_block_hash(
        &self,
        address: &str,
        hash: &str,
    ) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state_at_block_hash(address, hash).await
    }

    async fn get_contract_state_at_tx_hash(
        &self,
        address: &str,
        tx_hash: &str,
    ) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state_at_tx_hash(address, tx_hash).await
    }

    async fn get_contract_action(
        &self,
        address: &str,
    ) -> Result<Option<ContractAction>, ProviderError> {
        (**self).get_contract_action(address).await
    }

    async fn get_contract_action_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<ContractAction>, ProviderError> {
        (**self).get_contract_action_at_height(address, height).await
    }

    async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, ProviderError> {
        (**self).get_latest_contract_block_height(address).await
    }

    async fn get_transactions_by_hash(
        &self,
        hash: &str,
    ) -> Result<Vec<Transaction>, ProviderError> {
        (**self).get_transactions_by_hash(hash).await
    }

    async fn get_transactions_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Vec<Transaction>, ProviderError> {
        (**self).get_transactions_by_identifier(identifier).await
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

// ---------------------------------------------------------------------------
// Blanket impl for Box<T>
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl<T: Provider + ?Sized> Provider for Box<T> {
    async fn get_block_number(&self) -> Result<i64, ProviderError> {
        (**self).get_block_number().await
    }

    async fn get_network_id(&self) -> Result<String, ProviderError> {
        (**self).get_network_id().await
    }

    async fn get_block(&self) -> Result<Option<Block>, ProviderError> {
        (**self).get_block().await
    }

    async fn get_block_by_height(&self, height: i64) -> Result<Option<Block>, ProviderError> {
        (**self).get_block_by_height(height).await
    }

    async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Block>, ProviderError> {
        (**self).get_block_by_hash(hash).await
    }

    async fn get_block_with_transactions(
        &self,
        height: i64,
    ) -> Result<Option<Block>, ProviderError> {
        (**self).get_block_with_transactions(height).await
    }

    async fn get_contract_state(&self, address: &str) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state(address).await
    }

    async fn get_contract_state_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state_at_height(address, height).await
    }

    async fn get_contract_state_at_block_hash(
        &self,
        address: &str,
        hash: &str,
    ) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state_at_block_hash(address, hash).await
    }

    async fn get_contract_state_at_tx_hash(
        &self,
        address: &str,
        tx_hash: &str,
    ) -> Result<Option<String>, ProviderError> {
        (**self).get_contract_state_at_tx_hash(address, tx_hash).await
    }

    async fn get_contract_action(
        &self,
        address: &str,
    ) -> Result<Option<ContractAction>, ProviderError> {
        (**self).get_contract_action(address).await
    }

    async fn get_contract_action_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<ContractAction>, ProviderError> {
        (**self).get_contract_action_at_height(address, height).await
    }

    async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, ProviderError> {
        (**self).get_latest_contract_block_height(address).await
    }

    async fn get_transactions_by_hash(
        &self,
        hash: &str,
    ) -> Result<Vec<Transaction>, ProviderError> {
        (**self).get_transactions_by_hash(hash).await
    }

    async fn get_transactions_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Vec<Transaction>, ProviderError> {
        (**self).get_transactions_by_identifier(identifier).await
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

        async fn get_block(&self) -> Result<Option<Block>, ProviderError> {
            Ok(None)
        }

        async fn get_block_by_height(&self, _height: i64) -> Result<Option<Block>, ProviderError> {
            Ok(None)
        }

        async fn get_block_by_hash(&self, _hash: &str) -> Result<Option<Block>, ProviderError> {
            Ok(None)
        }

        async fn get_block_with_transactions(
            &self,
            _height: i64,
        ) -> Result<Option<Block>, ProviderError> {
            Ok(None)
        }

        async fn get_contract_state(
            &self,
            _address: &str,
        ) -> Result<Option<String>, ProviderError> {
            Ok(None)
        }

        async fn get_contract_state_at_height(
            &self,
            _address: &str,
            _height: i64,
        ) -> Result<Option<String>, ProviderError> {
            Ok(None)
        }

        async fn get_contract_state_at_block_hash(
            &self,
            _address: &str,
            _hash: &str,
        ) -> Result<Option<String>, ProviderError> {
            Ok(None)
        }

        async fn get_contract_state_at_tx_hash(
            &self,
            _address: &str,
            _tx_hash: &str,
        ) -> Result<Option<String>, ProviderError> {
            Ok(None)
        }

        async fn get_contract_action(
            &self,
            _address: &str,
        ) -> Result<Option<ContractAction>, ProviderError> {
            Ok(None)
        }

        async fn get_contract_action_at_height(
            &self,
            _address: &str,
            _height: i64,
        ) -> Result<Option<ContractAction>, ProviderError> {
            Ok(None)
        }

        async fn get_latest_contract_block_height(
            &self,
            _address: &str,
        ) -> Result<Option<i64>, ProviderError> {
            Ok(None)
        }

        async fn get_transactions_by_hash(
            &self,
            _hash: &str,
        ) -> Result<Vec<Transaction>, ProviderError> {
            Ok(vec![])
        }

        async fn get_transactions_by_identifier(
            &self,
            _identifier: &str,
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
