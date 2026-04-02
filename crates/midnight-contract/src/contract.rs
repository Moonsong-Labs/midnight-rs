use std::path::PathBuf;
use std::time::Duration;

use midnight_bindgen::{ContractState, InMemoryDB};
use midnight_provider::{MidnightProvider, Provider};

use crate::Prover;
use crate::call::{deploy_funded, submit, wait_for_deployment, with_zk_keys};
use crate::error::ContractError;

// ---------------------------------------------------------------------------
// ContractBuilder — fluent API for deploying contracts
// ---------------------------------------------------------------------------

/// Builder for deploying a contract.
///
/// Typically accessed via the generated `Contract::deploy()` method.
///
/// # Example
///
/// ```rust,ignore
/// let mut contract = counter::Contract::deploy()
///     .provider(&provider)
///     .initial_state(counter::LedgerInitialState { round: 0 })
///     .zk_keys("compiled/keys")
///     .deploy()
///     .await?;
///
/// contract.circuits().increment().await?;
/// ```
pub struct ContractBuilder<P = ()> {
    provider: P,
    initial_state: Option<ContractState<InMemoryDB>>,
    prover: Option<Prover>,
}

impl ContractBuilder<()> {
    pub fn new() -> Self {
        ContractBuilder {
            provider: (),
            initial_state: None,
            prover: None,
        }
    }
}

impl Default for ContractBuilder<()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P> ContractBuilder<P> {
    /// Set the provider (owns or borrows a `MidnightProvider`).
    pub fn provider<Q>(self, provider: Q) -> ContractBuilder<Q> {
        ContractBuilder {
            provider,
            initial_state: self.initial_state,
            prover: self.prover,
        }
    }

    /// Set the initial contract state.
    ///
    /// Accepts anything that converts to `ContractState<InMemoryDB>` — including
    /// the generated `LedgerInitialState` (via its `Into` impl).
    pub fn initial_state(mut self, state: impl Into<ContractState<InMemoryDB>>) -> Self {
        self.initial_state = Some(state.into());
        self
    }

    /// Set the path to the compiled contract directory containing `keys/` and `zkir/`.
    ///
    /// Convenience method that creates a `Prover::Local` with the given path.
    /// For remote proving, use `.prover()` instead.
    pub fn zk_keys(mut self, path: impl Into<PathBuf>) -> Self {
        self.prover = Some(Prover::local(path));
        self
    }

    /// Set the prover configuration (local or remote).
    pub fn prover(mut self, prover: Prover) -> Self {
        self.prover = Some(prover);
        self
    }
}

impl ContractBuilder<MidnightProvider> {
    /// Deploy the contract to a running Midnight node.
    ///
    /// Uses the provider's node URL and wallet seed. Syncs wallet state,
    /// builds a funded transaction, submits it, and waits for indexer confirmation.
    pub async fn deploy(self) -> Result<Contract<MidnightProvider>, ContractError> {
        self.deploy_inner().await
    }
}

impl<'a> ContractBuilder<&'a MidnightProvider> {
    /// Deploy the contract to a running Midnight node.
    pub async fn deploy(self) -> Result<Contract<&'a MidnightProvider>, ContractError> {
        self.deploy_inner().await
    }
}

// Shared deploy logic for both owned and borrowed MidnightProvider.
#[doc(hidden)]
pub trait DeployInner<P> {
    fn provider_ref(&self) -> &MidnightProvider;
    fn into_parts(self) -> (P, Option<ContractState<InMemoryDB>>, Option<Prover>);
}

impl DeployInner<MidnightProvider> for ContractBuilder<MidnightProvider> {
    fn provider_ref(&self) -> &MidnightProvider {
        &self.provider
    }
    fn into_parts(
        self,
    ) -> (
        MidnightProvider,
        Option<ContractState<InMemoryDB>>,
        Option<Prover>,
    ) {
        (self.provider, self.initial_state, self.prover)
    }
}

impl<'a> DeployInner<&'a MidnightProvider> for ContractBuilder<&'a MidnightProvider> {
    fn provider_ref(&self) -> &MidnightProvider {
        self.provider
    }
    fn into_parts(
        self,
    ) -> (
        &'a MidnightProvider,
        Option<ContractState<InMemoryDB>>,
        Option<Prover>,
    ) {
        (self.provider, self.initial_state, self.prover)
    }
}

impl<P> ContractBuilder<P>
where
    Self: DeployInner<P>,
    P: Provider,
{
    async fn deploy_inner(self) -> Result<Contract<P>, ContractError> {
        let node_url = self.provider_ref().node_url().to_string();
        let wallet_seed = self
            .provider_ref()
            .wallet_seed()
            .ok_or_else(|| {
                ContractError::Construction(
                    "provider has no wallet — call .with_wallet() on the provider".into(),
                )
            })?
            .to_string();

        let (provider, initial_state, prover) = self.into_parts();

        let mut state = initial_state
            .ok_or_else(|| ContractError::Construction("missing initial_state".into()))?;

        if let Some(ref prover) = prover {
            state = with_zk_keys(state, prover.keys_dir())?;
        }

        let prover_ref = prover.as_ref().ok_or_else(|| {
            ContractError::Construction(
                "no prover — call .zk_keys() or .prover() on the builder".into(),
            )
        })?;

        let result = deploy_funded(&state, &node_url, &wallet_seed, prover_ref).await?;
        let address = result.address_hex();

        submit(&node_url, &result.tx_bytes).await?;

        wait_for_deployment(
            &provider,
            &address,
            Duration::from_secs(60),
            Duration::from_secs(2),
        )
        .await?;

        Ok(Contract {
            address,
            node_url,
            prover,
            provider,
            state,
        })
    }
}

// ---------------------------------------------------------------------------
// Contract — a deployed contract handle
// ---------------------------------------------------------------------------

/// A deployed contract instance bound to a provider.
///
/// Holds the contract's on-chain address, cached ledger state, and a provider
/// for syncing state from the network. After `deploy()` or `connect()`, the
/// state is immediately available via `state()`.
pub struct Contract<P> {
    address: String,
    node_url: String,
    prover: Option<Prover>,
    provider: P,
    state: ContractState<InMemoryDB>,
}

impl<P> std::fmt::Debug for Contract<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contract")
            .field("address", &self.address)
            .field("node_url", &self.node_url)
            .finish_non_exhaustive()
    }
}

impl<P: Provider> Contract<P> {
    /// Connect to an already-deployed contract, fetching its current state.
    pub async fn connect(
        address: &str,
        node_url: &str,
        provider: P,
    ) -> Result<Self, ContractError> {
        let hex = provider
            .get_contract_state(address, None)
            .await?
            .ok_or_else(|| ContractError::NotFound(address.to_string()))?;
        let state = crate::call::deserialize_state(&hex)?;
        Ok(Self {
            address: address.to_string(),
            node_url: node_url.to_string(),
            prover: None,
            provider,
            state,
        })
    }

    /// Set the prover configuration for on-chain circuit calls.
    pub fn with_prover(mut self, prover: Prover) -> Self {
        self.prover = Some(prover);
        self
    }

    /// Set the path to the compiled contract directory containing `keys/` and `zkir/`.
    ///
    /// Convenience for `.with_prover(Prover::local(path))`.
    /// Required for on-chain circuit calls via `call()` / `circuits()`.
    pub fn with_zk_keys(self, path: impl Into<PathBuf>) -> Self {
        self.with_prover(Prover::local(path))
    }

    /// The contract's on-chain address (hex string).
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The node URL this contract is connected to.
    pub fn node_url(&self) -> &str {
        &self.node_url
    }

    /// Reference to the provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// The current cached contract state.
    ///
    /// After `deploy()`, this is the initial state. Call `sync()` to refresh
    /// from the chain.
    pub fn state(&self) -> &ContractState<InMemoryDB> {
        &self.state
    }

    /// Execute a circuit call on-chain.
    ///
    /// Runs the circuit IR locally, builds a funded transaction, submits it
    /// to the node, and updates the cached state.
    pub async fn call(
        &mut self,
        ir: &compact_codegen::ir::CircuitIrBody,
        circuit_name: &str,
    ) -> Result<(), ContractError>
    where
        P: std::ops::Deref<Target = MidnightProvider>,
    {
        self.call_with(ir, circuit_name, &[], &crate::interpreter::NoWitnesses, &[])
            .await
    }

    /// Execute a circuit call on-chain with arguments and witnesses.
    #[allow(clippy::too_many_arguments)]
    pub async fn call_with<W: crate::interpreter::WitnessProvider>(
        &mut self,
        ir: &compact_codegen::ir::CircuitIrBody,
        circuit_name: &str,
        args: &[(&str, crate::interpreter::Value)],
        witnesses: &W,
        helpers: &[compact_codegen::ir::HelperDef],
    ) -> Result<(), ContractError>
    where
        P: std::ops::Deref<Target = MidnightProvider>,
    {
        let provider: &MidnightProvider = &self.provider;
        let node_url = provider.node_url();
        let wallet_seed = provider
            .wallet_seed()
            .ok_or_else(|| ContractError::Construction("provider has no wallet".into()))?;
        let address = crate::call::parse_address(&self.address)?;

        let prover = self.prover.as_ref().ok_or_else(|| {
            ContractError::Construction(
                "no prover configured — call .with_zk_keys() or .with_prover() after connect, or .zk_keys()/.prover() on the builder".into(),
            )
        })?;

        let (tx_bytes, new_state) = crate::call::call_funded_with(
            ir,
            &self.state,
            circuit_name,
            address,
            node_url,
            wallet_seed,
            prover,
            args,
            witnesses,
            helpers,
        )
        .await?;

        submit(node_url, &tx_bytes).await?;

        // Wait for on-chain state to update
        tokio::time::sleep(Duration::from_secs(6)).await;
        self.sync().await.unwrap_or(());

        // Use local state (more accurate than fetched, since indexer may lag)
        self.state = new_state;
        Ok(())
    }

    /// Refresh the cached state from the chain.
    pub async fn sync(&mut self) -> Result<(), ContractError> {
        let hex = self
            .provider
            .get_contract_state(&self.address, None)
            .await?
            .ok_or_else(|| ContractError::NotFound(self.address.clone()))?;
        self.state = crate::call::deserialize_state(&hex)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use midnight_provider::{
        Block, BlockOffset, ContractAction, ContractActionOffset, Health, ProviderError,
        StateQuery, StateQueryResult, Transaction, TransactionOffset,
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
            Ok(self.state_hex.clone())
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

    fn mock_state_hex() -> String {
        use midnight_bindgen::{ContractMaintenanceAuthority, StateValue, StorageHashMap};
        let state: ContractState<InMemoryDB> = ContractState::new(
            StateValue::Array(vec![StateValue::from(0u64)].into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        );
        let mut bytes = Vec::new();
        midnight_serialize::tagged_serialize(&state, &mut bytes).unwrap();
        hex::encode(&bytes)
    }

    #[tokio::test]
    async fn connect_returns_contract_with_state() {
        let provider = MockProvider {
            state_hex: Some(mock_state_hex()),
        };
        let contract = Contract::connect("addr1", "ws://test", provider)
            .await
            .unwrap();
        assert_eq!(contract.address(), "addr1");
        let _ = contract.state();
    }

    #[tokio::test]
    async fn connect_returns_not_found_when_no_state() {
        let provider = MockProvider { state_hex: None };
        let err = Contract::connect("addr1", "ws://test", provider)
            .await
            .unwrap_err();
        assert!(matches!(err, ContractError::NotFound(_)));
    }

    #[tokio::test]
    async fn sync_refreshes_state() {
        let provider = MockProvider {
            state_hex: Some(mock_state_hex()),
        };
        let mut contract = Contract::connect("addr1", "ws://test", provider)
            .await
            .unwrap();
        contract.sync().await.unwrap();
        let _ = contract.state();
    }
}
