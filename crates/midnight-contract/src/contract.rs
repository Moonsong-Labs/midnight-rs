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
    zk_keys_dir: Option<PathBuf>,
    prover: Prover,
    deploy_timeout: Duration,
    deploy_poll_interval: Duration,
}

impl ContractBuilder<()> {
    pub fn new() -> Self {
        ContractBuilder {
            provider: (),
            initial_state: None,
            zk_keys_dir: None,
            prover: Prover::default(),
            deploy_timeout: Duration::from_secs(60),
            deploy_poll_interval: Duration::from_secs(2),
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
            zk_keys_dir: self.zk_keys_dir,
            prover: self.prover,
            deploy_timeout: self.deploy_timeout,
            deploy_poll_interval: self.deploy_poll_interval,
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
    /// Required for deployment and on-chain circuit calls.
    pub fn zk_keys(mut self, path: impl Into<PathBuf>) -> Self {
        self.zk_keys_dir = Some(path.into());
        self
    }

    /// Override the proving backend (default: `Prover::Local`).
    ///
    /// Use `Prover::Remote(url)` to delegate proving to an HTTP proof server.
    pub fn prover(mut self, prover: Prover) -> Self {
        self.prover = prover;
        self
    }

    /// Set the timeout for waiting for deployment confirmation (default: 60s).
    pub fn deploy_timeout(mut self, timeout: Duration) -> Self {
        self.deploy_timeout = timeout;
        self
    }

    /// Set the poll interval for checking deployment status (default: 2s).
    pub fn deploy_poll_interval(mut self, interval: Duration) -> Self {
        self.deploy_poll_interval = interval;
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
    fn into_parts(
        self,
    ) -> (
        P,
        Option<ContractState<InMemoryDB>>,
        Option<PathBuf>,
        Prover,
        Duration,
        Duration,
    );
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
        Option<PathBuf>,
        Prover,
        Duration,
        Duration,
    ) {
        (
            self.provider,
            self.initial_state,
            self.zk_keys_dir,
            self.prover,
            self.deploy_timeout,
            self.deploy_poll_interval,
        )
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
        Option<PathBuf>,
        Prover,
        Duration,
        Duration,
    ) {
        (
            self.provider,
            self.initial_state,
            self.zk_keys_dir,
            self.prover,
            self.deploy_timeout,
            self.deploy_poll_interval,
        )
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

        let (provider, initial_state, zk_keys_dir, prover, deploy_timeout, deploy_poll_interval) =
            self.into_parts();

        let zk_keys_dir = zk_keys_dir.ok_or_else(|| {
            ContractError::Construction("missing zk_keys — call .zk_keys() on the builder".into())
        })?;

        let mut state = initial_state
            .ok_or_else(|| ContractError::Construction("missing initial_state".into()))?;

        state = with_zk_keys(state, &zk_keys_dir)?;

        let result = deploy_funded(&state, &node_url, &wallet_seed, &zk_keys_dir, &prover).await?;
        let address = result.address_hex();

        submit(&node_url, &result.tx_bytes).await?;

        wait_for_deployment(&provider, &address, deploy_timeout, deploy_poll_interval).await?;

        Ok(Contract {
            address,
            node_url,
            zk_keys_dir: Some(zk_keys_dir),
            prover,
            provider,
            state,
            ttl: crate::call::DEFAULT_TTL,
            post_call_delay: Contract::<P>::DEFAULT_POST_CALL_DELAY,
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
    zk_keys_dir: Option<PathBuf>,
    prover: Prover,
    provider: P,
    state: ContractState<InMemoryDB>,
    /// Transaction time-to-live duration (default: 1 hour).
    ttl: Duration,
    /// Delay after submitting a call transaction before syncing state (default: 6s).
    post_call_delay: Duration,
}

impl<P> std::fmt::Debug for Contract<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contract")
            .field("address", &self.address)
            .field("node_url", &self.node_url)
            .finish_non_exhaustive()
    }
}

impl<P> Contract<P> {
    /// Default post-call delay: 6 seconds.
    pub const DEFAULT_POST_CALL_DELAY: Duration = Duration::from_secs(6);

    /// Set the transaction TTL duration (default: 1 hour).
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Set the delay after submitting a call before syncing state (default: 6s).
    pub fn with_post_call_delay(mut self, delay: Duration) -> Self {
        self.post_call_delay = delay;
        self
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
            zk_keys_dir: None,
            prover: Prover::default(),
            provider,
            state,
            ttl: crate::call::DEFAULT_TTL,
            post_call_delay: Self::DEFAULT_POST_CALL_DELAY,
        })
    }

    /// Set the path to the compiled contract directory containing `keys/` and `zkir/`.
    ///
    /// Required for on-chain circuit calls via `call()` / `circuits()`.
    pub fn with_zk_keys(mut self, path: impl Into<PathBuf>) -> Self {
        self.zk_keys_dir = Some(path.into());
        self
    }

    /// Override the proving backend (default: `Prover::Local`).
    ///
    /// Use `Prover::Remote(url)` to delegate proving to an HTTP proof server.
    pub fn with_prover(mut self, prover: Prover) -> Self {
        self.prover = prover;
        self
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
        self.call_with(ir, circuit_name, &[], &crate::interpreter::NoWitnesses, &[], &[])
            .await
    }

    /// Execute a circuit call on-chain with arguments and witnesses.
    #[allow(clippy::too_many_arguments)]
    pub async fn call_with(
        &mut self,
        ir: &compact_codegen::ir::CircuitIrBody,
        circuit_name: &str,
        args: &[(&str, crate::interpreter::Value)],
        witnesses: &dyn crate::interpreter::WitnessProvider,
        helpers: &[compact_codegen::ir::HelperDef],
        structs: &[compact_codegen::ir::StructDef],
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

        let zk_keys_dir = self.zk_keys_dir.as_deref().ok_or_else(|| {
            ContractError::Construction(
                "no zk_keys configured, call .with_zk_keys() after connect or .zk_keys() on the builder".into(),
            )
        })?;

        let (tx_bytes, new_state) = crate::call::call_funded_with(
            ir,
            &self.state,
            circuit_name,
            address,
            node_url,
            wallet_seed,
            zk_keys_dir,
            &self.prover,
            args,
            witnesses,
            helpers,
            structs,
        )
        .await?;

        submit(node_url, &tx_bytes).await?;

        // Wait for on-chain state to update
        tokio::time::sleep(self.post_call_delay).await;
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
