use std::marker::PhantomData;

use crate::FromHex;
use midnight_provider::Provider;

use crate::error::ContractError;

/// A deployed contract instance bound to a provider.
///
/// Generic over `P: Provider` so it works with any provider implementation
/// (owned, borrowed, `Arc`, etc. via blanket impls).
pub struct Contract<P, L> {
    address: String,
    provider: P,
    _ledger: PhantomData<L>,
}

impl<P: Provider, L: FromHex> Contract<P, L> {
    /// Create a new contract instance.
    pub fn new(address: &str, provider: P) -> Self {
        Self {
            address: address.to_string(),
            provider,
            _ledger: PhantomData,
        }
    }

    /// The contract's on-chain address.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Reference to the provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// Fetch the current ledger state, deserialized into the generated type.
    pub async fn ledger(&self) -> Result<L, ContractError> {
        let hex = self
            .provider
            .get_contract_state(&self.address)
            .await?
            .ok_or_else(|| ContractError::NotFound(self.address.clone()))?;
        Ok(L::from_hex(&hex)?)
    }

    /// Fetch the ledger state at a specific block height.
    pub async fn ledger_at_height(&self, height: i64) -> Result<L, ContractError> {
        let hex = self
            .provider
            .get_contract_state_at_height(&self.address, height)
            .await?
            .ok_or_else(|| ContractError::NotFound(self.address.clone()))?;
        Ok(L::from_hex(&hex)?)
    }

    /// Fetch the ledger state at a specific block hash.
    pub async fn ledger_at_block_hash(&self, hash: &str) -> Result<L, ContractError> {
        let hex = self
            .provider
            .get_contract_state_at_block_hash(&self.address, hash)
            .await?
            .ok_or_else(|| ContractError::NotFound(self.address.clone()))?;
        Ok(L::from_hex(&hex)?)
    }

    /// Fetch the ledger state at a specific transaction hash.
    pub async fn ledger_at_tx_hash(&self, tx_hash: &str) -> Result<L, ContractError> {
        let hex = self
            .provider
            .get_contract_state_at_tx_hash(&self.address, tx_hash)
            .await?
            .ok_or_else(|| ContractError::NotFound(self.address.clone()))?;
        Ok(L::from_hex(&hex)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use midnight_provider::{
        Block, ContractAction, Health, ProviderError, StateQuery, StateQueryResult, Transaction,
    };

    struct MockProvider {
        state_hex: Option<String>,
    }

    #[async_trait]
    impl midnight_provider::Provider for MockProvider {
        async fn get_block_number(&self) -> Result<i64, ProviderError> {
            Ok(0)
        }

        async fn get_network_id(&self) -> Result<String, ProviderError> {
            Ok("mock".into())
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
            Ok(self.state_hex.clone())
        }

        async fn get_contract_state_at_height(
            &self,
            _address: &str,
            _height: i64,
        ) -> Result<Option<String>, ProviderError> {
            Ok(self.state_hex.clone())
        }

        async fn get_contract_state_at_block_hash(
            &self,
            _address: &str,
            _hash: &str,
        ) -> Result<Option<String>, ProviderError> {
            Ok(self.state_hex.clone())
        }

        async fn get_contract_state_at_tx_hash(
            &self,
            _address: &str,
            _tx_hash: &str,
        ) -> Result<Option<String>, ProviderError> {
            Ok(self.state_hex.clone())
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
                node_connected: false,
                indexer_connected: false,
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

    #[derive(Debug)]
    struct FakeLedger(String);

    impl crate::FromHex for FakeLedger {
        fn from_hex(hex_state: &str) -> Result<Self, midnight_bindgen::StateError> {
            Ok(FakeLedger(hex_state.to_string()))
        }
    }

    #[tokio::test]
    async fn ledger_returns_deserialized_state() {
        let provider = MockProvider {
            state_hex: Some("deadbeef".into()),
        };
        let contract: Contract<_, FakeLedger> = Contract::new("addr1", provider);
        let ledger = contract.ledger().await.unwrap();
        assert_eq!(ledger.0, "deadbeef");
    }

    #[tokio::test]
    async fn ledger_returns_not_found_when_no_state() {
        let provider = MockProvider { state_hex: None };
        let contract: Contract<_, FakeLedger> = Contract::new("addr1", provider);
        let err = contract.ledger().await.unwrap_err();
        assert!(matches!(err, ContractError::NotFound(_)));
    }

    #[tokio::test]
    async fn ledger_at_height_works() {
        let provider = MockProvider {
            state_hex: Some("cafe".into()),
        };
        let contract: Contract<_, FakeLedger> = Contract::new("addr1", provider);
        let ledger = contract.ledger_at_height(100).await.unwrap();
        assert_eq!(ledger.0, "cafe");
    }

    #[tokio::test]
    async fn ledger_at_block_hash_works() {
        let provider = MockProvider {
            state_hex: Some("babe".into()),
        };
        let contract: Contract<_, FakeLedger> = Contract::new("addr1", provider);
        let ledger = contract.ledger_at_block_hash("abc").await.unwrap();
        assert_eq!(ledger.0, "babe");
    }

    #[tokio::test]
    async fn ledger_at_tx_hash_works() {
        let provider = MockProvider {
            state_hex: Some("face".into()),
        };
        let contract: Contract<_, FakeLedger> = Contract::new("addr1", provider);
        let ledger = contract.ledger_at_tx_hash("txhash").await.unwrap();
        assert_eq!(ledger.0, "face");
    }

    #[tokio::test]
    async fn contract_by_ref_provider() {
        let provider = MockProvider {
            state_hex: Some("abab".into()),
        };
        let contract: Contract<&MockProvider, FakeLedger> = Contract::new("addr1", &provider);
        let ledger = contract.ledger().await.unwrap();
        assert_eq!(ledger.0, "abab");
    }

    #[test]
    fn address_and_provider_accessors() {
        let provider = MockProvider { state_hex: None };
        let contract: Contract<_, FakeLedger> = Contract::new("myaddr", provider);
        assert_eq!(contract.address(), "myaddr");
        let _ = contract.provider();
    }
}
