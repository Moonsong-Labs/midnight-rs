use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use midnight_base_crypto::signatures::VerifyingKey;
use midnight_provider::MidnightProvider;
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::{BlockHash, ExtrinsicHash, TransferResult};

use crate::address::IntoContractAddress;
use crate::deploy::{deploy_funded, deployed_state};
use crate::error::ContractError;
use crate::state::populate_verifier_keys;
use crate::zk_config::{IntoZkConfig, ZkConfigProvider};
use midnight_provider::{
    PendingTx, PreparedTx, PrivateStateProvider, ProviderError, TransactionHash, TxInBlock,
};

/// A circuit call that landed on chain: the circuit's own result and events,
/// plus the identity of the transaction that carried it.
///
/// The identity is what lets a caller log the call, link to it in an explorer,
/// or reconcile it against their own records. Deploys have always exposed this
/// through `PendingDeploy`; calls used to compute it, branch on it, and throw
/// it away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOutcome<T> {
    /// The circuit's return value.
    pub value: T,
    /// Hash of the extrinsic that carried the call.
    pub extrinsic_hash: ExtrinsicHash,
    /// Hash of the Midnight transaction the extrinsic carried, the identity
    /// the indexer keys on. See [`PendingTx::transaction_hash`].
    pub transaction_hash: TransactionHash,
    /// Hash of the block it landed in.
    pub block_hash: BlockHash,
    /// The events that the circuit emitted, in emission order. They come from
    /// the local run of the call, and the chain logs the same events. See
    /// [`ExecutionResult::logs`](crate::runtime::ExecutionResult::logs).
    pub logs: Vec<crate::runtime::VersionedLogItem<InMemoryDB>>,
}

impl<T> CallOutcome<T> {
    /// Replace the circuit result, keeping the transaction identity and the
    /// events.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> CallOutcome<U> {
        CallOutcome {
            value: f(self.value),
            extrinsic_hash: self.extrinsic_hash,
            transaction_hash: self.transaction_hash,
            block_hash: self.block_hash,
            logs: self.logs,
        }
    }
}

/// The result of a circuit call that ran with no proof, fee or transaction.
///
/// It holds the circuit's value, the ledger after the call and the events that
/// the circuit emitted, from a run on the contract's state at one block.
/// [`Contract::simulate_with`] and the `simulate` method of a generated call
/// builder return it. The result is true for the block that the run read. A
/// later block can change the outcome of the real call.
#[derive(Debug, Clone)]
pub struct Simulated<T, L> {
    /// The circuit's return value.
    pub value: T,
    /// The contract's ledger state after the call.
    pub ledger: L,
    /// The block whose state and time the run read.
    pub block_hash: BlockHash,
    /// The events that the circuit emitted, in emission order. See
    /// [`ExecutionResult::logs`](crate::runtime::ExecutionResult::logs).
    pub logs: Vec<crate::runtime::VersionedLogItem<InMemoryDB>>,
}

/// A circuit call that ran locally, before the build of its transaction
/// reserves an input or makes a proof.
struct RanCall {
    /// The contract's state and the block time that the run read.
    state: crate::state::StateAtBlock,
    /// The contract's address.
    address: midnight_types::ContractAddress,
    /// The source of the keys that the build proves with.
    zk_config: Arc<dyn ZkConfigProvider>,
    /// The result of the run.
    exec_result: crate::runtime::ExecutionResult,
    /// The private state of the run.
    private_state: PrivateStateChange,
}

/// A circuit call built and proven, before it is submitted.
struct BuiltCall {
    /// The proven transaction, and the wallet inputs that its build reserved.
    transaction: TransferResult,
    /// The private state of the call.
    private_state: PrivateStateChange,
}

/// The private state of a call, before and after its witnesses ran.
struct PrivateStateChange {
    /// The private state the witnesses started from.
    baseline: Vec<u8>,
    /// The private state after the witnesses ran.
    after: Vec<u8>,
    /// The extrinsic that wrote `baseline`, when the journal has a head.
    depends_on: Option<[u8; 32]>,
}

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

/// Refuse to build a call for a contract with witnesses when no private-state
/// store is attached.
///
/// Without a store the witness baseline is an empty buffer and the post-call
/// buffer is discarded, so the call still succeeds and proves while the private
/// state silently resets to its default on every call. Failing at build time
/// mirrors midnight-js, which rejects the same configuration before building
/// anything.
fn require_private_state_for_witnesses(
    declares_witnesses: bool,
    has_store: bool,
) -> Result<(), ContractError> {
    if declares_witnesses && !has_store {
        return Err(ContractError::Construction(
            "this contract declares witnesses, which read and write private state, but no \
             private-state provider is attached: call `MidnightProvider::with_private_state(...)`. \
             Without one every call would silently start from the default private state."
                .into(),
        ));
    }
    Ok(())
}

/// The error for a deploy or a call with no zk config.
fn missing_zk_config() -> ContractError {
    ContractError::Construction(
        "missing zk config, call .with_zk_config(...) on the builder".into(),
    )
}

fn private_state_persist(baseline: &[u8], post_call: &[u8]) -> PrivateStatePersist {
    if post_call == baseline {
        PrivateStatePersist::Unchanged
    } else {
        PrivateStatePersist::Persist
    }
}

/// Settle a call from the result of its finality wait: return its
/// [`TxInBlock`] when the chain applied it, and an error when it did not.
///
/// `store` holds the call's pending snapshot under `extrinsic_hash`, or is
/// `None` when the call recorded none. An applied call confirms the snapshot.
/// A call that did not apply drops it with `mark_failed` before
/// [`ContractError::TransactionFailed`] returns, so the orphan snapshot never
/// becomes the next call's witness baseline. A wait that fails with a
/// [`SubmitError`](midnight_provider::SubmitError), the only other failure of
/// [`PendingTx::wait_finalized`], returns [`ContractError::SubmissionWait`] and
/// keeps the snapshot for the caller to reconcile (see that variant).
async fn settle_call(
    store: Option<&dyn PrivateStateProvider>,
    address: &str,
    extrinsic_hash: ExtrinsicHash,
    transaction_hash: TransactionHash,
    waited: Result<TxInBlock, ProviderError>,
) -> Result<TxInBlock, ContractError> {
    match waited {
        Ok(applied) => {
            if let Some(store) = store {
                // No height: subxt reports only the block hash, which alone
                // identifies the block.
                store
                    .confirm(address, extrinsic_hash.0, None, applied.block_hash.0)
                    .await?;
            }
            Ok(applied)
        }
        Err(error @ ProviderError::NotApplied(_)) => {
            if let Some(store) = store {
                store.mark_failed(address, extrinsic_hash.0).await?;
            }
            Err(error.into())
        }
        Err(ProviderError::Submission(source)) => Err(ContractError::SubmissionWait {
            transaction_hash,
            extrinsic_hash,
            source,
            snapshot_written: store.is_some(),
        }),
        Err(other) => Err(other.into()),
    }
}

/// Record a call's `Pending` snapshot when its witnesses changed the private
/// state, then submit the call.
///
/// Returns the submitted transaction, and `store` when it holds the snapshot.
/// A failed record hands the inputs of `prepared` back before it returns.
async fn record_and_submit(
    address: &str,
    prepared: PreparedTx,
    private_state: PrivateStateChange,
    store: Option<Arc<dyn PrivateStateProvider>>,
) -> Result<(PendingTx, Option<Arc<dyn PrivateStateProvider>>), ContractError> {
    let extrinsic_hash = prepared.extrinsic_hash();
    let transaction_hash = prepared.transaction_hash();
    let persist = private_state_persist(&private_state.baseline, &private_state.after);
    // Without a snapshot (no store, or the witnesses left the state
    // unchanged), the call has nothing to settle, and no error tells the
    // caller to reconcile one.
    let mut pending_snapshot_written = false;

    if let Some(store) = &store {
        // Explicit match so any future `PrivateStatePersist` variant
        // fails to compile here and forces a deliberate decision.
        match persist {
            PrivateStatePersist::Unchanged => {}
            PrivateStatePersist::Persist => {
                // If `append_pending` fails (e.g. SnapshotAlreadyExists
                // from a retry, a JournalConflict from a concurrent
                // call, or an InvalidFormat journal) the tx has NOT been
                // submitted yet, so we hand its inputs back, surface the
                // error and stop before anything hits the wire.
                let appended = store
                    .append_pending(
                        address,
                        extrinsic_hash.0,
                        private_state.depends_on,
                        &private_state.after,
                    )
                    .await;
                if let Err(source) = appended {
                    prepared.release().await;
                    return Err(ContractError::PendingSnapshotFailed {
                        extrinsic_hash,
                        source,
                    });
                }
                pending_snapshot_written = true;
            }
        }
    }

    // Keep the snapshot on a failed submit: a `SubmitRpc` does not say
    // whether the node holds the tx.
    let pending = match prepared.submit().await {
        Ok(pending) => pending,
        Err(ProviderError::Submission(source)) => {
            return Err(ContractError::SubmissionWait {
                transaction_hash,
                extrinsic_hash,
                source,
                snapshot_written: pending_snapshot_written,
            });
        }
        Err(other) => return Err(other.into()),
    };
    Ok((pending, store.filter(|_| pending_snapshot_written)))
}

/// How long to wait for the chain to finalize a submitted tx before treating
/// it as a stalled submission. Restores the bound the deleted
/// `wait_for_contract_update` used to enforce; `wait_finalized` itself has
/// no internal timeout, so without this wrap a stalled grandpa would block
/// the caller indefinitely.
const DEFAULT_TX_FINALIZE_TIMEOUT: Duration = Duration::from_secs(60);

/// A submitted circuit call, before the chain's verdict.
///
/// [`Contract::send_call_with`] and the `send` method of a generated call
/// builder return it. Use it to read the transaction's hashes before the wait,
/// or to choose the deadline of the wait. Call
/// [`wait_finalized`](Self::wait_finalized) to finish the call.
///
/// A dropped `PendingCall` does not retract the transaction, which can still
/// land. It leaves the call's private-state snapshot `Pending`, as a
/// [`ContractError::FinalizeTimeout`] does, and the recovery is the same.
///
/// Call `wait_finalized` before you start the next call on the same contract.
/// A call that starts before the verdict builds on this call's `Pending`
/// snapshot. A `mark_failed` of this snapshot then also drops the snapshot of
/// the later call. `docs/private-state.md` describes the hazard.
#[must_use = "call `wait_finalized`: a dropped `PendingCall` leaves its private-state snapshot `Pending`"]
pub struct PendingCall<T> {
    pending: PendingTx,
    value: T,
    logs: Vec<crate::runtime::VersionedLogItem<InMemoryDB>>,
    address: String,
    /// The store that holds the call's `Pending` snapshot, or `None` when the
    /// call recorded none.
    snapshot_store: Option<Arc<dyn PrivateStateProvider>>,
}

impl<T> PendingCall<T> {
    /// The hash of the submitted extrinsic. The call's private-state snapshot
    /// uses it as its key.
    pub fn extrinsic_hash(&self) -> ExtrinsicHash {
        self.pending.extrinsic_hash()
    }

    /// The hash of the Midnight transaction the extrinsic carries. See
    /// [`PendingTx::transaction_hash`].
    pub fn transaction_hash(&self) -> TransactionHash {
        self.pending.transaction_hash()
    }

    /// Wait for finality, then settle the call's private-state snapshot.
    ///
    /// The wait ends when the call is in a finalized block. A call that the
    /// chain applied confirms its snapshot. A call that the chain did not apply
    /// drops its snapshot with `mark_failed` before the error returns.
    ///
    /// The SDK sets no deadline on the wait. subxt can end it: see the
    /// [timeouts of `PendingTx`](PendingTx#timeouts-and-cancellation). To bound
    /// the wait, wrap it in [`tokio::time::timeout`]. Read the hashes before
    /// you call it, because a timeout drops the handle. [`Contract::call_with`]
    /// and the `.await` of a generated call builder bound the wait for
    /// finality to 60 s. They settle the snapshot after that wait.
    ///
    /// # Errors
    ///
    /// - [`ContractError::SubmissionWait`] when the wait fails with no verdict.
    ///   The snapshot stays `Pending`.
    /// - [`ContractError::TransactionFailed`] when the chain did not apply the
    ///   call.
    /// - [`ContractError::PrivateState`] when the store cannot confirm or drop
    ///   the snapshot.
    pub async fn wait_finalized(self) -> Result<CallOutcome<T>, ContractError> {
        self.finish(None).await
    }

    /// Wait for finality, bounded by `deadline` when set, then settle. The
    /// settle runs outside the bound, so the bound never cancels a store write.
    async fn finish(self, deadline: Option<Duration>) -> Result<CallOutcome<T>, ContractError> {
        let Self {
            pending,
            value,
            logs,
            address,
            snapshot_store,
        } = self;
        let extrinsic_hash = pending.extrinsic_hash();
        let transaction_hash = pending.transaction_hash();
        let wait = pending.wait_finalized();
        let waited = match deadline {
            None => wait.await,
            Some(timeout) => tokio::time::timeout(timeout, wait)
                .await
                .map_err(|_elapsed| ContractError::FinalizeTimeout {
                    transaction_hash,
                    extrinsic_hash,
                    timeout,
                    snapshot_written: snapshot_store.is_some(),
                })?,
        };
        let in_block = settle_call(
            snapshot_store.as_deref(),
            &address,
            extrinsic_hash,
            transaction_hash,
            waited.map(|(in_block, _pending)| in_block),
        )
        .await?;
        Ok(CallOutcome {
            value,
            extrinsic_hash,
            transaction_hash: in_block.transaction_hash,
            block_hash: in_block.block_hash,
            logs,
        })
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for PendingCall<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingCall")
            .field("address", &self.address)
            .field("pending", &self.pending)
            .field("value", &self.value)
            .field("snapshot_written", &self.snapshot_store.is_some())
            .finish_non_exhaustive()
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
/// ```rust,no_run
/// # mod counter {
/// #     compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
/// # }
/// # async fn deploy(
/// #     provider: midnight_provider::MidnightProvider,
/// # ) -> Result<(), midnight_contract::ContractError> {
/// let contract = counter::Contract::deploy(&provider)
///     .with_initial_state(counter::LedgerInitialState::default())
///     .with_zk_config(counter::ZK_CONFIG_DIR)
///     .await?;
/// # Ok(())
/// # }
/// ```
#[must_use = "does nothing until awaited or sent"]
pub struct DeployBuilder<'a, P> {
    provider: P,
    initial_state: Option<ContractState<InMemoryDB>>,
    zk_config: Option<Arc<dyn ZkConfigProvider>>,
    deploy_timeout: Duration,
    shielded_offer: Option<crate::ShieldedOffer>,
    maintenance_authority: Option<(Vec<VerifyingKey>, u32)>,
    declared_circuits: Option<Vec<String>>,
    declares_witnesses: bool,
    // Ties the awaited deploy to the provider's borrow. `IntoFuture` boxes the
    // deploy as `dyn Future`, and without a lifetime on the builder that box
    // defaults to `'static`, which rules out `Contract::deploy(&provider).await`.
    borrow: PhantomData<&'a ()>,
}

impl<P> DeployBuilder<'_, P> {
    pub(crate) fn new(provider: P) -> Self {
        Self {
            provider,
            initial_state: None,
            zk_config: None,
            deploy_timeout: Duration::from_secs(60),
            shielded_offer: None,
            maintenance_authority: None,
            declared_circuits: None,
            declares_witnesses: false,
            borrow: PhantomData,
        }
    }

    /// Record whether this contract declares witnesses, so calls on the
    /// resulting handle refuse to build without a private-state provider.
    /// Generated contracts set this from the compiled artifact.
    pub fn with_declared_witnesses(mut self, declares_witnesses: bool) -> Self {
        self.declares_witnesses = declares_witnesses;
        self
    }

    /// Declare the circuits this contract defines, so deployment registers
    /// exactly those entry points.
    ///
    /// Generated contracts set this from the compiled artifact. Without it the
    /// deployed operation set is whatever verifier keys happen to sit in the
    /// artifact directory, which makes a stale file a bogus entry point and a
    /// key that failed to build a silently missing one.
    pub fn with_declared_circuits(
        mut self,
        circuits: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.declared_circuits = Some(circuits.into_iter().map(Into::into).collect());
        self
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

    /// Set the deadline of [`PendingDeploy::into_contract`] (default: 60s).
    ///
    /// The deadline starts when `into_contract` starts, after `send` returns.
    /// It bounds the best-block wait that `into_contract` does when no wait ran
    /// before it, and then the read of the deployed contract. An explicit
    /// [`PendingDeploy::wait_best`] or [`PendingDeploy::wait_finalized`] has no
    /// deadline. See [`PendingTx`] to bound one with `tokio::time::timeout`.
    pub fn with_deploy_timeout(mut self, timeout: Duration) -> Self {
        self.deploy_timeout = timeout;
        self
    }

    /// Attach a shielded offer that the deploy transaction carries.
    ///
    /// The deploy spends the offer's coins from the attached wallet and pays
    /// each of its outputs. What the coins of a token hold beyond what the
    /// outputs pay of it goes back to the wallet as one change output. The
    /// inputs and outputs ride in the guaranteed Zswap offer of the deploy
    /// transaction.
    ///
    /// Get the coins from [`MidnightProvider::spendable_shielded_coins`]. The
    /// deploy reserves them, so no other build of this wallet spends them.
    /// The wallet gets them back when the build fails, and in the cases after
    /// the build that [`Self::send`] names. Each output pays
    /// the keys of a user, so the offer cannot fund a contract's shielded
    /// balance.
    ///
    /// With an offer, the deploy proves with no fee, and the wallet then pays
    /// the Dust fee in an intent of its own.
    ///
    /// The deploy fails with [`ContractError::ShieldedOfferShortfall`] when
    /// the outputs pay more of a token than the coins hold. When another
    /// build holds one of the coins, or the offer names a coin twice, it fails
    /// with the wallet error that [`ContractError::Provider`] describes.
    pub fn with_shielded_offer(mut self, offer: crate::ShieldedOffer) -> Self {
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

impl<P> DeployBuilder<'_, P>
where
    P: AsMidnightProvider + Send,
{
    /// Build, prove, and submit the deploy transaction without waiting for inclusion.
    ///
    /// Returns a [`PendingDeploy`] handle on which you can call
    /// [`PendingDeploy::wait_best`] / [`PendingDeploy::wait_finalized`] to observe
    /// inclusion states, then [`PendingDeploy::into_contract`] to read the
    /// deployed contract and obtain the [`Contract`]. Each wait fails with
    /// [`ContractError::TransactionFailed`] when the chain did not apply the
    /// deploy.
    ///
    /// For the common case where you don't need to observe both states, just
    /// `.await?` the builder directly.
    ///
    /// The deploy submits with
    /// [`MidnightProvider::submit_reserved`]. So the inputs that its build
    /// reserved, the fee Dust and the coins of a shielded offer, go back to
    /// the wallet when the node never gets the deploy, or when a wait reads
    /// [`SubmitError::Invalid`](midnight_provider::SubmitError::Invalid).
    pub async fn send(self) -> Result<PendingDeploy<P>, ContractError> {
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        Box::pin(self.send_inner()).await
    }

    async fn send_inner(self) -> Result<PendingDeploy<P>, ContractError> {
        let provider = self.provider.as_midnight_provider();

        let zk_config = self.zk_config.ok_or_else(missing_zk_config)?;

        let mut state = self.initial_state.ok_or_else(|| {
            ContractError::Construction(
                "missing initial_state, call .with_initial_state(...) on the builder".into(),
            )
        })?;

        state =
            populate_verifier_keys(state, zk_config.as_ref(), self.declared_circuits.as_deref())?;

        // Stamp the maintenance authority committee into the deployed state, if
        // requested. No signing key is stored — members sign ops externally.
        if let Some((committee, threshold)) = self.maintenance_authority {
            crate::maintenance::validate_committee(&committee, threshold)?;
            state = crate::maintenance::set_maintenance_authority(state, committee, threshold);
        }

        let result = deploy_funded(&state, provider, self.shielded_offer).await?;
        let address = result.address_hex();
        let pending = provider.submit_reserved(result.transaction).await?;

        Ok(PendingDeploy {
            pending,
            address,
            zk_config,
            provider: self.provider,
            deploy_timeout: self.deploy_timeout,
            declares_witnesses: self.declares_witnesses,
            seen: None,
        })
    }
}

impl<'a, P> IntoFuture for DeployBuilder<'a, P>
where
    P: AsMidnightProvider + Send + 'a,
{
    type Output = Result<Contract<P>, ContractError>;
    // `Pin<Box<dyn Future>>` rather than `impl Future` because the latter is
    // still unstable in associated type position (rust-lang/rust#63063).
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.send().await?.into_contract().await })
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
/// reads the deployed contract from the node).
pub struct PendingDeploy<P> {
    pending: PendingTx,
    address: String,
    zk_config: Arc<dyn ZkConfigProvider>,
    provider: P,
    deploy_timeout: Duration,
    declares_witnesses: bool,
    /// The applied inclusion that the last wait returned. With it,
    /// `into_contract` skips its own wait, and a `DeployTimeout` reports it.
    seen: Option<TxInBlock>,
}

impl<P> PendingDeploy<P> {
    /// The contract address the deploy will produce.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The hash of the submitted extrinsic.
    pub fn extrinsic_hash(&self) -> ExtrinsicHash {
        self.pending.extrinsic_hash()
    }

    /// The extrinsic hash formatted as a hex string (no `0x` prefix, matching
    /// the convention used by [`Contract::address`]).
    pub fn extrinsic_hash_hex(&self) -> String {
        self.pending.extrinsic_hash_hex()
    }

    /// The hash of the Midnight transaction the deploy extrinsic carries. See
    /// [`PendingTx::transaction_hash`].
    pub fn transaction_hash(&self) -> TransactionHash {
        self.pending.transaction_hash()
    }

    /// Wait until the deploy transaction lands in the best block, and return
    /// the inclusion when the chain applied it there.
    ///
    /// Consumes `self` and returns it alongside the inclusion details so
    /// callers can chain a subsequent `wait_finalized` or `into_contract`
    /// without `let mut`. See [`PendingTx::wait_best`] for caveats around
    /// re-orgs and call ordering. An `Err` consumes the handle. When the final
    /// verdict matters, call [`wait_finalized`](Self::wait_finalized) in place
    /// of `wait_best`.
    ///
    /// # Errors
    ///
    /// [`ContractError::TransactionFailed`] when the chain did not apply the
    /// deploy in the best block. [`ContractError::Provider`] when the wait
    /// fails for another reason.
    pub async fn wait_best(mut self) -> Result<(TxInBlock, Self), ContractError> {
        let (in_block, pending) = self.pending.wait_best().await?;
        self.pending = pending;
        self.seen = Some(in_block);
        Ok((in_block, self))
    }

    /// Wait until the deploy transaction is in a finalized block, and return
    /// the inclusion when the chain applied it there.
    ///
    /// Consumes `self` and returns it back. May be called without a prior
    /// `wait_best`; the best-block status is then skipped.
    ///
    /// # Errors
    ///
    /// [`ContractError::TransactionFailed`] when the chain did not apply the
    /// deploy. [`ContractError::Provider`] when the wait fails for another
    /// reason.
    pub async fn wait_finalized(mut self) -> Result<(TxInBlock, Self), ContractError> {
        let (in_block, pending) = self.pending.wait_finalized().await?;
        self.pending = pending;
        self.seen = Some(in_block);
        Ok((in_block, self))
    }
}

impl<P> PendingDeploy<P>
where
    P: AsMidnightProvider + Send,
{
    /// Read the deployed contract from the node, at the block that applied the
    /// deploy, and return the [`Contract`].
    ///
    /// When no [`wait_best`](Self::wait_best) or
    /// [`wait_finalized`](Self::wait_finalized) ran before it, `into_contract`
    /// first waits for the best block itself, as `wait_best` does. One
    /// deadline, the deploy timeout from
    /// [`DeployBuilder::with_deploy_timeout`], bounds that wait and the read
    /// together.
    ///
    /// # Errors
    ///
    /// - [`ContractError::TransactionFailed`] when its own wait finds that the
    ///   chain did not apply the deploy.
    /// - [`ContractError::DeployTimeout`] when the deadline passes first. Its
    ///   `in_block` tells which stage timed out, and so which recovery applies.
    ///   A read that fails runs again until the deadline, so a failed read
    ///   also gives this error.
    /// - [`ContractError::NotFound`] when the node holds no contract at
    ///   [`address`](Self::address) in the block that applied the deploy.
    /// - [`ContractError::StateFetch`] when the node returns a contract state
    ///   that does not decode.
    /// - [`ContractError::Provider`] when its own wait fails for another
    ///   reason. See [`PendingTx`] for the
    ///   [`SubmitError`](midnight_provider::SubmitError) kinds that it carries.
    pub async fn into_contract(self) -> Result<Contract<P>, ContractError> {
        let Self {
            pending,
            address,
            zk_config,
            provider,
            deploy_timeout,
            declares_witnesses,
            seen,
        } = self;
        let start = tokio::time::Instant::now();
        let in_block = match seen {
            Some(in_block) => in_block,
            None => {
                let transaction_hash = pending.transaction_hash();
                match tokio::time::timeout(deploy_timeout, pending.wait_best()).await {
                    Ok(waited) => waited?.0,
                    Err(_elapsed) => {
                        return Err(ContractError::DeployTimeout {
                            address,
                            transaction_hash,
                            timeout: deploy_timeout,
                            in_block: None,
                        });
                    }
                }
            }
        };
        deployed_state(
            provider.as_midnight_provider(),
            &address,
            in_block,
            deploy_timeout.saturating_sub(start.elapsed()),
            deploy_timeout,
        )
        .await?;

        Ok(Contract {
            address,
            zk_config: Some(zk_config),
            provider,
            at_block: None,
            declares_witnesses,
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
/// ```rust,no_run
/// # mod counter {
/// #     compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
/// # }
/// # fn connect(provider: midnight_provider::MidnightProvider, address: &str) {
/// let contract = counter::Contract::at(&provider, address)
///     .with_zk_config(counter::ZK_CONFIG_DIR)
///     .build();
/// # }
/// ```
#[must_use = "call .build() to get the contract handle"]
pub struct ConnectBuilder<P> {
    provider: P,
    address: String,
    zk_config: Option<Arc<dyn ZkConfigProvider>>,
    at_block: Option<BlockHash>,
    declares_witnesses: bool,
}

impl<P> ConnectBuilder<P> {
    pub(crate) fn new(provider: P, address: impl IntoContractAddress) -> Self {
        Self {
            provider,
            address: address.into_contract_address_string(),
            zk_config: None,
            at_block: None,
            declares_witnesses: false,
        }
    }

    /// Record whether this contract declares witnesses, so a call can refuse to
    /// build without a private-state provider. Generated contracts set this
    /// from the compiled artifact.
    pub fn with_declared_witnesses(mut self, declares_witnesses: bool) -> Self {
        self.declares_witnesses = declares_witnesses;
        self
    }

    /// Set the source of the contract's compiled ZK artifacts (prover/verifier
    /// keys, ZKIR). Accepts a compiled-contract directory path (`"compiled"`, a
    /// `PathBuf`, …) or any custom [`ZkConfigProvider`] wrapped in an `Arc` — see
    /// [`IntoZkConfig`]. Required for on-chain circuit calls after connecting.
    pub fn with_zk_config(mut self, zk_config: impl IntoZkConfig) -> Self {
        self.zk_config = Some(zk_config.into_zk_config());
        self
    }

    /// Pin queries to the block `hash`. Default is the node's best block.
    /// Both circuit calls and lazy ledger queries honour the pin through the
    /// node RPC, and a circuit call runs at the time of that block.
    pub fn at_block(mut self, hash: BlockHash) -> Self {
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
            declares_witnesses: self.declares_witnesses,
        }
    }
}

// ---------------------------------------------------------------------------
// Contract — a deployed contract handle
// ---------------------------------------------------------------------------

/// A deployed contract instance bound to a provider.
///
/// This is a stateless, immutable handle. It does not cache contract state.
/// Each circuit call reads the state and the block time from the node RPC
/// at one block: the `at_block` pin, else the node's best block. Ledger
/// queries go through the node RPC directly.
pub struct Contract<P> {
    address: String,
    zk_config: Option<Arc<dyn ZkConfigProvider>>,
    provider: P,
    /// Optional block pin for queries. `None` means latest.
    at_block: Option<BlockHash>,
    /// Whether the compiled contract declares witnesses, and so needs a
    /// private-state provider for its calls to be meaningful.
    declares_witnesses: bool,
}

impl<P: Clone> Clone for Contract<P> {
    fn clone(&self) -> Self {
        Self {
            address: self.address.clone(),
            zk_config: self.zk_config.clone(),
            provider: self.provider.clone(),
            at_block: self.at_block,
            declares_witnesses: self.declares_witnesses,
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
    pub fn deploy<'a, P>(provider: P) -> DeployBuilder<'a, P>
    where
        P: AsMidnightProvider,
    {
        DeployBuilder::new(provider)
    }

    /// Create a handle for an already-deployed contract at the given contract
    /// address: a hex string or a typed
    /// [`ContractAddress`](crate::ContractAddress) (see
    /// [`IntoContractAddress`]).
    ///
    /// This is synchronous, no network calls are made. Use `deploy()` to
    /// deploy a new contract.
    ///
    /// `provider` can be an owned or borrowed `MidnightProvider`.
    pub fn at<P>(provider: P, address: impl IntoContractAddress) -> ConnectBuilder<P>
    where
        P: AsMidnightProvider,
    {
        ConnectBuilder::new(provider, address)
    }
}

impl<P> Contract<P> {
    /// The contract's on-chain address (hex string).
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Reference to the provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// The block pin for queries. `None` means latest.
    pub fn at_block(&self) -> Option<BlockHash> {
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
    /// ```rust,no_run
    /// # async fn find_index(
    /// #     contract: &midnight_contract::Contract<midnight_provider::MidnightProvider>,
    /// #     my_key: &midnight_contract::SigningKey,
    /// # ) -> Result<(), midnight_contract::ContractError> {
    /// use midnight_contract::ContractMaintenanceVerifyingKey;
    ///
    /// let authority = contract.maintenance_authority().await?;
    /// let me = ContractMaintenanceVerifyingKey::Schnorr(my_key.verifying_key());
    /// let my_index = authority.committee.iter().position(|member| *member == me);
    /// # Ok(())
    /// # }
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
        crate::state::fetch_state(provider, &self.address, self.at_block).await
    }

    /// Execute a circuit call on-chain.
    ///
    /// Reads the state and the block time at the node's best block (or at
    /// the `at_block` pin), runs the circuit IR locally at that time, builds
    /// a funded transaction, and submits it to the node.
    pub async fn call(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        circuit_name: &str,
    ) -> Result<CallOutcome<crate::runtime::Value>, ContractError>
    where
        P: AsMidnightProvider,
    {
        self.call_with(
            circuit,
            program,
            circuit_name,
            &[],
            &crate::runtime::NoWitnesses,
            &[],
            crate::call::ShieldedInputs::default(),
        )
        .await
    }

    /// Run a circuit call locally, with no proof, fee or transaction.
    ///
    /// It returns the circuit's value, the contract state after the call and
    /// the events that the circuit emitted.
    /// It does the work of a call before the build. It reads the state and
    /// the block time at the node's best block (or at the `at_block` pin), and
    /// runs the circuit IR locally at that time. The witnesses start from the
    /// head of the private-state journal. The run writes no journal entry,
    /// reserves no input, makes no proof and submits nothing.
    ///
    /// The provider needs a wallet only for a circuit that calls
    /// `ownPublicKey()`, which returns the wallet's coin public key. The
    /// handle needs no zk config.
    ///
    /// # Errors
    ///
    /// - [`ContractError::InvalidContractAddress`] when the handle's contract
    ///   address does not parse.
    /// - [`ContractError::Construction`] when the contract declares witnesses
    ///   and the provider has no private-state store.
    /// - [`ContractError::NotFound`] when the node has no contract at the
    ///   address.
    /// - [`ContractError::StateFetch`] when the node's contract state does not
    ///   decode.
    /// - [`ContractError::PrivateState`] when the store cannot read the
    ///   journal head.
    /// - [`ContractError::Interpreter`] when the circuit or a witness fails,
    ///   or when an argument does not encode. With no wallet on the provider,
    ///   a circuit that calls `ownPublicKey()` fails with this error.
    /// - [`ContractError::Provider`] when the read of the state or the block
    ///   time fails.
    pub async fn simulate_with(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
    ) -> Result<Simulated<crate::runtime::Value, ContractState<InMemoryDB>>, ContractError>
    where
        P: AsMidnightProvider,
    {
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        Box::pin(self.simulate_with_inner(circuit, program, args, witnesses)).await
    }

    async fn simulate_with_inner(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
    ) -> Result<Simulated<crate::runtime::Value, ContractState<InMemoryDB>>, ContractError>
    where
        P: AsMidnightProvider,
    {
        let provider: &MidnightProvider = self.provider.as_midnight_provider();
        let address = crate::address::parse_contract_address(&self.address)?;

        let store = provider.private_state();
        require_private_state_for_witnesses(self.declares_witnesses, store.is_some())?;

        let state = crate::state::state_at_block(provider, &self.address, self.at_block).await?;

        let (mut private_state, _) = self.witness_baseline(store.as_deref()).await?;
        let coin_public_key = match provider.shielded_public_keys().await {
            Ok((coin_public_key, _)) => Some(coin_public_key),
            Err(midnight_provider::ProviderError::NoWallet) => None,
            Err(e) => return Err(e.into()),
        };

        let run = crate::call::run_call(
            circuit,
            program,
            &state.view,
            state.time,
            address,
            args,
            witnesses,
            Some(&mut private_state),
            coin_public_key,
        )?;
        Ok(Simulated {
            value: run.result,
            ledger: run.state,
            block_hash: state.hash,
            logs: run.logs,
        })
    }

    /// Build and prove a circuit call transaction **without submitting** it.
    /// The [`TransferResult`] holds the tagged-serialized proven bytes and the
    /// wallet inputs that the build reserved.
    ///
    /// Use this to obtain a contract call as a transaction you combine with
    /// others before submitting: e.g. merge a counterparty's proven transaction
    /// via [`MidnightProvider::merge_transactions`](midnight_provider::MidnightProvider::merge_transactions),
    /// then [`submit`](midnight_provider::MidnightProvider::submit) the result.
    /// It is the build-only mirror of [`Self::call_with`], which builds and
    /// submits in one step. When `pay_fees` is false, the transaction pays no
    /// Dust fee, for another wallet to sponsor with
    /// [`MidnightProvider::balance_transaction`](midnight_provider::MidnightProvider::balance_transaction).
    ///
    /// A build-only call records no private-state snapshot. So it refuses a
    /// call whose witnesses change the private state, after the local run of
    /// the circuit and before the build. A call that leaves the private state
    /// as it was, or a contract with no witnesses, builds as usual.
    ///
    /// The build reserves the wallet inputs that the transaction spends, as
    /// [`Self::call_with`] does, and [`TransferResult::spent`] names them.
    /// Submit a result that pays its fee with
    /// [`MidnightProvider::submit_reserved`](midnight_provider::MidnightProvider::submit_reserved),
    /// which hands them back when the node rejects the transaction. Wrap a
    /// fee-less result for the fee payer with
    /// [`DustlessTransaction::from_build`](midnight_provider::DustlessTransaction::from_build).
    /// When the transaction will not land, pass the inputs to
    /// [`MidnightProvider::release`](midnight_provider::MidnightProvider::release).
    /// Otherwise they stay reserved until a sync sees the chain spend them, or
    /// until their TTL passes.
    ///
    /// # Errors
    ///
    /// - [`ContractError::UnrecordedPrivateState`] when the witnesses change
    ///   the private state. The call reserves no input and makes no proof.
    ///   Submit the call with [`Self::send_call_with`] or [`Self::call_with`].
    /// - [`ContractError::InvalidContractAddress`] when the handle's contract
    ///   address does not parse.
    /// - [`ContractError::Construction`] when the contract declares witnesses
    ///   and the provider has no private-state store, or when the handle has
    ///   no zk config. Both checks come before any query to the node.
    /// - [`ContractError::NotFound`] when the node has no contract at the
    ///   address.
    /// - [`ContractError::StateFetch`] when the node's contract state does not
    ///   decode.
    /// - [`ContractError::PrivateState`] when the store cannot read the
    ///   journal head.
    /// - [`ContractError::Interpreter`] when the circuit or a witness fails,
    ///   or when an argument does not encode.
    /// - [`ContractError::Serialization`] when the transaction or one of its
    ///   ledger values does not serialize.
    /// - [`ContractError::Provider`] when the provider has no wallet, or when
    ///   the read of the state or the block time, the build or the proof
    ///   fails.
    #[allow(clippy::too_many_arguments)]
    pub async fn build_call_with(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        coin_encryption_keys: &[crate::ShieldedRecipient],
        shielded: crate::call::ShieldedInputs,
        pay_fees: bool,
    ) -> Result<TransferResult, ContractError>
    where
        P: AsMidnightProvider,
    {
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        Box::pin(self.build_call_with_inner(
            circuit,
            program,
            circuit_name,
            args,
            witnesses,
            coin_encryption_keys,
            shielded,
            pay_fees,
        ))
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the arguments of `build_call_with`"
    )]
    async fn build_call_with_inner(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        coin_encryption_keys: &[crate::ShieldedRecipient],
        shielded: crate::call::ShieldedInputs,
        pay_fees: bool,
    ) -> Result<TransferResult, ContractError>
    where
        P: AsMidnightProvider,
    {
        let store = self.provider.as_midnight_provider().private_state();
        require_private_state_for_witnesses(self.declares_witnesses, store.is_some())?;

        let ran = Box::pin(self.run_for_build(circuit, program, args, witnesses, store.as_deref()))
            .await?;
        if private_state_persist(&ran.private_state.baseline, &ran.private_state.after)
            == PrivateStatePersist::Persist
        {
            return Err(ContractError::UnrecordedPrivateState {
                address: self.address.clone(),
            });
        }

        let (built, _result) = Box::pin(self.build_call(
            ran,
            circuit,
            circuit_name,
            args,
            coin_encryption_keys,
            shielded,
            pay_fees,
        ))
        .await?;
        Ok(built.transaction)
    }

    /// The head of the contract's private-state journal, which the witnesses
    /// start from, and the extrinsic that wrote it. With no `store`, or an
    /// empty journal, the witnesses start from an empty buffer.
    async fn witness_baseline(
        &self,
        store: Option<&dyn PrivateStateProvider>,
    ) -> Result<(Vec<u8>, Option<[u8; 32]>), ContractError> {
        let Some(store) = store else {
            return Ok((Vec::new(), None));
        };
        // `head_with_extrinsic` returns both fields from a single underlying
        // read, so a concurrent `append_pending` can't produce a torn read
        // where data and extrinsic_hash come from different journal versions.
        Ok(match store.head_with_extrinsic(&self.address).await? {
            Some((data, ext)) => (data, Some(ext)),
            None => (Vec::new(), None),
        })
    }

    /// Run the circuit on the state at the handle's block, for a call that the
    /// provider's wallet builds next. The witnesses start from the head of
    /// `store`'s journal.
    ///
    /// `ownPublicKey()` returns the coin public key of the wallet. The run
    /// reserves no input and makes no proof, so a failed `assert` costs no
    /// resync and no proof.
    async fn run_for_build(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        store: Option<&dyn PrivateStateProvider>,
    ) -> Result<RanCall, ContractError>
    where
        P: AsMidnightProvider,
    {
        let provider: &MidnightProvider = self.provider.as_midnight_provider();
        let address = crate::address::parse_contract_address(&self.address)?;
        let zk_config = self.zk_config.clone().ok_or_else(missing_zk_config)?;
        let state = crate::state::state_at_block(provider, &self.address, self.at_block).await?;
        let (baseline, depends_on) = self.witness_baseline(store).await?;
        let (coin_public_key, _) = provider.shielded_public_keys().await?;

        let mut private_state = baseline.clone();
        let exec_result = crate::call::run_call(
            circuit,
            program,
            &state.view,
            state.time,
            address,
            args,
            witnesses,
            Some(&mut private_state),
            Some(coin_public_key),
        )?;
        Ok(RanCall {
            state,
            address,
            zk_config,
            exec_result,
            private_state: PrivateStateChange {
                baseline,
                after: private_state,
                depends_on,
            },
        })
    }

    /// Build the proven transaction of a call that ran. When `pay_fees` is
    /// false the transaction is left fee-less. Returns the built call, with the
    /// wallet inputs that its build reserved, and the circuit's raw result.
    #[expect(
        clippy::too_many_arguments,
        reason = "the inputs of a call, as `build_call_with` and `call_with` take them"
    )]
    async fn build_call(
        &self,
        ran: RanCall,
        circuit: &compact_codegen::ir::Circuit,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        coin_encryption_keys: &[crate::ShieldedRecipient],
        shielded: crate::call::ShieldedInputs,
        pay_fees: bool,
    ) -> Result<(BuiltCall, crate::runtime::Value), ContractError>
    where
        P: AsMidnightProvider,
    {
        let transaction = crate::call::call_transaction(
            circuit,
            args,
            &ran.exec_result,
            &ran.state,
            circuit_name,
            ran.address,
            self.provider.as_midnight_provider(),
            ran.zk_config,
            coin_encryption_keys,
            shielded,
            pay_fees,
        )
        .await?;
        Ok((
            BuiltCall {
                transaction,
                private_state: ran.private_state,
            },
            ran.exec_result.result,
        ))
    }

    /// Execute a circuit call on-chain with arguments and witnesses.
    ///
    /// Reads the state and the block time at the node's best block (or at
    /// the `at_block` pin), runs the circuit IR locally at that time, builds
    /// a funded transaction, proves it, and submits to the node. The
    /// contract handle is not mutated.
    ///
    /// It runs [`Self::send_call_with`], then [`PendingCall::wait_finalized`]
    /// with a bound of 60 s on the wait for finality. The snapshot settles
    /// after that wait, so the bound does not cancel the store write.
    ///
    /// # Errors
    ///
    /// The errors of [`Self::send_call_with`] and of
    /// [`PendingCall::wait_finalized`], and [`ContractError::FinalizeTimeout`]
    /// when the call does not finalize in 60 s.
    #[allow(clippy::too_many_arguments)]
    pub async fn call_with(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        // The encryption key of each recipient of the shielded outputs this
        // circuit creates (mints/sends). Each output whose coin public key
        // matches a recipient gets a discovery ciphertext, so the recipient's
        // wallet finds the coin through normal sync (no `watchFor`). The
        // calling wallet's own coin public key needs no entry. Pass `&[]` for
        // none.
        coin_encryption_keys: &[crate::ShieldedRecipient],
        // Shielded (Zswap) coins/offer to attach, funding a circuit's
        // shielded-token deficit (e.g. `receiveShielded` on the caller's coin)
        // from the caller's wallet. Pass `ShieldedInputs::default()` for none.
        shielded: crate::call::ShieldedInputs,
    ) -> Result<CallOutcome<crate::runtime::Value>, ContractError>
    where
        P: AsMidnightProvider,
    {
        let pending = self
            .send_call_with(
                circuit,
                program,
                circuit_name,
                args,
                witnesses,
                coin_encryption_keys,
                shielded,
                Ok,
            )
            .await?;
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        Box::pin(pending.finish(Some(DEFAULT_TX_FINALIZE_TIMEOUT))).await
    }

    /// Submit a circuit call, and return a [`PendingCall`] before the chain's
    /// verdict.
    ///
    /// It does the work of [`Self::call_with`] up to the submit. It runs the
    /// circuit, builds and proves the funded transaction, and builds its
    /// extrinsic. Then it decodes the circuit's result with `decode`, records
    /// the private-state snapshot as `Pending`, and submits. A result that does
    /// not decode stops the call before the store records a snapshot and before
    /// the node gets the transaction. Call [`PendingCall::wait_finalized`] to
    /// finish the call.
    ///
    /// # Errors
    ///
    /// - [`ContractError::InvalidContractAddress`] when the handle's contract
    ///   address does not parse.
    /// - [`ContractError::NotFound`] when the node has no contract at the
    ///   address.
    /// - [`ContractError::StateFetch`] when the node's contract state does not
    ///   decode.
    /// - [`ContractError::PrivateState`] when the store cannot read the
    ///   journal head.
    /// - [`ContractError::Interpreter`] when the circuit or a witness fails,
    ///   or when an argument does not encode.
    /// - [`ContractError::Construction`] when the call transaction cannot be
    ///   made. For example, the handle has no zk config, or a contract with
    ///   witnesses has no private-state store.
    /// - [`ContractError::Serialization`] when the transaction or one of its
    ///   ledger values does not serialize.
    /// - [`ContractError::Provider`] when the read of the state or the block
    ///   time, the build or the proof fails. It is also the error when the
    ///   connection to the node or the build of the extrinsic fails. The call
    ///   records no snapshot and submits nothing.
    /// - The error of `decode` when the circuit's result does not decode. For
    ///   a generated call builder, that is [`ContractError::Interpreter`].
    ///   The call records no snapshot and submits nothing.
    /// - [`ContractError::PendingSnapshotFailed`] when the store cannot record
    ///   the snapshot. The call submits nothing.
    /// - [`ContractError::SubmissionWait`] when the submit call fails with
    ///   [`SubmitError::SubmitRpc`](midnight_provider::SubmitError::SubmitRpc).
    ///   The `Pending` snapshot stays, because the node can still hold the
    ///   transaction. The variant says how to reconcile it.
    ///
    /// The build reserves the wallet inputs that the transaction spends: the
    /// fee Dust, and the coins of `shielded`. An error before the submit hands
    /// them back before it returns. A wait on the [`PendingCall`] that reads
    /// [`SubmitError::Invalid`](midnight_provider::SubmitError::Invalid) also
    /// hands them back. Any other error from the submit on keeps them reserved
    /// until a sync sees the chain spend them, or until their TTL passes.
    #[expect(
        clippy::too_many_arguments,
        reason = "the arguments of `call_with`, plus the decoder"
    )]
    pub async fn send_call_with<T>(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        coin_encryption_keys: &[crate::ShieldedRecipient],
        shielded: crate::call::ShieldedInputs,
        decode: impl FnOnce(crate::runtime::Value) -> Result<T, ContractError>,
    ) -> Result<PendingCall<T>, ContractError>
    where
        P: AsMidnightProvider,
    {
        let provider: &MidnightProvider = self.provider.as_midnight_provider();
        let store = provider.private_state();
        require_private_state_for_witnesses(self.declares_witnesses, store.is_some())?;

        // Each step is boxed; see the frame-size note on
        // `MidnightProvider::resync_wallet`.
        let mut ran =
            Box::pin(self.run_for_build(circuit, program, args, witnesses, store.as_deref()))
                .await?;
        let logs = std::mem::take(&mut ran.exec_result.logs);
        let (built, result) = Box::pin(self.build_call(
            ran,
            circuit,
            circuit_name,
            args,
            coin_encryption_keys,
            shielded,
            // The submit path always self-funds its fees.
            true,
        ))
        .await?;
        let BuiltCall {
            transaction,
            private_state,
        } = built;

        // Prepare the tx so its extrinsic_hash is known, then record the
        // pending snapshot keyed by that hash *before* submitting. Recording
        // first closes the window where a crash between submit and append
        // would leave the tx on the wire with no journal entry, so the next
        // call would build on a stale baseline. The trade is benign: if the
        // process dies after the append but before submit, the tx never
        // reached the mempool, leaving a provisional pending entry that
        // reconciliation resolves.
        let prepared = Box::pin(provider.prepare_reserved(transaction)).await?;

        // Decode before the append and the submit: an error after the submit
        // would leave the call in flight with no `PendingCall` to settle it.
        let value = match decode(result) {
            Ok(value) => value,
            Err(err) => {
                prepared.release().await;
                return Err(err);
            }
        };

        let (pending, snapshot_store) = Box::pin(record_and_submit(
            &self.address,
            prepared,
            private_state,
            store,
        ))
        .await?;
        Ok(PendingCall {
            pending,
            value,
            logs,
            address: self.address.clone(),
            snapshot_store,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider {
        inner: MidnightProvider,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                inner: MidnightProvider::new("ws://test"),
            }
        }
    }

    impl AsMidnightProvider for MockProvider {
        fn as_midnight_provider(&self) -> &MidnightProvider {
            &self.inner
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
        let hash = BlockHash::repeat_byte(0xab);
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

    const CALL_ADDRESS: &str = "0200aa";
    const LAST_GOOD: [u8; 32] = [9; 32];

    /// A call, and a store whose journal holds the call's pending snapshot on
    /// top of the snapshot `LAST_GOOD`.
    async fn landed_call() -> (
        tempfile::TempDir,
        midnight_provider::FsPrivateStateProvider,
        TxInBlock,
    ) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = midnight_provider::FsPrivateStateProvider::new(dir.path());
        let call = TxInBlock {
            block_hash: BlockHash::repeat_byte(1),
            extrinsic_hash: ExtrinsicHash::repeat_byte(2),
            transaction_hash: [3; 32].into(),
            verdict: midnight_provider::Verdict::Success,
        };
        store
            .append_pending(CALL_ADDRESS, LAST_GOOD, None, b"before")
            .await
            .unwrap();
        store
            .append_pending(
                CALL_ADDRESS,
                call.extrinsic_hash.0,
                Some(LAST_GOOD),
                b"after",
            )
            .await
            .unwrap();
        (dir, store, call)
    }

    #[tokio::test]
    async fn an_applied_call_confirms_its_snapshot_in_its_block() {
        let (_dir, store, call) = landed_call().await;

        settle_call(
            Some(&store),
            CALL_ADDRESS,
            call.extrinsic_hash,
            call.transaction_hash,
            Ok(call),
        )
        .await
        .expect("a call the chain applied must settle");

        let snapshots = store.snapshots(CALL_ADDRESS).await.unwrap();
        let snapshot = snapshots
            .iter()
            .find(|s| s.extrinsic_hash == hex::encode(call.extrinsic_hash))
            .expect("the call's snapshot stays in the journal");
        assert_eq!(
            snapshot.status,
            midnight_provider::SnapshotStatus::Confirmed
        );
        assert_eq!(snapshot.block_hash, Some(hex::encode(call.block_hash)));
    }

    /// A call the chain did not apply drops its pending snapshot before the
    /// error returns. Otherwise the orphan snapshot stays the journal head, and
    /// the next call builds on a state the chain never reached. The wait
    /// reports it as an error, so an arm that takes every wait error for an
    /// unknown fate keeps the orphan too. A `From<ProviderError>` that sends
    /// `NotApplied` to `ContractError::Provider` fails the variant check.
    #[tokio::test]
    async fn a_call_that_did_not_apply_leaves_the_last_good_snapshot_as_head() {
        let (_dir, store, call) = landed_call().await;
        let waited = Err(midnight_provider::NotApplied(TxInBlock {
            verdict: midnight_provider::Verdict::Failure,
            ..call
        })
        .into());

        let err = settle_call(
            Some(&store),
            CALL_ADDRESS,
            call.extrinsic_hash,
            call.transaction_hash,
            waited,
        )
        .await
        .expect_err("a call the chain did not apply must fail");

        assert!(
            matches!(err, ContractError::TransactionFailed(_)),
            "got {err:?}"
        );
        assert_eq!(
            store.head_extrinsic(CALL_ADDRESS).await.unwrap(),
            Some(LAST_GOOD)
        );
    }

    /// A wait that fails with no verdict leaves the call's fate unknown, so the
    /// pending snapshot stays: it is the only local record of a call that can
    /// still land. An arm that drops the snapshot on every wait error loses it.
    #[tokio::test]
    async fn a_call_of_unknown_fate_keeps_its_pending_snapshot() {
        let (_dir, store, call) = landed_call().await;
        let waited = Err(ProviderError::Submission(
            midnight_provider::SubmitError::Dropped {
                message: "pool full".into(),
            },
        ));

        let err = settle_call(
            Some(&store),
            CALL_ADDRESS,
            call.extrinsic_hash,
            call.transaction_hash,
            waited,
        )
        .await
        .expect_err("a call of unknown fate must fail");

        assert!(
            matches!(
                err,
                ContractError::SubmissionWait {
                    snapshot_written: true,
                    ..
                }
            ),
            "got {err:?}"
        );
        assert_eq!(
            store.head_extrinsic(CALL_ADDRESS).await.unwrap(),
            Some(call.extrinsic_hash.0)
        );
    }

    /// A contract whose witnesses are stateful needs a private-state store.
    /// Without one the baseline is an empty buffer and the post-call buffer is
    /// dropped, so every call silently starts from `Default::default()` while
    /// still succeeding and proving.
    #[test]
    fn witness_contract_without_a_store_is_rejected() {
        let err = require_private_state_for_witnesses(true, false)
            .expect_err("a witness contract with no store must not build a call");
        let msg = err.to_string();
        assert!(
            msg.contains("with_private_state"),
            "error should name the setter to call, got: {msg}"
        );
    }

    #[test]
    fn witness_contract_with_a_store_is_allowed() {
        assert!(require_private_state_for_witnesses(true, true).is_ok());
    }

    #[test]
    fn contract_without_witnesses_needs_no_store() {
        assert!(require_private_state_for_witnesses(false, false).is_ok());
    }

    /// Codegen sets the flag from the compiled artifact; it must survive the
    /// builder into the handle.
    #[test]
    fn declared_witnesses_flag_reaches_the_contract_handle() {
        let addr = "0200aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";
        let with = Contract::at(MockProvider::new(), addr)
            .with_declared_witnesses(true)
            .build();
        assert!(with.declares_witnesses);
        let without = Contract::at(MockProvider::new(), addr).build();
        assert!(!without.declares_witnesses);
    }
}
