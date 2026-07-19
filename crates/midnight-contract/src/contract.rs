use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use midnight_base_crypto::signatures::VerifyingKey;
use midnight_provider::{MidnightProvider, NodeBlockHash, Provider};
use midnight_typed_state::{ContractState, InMemoryDB};

use crate::address::IntoAddress;
use crate::deploy::{deploy_funded, wait_for_deployment};
use crate::error::ContractError;
use crate::state::populate_verifier_keys;
use crate::zk_config::{IntoZkConfig, ZkConfigProvider};
use midnight_provider::{PendingTx, TxInBlock};

/// What to do with a contract's private state after a call, comparing the
/// post-call buffer against the pre-call `baseline`.
///
/// In the single-slot world this had three variants (Unchanged / Store /
/// Remove). The journal model collapses Store and Remove into one `Persist`
/// arm: both record the post-call buffer as a new snapshot, and an empty
/// buffer is a legitimate (but distinguishable) journal state. The old
/// `Remove` variant claimed to drop the slot, which the journal can't
/// honour without breaking lineage.
#[derive(Debug, PartialEq, Eq)]
enum PrivateStatePersist {
    /// Witnesses didn't change it. No journal write.
    Unchanged,
    /// Witnesses produced a new buffer (empty or not). Append a snapshot.
    Persist,
}

fn private_state_persist(baseline: &[u8], post_call: &[u8]) -> PrivateStatePersist {
    if post_call == baseline {
        PrivateStatePersist::Unchanged
    } else {
        PrivateStatePersist::Persist
    }
}

/// How long to wait for the chain to finalize a submitted tx before treating
/// it as a stalled submission. Restores the bound the deleted
/// `wait_for_contract_update` used to enforce; `wait_finalized` itself has
/// no internal timeout, so without this wrap a stalled grandpa would block
/// the caller indefinitely.
const DEFAULT_TX_FINALIZE_TIMEOUT: Duration = Duration::from_secs(60);

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
///     .with_zk_config("compiled")
///     .await?;
/// ```
pub struct DeployBuilder<P> {
    provider: P,
    initial_state: Option<ContractState<InMemoryDB>>,
    zk_config: Option<Arc<dyn ZkConfigProvider>>,
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
            zk_config: None,
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

    /// Set the source of the contract's compiled ZK artifacts (prover/verifier
    /// keys, ZKIR). Accepts a compiled-contract directory path (`"compiled"`, a
    /// `PathBuf`, …) or any custom [`ZkConfigProvider`] wrapped in an `Arc` — see
    /// [`IntoZkConfig`]. Required for deployment and on-chain circuit calls.
    pub fn with_zk_config(mut self, zk_config: impl IntoZkConfig) -> Self {
        self.zk_config = Some(zk_config.into_zk_config());
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

        let zk_config = self.zk_config.ok_or_else(|| {
            ContractError::Construction(
                "missing zk config, call .with_zk_config(...) on the builder".into(),
            )
        })?;

        let mut state = self.initial_state.ok_or_else(|| {
            ContractError::Construction(
                "missing initial_state, call .with_initial_state(...) on the builder".into(),
            )
        })?;

        state = populate_verifier_keys(state, zk_config.as_ref())?;

        // Stamp the maintenance authority committee into the deployed state, if
        // requested. No signing key is stored — members sign ops externally.
        if let Some((committee, threshold)) = self.maintenance_authority {
            crate::maintenance::validate_committee(&committee, threshold)?;
            state = crate::maintenance::set_maintenance_authority(state, committee, threshold);
        }

        let result =
            deploy_funded(&state, provider, zk_config.clone(), self.shielded_offer).await?;
        let address = result.address_hex();
        let pending = provider.submit(&result.tx_bytes).await?;

        Ok(PendingDeploy {
            pending,
            address,
            zk_config,
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
            let (in_block, pending) = pending.wait_best().await?;
            // Surface a failed deploy as a typed `TransactionFailed` instead
            // of letting `into_contract` poll the indexer fruitlessly until
            // it times out: if the chain rejected the deploy the contract
            // never appears on-chain.
            check_verdict(&in_block)?;
            pending.into_contract().await
        })
    }
}

/// Map a transaction's chain verdict to an error when the chain didn't apply
/// it. Lets the deploy and maintenance flows fail fast and typed (mirroring
/// the branch [`Contract::call_with`] does on its own verdict) instead of a
/// confusing downstream timeout.
pub(crate) fn check_verdict(in_block: &TxInBlock) -> Result<(), ContractError> {
    match in_block.verdict {
        midnight_provider::Verdict::Success => Ok(()),
        verdict @ (midnight_provider::Verdict::PartialSuccess
        | midnight_provider::Verdict::Failure) => {
            let status = match verdict {
                midnight_provider::Verdict::PartialSuccess => "PartialSuccess",
                midnight_provider::Verdict::Failure => "Failure",
                midnight_provider::Verdict::Success => unreachable!(),
            };
            Err(ContractError::TransactionFailed {
                extrinsic_hash: in_block.extrinsic_hash,
                status: status.to_string(),
            })
        }
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
    zk_config: Arc<dyn ZkConfigProvider>,
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
            zk_config: Some(self.zk_config),
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
///     .with_zk_config("compiled")
///     .build();
/// ```
pub struct ConnectBuilder<P> {
    provider: P,
    address: String,
    zk_config: Option<Arc<dyn ZkConfigProvider>>,
    at_block: Option<NodeBlockHash>,
}

impl<P> ConnectBuilder<P> {
    pub(crate) fn new(provider: P, address: impl IntoAddress) -> Self {
        Self {
            provider,
            address: address.into_address_string(),
            zk_config: None,
            at_block: None,
        }
    }

    /// Set the source of the contract's compiled ZK artifacts (prover/verifier
    /// keys, ZKIR). Accepts a compiled-contract directory path (`"compiled"`, a
    /// `PathBuf`, …) or any custom [`ZkConfigProvider`] wrapped in an `Arc` — see
    /// [`IntoZkConfig`]. Required for on-chain circuit calls after connecting.
    pub fn with_zk_config(mut self, zk_config: impl IntoZkConfig) -> Self {
        self.zk_config = Some(zk_config.into_zk_config());
        self
    }

    /// Pin queries to the block `hash`. Default is latest. Both circuit
    /// calls and lazy ledger queries honour the pin through the node RPC.
    pub fn at_block(mut self, hash: NodeBlockHash) -> Self {
        self.at_block = Some(hash);
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
            zk_config: self.zk_config,
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
    zk_config: Option<Arc<dyn ZkConfigProvider>>,
    provider: P,
    /// Optional block pin for queries. `None` means latest.
    at_block: Option<NodeBlockHash>,
}

impl<P: Clone> Clone for Contract<P> {
    fn clone(&self) -> Self {
        Self {
            address: self.address.clone(),
            zk_config: self.zk_config.clone(),
            provider: self.provider.clone(),
            at_block: self.at_block,
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

    /// Create a handle for an already-deployed contract at the given address:
    /// a hex string or a typed [`ContractAddress`](crate::ContractAddress)
    /// (see [`IntoAddress`]).
    ///
    /// This is synchronous, no network calls are made. Use `deploy()` to
    /// deploy a new contract.
    ///
    /// `provider` can be an owned or borrowed `MidnightProvider`.
    pub fn at<P>(provider: P, address: impl IntoAddress) -> ConnectBuilder<P>
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
    pub fn at_block(&self) -> Option<NodeBlockHash> {
        self.at_block
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
    ) -> Result<midnight_typed_state::ContractMaintenanceAuthority, ContractError>
    where
        P: AsMidnightProvider,
    {
        Ok(self.fetch_state().await?.maintenance_authority)
    }

    /// Fetch the contract's `ContractState`, honoring the handle's `at_block`
    /// pin (latest when unpinned), through the node RPC.
    async fn fetch_state(&self) -> Result<ContractState<InMemoryDB>, ContractError>
    where
        P: AsMidnightProvider,
    {
        let provider = self.provider.as_midnight_provider();
        crate::state::fetch_state_from_node(provider, &self.address, self.at_block).await
    }

    /// Execute a circuit call on-chain.
    ///
    /// Fetches fresh state from the node RPC (pinned when `at_block` is
    /// set), runs the circuit IR locally, builds a funded transaction,
    /// and submits it to the node.
    pub async fn call(
        &self,
        ir: &compact_codegen::ir::CircuitIrBody,
        circuit_name: &str,
    ) -> Result<Option<crate::runtime::Value>, ContractError>
    where
        P: AsMidnightProvider,
    {
        self.call_with(
            ir,
            circuit_name,
            &[],
            &crate::runtime::NoWitnesses,
            crate::call::CircuitDefs::default(),
            &[],
            crate::call::ShieldedInputs::default(),
        )
        .await
    }

    /// Build and prove a circuit call transaction, returning its tagged-serialized
    /// proven bytes **without submitting**.
    ///
    /// Use this to obtain a contract call as a transaction you combine with
    /// others before submitting: e.g. merge a counterparty's proven transaction
    /// via [`MidnightProvider::merge_transactions`](midnight_provider::MidnightProvider::merge_transactions),
    /// then [`submit`](midnight_provider::MidnightProvider::submit) the result.
    /// It is the build-only mirror of [`Self::call_with`], which builds and
    /// submits in one step.
    ///
    /// Because nothing is submitted, the post-call private state is **not**
    /// journaled: for a private-state contract the caller owns persisting it.
    /// The dust UTXOs the build selected are reserved on the wallet (as with
    /// `call_with`), since the transaction is expected to be submitted.
    #[allow(clippy::too_many_arguments)]
    pub async fn build_call_with(
        &self,
        ir: &compact_codegen::ir::CircuitIrBody,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        defs: crate::call::CircuitDefs<'_>,
        coin_encryption_keys: &[(
            midnight_helpers::CoinPublicKey,
            midnight_helpers::EncryptionPublicKey,
        )],
        shielded: crate::call::ShieldedInputs,
    ) -> Result<Vec<u8>, ContractError>
    where
        P: AsMidnightProvider,
    {
        let provider: &MidnightProvider = self.provider.as_midnight_provider();
        let address = crate::address::parse_address(&self.address)?;

        let zk_config = self.zk_config.clone().ok_or_else(|| {
            ContractError::Construction(
                "no zk config, call .with_zk_config(...) on the builder".into(),
            )
        })?;

        let state =
            crate::state::fetch_state_from_node(provider, &self.address, self.at_block).await?;

        // Load the private-state head as the witness baseline (empty if none).
        // Not journaled: this path does not submit, so a private-state
        // contract's post-call buffer is the caller's to persist.
        let baseline = match provider.private_state() {
            Some(store) => store
                .head_with_extrinsic(&self.address)
                .await?
                .map(|(data, _ext)| data)
                .unwrap_or_default(),
            None => Vec::new(),
        };

        let mut private_state = baseline;
        let mut witness_ctx = crate::runtime::WitnessContext::new(&mut private_state);

        let (tx_bytes, _new_state, _result) = crate::call::call_funded_with(
            ir,
            &state,
            circuit_name,
            address,
            provider,
            zk_config,
            args,
            witnesses,
            Some(&mut witness_ctx),
            defs,
            coin_encryption_keys,
            shielded,
        )
        .await?;

        Ok(tx_bytes)
    }

    /// Execute a circuit call on-chain with arguments and witnesses.
    ///
    /// Fetches fresh state from the node RPC (pinned when `at_block` is
    /// set), runs the circuit IR locally, builds a funded transaction,
    /// proves it, and submits to the node. The contract
    /// handle is not mutated.
    #[allow(clippy::too_many_arguments)]
    pub async fn call_with(
        &self,
        ir: &compact_codegen::ir::CircuitIrBody,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        defs: crate::call::CircuitDefs<'_>,
        // `coin_public_key → encryption_public_key` mappings applied to the
        // shielded outputs this circuit creates (mints/sends). For each output
        // whose coin public key is present, the SDK attaches a discovery
        // ciphertext so the recipient's wallet finds the coin through normal
        // sync (no `watchFor`). Pass `&[]` for none.
        coin_encryption_keys: &[(
            midnight_helpers::CoinPublicKey,
            midnight_helpers::EncryptionPublicKey,
        )],
        // Shielded (Zswap) coins/offer to attach, funding a circuit's
        // shielded-token deficit (e.g. `receiveShielded` on the caller's coin)
        // from the caller's wallet. Pass `ShieldedInputs::default()` for none.
        shielded: crate::call::ShieldedInputs,
    ) -> Result<Option<crate::runtime::Value>, ContractError>
    where
        P: AsMidnightProvider,
    {
        let provider: &MidnightProvider = self.provider.as_midnight_provider();
        let address = crate::address::parse_address(&self.address)?;

        let zk_config = self.zk_config.clone().ok_or_else(|| {
            ContractError::Construction(
                "no zk config, call .with_zk_config(...) on the builder".into(),
            )
        })?;

        // Fetch fresh state from the node RPC, pinned when `at_block` is set.
        let state =
            crate::state::fetch_state_from_node(provider, &self.address, self.at_block).await?;

        // Load the journal head as the witness baseline; capture its
        // extrinsic_hash so the snapshot we write below can record the
        // dependency. `head_with_extrinsic` returns both fields from a
        // single underlying read so a concurrent `append_pending` can't
        // produce a torn read where data and extrinsic_hash come from
        // different journal versions. With no provider attached the buffer
        // is just empty.
        let ps_store = provider.private_state();
        let (baseline, depends_on) = match &ps_store {
            Some(store) => match store.head_with_extrinsic(&self.address).await? {
                Some((data, ext)) => (data, Some(ext)),
                None => (Vec::new(), None),
            },
            None => (Vec::new(), None),
        };

        let mut private_state = baseline.clone();
        let mut witness_ctx = crate::runtime::WitnessContext::new(&mut private_state);

        let (tx_bytes, _new_state, result) = crate::call::call_funded_with(
            ir,
            &state,
            circuit_name,
            address,
            provider,
            zk_config,
            args,
            witnesses,
            Some(&mut witness_ctx),
            defs,
            coin_encryption_keys,
            shielded,
        )
        .await?;

        // Prepare the tx (build + validate against the node) so its
        // extrinsic_hash is known, then record the pending snapshot keyed by
        // that hash *before* submitting. Recording first closes the window
        // where a crash between submit and append would leave the tx on the
        // wire with no journal entry, so the next call would build on a stale
        // baseline. The trade is benign: if the process dies after the append
        // but before submit, the tx never reached the mempool, leaving a
        // provisional pending entry that reconciliation resolves; and if
        // submit itself fails we roll the entry back below.
        let prepared = provider.prepare(&tx_bytes).await?;
        let extrinsic_hash = prepared.extrinsic_hash();
        let persist = private_state_persist(&baseline, &private_state);
        // True iff we successfully recorded a pending snapshot for this
        // tx. Used below to phrase error messages correctly: if no
        // snapshot was written (no provider attached, or witnesses left
        // state unchanged) the caller shouldn't be told to reconcile one
        // that doesn't exist.
        let mut pending_snapshot_written = false;

        if let Some(store) = &ps_store {
            // Explicit match so any future `PrivateStatePersist` variant
            // fails to compile here and forces a deliberate decision.
            match persist {
                PrivateStatePersist::Unchanged => {}
                PrivateStatePersist::Persist => {
                    // If `append_pending` fails (e.g. SnapshotAlreadyExists
                    // from a retry, a JournalConflict from a concurrent
                    // call, or an InvalidFormat journal) the tx has NOT been
                    // submitted yet, so we surface the error and stop before
                    // anything hits the wire.
                    store
                        .append_pending(&self.address, extrinsic_hash, depends_on, &private_state)
                        .await
                        .map_err(|e| ContractError::PendingSnapshotFailed {
                            extrinsic_hash: hex::encode(extrinsic_hash),
                            source: e,
                        })?;
                    pending_snapshot_written = true;
                }
            }
        }

        // Submit now that the journal record (if any) is durable. If submit
        // fails the tx never reached the mempool, so roll back the
        // speculative pending entry to keep the journal consistent.
        let pending = match prepared.submit().await {
            Ok(pending) => pending,
            Err(e) => {
                if pending_snapshot_written && let Some(store) = &ps_store {
                    // Best-effort: the tx didn't go out, so dropping the
                    // entry restores the pre-call leaf. A failure here just
                    // leaves a provisional entry that reconciliation handles.
                    let _ = store.mark_failed(&self.address, extrinsic_hash).await;
                }
                return Err(e.into());
            }
        };

        // Wait for the chain to finalize the tx, bounded by
        // `DEFAULT_TX_FINALIZE_TIMEOUT`. Past finality the block can't be
        // reorged out under honest-majority assumptions, so confirming the
        // snapshot here is durable.
        //
        // On timeout or error we deliberately do NOT auto-cleanup the
        // pending snapshot. Per `PendingTx` docs, cancelling the wait
        // (timeout / drop / error) does not retract the tx from the
        // mempool: the node may still include it in a later block. The
        // pending snapshot is the only local record needed to reconcile
        // when it does, so deleting it on timeout would silently lose
        // state for any tx that eventually lands. Wait failures surface as
        // `ContractError::SubmissionWait` (carrying the extrinsic_hash and
        // the typed provider error, i.e. the `SubmitError` kind) and
        // timeouts as `ContractError::FinalizeTimeout`, so the caller can
        // tell a definitive rejection (`Invalid` — `mark_failed` is safe)
        // from an ambiguous drop or timeout, then query the chain and
        // invoke `confirm` (it landed) or `mark_failed` (it didn't).
        let wait_result =
            tokio::time::timeout(DEFAULT_TX_FINALIZE_TIMEOUT, pending.wait_finalized()).await;
        // Both error variants carry `snapshot_written` so their Display only
        // tells the caller to reconcile a pending snapshot when one was
        // actually recorded above. Avoids telling a caller without a
        // provider (or with an Unchanged call) to reconcile a snapshot that
        // doesn't exist.
        let in_block = match wait_result {
            Ok(Ok((in_block, _pending))) => in_block,
            Ok(Err(e)) => {
                return Err(ContractError::SubmissionWait {
                    extrinsic_hash: hex::encode(extrinsic_hash),
                    source: e,
                    snapshot_written: pending_snapshot_written,
                });
            }
            Err(_elapsed) => {
                return Err(ContractError::FinalizeTimeout {
                    extrinsic_hash: hex::encode(extrinsic_hash),
                    timeout: DEFAULT_TX_FINALIZE_TIMEOUT,
                    snapshot_written: pending_snapshot_written,
                });
            }
        };

        // Branch on the chain's verdict for our extrinsic. The Midnight
        // pallet emits `TxApplied` for full success and `TxPartialSuccess`
        // when at least one fallible segment failed; the dispatch erroring
        // entirely surfaces as `System::ExtrinsicFailed`. `call_with`
        // submits a single-contract-action tx, so PartialSuccess and
        // Failure both mean "the contract state did not advance" and route
        // the same way: cascade-drop the pending snapshot (if any) and
        // return a typed `TransactionFailed` error to the caller.
        match in_block.verdict {
            midnight_provider::Verdict::Success => {
                if let Some(store) = &ps_store {
                    match persist {
                        PrivateStatePersist::Unchanged => {}
                        PrivateStatePersist::Persist => {
                            // We only know the block hash from subxt; the
                            // height is human-inspection metadata and isn't
                            // load-bearing for recovery (the block_hash
                            // uniquely identifies the block). `None` makes
                            // "unknown" distinguishable from a genuine
                            // genesis confirmation; a follow-up may fill it
                            // in via a one-shot block query.
                            store
                                .confirm(&self.address, extrinsic_hash, None, in_block.block_hash)
                                .await?;
                        }
                    }
                }
                Ok(result)
            }
            verdict @ (midnight_provider::Verdict::PartialSuccess
            | midnight_provider::Verdict::Failure) => {
                if let Some(store) = &ps_store {
                    if pending_snapshot_written {
                        // Drop the orphan Pending snapshot we wrote above
                        // so the next call's witness baseline is the
                        // last-known-good state, not the post-call buffer
                        // for a tx that the chain rejected. cascade_drop
                        // handles dependents too.
                        store.mark_failed(&self.address, extrinsic_hash).await?;
                    }
                }
                let status = match verdict {
                    midnight_provider::Verdict::PartialSuccess => "PartialSuccess",
                    midnight_provider::Verdict::Failure => "Failure",
                    midnight_provider::Verdict::Success => unreachable!(),
                };
                Err(ContractError::TransactionFailed {
                    extrinsic_hash,
                    status: status.to_string(),
                })
            }
        }
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
    fn at_with_block_hash() {
        let provider = MockProvider::new();
        let hash = NodeBlockHash::repeat_byte(0xab);
        let contract = Contract::at(provider, "addr1").at_block(hash).build();
        assert_eq!(contract.address(), "addr1");
        assert_eq!(contract.at_block(), Some(hash));
    }

    #[test]
    fn private_state_persist_decision() {
        use PrivateStatePersist::*;
        // Unchanged: a witness didn't touch the state (incl. the stateless
        // empty == empty case). Nothing is written.
        assert_eq!(private_state_persist(b"abc", b"abc"), Unchanged);
        assert_eq!(private_state_persist(b"", b""), Unchanged);
        // Persist: a witness produced a different buffer. The journal
        // model records both "new non-empty" and "cleared to empty" as
        // snapshots so lineage stays intact; consumers that need to
        // distinguish "cleared" from "never written" check whether the
        // head snapshot's data is empty.
        assert_eq!(private_state_persist(b"", b"new"), Persist);
        assert_eq!(private_state_persist(b"old", b"new"), Persist);
        assert_eq!(private_state_persist(b"old", b""), Persist);
    }

    fn in_block(verdict: midnight_provider::Verdict) -> TxInBlock {
        TxInBlock {
            block_hash: [1u8; 32],
            extrinsic_hash: [2u8; 32],
            verdict,
        }
    }

    #[test]
    fn check_verdict_passes_on_success() {
        assert!(check_verdict(&in_block(midnight_provider::Verdict::Success)).is_ok());
    }

    #[test]
    fn check_verdict_fails_on_partial_success_and_failure() {
        let err = check_verdict(&in_block(midnight_provider::Verdict::PartialSuccess)).unwrap_err();
        assert!(
            matches!(err, ContractError::TransactionFailed { ref status, extrinsic_hash }
                if status == "PartialSuccess" && extrinsic_hash == [2u8; 32]),
            "got {err:?}"
        );
        let err = check_verdict(&in_block(midnight_provider::Verdict::Failure)).unwrap_err();
        assert!(
            matches!(err, ContractError::TransactionFailed { ref status, .. } if status == "Failure"),
            "got {err:?}"
        );
    }
}
