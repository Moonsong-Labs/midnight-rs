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
// BlockRef — pin queries to a specific block
// ---------------------------------------------------------------------------

/// Pin queries to a specific block instead of latest.
///
/// `Height` is supported for circuit calls (full state fetches) via the indexer
/// GraphQL API (`ContractActionOffset`). Lazy ledger queries
/// (`contract.ledger()`) go through the node RPC, which only accepts a block
/// hash, so `Height` is **not** supported for those queries and falls back to
/// latest. Use `Hash` for fully consistent block-pinned access across both
/// circuit calls and ledger queries.
#[derive(Debug, Clone)]
pub enum BlockRef {
    /// Pin to a block by height. Supported for circuit calls (via the indexer).
    /// Lazy ledger queries fall back to latest because the node RPC only
    /// accepts block hashes.
    Height(i64),
    /// Pin to a block by hash. Supported by both circuit calls (node RPC) and
    /// lazy ledger queries (node RPC).
    Hash(String),
}

impl BlockRef {
    /// Convert to a `ContractActionOffset` for the indexer GraphQL API.
    pub(crate) fn to_contract_action_offset(&self) -> midnight_provider::ContractActionOffset {
        match self {
            BlockRef::Height(h) => midnight_provider::ContractActionOffset::block_height(*h),
            BlockRef::Hash(h) => midnight_provider::ContractActionOffset::block_hash(h),
        }
    }
}

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
///     .with_initial_state(counter::LedgerInitialState::default())
///     .with_zk_keys("compiled")
///     .await?;
/// ```
pub struct DeployBuilder<'a, P> {
    provider: P,
    initial_state: Option<ContractState<InMemoryDB>>,
    zk_keys_dir: Option<PathBuf>,
    prover: Prover,
    ttl: Duration,
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
            ttl: crate::call::DEFAULT_TTL,
            deploy_timeout: Duration::from_secs(60),
            deploy_poll_interval: Duration::from_secs(2),
            _lifetime: PhantomData,
        }
    }

    /// Set the initial contract state.
    ///
    /// Accepts anything that converts to `ContractState<InMemoryDB>` — including
    /// the generated `LedgerInitialState` (via its `Into` impl).
    pub fn with_initial_state(mut self, state: impl Into<ContractState<InMemoryDB>>) -> Self {
        self.initial_state = Some(state.into());
        self
    }

    /// Set the path to the compiled contract directory containing `keys/` and `zkir/`.
    ///
    /// Required for deployment and on-chain circuit calls.
    pub fn with_zk_keys(mut self, path: impl Into<PathBuf>) -> Self {
        self.zk_keys_dir = Some(path.into());
        self
    }

    /// Override the proving backend (default: `Prover::Local`).
    pub fn with_prover(mut self, prover: Prover) -> Self {
        self.prover = prover;
        self
    }

    /// Set the timeout for waiting for deployment confirmation (default: 60s).
    pub fn with_deploy_timeout(mut self, timeout: Duration) -> Self {
        self.deploy_timeout = timeout;
        self
    }

    /// Set the poll interval for checking deployment status (default: 2s).
    pub fn with_deploy_poll_interval(mut self, interval: Duration) -> Self {
        self.deploy_poll_interval = interval;
        self
    }

    /// Set the transaction TTL duration (default: 1 hour).
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
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
                    "missing zk_keys — call .with_zk_keys(...) on the builder".into(),
                )
            })?;

            let mut state = self.initial_state.ok_or_else(|| {
                ContractError::Construction(
                    "missing initial_state — call .with_initial_state(...) on the builder".into(),
                )
            })?;

            state = with_zk_keys(state, &zk_keys_dir)?;

            let result =
                deploy_funded(&state, &node_url, &wallet_seed, &zk_keys_dir, &self.prover).await?;
            let address = result.address_hex();

            let mut pending = submit(&node_url, &result.tx_bytes).await?;
            pending.wait_best().await?;

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
                ttl: self.ttl,
                at_block: None,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// ConnectBuilder — typestate builder for connecting to a deployed contract.
// ---------------------------------------------------------------------------

/// Builder for referencing an already-deployed contract.
///
/// Typically accessed via `Contract::at(&provider, address)`. Call `.build()`
/// to get the `Contract<P>` handle. This is fully synchronous, no network
/// calls are made.
///
/// # Example
///
/// ```rust,ignore
/// let contract = counter::Contract::at(&provider, address)
///     .with_zk_keys("compiled")
///     .build();
/// ```
pub struct ConnectBuilder<P> {
    provider: P,
    address: String,
    zk_keys_dir: Option<PathBuf>,
    prover: Prover,
    ttl: Duration,
    at_block: Option<BlockRef>,
}

impl<P> ConnectBuilder<P> {
    pub(crate) fn new(provider: P, address: impl Into<String>) -> Self {
        Self {
            provider,
            address: address.into(),
            zk_keys_dir: None,
            prover: Prover::default(),
            ttl: crate::call::DEFAULT_TTL,
            at_block: None,
        }
    }

    /// Set the path to the compiled contract directory containing `keys/` and `zkir/`.
    ///
    /// Required for on-chain circuit calls after connecting.
    pub fn with_zk_keys(mut self, path: impl Into<PathBuf>) -> Self {
        self.zk_keys_dir = Some(path.into());
        self
    }

    /// Override the proving backend (default: `Prover::Local`).
    pub fn with_prover(mut self, prover: Prover) -> Self {
        self.prover = prover;
        self
    }

    /// Pin queries to a specific block. Default is latest.
    pub fn at_block(mut self, block_ref: BlockRef) -> Self {
        self.at_block = Some(block_ref);
        self
    }

    /// Set the transaction TTL duration (default: 1 hour).
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Build the contract handle.
    ///
    /// This is synchronous. No network calls are made.
    pub fn build(self) -> Contract<P>
    where
        P: AsMidnightProvider,
    {
        let node_url = self.provider.as_midnight_provider().node_url().to_string();
        Contract {
            address: self.address,
            node_url,
            zk_keys_dir: self.zk_keys_dir,
            prover: self.prover,
            provider: self.provider,
            ttl: self.ttl,
            at_block: self.at_block,
        }
    }
}

// ---------------------------------------------------------------------------
// Contract — a deployed contract handle
// ---------------------------------------------------------------------------

/// A deployed contract instance bound to a provider.
///
/// This is a stateless, immutable handle. It does not cache contract state.
/// Each circuit call fetches fresh state from the node RPC (or the indexer
/// when pinned by block height). Ledger queries go through the node RPC
/// directly.
pub struct Contract<P> {
    address: String,
    node_url: String,
    zk_keys_dir: Option<PathBuf>,
    prover: Prover,
    provider: P,
    /// Transaction time-to-live duration (default: 1 hour).
    ttl: Duration,
    /// Optional block pin for queries. `None` means latest.
    at_block: Option<BlockRef>,
}

impl<P: Clone> Clone for Contract<P> {
    fn clone(&self) -> Self {
        Self {
            address: self.address.clone(),
            node_url: self.node_url.clone(),
            zk_keys_dir: self.zk_keys_dir.clone(),
            prover: self.prover.clone(),
            provider: self.provider.clone(),
            ttl: self.ttl,
            at_block: self.at_block.clone(),
        }
    }
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

    /// Create a handle for an already-deployed contract at the given address.
    ///
    /// This is synchronous, no network calls are made. Use `deploy()` to
    /// deploy a new contract.
    ///
    /// `provider` can be an owned or borrowed `MidnightProvider`.
    pub fn at<P>(provider: P, address: impl Into<String>) -> ConnectBuilder<P>
    where
        P: AsMidnightProvider + Provider,
    {
        ConnectBuilder::new(provider, address)
    }
}

impl<P: Provider> Contract<P> {
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

    /// The block pin for queries. `None` means latest.
    pub fn at_block(&self) -> Option<&BlockRef> {
        self.at_block.as_ref()
    }

    /// Execute a circuit call on-chain.
    ///
    /// Fetches fresh state from the node RPC (or the indexer when pinned by
    /// block height), runs the circuit IR locally, builds a funded transaction,
    /// and submits it to the node.
    pub async fn call(
        &self,
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
    ///
    /// Fetches fresh state from the node RPC (or the indexer when pinned by
    /// block height via `at_block`), runs the circuit IR locally, builds a
    /// funded transaction, proves it, and submits to the node. The contract
    /// handle is not mutated.
    #[allow(clippy::too_many_arguments)]
    pub async fn call_with(
        &self,
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
                "no zk_keys configured, call .with_zk_keys(...) on the builder".into(),
            )
        })?;

        // Fetch fresh state, using the node RPC for hash-pinned or latest,
        // and the indexer for height-pinned queries.
        let state = match self.at_block.as_ref() {
            Some(BlockRef::Hash(h)) => {
                crate::call::fetch_state_from_node(provider, &self.address, Some(h.as_str()))
                    .await?
            }
            Some(block_ref) => {
                let offset = block_ref.to_contract_action_offset();
                crate::call::fetch_state_at(&self.provider, &self.address, Some(offset)).await?
            }
            None => crate::call::fetch_state_from_node(provider, &self.address, None).await?,
        };

        let (tx_bytes, _new_state, result) = crate::call::call_funded_with(
            ir,
            &state,
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

        // Record the contract's block height before submitting so we can
        // detect when the indexer has processed the new transaction.
        let height_before = self
            .provider
            .get_latest_contract_block_height(&self.address)
            .await
            .unwrap_or(None);

        let mut pending = submit(node_url, &tx_bytes).await?;
        pending.wait_best().await?;

        // Wait for the indexer to process a new block for this contract.
        crate::call::wait_for_contract_update(
            &self.provider,
            &self.address,
            height_before,
            crate::call::DEFAULT_TX_TIMEOUT,
            crate::call::DEFAULT_TX_POLL_INTERVAL,
        )
        .await?;

        Ok(result)
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
        inner: MidnightProvider,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
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

    #[test]
    fn at_constructs_handle() {
        let provider = MockProvider::new();
        let contract = Contract::at(provider, "addr1").build();
        assert_eq!(contract.address(), "addr1");
        assert!(contract.at_block().is_none());
    }

    #[test]
    fn at_with_block_ref() {
        let provider = MockProvider::new();
        let contract = Contract::at(provider, "addr1")
            .at_block(BlockRef::Hash("abc123".into()))
            .build();
        assert_eq!(contract.address(), "addr1");
        assert!(matches!(contract.at_block(), Some(BlockRef::Hash(h)) if h == "abc123"));
    }

    #[test]
    fn block_ref_to_offset_height() {
        let br = BlockRef::Height(42);
        let offset = br.to_contract_action_offset();
        assert!(
            matches!(offset, ContractActionOffset::BlockHeight { .. }),
            "expected BlockHeight variant"
        );
    }

    #[test]
    fn block_ref_to_offset_hash() {
        let br = BlockRef::Hash("deadbeef".into());
        let offset = br.to_contract_action_offset();
        assert!(
            matches!(offset, ContractActionOffset::BlockHash { .. }),
            "expected BlockHash variant"
        );
    }
}
