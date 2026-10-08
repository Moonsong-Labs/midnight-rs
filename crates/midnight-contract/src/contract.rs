use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
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
use midnight_provider::{
    PendingTx, PrivateStateProvider, ProviderError, TransactionHash, TxInBlock,
};

/// A circuit call that landed on chain: the circuit's own result, plus the
/// identity of the transaction that carried it.
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
    pub extrinsic_hash: [u8; 32],
    /// Hash of the Midnight transaction the extrinsic carried, the identity
    /// the indexer keys on. See [`PendingTx::transaction_hash`].
    pub transaction_hash: TransactionHash,
    /// Hash of the block it landed in.
    pub block_hash: [u8; 32],
}

impl<T> CallOutcome<T> {
    /// Replace the circuit result, keeping the transaction identity.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> CallOutcome<U> {
        CallOutcome {
            value: f(self.value),
            extrinsic_hash: self.extrinsic_hash,
            transaction_hash: self.transaction_hash,
            block_hash: self.block_hash,
        }
    }
}

/// The result of a circuit call that ran with no proof, fee or transaction.
///
/// It holds the circuit's value and the ledger after the call, from a run on
/// the contract's state at one block. [`Contract::simulate_with`] and the
/// `simulate` method of a generated call builder return it. The result is
/// true for the block that the run read. A later block can change the outcome
/// of the real call.
#[derive(Debug, Clone)]
pub struct Simulated<T, L> {
    /// The circuit's return value.
    pub value: T,
    /// The contract's ledger state after the call.
    pub ledger: L,
    /// The block whose state and time the run read.
    pub block_hash: [u8; 32],
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
/// becomes the next call's witness baseline. Any other wait error returns
/// [`ContractError::SubmissionWait`] and keeps the snapshot for the caller to
/// reconcile (see that variant).
async fn settle_call(
    store: Option<&dyn PrivateStateProvider>,
    address: &str,
    extrinsic_hash: [u8; 32],
    transaction_hash: TransactionHash,
    waited: Result<TxInBlock, ProviderError>,
) -> Result<TxInBlock, ContractError> {
    match waited {
        Ok(applied) => {
            if let Some(store) = store {
                // No height: subxt reports only the block hash, which alone
                // identifies the block.
                store
                    .confirm(address, extrinsic_hash, None, applied.block_hash)
                    .await?;
            }
            Ok(applied)
        }
        Err(error @ ProviderError::NotApplied(_)) => {
            if let Some(store) = store {
                store.mark_failed(address, extrinsic_hash).await?;
            }
            Err(error.into())
        }
        Err(source) => Err(ContractError::SubmissionWait {
            transaction_hash: Box::new(transaction_hash),
            extrinsic_hash: hex::encode(extrinsic_hash).into(),
            source,
            snapshot_written: store.is_some(),
        }),
    }
}

/// [`settle_call`], then `decode` on the circuit's result.
///
/// The settle comes first, as [`PendingCall::wait_finalized`] promises, so a
/// result that does not decode still leaves the snapshot settled.
async fn settle_and_decode<T>(
    store: Option<&dyn PrivateStateProvider>,
    address: &str,
    extrinsic_hash: [u8; 32],
    transaction_hash: TransactionHash,
    waited: Result<TxInBlock, ProviderError>,
    result: Option<crate::runtime::Value>,
    decode: fn(Option<crate::runtime::Value>) -> Result<T, ContractError>,
) -> Result<CallOutcome<T>, ContractError> {
    let in_block = settle_call(store, address, extrinsic_hash, transaction_hash, waited).await?;
    Ok(CallOutcome {
        value: decode(result)?,
        extrinsic_hash,
        transaction_hash: in_block.transaction_hash,
        block_hash: in_block.block_hash,
    })
}

/// How long to wait for the chain to finalize a submitted tx before treating
/// it as a stalled submission. Restores the bound the deleted
/// `wait_for_contract_update` used to enforce; `wait_finalized` itself has
/// no internal timeout, so without this wrap a stalled grandpa would block
/// the caller indefinitely.
const DEFAULT_TX_FINALIZE_TIMEOUT: Duration = Duration::from_secs(60);

/// A submitted circuit call, before the chain's verdict.
///
/// Returned by [`Contract::send_call_with`] and by the `send` method of a
/// generated call builder. Use it to read the transaction's hashes before the
/// wait, or to choose the deadline of the wait. Call
/// [`wait_finalized`](Self::wait_finalized) to finish the call.
///
/// The handle owns its data, so you can keep it across other calls.
///
/// A dropped `PendingCall` does not retract the transaction, which can still
/// land. It leaves the call's private-state snapshot `Pending`, as a
/// [`ContractError::FinalizeTimeout`] does, and the recovery is the same. The
/// inputs that the build reserved stay reserved until a sync sees the
/// transaction land, or until their TTL elapses.
#[must_use = "call `wait_finalized`: a dropped `PendingCall` leaves its private-state snapshot `Pending`"]
pub struct PendingCall<T> {
    pending: PendingTx,
    result: Option<crate::runtime::Value>,
    decode: fn(Option<crate::runtime::Value>) -> Result<T, ContractError>,
    address: String,
    /// The store that holds the call's `Pending` snapshot, or `None` when the
    /// call recorded none.
    snapshot_store: Option<Arc<dyn PrivateStateProvider>>,
}

impl<T> PendingCall<T> {
    /// The hash of the submitted extrinsic. The call's private-state snapshot
    /// uses it as its key.
    pub fn extrinsic_hash(&self) -> [u8; 32] {
        self.pending.extrinsic_hash()
    }

    /// The hash of the Midnight transaction the extrinsic carries. See
    /// [`PendingTx::transaction_hash`].
    pub fn transaction_hash(&self) -> TransactionHash {
        self.pending.transaction_hash()
    }

    /// Wait for finality, then settle the snapshot and decode the result.
    ///
    /// The wait ends when the call is in a finalized block. A call that the
    /// chain applied confirms its private-state snapshot. A call that the
    /// chain did not apply drops its snapshot with `mark_failed` before the
    /// error returns. The decode runs after that, so a decode failure does not
    /// leave the snapshot `Pending`.
    ///
    /// The wait has no deadline, like [`PendingTx::wait_finalized`]. Wrap it
    /// in [`tokio::time::timeout`] to bound it, and read the hashes first,
    /// because the timeout drops the handle. The `.await` of a call builder and
    /// [`Contract::call_with`] bound the wait for finality to 60 s, and settle
    /// the snapshot after that wait.
    ///
    /// # Errors
    ///
    /// - [`ContractError::SubmissionWait`] when the wait for finality fails
    ///   with no verdict. The snapshot stays `Pending`.
    /// - [`ContractError::TransactionFailed`] when the chain did not apply the
    ///   call.
    /// - [`ContractError::PrivateState`] when the store cannot confirm or drop
    ///   the snapshot.
    /// - The error of the decoder when the circuit's result does not decode as
    ///   `T`. For a generated call builder, that is
    ///   [`ContractError::Interpreter`].
    pub async fn wait_finalized(self) -> Result<CallOutcome<T>, ContractError> {
        self.finish(None).await
    }

    /// [`Self::wait_finalized`], with the wait for finality bounded by
    /// `deadline` when one is set.
    ///
    /// The deadline does not cover the settle, so it never cancels the store
    /// write of a call that finalized.
    async fn finish(self, deadline: Option<Duration>) -> Result<CallOutcome<T>, ContractError> {
        let Self {
            pending,
            result,
            decode,
            address,
            snapshot_store,
        } = self;
        let extrinsic_hash = pending.extrinsic_hash();
        let transaction_hash = pending.transaction_hash();
        let snapshot_written = snapshot_store.is_some();
        let wait = pending.wait_finalized();
        let waited = match deadline {
            None => wait.await,
            Some(deadline) => tokio::time::timeout(deadline, wait)
                .await
                .map_err(|_elapsed| ContractError::FinalizeTimeout {
                    transaction_hash: Box::new(transaction_hash),
                    extrinsic_hash: hex::encode(extrinsic_hash).into(),
                    timeout: deadline,
                    snapshot_written,
                })?,
        };
        settle_and_decode(
            snapshot_store.as_deref(),
            &address,
            extrinsic_hash,
            transaction_hash,
            waited.map(|(in_block, _pending)| in_block),
            result,
            decode,
        )
        .await
    }
}

impl<T> std::fmt::Debug for PendingCall<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingCall")
            .field("address", &self.address)
            .field("extrinsic_hash", &self.pending.extrinsic_hash_hex())
            .field("transaction_hash", &self.pending.transaction_hash())
            .field("result", &self.result)
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
///     .with_zk_config("compiled")
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
    deploy_poll_interval: Duration,
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
            deploy_poll_interval: Duration::from_secs(2),
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
    /// before it, and then the indexer poll. An explicit
    /// [`PendingDeploy::wait_best`] or [`PendingDeploy::wait_finalized`] has no
    /// deadline. See [`PendingTx`] to bound one with `tokio::time::timeout`.
    pub fn with_deploy_timeout(mut self, timeout: Duration) -> Self {
        self.deploy_timeout = timeout;
        self
    }

    /// Set the poll interval for checking deployment status (default: 2s).
    pub fn with_deploy_poll_interval(mut self, interval: Duration) -> Self {
        self.deploy_poll_interval = interval;
        self
    }

    /// Attach a hand-built shielded (zswap) offer to ride alongside the deploy
    /// in the same transaction segment.
    ///
    /// The SDK does not derive shielded inputs/outputs from a contract's
    /// initial state — if your deployment needs to spend or produce shielded
    /// coins (e.g. seeding a contract with a shielded balance), construct the
    /// offer with the `OfferInfo`, `InputInfo` and `OutputInfo` of the chain's
    /// ledger generation ([`crate::ledger_8`] or [`crate::ledger_9`]) and pass
    /// it here. The generation's `TransferBuilder::prepare_shielded` source is
    /// the canonical worked example. The deploy fails if the offer's generation
    /// is not the one the wallet's state is in.
    ///
    /// Coins in `InputInfo::origin` must come from the provider's wallet seed
    /// (the same seed that pays the dust fee).
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
    P: AsMidnightProvider + Provider + Send,
{
    /// Build, prove, and submit the deploy transaction without waiting for inclusion.
    ///
    /// Returns a [`PendingDeploy`] handle on which you can call
    /// [`PendingDeploy::wait_best`] / [`PendingDeploy::wait_finalized`] to observe
    /// inclusion states, then [`PendingDeploy::into_contract`] to wait for the
    /// indexer and obtain the [`Contract`]. Each wait fails with
    /// [`ContractError::TransactionFailed`] when the chain did not apply the
    /// deploy.
    ///
    /// For the common case where you don't need to observe both states, just
    /// `.await?` the builder directly.
    pub async fn send(self) -> Result<PendingDeploy<P>, ContractError> {
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        Box::pin(self.send_inner()).await
    }

    async fn send_inner(self) -> Result<PendingDeploy<P>, ContractError> {
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
        let pending = provider
            .submit_reserved(&result.tx_bytes, vec![result.reserved])
            .await?;

        Ok(PendingDeploy {
            pending,
            address,
            zk_config,
            provider: self.provider,
            deploy_timeout: self.deploy_timeout,
            deploy_poll_interval: self.deploy_poll_interval,
            declares_witnesses: self.declares_witnesses,
            seen: None,
        })
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
/// waits for the indexer).
pub struct PendingDeploy<P> {
    pending: PendingTx,
    address: String,
    zk_config: Arc<dyn ZkConfigProvider>,
    provider: P,
    deploy_timeout: Duration,
    deploy_poll_interval: Duration,
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
    pub fn extrinsic_hash(&self) -> [u8; 32] {
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
    P: AsMidnightProvider + Provider + Send,
{
    /// Wait for the indexer to show the deployed contract, and return the
    /// [`Contract`].
    ///
    /// When no [`wait_best`](Self::wait_best) or
    /// [`wait_finalized`](Self::wait_finalized) ran before it, `into_contract`
    /// first waits for the best block itself, as `wait_best` does. One
    /// deadline, the deploy timeout from
    /// [`DeployBuilder::with_deploy_timeout`], bounds that wait and the indexer
    /// poll together.
    ///
    /// # Errors
    ///
    /// - [`ContractError::TransactionFailed`] when its own wait finds that the
    ///   chain did not apply the deploy.
    /// - [`ContractError::DeployTimeout`] when the deadline passes first. Its
    ///   `in_block` tells which stage timed out, and so which recovery applies.
    /// - [`ContractError::StateFetch`] when the indexer returns a contract
    ///   state that does not decode.
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
            deploy_poll_interval,
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
        wait_for_deployment(
            &provider,
            &address,
            in_block,
            deploy_timeout.saturating_sub(start.elapsed()),
            deploy_timeout,
            deploy_poll_interval,
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
///     .with_zk_config("compiled")
///     .build();
/// # }
/// ```
#[must_use = "call .build() to get the contract handle"]
pub struct ConnectBuilder<P> {
    provider: P,
    address: String,
    zk_config: Option<Arc<dyn ZkConfigProvider>>,
    at_block: Option<NodeBlockHash>,
    declares_witnesses: bool,
}

impl<P> ConnectBuilder<P> {
    pub(crate) fn new(provider: P, address: impl IntoAddress) -> Self {
        Self {
            provider,
            address: address.into_address_string(),
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
    at_block: Option<NodeBlockHash>,
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
        crate::state::fetch_state_from_node(provider, &self.address, self.at_block).await
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
    ) -> Result<CallOutcome<Option<crate::runtime::Value>>, ContractError>
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
    /// It returns the circuit's value and the contract state after the call.
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
    /// - [`ContractError::InvalidAddress`] when the handle's address does not
    ///   parse.
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
    ) -> Result<Simulated<Option<crate::runtime::Value>, ContractState<InMemoryDB>>, ContractError>
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
    ) -> Result<Simulated<Option<crate::runtime::Value>, ContractState<InMemoryDB>>, ContractError>
    where
        P: AsMidnightProvider,
    {
        let provider: &MidnightProvider = self.provider.as_midnight_provider();
        let address = crate::address::parse_address(&self.address)?;

        let store = provider.private_state();
        require_private_state_for_witnesses(self.declares_witnesses, store.is_some())?;

        let state = crate::state::state_at_block(provider, &self.address, self.at_block).await?;

        let mut private_state = match &store {
            Some(store) => store.head(&self.address).await?.unwrap_or_default(),
            None => Vec::new(),
        };
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
        })
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
    /// journaled, and this method does not return it either, so use this path
    /// for stateless calls (e.g. a burn); a private-state contract's post-call
    /// state changes would be lost.
    ///
    /// The inputs the build selected stay reserved on the wallet until a sync
    /// sees the transaction land, or until their TTL elapses. This includes
    /// the fee Dust and any pinned shielded coins. The bytes carry no
    /// reservation, so a rejection hands nothing back.
    #[allow(clippy::too_many_arguments)]
    pub async fn build_call_with(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        coin_encryption_keys: &[(crate::CoinPublicKey, crate::EncryptionPublicKey)],
        shielded: crate::call::ShieldedInputs,
        // When false, build the call proven but Dustless (fee-less), for another
        // wallet to sponsor via `MidnightProvider::balance_transaction`.
        pay_fees: bool,
    ) -> Result<Vec<u8>, ContractError>
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

    #[allow(clippy::too_many_arguments)]
    async fn build_call_with_inner(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        coin_encryption_keys: &[(crate::CoinPublicKey, crate::EncryptionPublicKey)],
        shielded: crate::call::ShieldedInputs,
        pay_fees: bool,
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

        let state = crate::state::state_at_block(provider, &self.address, self.at_block).await?;

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

        let (tx_bytes, _reserved, _new_state, _result) = crate::call::call_funded_with(
            circuit,
            program,
            &state,
            circuit_name,
            address,
            provider,
            zk_config,
            args,
            witnesses,
            Some(&mut private_state),
            coin_encryption_keys,
            shielded,
            pay_fees,
        )
        .await?;

        Ok(tx_bytes)
    }

    /// Execute a circuit call on-chain with arguments and witnesses.
    ///
    /// Reads the state and the block time at the node's best block (or at
    /// the `at_block` pin), runs the circuit IR locally at that time, builds
    /// a funded transaction, proves it, and submits to the node. The
    /// contract handle is not mutated.
    ///
    /// It is [`Self::send_call_with`] followed by
    /// [`PendingCall::wait_finalized`], and it bounds the wait for finality to
    /// 60 s. The snapshot settles after that wait, so the bound does not
    /// cancel the store write.
    ///
    /// # Errors
    ///
    /// The errors of [`Self::send_call_with`] and of
    /// [`PendingCall::wait_finalized`], and
    /// [`ContractError::FinalizeTimeout`] when the call does not finalize in
    /// 60 s.
    #[allow(clippy::too_many_arguments)]
    pub async fn call_with(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        // `coin_public_key → encryption_public_key` mappings applied to the
        // shielded outputs this circuit creates (mints/sends). For each output
        // whose coin public key is present, the SDK attaches a discovery
        // ciphertext so the recipient's wallet finds the coin through normal
        // sync (no `watchFor`). The calling wallet's own coin public key needs
        // no entry. Pass `&[]` for none.
        coin_encryption_keys: &[(crate::CoinPublicKey, crate::EncryptionPublicKey)],
        // Shielded (Zswap) coins/offer to attach, funding a circuit's
        // shielded-token deficit (e.g. `receiveShielded` on the caller's coin)
        // from the caller's wallet. Pass `ShieldedInputs::default()` for none.
        shielded: crate::call::ShieldedInputs,
    ) -> Result<CallOutcome<Option<crate::runtime::Value>>, ContractError>
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
    /// It does the work of [`Self::call_with`] up to the submit: it runs the
    /// circuit, builds and proves the funded transaction, records the
    /// private-state snapshot as `Pending`, and submits. Call
    /// [`PendingCall::wait_finalized`] to finish the call. `decode` turns the
    /// circuit's raw result into `T` there, after the verdict.
    ///
    /// # Errors
    ///
    /// - [`ContractError::InvalidAddress`] when the handle's address does not
    ///   parse.
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
    /// - [`ContractError::PendingSnapshotFailed`] when the store cannot record
    ///   the snapshot. Nothing was submitted.
    /// - [`ContractError::Provider`] when the read of the state or the block
    ///   time, the build or the proof fails, or when the node cannot be
    ///   reached or the extrinsic cannot be built before the submit
    ///   ([`SubmitError::NotSubmitted`](midnight_provider::SubmitError::NotSubmitted)).
    /// - [`ContractError::SubmissionWait`] when the submit call fails
    ///   ([`SubmitError::SubmitRpc`](midnight_provider::SubmitError::SubmitRpc)).
    ///   The SDK cannot tell a refusal from a lost response, so the `Pending`
    ///   snapshot stays. The variant says how to reconcile it.
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
        coin_encryption_keys: &[(crate::CoinPublicKey, crate::EncryptionPublicKey)],
        shielded: crate::call::ShieldedInputs,
        decode: fn(Option<crate::runtime::Value>) -> Result<T, ContractError>,
    ) -> Result<PendingCall<T>, ContractError>
    where
        P: AsMidnightProvider,
    {
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        let (pending, result, snapshot_store) = Box::pin(self.send_call_with_inner(
            circuit,
            program,
            circuit_name,
            args,
            witnesses,
            coin_encryption_keys,
            shielded,
        ))
        .await?;
        Ok(PendingCall {
            pending,
            result,
            decode,
            address: self.address.clone(),
            snapshot_store,
        })
    }

    /// The work of [`Self::send_call_with`] that does not depend on `T`, so a
    /// new return type does not compile it again.
    ///
    /// Returns the submitted transaction, the circuit's raw result, and the
    /// store when it holds the call's `Pending` snapshot.
    #[allow(clippy::too_many_arguments)]
    async fn send_call_with_inner(
        &self,
        circuit: &compact_codegen::ir::Circuit,
        program: &compact_interpreter::Program<'_>,
        circuit_name: &str,
        args: &[(&str, crate::runtime::Value)],
        witnesses: &dyn crate::runtime::WitnessProvider,
        coin_encryption_keys: &[(crate::CoinPublicKey, crate::EncryptionPublicKey)],
        shielded: crate::call::ShieldedInputs,
    ) -> Result<
        (
            PendingTx,
            Option<crate::runtime::Value>,
            Option<Arc<dyn PrivateStateProvider>>,
        ),
        ContractError,
    >
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

        let state = crate::state::state_at_block(provider, &self.address, self.at_block).await?;

        // Load the journal head as the witness baseline; capture its
        // extrinsic_hash so the snapshot we write below can record the
        // dependency. `head_with_extrinsic` returns both fields from a
        // single underlying read so a concurrent `append_pending` can't
        // produce a torn read where data and extrinsic_hash come from
        // different journal versions. With no provider attached the buffer
        // is just empty.
        let ps_store = provider.private_state();
        require_private_state_for_witnesses(self.declares_witnesses, ps_store.is_some())?;
        let (baseline, depends_on) = match &ps_store {
            Some(store) => match store.head_with_extrinsic(&self.address).await? {
                Some((data, ext)) => (data, Some(ext)),
                None => (Vec::new(), None),
            },
            None => (Vec::new(), None),
        };

        let mut private_state = baseline.clone();

        let (tx_bytes, reserved, _new_state, result) = crate::call::call_funded_with(
            circuit,
            program,
            &state,
            circuit_name,
            address,
            provider,
            zk_config,
            args,
            witnesses,
            Some(&mut private_state),
            coin_encryption_keys,
            shielded,
            // The submit path always self-funds its fees.
            true,
        )
        .await?;

        // Prepare the tx so its extrinsic_hash is known, then record the
        // pending snapshot keyed by that hash *before* submitting. Recording
        // first closes the window where a crash between submit and append
        // would leave the tx on the wire with no journal entry, so the next
        // call would build on a stale baseline. The trade is benign: if the
        // process dies after the append but before submit, the tx never
        // reached the mempool, leaving a provisional pending entry that
        // reconciliation resolves.
        let prepared = provider.prepare_reserved(&tx_bytes, reserved).await?;
        let extrinsic_hash = prepared.extrinsic_hash();
        let transaction_hash = prepared.transaction_hash();
        let persist = private_state_persist(&baseline, &private_state);
        // True iff we successfully recorded a pending snapshot for this
        // tx. Without one (no provider attached, or witnesses left state
        // unchanged) the call has no snapshot to settle, and the caller
        // shouldn't be told to reconcile one that doesn't exist.
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
                    // anything hits the wire. Dropping `prepared` hands the
                    // reserved inputs back.
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

        // Submit now that the journal record (if any) is durable. A failed
        // submit is a `SubmitRpc`, which can be a refusal or a lost response,
        // so the pending entry stays for the caller to reconcile.
        let pending = match prepared.submit().await {
            Ok(pending) => pending,
            Err(source) => {
                return Err(ContractError::SubmissionWait {
                    transaction_hash: Box::new(transaction_hash),
                    extrinsic_hash: hex::encode(extrinsic_hash).into(),
                    source,
                    snapshot_written: pending_snapshot_written,
                });
            }
        };

        Ok((
            pending,
            result,
            ps_store.filter(|_| pending_snapshot_written),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use midnight_provider::{ContractActionOffset, StateQuery, StateQueryResult};

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
            block_hash: [1; 32],
            extrinsic_hash: [2; 32],
            transaction_hash: [3; 32].into(),
            verdict: midnight_provider::Verdict::Success,
        };
        store
            .append_pending(CALL_ADDRESS, LAST_GOOD, None, b"before")
            .await
            .unwrap();
        store
            .append_pending(CALL_ADDRESS, call.extrinsic_hash, Some(LAST_GOOD), b"after")
            .await
            .unwrap();
        (dir, store, call)
    }

    async fn snapshot_of(
        store: &midnight_provider::FsPrivateStateProvider,
        call: &TxInBlock,
    ) -> midnight_provider::Snapshot {
        store
            .snapshots(CALL_ADDRESS)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.extrinsic_hash == hex::encode(call.extrinsic_hash))
            .expect("the call's snapshot stays in the journal")
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

        let snapshot = snapshot_of(&store, &call).await;
        assert_eq!(
            snapshot.status,
            midnight_provider::SnapshotStatus::Confirmed
        );
        assert_eq!(snapshot.block_hash, Some(hex::encode(call.block_hash)));
    }

    /// A result that does not decode must not leave the snapshot of an applied
    /// call `Pending`, because the chain advanced.
    #[tokio::test]
    async fn an_applied_call_whose_result_does_not_decode_confirms_its_snapshot() {
        fn undecodable(_: Option<crate::runtime::Value>) -> Result<(), ContractError> {
            Err(ContractError::Construction("undecodable".into()))
        }
        let (_dir, store, call) = landed_call().await;

        let err = settle_and_decode(
            Some(&store),
            CALL_ADDRESS,
            call.extrinsic_hash,
            call.transaction_hash,
            Ok(call),
            None,
            undecodable,
        )
        .await
        .expect_err("the decoder's error must reach the caller");

        assert!(
            matches!(&err, ContractError::Construction(m) if m == "undecodable"),
            "got {err:?}"
        );
        assert_eq!(
            snapshot_of(&store, &call).await.status,
            midnight_provider::SnapshotStatus::Confirmed
        );
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
            Some(call.extrinsic_hash)
        );
    }

    /// The indexer poll gets only the time that the block wait left of the
    /// deploy deadline. It stops then, not at the next poll after it.
    /// The timeout keeps the inclusion that the deploy reached, so the caller
    /// connects and does not pay for a second deploy.
    #[tokio::test]
    async fn an_indexer_that_never_shows_the_contract_times_out_at_the_deadline() {
        let applied = TxInBlock {
            block_hash: [1; 32],
            extrinsic_hash: [2; 32],
            transaction_hash: [3; 32].into(),
            verdict: midnight_provider::Verdict::Success,
        };
        let timeout = Duration::from_secs(3600);
        let remaining = Duration::from_millis(50);
        let poll_interval = Duration::from_secs(3600);

        let waited = tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_deployment(
                &MockProvider::new(),
                CALL_ADDRESS,
                applied,
                remaining,
                timeout,
                poll_interval,
            ),
        )
        .await
        .expect("the poll must stop at the deadline");

        let err = waited.expect_err("a contract the indexer never shows must time out");
        assert!(
            matches!(
                &err,
                ContractError::DeployTimeout { in_block: Some(in_block), .. }
                    if in_block.block_hash == applied.block_hash
            ),
            "got {err:?}"
        );
        assert!(
            err.to_string().contains("Contract::at"),
            "the timeout after inclusion must tell the caller to connect, got: {err}"
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
