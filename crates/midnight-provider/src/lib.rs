mod error;
mod provider;
mod types;

pub use error::ProviderError;
pub use provider::MidnightProvider;
pub use types::Health;

pub use midnight_indexer_client::{
    self as indexer, Block, ContractAction, ContractBalance, ContractCall, ContractDeploy,
    ContractUpdate, GraphQLError, IndexerClient, IndexerError, RegularTransaction, Segment,
    SystemTransaction, Transaction, TransactionFees, TransactionResult, TransactionResultStatus,
    UnshieldedUtxo,
};

use std::sync::Arc;

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    async fn get_block_number(&self) -> Result<i64, ProviderError>;
    async fn get_network_id(&self) -> Result<String, ProviderError>;
    async fn get_block(&self) -> Result<Option<Block>, ProviderError>;
    async fn get_block_by_height(&self, height: i64) -> Result<Option<Block>, ProviderError>;
    async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Block>, ProviderError>;
    async fn get_block_with_transactions(
        &self,
        height: i64,
    ) -> Result<Option<Block>, ProviderError>;
    async fn get_contract_state(&self, address: &str) -> Result<Option<String>, ProviderError>;
    async fn get_contract_state_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<String>, ProviderError>;
    async fn get_contract_state_at_block_hash(
        &self,
        address: &str,
        hash: &str,
    ) -> Result<Option<String>, ProviderError>;
    async fn get_contract_state_at_tx_hash(
        &self,
        address: &str,
        tx_hash: &str,
    ) -> Result<Option<String>, ProviderError>;
    async fn get_contract_action(
        &self,
        address: &str,
    ) -> Result<Option<ContractAction>, ProviderError>;
    async fn get_contract_action_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<ContractAction>, ProviderError>;
    async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, ProviderError>;
    async fn get_transactions_by_hash(
        &self,
        hash: &str,
    ) -> Result<Vec<Transaction>, ProviderError>;
    async fn get_transactions_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Vec<Transaction>, ProviderError>;
    async fn health(&self) -> Result<Health, ProviderError>;
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
