use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use midnight_bindgen::{ContractState, InMemoryDB};
use midnight_provider::{MidnightProvider, Provider};

use crate::Prover;
use crate::call::{deploy_funded, submit, wait_for_deployment, with_zk_keys};
use crate::error::ContractError;

// ---------------------------------------------------------------------------
// AsMidnightProvider — trait so owned, borrowed, and smart-pointer
// `MidnightProvider` values can drive the deploy/connect builders.
// ---------------------------------------------------------------------------

/// Types that can hand out a reference to a `MidnightProvider`.
///
/// Implemented directly for `MidnightProvider`, and transitively for
/// `&T`, `Box<T>`, and `Arc<T>` where `T: AsMidnightProvider`.
pub trait AsMidnightProvider {
    fn as_midnight_provider(&self) -> &MidnightProvider;
}

impl AsMidnightProvider for MidnightProvider {
    fn as_midnight_provider(&self) -> &MidnightProvider {
        self
    }
}

impl<T: AsMidnightProvider + ?Sized> AsMidnightProvider for &T {
    fn as_midnight_provider(&self) -> &MidnightProvider {
        (**self).as_midnight_provider()
    }
}

impl<T: AsMidnightProvider + ?Sized> AsMidnightProvider for Box<T> {
    fn as_midnight_provider(&self) -> &MidnightProvider {
        (**self).as_midnight_provider()
    }
}

impl<T: AsMidnightProvider + ?Sized> AsMidnightProvider for Arc<T> {
    fn as_midnight_provider(&self) -> &MidnightProvider {
        (**self).as_midnight_provider()
    }
}

// ---------------------------------------------------------------------------
// DeployBuilder — typestate builder for deploying a contract.
// ---------------------------------------------------------------------------

/// Builder for deploying a contract.
///
/// Typically accessed via `Contract::deploy(&provider)`. Await the builder to
/// run the deployment.
///
/// # Example
///
/// ```rust,ignore
/// let contract = counter::Contract::deploy(&provider)
///     .initial_state(counter::LedgerInitialState::default())
///     .zk_keys("compiled")
///     .await?;
/// ```
pub struct DeployBuilder<'a, P> {
    provider: P,
    initial_state: Option<ContractState<InMemoryDB>>,
    zk_keys_dir: Option<PathBuf>,
    prover: Prover,
    deploy_timeout: Duration,
    deploy_poll_interval: Duration,
    _lifetime: PhantomData<&'a ()>,
}

impl<'a, P> DeployBuilder<'a, P> {
    pub(crate) fn new(provider: P) -> Self {
        Self {
            provider,
            initial_state: None,
            zk_keys_dir: None,
            prover: Prover::default(),
            deploy_timeout: Duration::from_secs(60),
            deploy_poll_interval: Duration::from_secs(2),
            _lifetime: PhantomData,
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

impl<'a, P> IntoFuture for DeployBuilder<'a, P>
where
    P: AsMidnightProvider + Provider + Send + 'a,
{
    type Output = Result<Contract<P>, ContractError>;
    // `Pin<Box<dyn Future>>` rather than `impl Future` because the latter is
    // still unstable in associated type position (rust-lang/rust#63063).
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let node_url = self.provider.as_midnight_provider().node_url().to_string();
            let wallet_seed = self
                .provider
                .as_midnight_provider()
                .wallet_seed()
                .ok_or_else(|| {
                    ContractError::Construction(
                        "provider has no wallet — call .with_wallet() on the provider".into(),
                    )
                })?
                .to_string();

            let zk_keys_dir = self.zk_keys_dir.ok_or_else(|| {
                ContractError::Construction(
                    "missing zk_keys — call .zk_keys() on the builder".into(),
                )
            })?;

            let mut state = self.initial_state.ok_or_else(|| {
                ContractError::Construction(
                    "missing initial_state — call .initial_state(...) on the builder".into(),
                )
            })?;

            state = with_zk_keys(state, &zk_keys_dir)?;

            let result =
                deploy_funded(&state, &node_url, &wallet_seed, &zk_keys_dir, &self.prover).await?;
            let address = result.address_hex();

            submit(&node_url, &result.tx_bytes).await?;

            wait_for_deployment(
                &self.provider,
                &address,
                self.deploy_timeout,
                self.deploy_poll_interval,
            )
            .await?;

            Ok(Contract {
                address,
                node_url,
                zk_keys_dir: Some(zk_keys_dir),
                prover: self.prover,
                provider: self.provider,
                state,
                ttl: crate::call::DEFAULT_TTL,
                post_call_delay: Contract::<P>::DEFAULT_POST_CALL_DELAY,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// ConnectBuilder — typestate builder for connecting to a deployed contract.
// ---------------------------------------------------------------------------

/// Builder for connecting to an already-deployed contract.
///
/// Typically accessed via `Contract::connect(&provider, address)`. Await the
/// builder to fetch the current contract state and return a `Contract<P>`.
///
/// # Example
///
/// ```rust,ignore
/// let contract = counter::Contract::connect(&provider, address)
///     .zk_keys("compiled")
///     .await?;
/// ```
pub struct ConnectBuilder<'a, P> {
    provider: P,
    address: String,
    zk_keys_dir: Option<PathBuf>,
    prover: Prover,
    _lifetime: PhantomData<&'a ()>,
}

impl<'a, P> ConnectBuilder<'a, P> {
    pub(crate) fn new(provider: P, address: impl Into<String>) -> Self {
        Self {
            provider,
            address: address.into(),
            zk_keys_dir: None,
            prover: Prover::default(),
            _lifetime: PhantomData,
        }
    }

    /// Set the path to the compiled contract directory containing `keys/` and `zkir/`.
    ///
    /// Required for on-chain circuit calls after connecting.
    pub fn zk_keys(mut self, path: impl Into<PathBuf>) -> Self {
        self.zk_keys_dir = Some(path.into());
        self
    }

    /// Override the proving backend (default: `Prover::Local`).
    pub fn prover(mut self, prover: Prover) -> Self {
        self.prover = prover;
        self
    }
}

impl<'a, P> IntoFuture for ConnectBuilder<'a, P>
where
    P: AsMidnightProvider + Provider + Send + 'a,
{
    type Output = Result<Contract<P>, ContractError>;
    // See DeployBuilder::IntoFuture comment — `impl Future` in assoc type
    // position is still unstable.
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let node_url = self.provider.as_midnight_provider().node_url().to_string();
            let hex = self
                .provider
                .get_contract_state(&self.address, None)
                .await?
                .ok_or_else(|| ContractError::NotFound(self.address.clone()))?;
            let state = crate::call::deserialize_state(&hex)?;
            Ok(Contract {
                address: self.address,
                node_url,
                zk_keys_dir: self.zk_keys_dir,
                prover: self.prover,
                provider: self.provider,
                state,
                ttl: crate::call::DEFAULT_TTL,
                post_call_delay: Contract::<P>::DEFAULT_POST_CALL_DELAY,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Contract — a deployed contract handle
// ---------------------------------------------------------------------------

/// A deployed contract instance bound to a provider.
///
/// Holds the contract's on-chain address, cached ledger state, and a provider
/// for syncing state from the network. After awaiting `deploy()` or
/// `connect()`, the state is immediately available via `state()`.
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

impl Contract<()> {
    /// Start building a deployment for this contract.
    ///
    /// `provider` can be an owned or borrowed `MidnightProvider`.
    pub fn deploy<'a, P>(provider: P) -> DeployBuilder<'a, P>
    where
        P: AsMidnightProvider + Provider + 'a,
    {
        DeployBuilder::new(provider)
    }

    /// Start building a connection to an already-deployed contract.
    ///
    /// `provider` can be an owned or borrowed `MidnightProvider`.
    pub fn connect<'a, P>(provider: P, address: impl Into<String>) -> ConnectBuilder<'a, P>
    where
        P: AsMidnightProvider + Provider + 'a,
    {
        ConnectBuilder::new(provider, address)
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
    ) -> Result<Option<crate::interpreter::Value>, ContractError>
    where
        P: AsMidnightProvider,
    {
        self.call_with(
            ir,
            circuit_name,
            &[],
            &crate::interpreter::NoWitnesses,
            &[],
            &[],
            &[],
        )
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
        enums: &[compact_codegen::ir::EnumDef],
    ) -> Result<Option<crate::interpreter::Value>, ContractError>
    where
        P: AsMidnightProvider,
    {
        let provider: &MidnightProvider = self.provider.as_midnight_provider();
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

        let (tx_bytes, new_state, result) = crate::call::call_funded_with(
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
            enums,
        )
        .await?;

        submit(node_url, &tx_bytes).await?;

        // Wait for on-chain state to update
        tokio::time::sleep(self.post_call_delay).await;
        self.sync().await.unwrap_or(());

        // Use local state (more accurate than fetched, since indexer may lag)
        self.state = new_state;
        Ok(result)
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
        inner: MidnightProvider,
    }

    impl MockProvider {
        fn new(state_hex: Option<String>) -> Self {
            Self {
                state_hex,
                inner: MidnightProvider::new("ws://test", "http://test").unwrap(),
            }
        }
    }

    impl AsMidnightProvider for MockProvider {
        fn as_midnight_provider(&self) -> &MidnightProvider {
            &self.inner
        }
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
        let provider = MockProvider::new(Some(mock_state_hex()));
        let contract = Contract::connect(provider, "addr1").await.unwrap();
        assert_eq!(contract.address(), "addr1");
        let _ = contract.state();
    }

    #[tokio::test]
    async fn connect_returns_not_found_when_no_state() {
        let provider = MockProvider::new(None);
        let err = Contract::connect(provider, "addr1").await.unwrap_err();
        assert!(matches!(err, ContractError::NotFound(_)));
    }

    #[tokio::test]
    async fn sync_refreshes_state() {
        let provider = MockProvider::new(Some(mock_state_hex()));
        let mut contract = Contract::connect(provider, "addr1").await.unwrap();
        contract.sync().await.unwrap();
        let _ = contract.state();
    }
}
