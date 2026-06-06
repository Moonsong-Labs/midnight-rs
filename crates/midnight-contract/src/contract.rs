use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use midnight_base_crypto::signatures::VerifyingKey;
use midnight_bindgen_runtime::{ContractState, InMemoryDB};
use midnight_provider::{MidnightProvider, Provider};

use crate::Prover;
use crate::deploy::{deploy_funded, wait_for_deployment};
use crate::error::ContractError;
use crate::state::with_zk_keys;
use midnight_provider::{PendingTx, TxInBlock};

/// What to do with a contract's private state after a call, comparing the
/// post-call buffer against the pre-call `baseline`.
#[derive(Debug, PartialEq, Eq)]
enum PrivateStatePersist {
    /// Witnesses didn't change it — leave the store untouched.
    Unchanged,
    /// A witness cleared it to empty — remove it so the next call doesn't
    /// reload stale state.
    Remove,
    /// A witness produced new non-empty state — store it.
    Store,
}

fn private_state_persist(baseline: &[u8], post_call: &[u8]) -> PrivateStatePersist {
    if post_call == baseline {
        PrivateStatePersist::Unchanged
    } else if post_call.is_empty() {
        PrivateStatePersist::Remove
    } else {
        PrivateStatePersist::Store
    }
}

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
pub struct DeployBuilder<P> {
    provider: P,
    initial_state: Option<ContractState<InMemoryDB>>,
    zk_keys_dir: Option<PathBuf>,
    prover: Prover,
    deploy_timeout: Duration,
    deploy_poll_interval: Duration,
    shielded_offer: Option<midnight_helpers::OfferInfo<midnight_helpers::DefaultDB>>,
    maintenance_authority: Option<(Vec<VerifyingKey>, u32)>,
}

impl<P> DeployBuilder<P> {
    pub(crate) fn new(provider: P) -> Self {
        Self {
            provider,
            initial_state: None,
            zk_keys_dir: None,
            prover: Prover::default(),
            deploy_timeout: Duration::from_secs(60),
            deploy_poll_interval: Duration::from_secs(2),
            shielded_offer: None,
            maintenance_authority: None,
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

    /// Attach a hand-built shielded (zswap) [`OfferInfo`] to ride alongside
    /// the deploy in the same transaction segment.
    ///
    /// The SDK does not derive shielded inputs/outputs from a contract's
    /// initial state — if your deployment needs to spend or produce shielded
    /// coins (e.g. seeding a contract with a shielded balance), construct the
    /// offer with [`InputInfo`](midnight_helpers::InputInfo) /
    /// [`OutputInfo`](midnight_helpers::OutputInfo) and pass it here. The
    /// [`TransferBuilder::shielded`](midnight_wallet::TransferBuilder::shielded)
    /// source is the canonical worked example.
    ///
    /// Coins in `InputInfo::origin` must come from the provider's wallet seed
    /// (the same seed that pays the dust fee).
    pub fn with_shielded_offer(
        mut self,
        offer: midnight_helpers::OfferInfo<midnight_helpers::DefaultDB>,
    ) -> Self {
        self.shielded_offer = Some(offer);
        self
    }

    /// Make the deployed contract governable by setting its maintenance
    /// authority to `committee` (the verifying keys allowed to authorize
    /// updates) with the given `threshold` (how many must sign).
    ///
    /// The SDK stores no signing key: each committee member keeps their own and
    /// signs maintenance operations externally (see
    /// [`Contract::maintenance`]). For a single-owner contract, pass
    /// `vec![key.verifying_key()]` and `1`.
    ///
    /// Without this the contract deploys with an empty committee and can never
    /// accept a maintenance update (verifier-key rotation, authority
    /// replacement).
    pub fn with_maintenance_authority(
        mut self,
        committee: Vec<VerifyingKey>,
        threshold: u32,
    ) -> Self {
        self.maintenance_authority = Some((committee, threshold));
        self
    }
}

impl<P> DeployBuilder<P>
where
    P: AsMidnightProvider + Provider + Send,
{
    /// Build, prove, and submit the deploy transaction without waiting for inclusion.
    ///
    /// Returns a [`PendingDeploy`] handle on which you can call
    /// [`PendingDeploy::wait_best`] / [`PendingDeploy::wait_finalized`] to observe
    /// inclusion states, then [`PendingDeploy::into_contract`] to wait for the
    /// indexer and obtain the [`Contract`].
    ///
    /// For the common case where you don't need to observe both states, just
    /// `.await?` the builder directly.
    pub async fn send(self) -> Result<PendingDeploy<P>, ContractError> {
        let provider = self.provider.as_midnight_provider();

        let zk_keys_dir = self.zk_keys_dir.ok_or_else(|| {
            ContractError::Construction(
                "missing zk_keys, call .with_zk_keys(...) on the builder".into(),
            )
        })?;

        let mut state = self.initial_state.ok_or_else(|| {
            ContractError::Construction(
                "missing initial_state, call .with_initial_state(...) on the builder".into(),
            )
        })?;

        state = with_zk_keys(state, &zk_keys_dir)?;

        // Stamp the maintenance authority committee into the deployed state, if
        // requested. No signing key is stored — members sign ops externally.
        if let Some((committee, threshold)) = self.maintenance_authority {
            crate::maintenance::validate_committee(&committee, threshold)?;
            state = crate::maintenance::set_maintenance_authority(state, committee, threshold);
        }

        let result = deploy_funded(
            &state,
            provider,
            &zk_keys_dir,
            &self.prover,
            self.shielded_offer,
        )
        .await?;
        let address = result.address_hex();
        let pending = provider.submit(&result.tx_bytes).await?;

        Ok(PendingDeploy {
            pending,
            address,
            zk_keys_dir,
            prover: self.prover,
            provider: self.provider,
            deploy_timeout: self.deploy_timeout,
            deploy_poll_interval: self.deploy_poll_interval,
        })
    }
}

impl<P> IntoFuture for DeployBuilder<P>
where
    P: AsMidnightProvider + Provider + Send + 'static,
{
    type Output = Result<Contract<P>, ContractError>;
    // `Pin<Box<dyn Future>>` rather than `impl Future` because the latter is
    // still unstable in associated type position (rust-lang/rust#63063).
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let pending = self.send().await?;
            let (_, pending) = pending.wait_best().await?;
            pending.into_contract().await
        })
    }
}

// ---------------------------------------------------------------------------
// PendingDeploy: handle for an in-flight deploy transaction.
// ---------------------------------------------------------------------------

/// Handle to an in-flight deploy. Returned by [`DeployBuilder::send`].
///
/// Provides access to the watch stream so you can observe inclusion in the
/// best block (`wait_best`) and finalization (`wait_finalized`) before
/// promoting it to a [`Contract`] via [`PendingDeploy::into_contract`] (which
/// waits for the indexer).
pub struct PendingDeploy<P> {
    pending: PendingTx,
    address: String,
    zk_keys_dir: PathBuf,
    prover: Prover,
    provider: P,
    deploy_timeout: Duration,
    deploy_poll_interval: Duration,
}

impl<P> PendingDeploy<P> {
    /// The contract address the deploy will produce.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The hash of the submitted extrinsic.
    pub fn extrinsic_hash(&self) -> [u8; 32] {
        self.pending.extrinsic_hash()
    }

    /// The extrinsic hash formatted as a hex string (no `0x` prefix, matching
    /// the convention used by [`Contract::address`]).
    pub fn extrinsic_hash_hex(&self) -> String {
        self.pending.extrinsic_hash_hex()
    }

    /// Wait until the deploy transaction lands in the best block.
    ///
    /// Consumes `self` and returns it alongside the inclusion details so
    /// callers can chain a subsequent `wait_finalized` or `into_contract`
    /// without `let mut`. See [`PendingTx::wait_best`] for caveats around
    /// re-orgs and call ordering.
    pub async fn wait_best(mut self) -> Result<(TxInBlock, Self), ContractError> {
        let (in_block, pending) = self.pending.wait_best().await?;
        self.pending = pending;
        Ok((in_block, self))
    }

    /// Wait until the deploy transaction is in a finalized block.
    ///
    /// Consumes `self` and returns it back. May be called without a prior
    /// `wait_best`; the best-block status is then skipped.
    pub async fn wait_finalized(mut self) -> Result<(TxInBlock, Self), ContractError> {
        let (in_block, pending) = self.pending.wait_finalized().await?;
        self.pending = pending;
        Ok((in_block, self))
    }
}

impl<P> PendingDeploy<P>
where
    P: AsMidnightProvider + Provider + Send,
{
    /// Wait for the indexer to surface the deployed contract and return the
    /// [`Contract`] handle.
    pub async fn into_contract(self) -> Result<Contract<P>, ContractError> {
        wait_for_deployment(
            &self.provider,
            &self.address,
            self.deploy_timeout,
            self.deploy_poll_interval,
        )
        .await?;

        Ok(Contract {
            address: self.address,
            zk_keys_dir: Some(self.zk_keys_dir),
            prover: self.prover,
            provider: self.provider,
            at_block: None,
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
    at_block: Option<BlockRef>,
}

impl<P> ConnectBuilder<P> {
    pub(crate) fn new(provider: P, address: impl Into<String>) -> Self {
        Self {
            provider,
            address: address.into(),
            zk_keys_dir: None,
            prover: Prover::default(),
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

    /// Build the contract handle.
    ///
    /// This is synchronous. No network calls are made.
    pub fn build(self) -> Contract<P>
    where
        P: AsMidnightProvider,
    {
        Contract {
            address: self.address,
            zk_keys_dir: self.zk_keys_dir,
            prover: self.prover,
            provider: self.provider,
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
    zk_keys_dir: Option<PathBuf>,
    prover: Prover,
    provider: P,
    /// Optional block pin for queries. `None` means latest.
    at_block: Option<BlockRef>,
}

impl<P: Clone> Clone for Contract<P> {
    fn clone(&self) -> Self {
        Self {
            address: self.address.clone(),
            zk_keys_dir: self.zk_keys_dir.clone(),
            prover: self.prover.clone(),
            provider: self.provider.clone(),
            at_block: self.at_block.clone(),
        }
    }
}

impl<P> std::fmt::Debug for Contract<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contract")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl Contract<()> {
    /// Start building a deployment for this contract.
    ///
    /// `provider` can be an owned or borrowed `MidnightProvider`. The provider
    /// must have a synced wallet attached via `MidnightProvider::with_wallet`.
    pub fn deploy<P>(provider: P) -> DeployBuilder<P>
    where
        P: AsMidnightProvider + Provider,
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

    /// Reference to the provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// The block pin for queries. `None` means latest.
    pub fn at_block(&self) -> Option<&BlockRef> {
        self.at_block.as_ref()
    }

    /// The proving backend configured for this handle.
    pub(crate) fn prover(&self) -> &Prover {
        &self.prover
    }

    /// Maintenance / governance operations for this contract (verifier-key
    /// rotation, authority replacement). See [`crate::maintenance`].
    ///
    /// Operations are signed externally by the committee members set at deploy
    /// via [`DeployBuilder::with_maintenance_authority`]; the SDK holds no key.
    /// Use [`Self::maintenance_authority`] to read the current committee.
    pub fn maintenance(&self) -> crate::maintenance::ContractMaintenance<'_, P>
    where
        P: AsMidnightProvider,
    {
        crate::maintenance::ContractMaintenance::new(self)
    }

    /// Read the contract's current maintenance authority (committee, threshold,
    /// and counter) from on-chain state.
    ///
    /// Use it to find your position in the committee — the index you sign at
    /// when calling [`PreparedMaintenance::add_signature`](crate::PreparedMaintenance::add_signature):
    ///
    /// ```rust,ignore
    /// let authority = contract.maintenance_authority().await?;
    /// let my_index = authority
    ///     .committee
    ///     .iter()
    ///     .position(|vk| *vk == my_key.verifying_key());
    /// ```
    pub async fn maintenance_authority(
        &self,
    ) -> Result<midnight_bindgen_runtime::ContractMaintenanceAuthority, ContractError>
    where
        P: AsMidnightProvider,
    {
        Ok(self.fetch_state().await?.maintenance_authority)
    }

    /// Fetch the contract's `ContractState`, honoring the handle's `at_block`
    /// pin (latest when unpinned). Mirrors the fetch logic of the circuit-call
    /// path: hash pins and latest go through the node RPC; height pins go
    /// through the indexer.
    async fn fetch_state(&self) -> Result<ContractState<InMemoryDB>, ContractError>
    where
        P: AsMidnightProvider,
    {
        let provider = self.provider.as_midnight_provider();
        match self.at_block.as_ref() {
            Some(BlockRef::Hash(h)) => {
                crate::state::fetch_state_from_node(provider, &self.address, Some(h.as_str())).await
            }
            Some(block_ref) => {
                let offset = block_ref.to_contract_action_offset();
                crate::state::fetch_state_at(&self.provider, &self.address, Some(offset)).await
            }
            None => crate::state::fetch_state_from_node(provider, &self.address, None).await,
        }
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
        let address = crate::address::parse_address(&self.address)?;

        let zk_keys_dir = self.zk_keys_dir.as_deref().ok_or_else(|| {
            ContractError::Construction(
                "no zk_keys configured, call .with_zk_keys(...) on the builder".into(),
            )
        })?;

        // Fetch fresh state, using the node RPC for hash-pinned or latest,
        // and the indexer for height-pinned queries.
        let state = match self.at_block.as_ref() {
            Some(BlockRef::Hash(h)) => {
                crate::state::fetch_state_from_node(provider, &self.address, Some(h.as_str()))
                    .await?
            }
            Some(block_ref) => {
                let offset = block_ref.to_contract_action_offset();
                crate::state::fetch_state_at(&self.provider, &self.address, Some(offset)).await?
            }
            None => crate::state::fetch_state_from_node(provider, &self.address, None).await?,
        };

        // Load the journal head as the witness baseline; capture its
        // extrinsic_hash so the snapshot we write below can record the
        // dependency. With no provider attached the buffer is just empty.
        let ps_store = provider.private_state();
        let (baseline, depends_on) = match &ps_store {
            Some(store) => (
                store.head(&self.address).await?.unwrap_or_default(),
                store.head_extrinsic(&self.address).await?,
            ),
            None => (Vec::new(), None),
        };

        let mut private_state = baseline.clone();
        let mut witness_ctx = crate::interpreter::WitnessContext::new(&mut private_state);

        let (tx_bytes, _new_state, result) = crate::call::call_funded_with(
            ir,
            &state,
            circuit_name,
            address,
            provider,
            zk_keys_dir,
            &self.prover,
            args,
            witnesses,
            Some(&mut witness_ctx),
            helpers,
            structs,
            enums,
        )
        .await?;

        // Submit, then record a pending snapshot keyed by the producing tx's
        // extrinsic_hash *before* we await finalization. If this process dies
        // mid-wait, the pending snapshot survives on disk; the caller can
        // reconcile it against the chain manually via `confirm` /
        // `mark_failed` / `rollback_from` (we don't drive that reconciliation
        // automatically on the next call yet).
        let pending = provider.submit(&tx_bytes).await?;
        let extrinsic_hash = pending.extrinsic_hash();

        if let Some(store) = &ps_store {
            match private_state_persist(&baseline, &private_state) {
                PrivateStatePersist::Unchanged => {}
                _ => {
                    store
                        .append_pending(&self.address, extrinsic_hash, depends_on, &private_state)
                        .await?;
                }
            }
        }

        // Wait for the chain to finalize the tx. Past finality, the block it
        // landed in cannot be reorged out under honest-majority assumptions,
        // so confirming the snapshot here is durable.
        let (in_block, _pending) = pending.wait_finalized().await?;

        if let Some(store) = &ps_store {
            // We only know the block hash from subxt; the block height is
            // metadata used for human inspection and isn't load-bearing for
            // recovery (the block_hash uniquely identifies the block). We
            // record 0 as a sentinel; future work can fill it in.
            //
            // Optimistic-confirm: the chain may have reported `PartialSuccess`
            // / `Failure` for the fallible phase, which this code path does
            // not yet detect (would need node event parsing). Callers can
            // discover failure out-of-band and invoke
            // `PrivateStateProvider::mark_failed(address, extrinsic_hash)` to
            // cascade-roll back this and any dependent snapshots. See
            // `docs/private-state.md`.
            match private_state_persist(&baseline, &private_state) {
                PrivateStatePersist::Unchanged => {}
                _ => {
                    store
                        .confirm(&self.address, extrinsic_hash, 0, in_block.block_hash)
                        .await?;
                }
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use midnight_provider::{ContractActionOffset, ProviderError, StateQuery, StateQueryResult};

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
        async fn get_contract_state(
            &self,
            _address: &str,
            _offset: Option<ContractActionOffset>,
        ) -> Result<Option<String>, ProviderError> {
            Ok(None)
        }
        async fn get_latest_contract_block_height(
            &self,
            _address: &str,
        ) -> Result<Option<i64>, ProviderError> {
            Ok(None)
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

    #[test]
    fn private_state_persist_decision() {
        use PrivateStatePersist::*;
        // Unchanged: a witness didn't touch the state (incl. the stateless
        // empty == empty case) — nothing is written.
        assert_eq!(private_state_persist(b"abc", b"abc"), Unchanged);
        assert_eq!(private_state_persist(b"", b""), Unchanged);
        // Store: a witness produced new non-empty state.
        assert_eq!(private_state_persist(b"", b"new"), Store);
        assert_eq!(private_state_persist(b"old", b"new"), Store);
        // Remove: a witness cleared previously non-empty state to empty, so the
        // stale value is removed rather than left on disk.
        assert_eq!(private_state_persist(b"old", b""), Remove);
    }
}
