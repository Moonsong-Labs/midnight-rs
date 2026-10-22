use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use subxt::OnlineClient;
use subxt::backend::CombinedBackend;
use subxt::config::RpcConfigFor;
use subxt::ext::futures::StreamExt;
use subxt::rpcs::client::reconnecting_rpc_client::RpcClient as ReconnectingRpcClient;
use subxt::rpcs::client::{RpcClient, RpcParams, rpc_params};
use subxt::rpcs::{ChainHeadRpcMethods, LegacyRpcMethods};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::transfer::{DustRegistration, ShieldedSwap, ShieldedTransfer, UnshieldedTransfer};
use crate::{
    Health, PendingTx, ProofProviders, Provider, ProviderError, StateQuery, StateQueryResult,
    ledger_8, ledger_9, submit,
};
use midnight_private_state::PrivateStateProvider;
use midnight_types::{
    BlockHash, ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, LedgerVersion,
    Network, ShieldedTokenType, SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo,
    TransferRequest, TransferResult, UnshieldedTokenType, WalletBalance,
};
use midnight_wallet_facade::{IntoWalletFacade, WalletFacade};

/// The bound of a node query, its connect included.
///
/// The subxt reconnecting client retries a refused dial forever. While the
/// node is down, it also keeps each new call in its queue with no deadline,
/// and its own request timeout covers only a request on a live socket. So a
/// query with no bound waits for the whole outage.
///
/// A timeout during the first connect drops that connect, and the next query
/// dials again. After the first connect, the timeout ends only the wait: the
/// client sends the queued call after the reconnect and drops the late
/// answer.
pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// The bound of a contract state query, its connect included. See
/// [`QUERY_TIMEOUT`] for why a query needs a bound.
///
/// A full contract state grows with the contract, so a state query gets the
/// client's own request timeout and not the shorter bound of the other
/// queries.
const STATE_QUERY_TIMEOUT: Duration = Duration::from_secs(60);

/// Cached node connection over a single auto-reconnecting websocket: the
/// subxt `RpcClient` carries every raw RPC (standard Substrate and custom
/// `midnight_*` methods alike), and the `OnlineClient` on top of it serves
/// the calls that need runtime metadata, such as submission.
#[derive(Clone)]
struct NodeConnection {
    rpc: RpcClient,
    client: OnlineClient<subxt::SubstrateConfig>,
}

/// A [`Provider`] backed by a node WebSocket connection for direct RPC
/// communication. It also holds the indexer URL that a wallet synced from it
/// replays.
///
/// The node connection is established lazily on first use, cached for the
/// provider's lifetime, and auto-reconnects with backoff on network drops.
/// One websocket carries everything: raw Substrate and `midnight_*` RPCs
/// through the subxt `RpcClient`, and the calls that need runtime metadata,
/// such as transaction submission, through the `OnlineClient` built on the
/// same transport.
///
/// Each node read ends within a bound that includes its connect: 60 s for a
/// contract state read, and 10 s for every other read. A read that the node
/// does not answer in that time returns [`ProviderError::Rpc`].
/// [`Self::health`] bounds its connect and each of its two queries to 10 s,
/// and reports a missing answer in its fields. [`Self::prepare`] and
/// [`Self::submit`] bound their connect and their metadata read to 10 s. The
/// SDK sets no deadline on the submit call or on the wait for a submitted
/// transaction (see [`PendingTx`]).
pub struct MidnightProvider {
    indexer_url: String,
    node_url: String,
    wallet: Option<AttachedWallet>,
    /// Proof backends for transaction building, one per ledger generation.
    /// Defaults to the local provers on first use; override with
    /// [`Self::with_proof_provider`] to use a remote prover or a custom
    /// implementation.
    proof_provider: Option<ProofProviders>,
    /// Optional store for per-contract private state and maintenance signing
    /// keys. Set with [`Self::with_private_state`]; absent for contracts whose
    /// witnesses are stateless.
    private_state: Option<Arc<dyn PrivateStateProvider>>,
    conn: Arc<RwLock<Option<NodeConnection>>>,
}

impl MidnightProvider {
    /// Create a provider from node WebSocket URL and indexer HTTP URL.
    ///
    /// The node connection is **not** established here; it is deferred to
    /// the first call that requires it.
    ///
    /// A wallet is built on its own and attached with
    /// [`Self::with_wallet`]. With the local implementation from
    /// `midnight-wallet`:
    /// ```rust,no_run
    /// # use midnight_provider::MidnightProvider;
    /// # use midnight_wallet::Wallet;
    /// # async fn f(phrase: &str) -> anyhow::Result<()> {
    /// # const NODE_URL: &str = "ws://localhost:9944";
    /// # const INDEXER_URL: &str = "http://localhost:8088";
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL);
    /// let wallet = Wallet::from_mnemonic(phrase)?.sync(&provider).await?;
    /// let provider = provider.with_wallet(wallet);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(node_url: &str, indexer_url: &str) -> Self {
        Self {
            indexer_url: indexer_url.to_string(),
            node_url: node_url.to_string(),
            wallet: None,
            proof_provider: None,
            private_state: None,
            conn: Arc::new(RwLock::new(None)),
        }
    }

    /// Override the proof backend used by [`Self::transfer_shielded`],
    /// [`Self::transfer_unshielded`], and [`Self::register_dust`].
    ///
    /// Defaults to [`ProofProviders::local`] if unset. Pass a
    /// [`RemoteProofServer`](crate::RemoteProofServer) to offload proving to an
    /// HTTP proof server, or any custom prover for each generation:
    ///
    /// ```rust,no_run
    /// # fn f() -> anyhow::Result<()> {
    /// # const NODE_URL: &str = "ws://localhost:9944";
    /// # const INDEXER_URL: &str = "http://localhost:8088";
    /// use std::sync::Arc;
    /// use midnight_provider::{MidnightProvider, RemoteProofServer};
    ///
    /// let prover = Arc::new(RemoteProofServer::new("http://localhost:6300".to_string()));
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL).with_proof_provider(prover);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_proof_provider(mut self, proof_provider: impl Into<ProofProviders>) -> Self {
        self.proof_provider = Some(proof_provider.into());
        self
    }

    /// The proof backend used to prove transactions built through this
    /// provider (transfers, dust registration, and every contract deploy /
    /// call / maintenance op driven by a `Contract` built on it).
    ///
    /// Returns the backends set via [`Self::with_proof_provider`], or
    /// [`ProofProviders::local`] when none was configured. Cheap to clone
    /// (`Arc`).
    pub fn proof_provider(&self) -> ProofProviders {
        self.proof_provider
            .clone()
            .unwrap_or_else(ProofProviders::local)
    }

    /// Attach a [`PrivateStateProvider`] for per-contract private state (and an
    /// optional per-contract signing-key slot; contract governance signs
    /// externally and does not use it).
    ///
    /// Optional: contracts whose witnesses are stateless never need it. When
    /// attached, a circuit call loads the contract's private state before
    /// execution and threads it through the witnesses via `WitnessContext`. It
    /// records the state after the call as a `Pending` snapshot before the
    /// submit. It then settles the snapshot by the chain's verdict.
    ///
    /// Start the next call on a contract only after the snapshot of the earlier
    /// call is no longer `Pending`. `docs/private-state.md` describes the
    /// journal and this rule.
    ///
    /// A store holds one wallet's private state, so open it for the unshielded
    /// address of the wallet that makes the calls.
    ///
    /// ```rust,no_run
    /// # use midnight_provider::MidnightProvider;
    /// # fn f(wallet_address: &str) -> anyhow::Result<()> {
    /// # const NODE_URL: &str = "ws://localhost:9944";
    /// # const INDEXER_URL: &str = "http://localhost:8088";
    /// use std::sync::Arc;
    /// use midnight_provider::FsPrivateStateProvider;
    ///
    /// let store = Arc::new(FsPrivateStateProvider::with_default_dir(wallet_address).unwrap());
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL).with_private_state(store);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_private_state(mut self, store: Arc<dyn PrivateStateProvider>) -> Self {
        self.private_state = Some(store);
        self
    }

    /// The attached [`PrivateStateProvider`], or `None` if none was set via
    /// [`Self::with_private_state`]. Cheap to clone (`Arc`) and safe to share
    /// across tasks.
    pub fn private_state(&self) -> Option<Arc<dyn PrivateStateProvider>> {
        self.private_state.clone()
    }

    /// The indexer URL this provider was built with, which a wallet synced
    /// from this provider replays.
    pub fn indexer_url(&self) -> &str {
        &self.indexer_url
    }

    /// Attach a wallet, and build transaction contexts from it.
    ///
    /// A synced `Wallet` from `midnight-wallet` goes in behind a `LocalWallet`.
    /// Anything else that implements [`WalletFacade`],
    /// [`ledger_8::WalletBuilds`] and [`ledger_9::WalletBuilds`] goes in as
    /// itself. To keep a handle on the attached wallet, attach an
    /// `Arc<LocalWallet>`, or an `Arc` of your own wallet:
    ///
    /// ```rust,no_run
    /// use std::sync::Arc;
    /// use midnight_provider::MidnightProvider;
    /// use midnight_wallet::{LocalWallet, Wallet, WalletFacade};
    ///
    /// # async fn example(provider: MidnightProvider, phrase: &str) -> Result<(), Box<dyn std::error::Error>> {
    /// let wallet = Arc::new(LocalWallet::new(Wallet::from_mnemonic(phrase)?.sync(&provider).await?));
    /// let provider = provider.with_wallet(Arc::clone(&wallet));
    /// let balance = wallet.balance().await;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The wallet serializes its own resyncs, rescans, coin watches and
    /// forgets, so the handle can run them too.
    ///
    /// Every read of the wallet, such as [`Self::balance`], returns the state
    /// of the last sync or resync and does not resync. [`Self::builds`], and
    /// so every build, resyncs first. To keep the reads fresh, start
    /// `LocalWallet::sync_in_background` on the handle, with this provider in
    /// an `Arc` as its chain. For one fresh read, call [`Self::resync_wallet`]
    /// before it.
    pub fn with_wallet(mut self, wallet: impl IntoWalletFacade) -> Self {
        let wallet = Arc::new(wallet.into_facade());
        self.wallet = Some(AttachedWallet {
            facade: wallet.clone(),
            ledger_8: wallet.clone(),
            ledger_9: wallet,
        });
        self
    }

    /// The attached wallet, as the API it implements.
    pub(crate) fn facade(&self) -> Option<Arc<dyn WalletFacade>> {
        self.wallet.as_ref().map(|wallet| wallet.facade.clone())
    }

    /// The attached wallet, or [`ProviderError::NoWallet`] when there is none.
    fn attached(&self) -> Result<&AttachedWallet, ProviderError> {
        self.wallet.as_ref().ok_or(ProviderError::NoWallet)
    }

    /// The ledger generation the chain runs at the current finalized block.
    ///
    /// The node answers through its `MidnightRuntimeApi::get_ledger_parameters`
    /// runtime API, and the tag of those parameters names the generation. It
    /// needs no wallet and runs no resync.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Rpc`] when the node cannot be reached, when the runtime
    /// returns an error, or when it returns parameters of a generation this
    /// build does not link.
    pub async fn ledger_version(&self) -> Result<LedgerVersion, ProviderError> {
        let parameters = self
            .call_runtime_api::<Result<Vec<u8>, subxt::dynamic::Value>>("get_ledger_parameters")
            .await?
            .map_err(|e| ProviderError::Rpc(format!("get_ledger_parameters failed: {e}")))?;
        LedgerVersion::of_ledger_parameters(&parameters)
            .map_err(|e| ProviderError::Rpc(format!("decoding the ledger parameters: {e}")))
    }

    /// Resync the attached wallet, then hand out its builds for the generation
    /// its state is in.
    ///
    /// The resync lets the builds see the chain's current view: the proof
    /// root, the TTL anchor, and, after a hard fork, the new generation.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn builds(&self) -> Result<Builds<'_>, ProviderError> {
        self.resync_wallet().await?;
        let wallet = self.attached()?;
        let provers = self.proof_provider();
        Ok(match wallet.facade.ledger_version().await {
            LedgerVersion::V8 => Builds::Ledger8(ledger_8::Builds::new(
                self,
                wallet.ledger_8.clone(),
                provers.ledger_8(),
            )),
            LedgerVersion::V9 => Builds::Ledger9(ledger_9::Builds::new(
                self,
                wallet.ledger_9.clone(),
                provers.ledger_9(),
            )),
        })
    }

    /// Return the wallet balance. [`Self::with_wallet`] says how fresh it is.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn balance(&self) -> Result<WalletBalance, ProviderError> {
        Ok(self.attached()?.facade.balance().await)
    }

    /// Enumerate the wallet's spendable shielded coins with their full coin
    /// info (nonce, token type, value) and pinning nullifier.
    ///
    /// Use this to address a specific coin for a circuit that spends it (e.g.
    /// `receiveShielded`), then hand the coin to the contract call builder's
    /// `with_shielded_inputs`. See [`SpendableShieldedCoin`].
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn spendable_shielded_coins(
        &self,
    ) -> Result<Vec<SpendableShieldedCoin>, ProviderError> {
        Ok(self.attached()?.facade.spendable_shielded_coins().await)
    }

    /// Whether the attached wallet has completed dust sync.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn dust_synced(&self) -> Result<bool, ProviderError> {
        Ok(self.attached()?.facade.dust_synced().await)
    }

    /// The unshielded UTXOs the attached wallet tracks.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn unshielded_utxos(&self) -> Result<Vec<TrackedUtxo>, ProviderError> {
        Ok(self.attached()?.facade.unshielded_utxos().await)
    }

    /// The chain's Dust and TTL parameters, as the attached wallet last synced
    /// them.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn parameters(&self) -> Result<ChainParameters, ProviderError> {
        Ok(self.attached()?.facade.parameters().await)
    }

    /// How far the attached wallet's sync has reached. See
    /// `Wallet::sync_cursors` on the implementing wallet.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn sync_cursors(&self) -> Result<SyncCursors, ProviderError> {
        Ok(self.attached()?.facade.sync_cursors().await)
    }

    /// Re-sync the wallet against the indexer.
    ///
    /// Resumes from the wallet's current event cursors, applies any new
    /// zswap/dust/unshielded events, refreshes the latest block context and
    /// ledger parameters, and commits the result (re-persisting it when the
    /// wallet was synced with a storage directory). Fails if no wallet is
    /// attached.
    ///
    /// Locking: the slow replay I/O runs **without** the wallet lock, so
    /// concurrent reads ([`Self::balance`], [`Self::dust_synced`], ...) keep
    /// completing while a resync is in flight; the wallet lock is only taken
    /// briefly to snapshot the replay inputs (read) and to commit the result
    /// (write). The wallet serializes concurrent resyncs.
    pub async fn resync_wallet(&self) -> Result<(), ProviderError> {
        // Boxed so the replay stays off the caller's frame. A debug build
        // makes this future tens of kilobytes, and an inlined one is carried
        // by every future that awaits it, up to the test or task that owns
        // the stack. Every build path awaits this one.
        Box::pin(self.resync_wallet_inner()).await
    }

    async fn resync_wallet_inner(&self) -> Result<(), ProviderError> {
        let wallet = &self.attached()?.facade;

        // The wallet resumes against its own indexer and checks its own chain
        // pin; this provider is only the node view those checks ask.
        wallet.resync(self).await?;
        Ok(())
    }

    /// Register a coin the wallet owns but cannot discover, then replay the
    /// shielded stream so the registration is honoured.
    ///
    /// A shielded coin normally reaches its owner through the discovery
    /// ciphertext on its output. When that ciphertext is missing, or is
    /// sealed to a key this wallet does not hold, the coin is still owned by
    /// the wallet's coin public key and still spendable, as long as the caller
    /// can rebuild the [`CoinInfo`] (nonce, token type, value) from somewhere
    /// else, such as a contract that evolves one public nonce per mint. Pass
    /// that rebuilt coin here and the wallet claims it from the chain's own
    /// output, decrypting nothing:
    ///
    /// ```rust,no_run
    /// # async fn f(
    /// #     provider: midnight_provider::MidnightProvider,
    /// #     nonce: [u8; 32],
    /// #     token_type: [u8; 32],
    /// #     value: u128,
    /// # ) -> anyhow::Result<()> {
    /// use midnight_provider::{CoinInfo, HashOutput, Nonce, ShieldedTokenType};
    ///
    /// provider
    ///     .watch_for_coin(CoinInfo {
    ///         nonce: Nonce(HashOutput(nonce)),
    ///         type_: ShieldedTokenType(HashOutput(token_type)),
    ///         value,
    ///     })
    ///     .await?;
    ///
    /// // Claimed coins now appear in the wallet's spendable set.
    /// let coins = provider.spendable_shielded_coins().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The replay covers the whole shielded stream, because a registration
    /// only takes effect while the coin's output is still ahead of the sync
    /// cursor. It costs more than a resync, so register every coin you know
    /// about in one call rather than calling this per coin, and treat it as a
    /// recovery step, not a routine one. Coins already recovered keep their
    /// place: each replay re-registers what the wallet holds. To check the
    /// outcome, look for the coin in [`Self::spendable_shielded_coins`]. If
    /// the rebuilt coin was wrong, drop the registration with
    /// [`Self::forget_coin`].
    ///
    /// If the replay fails, retry with [`Self::rescan_shielded`]: the wallet
    /// keeps the registration (see [`WalletFacade::watch_for_coins`]).
    ///
    /// Locking matches [`Self::resync_wallet`]: the replay runs without the
    /// wallet lock, and the wallet serializes the registration and the replay
    /// against resyncs. Recording the registration holds the wallet's write
    /// lock across a full state save, which blocks readers for as long as that
    /// write takes.
    pub async fn watch_for_coin(&self, coin: CoinInfo) -> Result<(), ProviderError> {
        self.watch_for_coins([coin]).await
    }

    /// [`Self::watch_for_coin`] for several coins, with one replay.
    ///
    /// Registering nothing is a no-op: no write, no replay.
    pub async fn watch_for_coins(
        &self,
        coins: impl IntoIterator<Item = CoinInfo> + Send,
    ) -> Result<(), ProviderError> {
        let wallet = &self.attached()?.facade;
        wallet
            .watch_for_coins(coins.into_iter().collect(), self)
            .await?;
        Ok(())
    }

    /// Drop a registration [`Self::watch_for_coin`] made, for a coin that
    /// turned out to be wrong.
    ///
    /// A registration whose rebuilt `CoinInfo` matches no on-chain output
    /// rides along on every later replay. This removes one. A coin the wallet
    /// already claimed is untouched, and no replay runs.
    pub async fn forget_coin(&self, coin: CoinInfo) -> Result<(), ProviderError> {
        self.forget_coins([coin]).await
    }

    /// [`Self::forget_coin`] for several coins, with one write.
    pub async fn forget_coins(
        &self,
        coins: impl IntoIterator<Item = CoinInfo> + Send,
    ) -> Result<(), ProviderError> {
        let wallet = &self.attached()?.facade;
        wallet.forget_coins(coins.into_iter().collect()).await?;
        Ok(())
    }

    /// Replay the shielded event stream from its first event and rebuild the
    /// wallet's shielded state from it.
    ///
    /// [`Self::watch_for_coin`] already does this for the coins it registers.
    /// Call this directly to rebuild shielded state that a resync cannot
    /// repair because its cursor has moved past the events in question.
    /// Dust state, unshielded state, and their cursors are left alone.
    ///
    /// A wallet whose state is in an earlier ledger generation than the chain
    /// crosses the fork first, with a resync against this provider.
    pub async fn rescan_shielded(&self) -> Result<(), ProviderError> {
        let wallet = &self.attached()?.facade;
        wallet.rescan_shielded(self).await?;
        Ok(())
    }

    /// Build a shielded (Zswap) transfer transaction.
    ///
    /// Returns a pending builder. `.await?` builds + submits and returns the
    /// resulting [`PendingTx`]; `.build().await?` returns the raw
    /// [`TransferResult`] without submitting (e.g. for inspection or custom
    /// routing). Either path selects the inputs and records them in the
    /// wallet's pending list as one transition, so a second build in this
    /// process cannot draw the same input. Proving runs after that, with the
    /// wallet free.
    pub fn transfer_shielded<'a>(
        &'a self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> ShieldedTransfer<'a> {
        ShieldedTransfer::new(self, token_type, amount, recipient)
    }

    /// Build one half of a native two-party shielded token swap.
    ///
    /// The half spends `give_amount` of `give_token` and creates an output for
    /// `receive_amount` of `receive_token` payable to this wallet. It is net
    /// unbalanced (`+give_token`, `-receive_token`) and therefore fee-less by
    /// construction, so awaiting the returned [`ShieldedSwap`] yields a
    /// [`DustlessTransaction`](crate::DustlessTransaction) directly rather than
    /// submitting.
    ///
    /// The counterparty builds the exact mirror
    /// (`shielded_swap(receive_token, receive_amount, give_token, give_amount)`).
    /// A sponsor (either party or a third party) then combines the two halves
    /// with [`Self::merge_transactions`] into a balanced, fee-less transaction,
    /// funds its Dust with [`Self::balance_transaction`], and submits:
    ///
    /// ```rust,no_run
    /// # use midnight_provider::{MidnightProvider, ShieldedTokenType};
    /// # async fn f(
    /// #     alice: MidnightProvider,
    /// #     bob: MidnightProvider,
    /// #     sponsor: MidnightProvider,
    /// #     token_x: ShieldedTokenType,
    /// #     dx: u128,
    /// #     token_y: ShieldedTokenType,
    /// #     dy: u128,
    /// # ) -> anyhow::Result<()> {
    /// let a_half = alice.shielded_swap(token_x, dx, token_y, dy).await?;
    /// let b_half = bob.shielded_swap(token_y, dy, token_x, dx).await?;
    /// let merged = sponsor.merge_transactions(&[a_half.into_bytes(), b_half.into_bytes()])?;
    /// let funded = sponsor.balance_transaction(&merged).await?;
    /// sponsor.submit_reserved(funded).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The two halves must carry exactly mirrored `(token, amount)` pairs or the
    /// merge won't balance; the builder can't enforce the counterparty's side.
    /// See [`Self::transfer_shielded`] for reservation semantics.
    pub fn shielded_swap<'a>(
        &'a self,
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
    ) -> ShieldedSwap<'a> {
        ShieldedSwap::new(self, give_token, give_amount, receive_token, receive_amount)
    }

    /// Build an unshielded (UTXO) transfer transaction. See
    /// [`Self::transfer_shielded`] for reservation semantics and the
    /// `.await` vs `.build()` distinction.
    pub fn transfer_unshielded<'a>(
        &'a self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> UnshieldedTransfer<'a> {
        UnshieldedTransfer::new(self, token_type, amount, recipient)
    }

    /// Build a dust-address registration transaction. See
    /// [`Self::transfer_shielded`] for reservation semantics and the
    /// `.await` vs `.build()` distinction.
    ///
    /// One call registers one tNIGHT UTXO and pays its own fee from that
    /// UTXO. Call it again until [`DustBalance::unregistered_night_utxos`]
    /// is 0. [`TransferBuilder::prepare_register_dust`] gives the full rule
    /// and the meaning of `utxo_ctime`.
    ///
    /// [`DustBalance::unregistered_night_utxos`]: midnight_types::DustBalance::unregistered_night_utxos
    /// [`TransferBuilder::prepare_register_dust`]: midnight_types::ledger_9::TransferBuilder::prepare_register_dust
    pub fn register_dust(&self, utxo_ctime: Option<u64>) -> DustRegistration<'_> {
        DustRegistration::new(self, utxo_ctime)
    }

    // -- Internal build paths driven by the transfer/register builders. --

    /// Prepare a build under the attached wallet, then prove without it, on
    /// the generation the wallet's state is in.
    pub(crate) async fn build_then_prove(
        &self,
        request: TransferRequest,
    ) -> Result<TransferResult, ProviderError> {
        // Boxed; see the frame-size note on `resync_wallet`.
        Box::pin(self.build_then_prove_inner(request)).await
    }

    async fn build_then_prove_inner(
        &self,
        request: TransferRequest,
    ) -> Result<TransferResult, ProviderError> {
        // Each arm is boxed so the caller's frame holds one future, not both
        // generations' (see the frame-size note on `resync_wallet`).
        match self.builds().await? {
            Builds::Ledger8(builds) => Box::pin(builds.build_then_prove(request)).await,
            Builds::Ledger9(builds) => Box::pin(builds.build_then_prove(request)).await,
        }
    }

    /// Submit proven transaction bytes to the node over the WebSocket RPC.
    ///
    /// Returns a [`PendingTx`] handle that lets the caller await inclusion
    /// (`wait_best`) and finalization (`wait_finalized`). The provider's
    /// `node_url` is used as the connection target — callers don't repeat it.
    ///
    /// This is for bytes that this wallet did not build. It carries no
    /// reservation, so a rejection hands no inputs back. For a
    /// [`TransferResult`] that this wallet built, use
    /// [`Self::submit_reserved`].
    pub async fn submit(&self, tx_bytes: &[u8]) -> Result<PendingTx, ProviderError> {
        let conn = query_within(QUERY_TIMEOUT, self.get_or_connect()).await?;
        submit::submit_bytes(&conn.client, tx_bytes).await
    }

    /// Wrap proven transaction bytes in the unsigned `send_mn_transaction`
    /// extrinsic without submitting it, returning a [`crate::PreparedTx`]
    /// whose extrinsic and transaction hashes are already known. Submit it
    /// with [`crate::PreparedTx::submit`]. Lets a caller durably record state
    /// keyed by the extrinsic hash *before* the transaction reaches the
    /// mempool.
    ///
    /// This builds the extrinsic locally from the node's metadata, which
    /// checks only the call's shape. The node validates the transaction only
    /// at submit.
    ///
    /// This is for bytes that this wallet did not build. The `PreparedTx`
    /// carries no reservation. For a [`TransferResult`] that this wallet
    /// built, use [`Self::prepare_reserved`].
    pub async fn prepare(&self, tx_bytes: &[u8]) -> Result<submit::PreparedTx, ProviderError> {
        let conn = query_within(QUERY_TIMEOUT, self.get_or_connect()).await?;
        submit::prepare_bytes(&conn.client, tx_bytes).await
    }

    /// [`Self::prepare`] for a transaction that this provider's wallet built,
    /// with the reservation of its build on the returned
    /// [`PreparedTx`](crate::PreparedTx).
    ///
    /// The inputs in `result.spent` go back to the wallet when the prepare
    /// fails, before the error returns, and when the `PreparedTx` drops or is
    /// released before its submit. Its submit moves the reservation to the
    /// [`PendingTx`], which hands the inputs back when a wait reads
    /// [`SubmitError::Invalid`](crate::SubmitError::Invalid).
    ///
    /// Submit the transaction only through the returned `PreparedTx`. Bytes
    /// that you send another way can land after a drop of the `PreparedTx`
    /// hands their inputs back, and a later build can then spend the same
    /// inputs.
    ///
    /// # Errors
    ///
    /// The errors of [`Self::prepare`].
    pub async fn prepare_reserved(
        &self,
        result: TransferResult,
    ) -> Result<submit::PreparedTx, ProviderError> {
        let held = HeldInputs::of(result.spent, self.facade());
        match self.prepare(&result.tx_bytes).await {
            Ok(prepared) => Ok(prepared.holding(held)),
            // Release on every error: a prepare sends nothing to the node.
            Err(err) => {
                held.release().await;
                Err(err)
            }
        }
    }

    /// Merge proven transactions into one, for multi-party flows: e.g. combining
    /// a contract call built without submitting (`Contract::build_call_with`, or
    /// the generated `circuits().<circuit>().build().await`) with a
    /// counterparty's already-proven transaction before submitting.
    ///
    /// Each input is a tagged-serialized proven transaction (the byte output of
    /// any build path). The result is the merged transaction, ready for
    /// [`Self::submit`] / [`Self::prepare`]. Merging combines the transactions'
    /// intents and Zswap offers and sums their binding randomness; it does NOT
    /// rebalance, so every input must already balance its own tokens.
    ///
    /// **Intent segments must not collide.** The ledger rejects a merge where
    /// two inputs both carry an intent at the same segment. A self-funded build
    /// attaches its Dust-fee intent at the fallible segment (1); a contract call
    /// and an unshielded (UTXO) transfer place their action there too. So at
    /// most one merged input may carry a segment-1 intent, and two self-funded
    /// transactions cannot be merged directly. The supported multi-party shape
    /// is "one party pays": the contributors build fee-less
    /// ([`crate::DustlessBuilder::without_dust`]) and a single payer covers the
    /// fees with [`Self::balance_transaction`] (whose fee intent rides a distinct,
    /// non-colliding segment). A Dustless *shielded* transfer carries no intent
    /// at all (pure Zswap), so it always merges cleanly.
    ///
    /// Errors ([`ProviderError::Transaction`]) when given no transactions, when
    /// a byte string fails to deserialize, when the transactions are of
    /// different ledger generations, or when two transactions cannot be
    /// merged (colliding intent segments or mismatched network ids). Purely
    /// local; nothing is sent to the node.
    pub fn merge_transactions(&self, txs: &[Vec<u8>]) -> Result<Vec<u8>, ProviderError> {
        let first = txs.first().ok_or_else(|| {
            ProviderError::Transaction(
                "merge_transactions requires at least one transaction".into(),
            )
        })?;
        let ledger = transaction_ledger_version(first)?;
        for tx in &txs[1..] {
            let other = transaction_ledger_version(tx)?;
            if other != ledger {
                return Err(ProviderError::Transaction(format!(
                    "cannot merge a {ledger} transaction with a {other} transaction"
                )));
            }
        }
        match ledger {
            LedgerVersion::V8 => ledger_8::merge_transactions(txs),
            LedgerVersion::V9 => ledger_9::merge_transactions(txs),
        }
    }

    /// Pay the fees for an external party's proven, fee-less transaction from
    /// this provider's wallet, returning the completed transaction ready to
    /// submit.
    ///
    /// This is the "one party pays fees" flow (midnight-js `balanceTransaction`):
    /// the caller (fee payer) covers the Dust fees for a transaction someone
    /// else already built and proved, without holding their keys. Purely
    /// additive, the external transaction's proofs are untouched; a separately
    /// proven Dust-paying transaction is combined in via `Transaction::merge`.
    ///
    /// Covers **fees only**: the transaction must already be balanced on every
    /// non-fee token. A token deficit (e.g. an unfunded swap side) is rejected;
    /// covering it from the funding wallet is a planned follow-up.
    ///
    /// The wallet draws the Dust and reserves it as one transition, so two
    /// calls on one provider cannot draw the same Dust. Proving, the merge
    /// and the price run afterwards, with the wallet free, and a failure in
    /// any of them hands the Dust back.
    ///
    /// The result holds the merged transaction, the Dust it reserved in
    /// `spent`, and the fee of the merged transaction in `fee_speck`. Submit
    /// it with [`Self::submit_reserved`], which hands the Dust back when the
    /// node never gets the transaction or rejects it. A transaction that
    /// already pays its own fee comes back unchanged, with nothing reserved.
    pub async fn balance_transaction(
        &self,
        tx_bytes: &[u8],
    ) -> Result<TransferResult, ProviderError> {
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        Box::pin(self.balance_transaction_inner(tx_bytes)).await
    }

    async fn balance_transaction_inner(
        &self,
        tx_bytes: &[u8],
    ) -> Result<TransferResult, ProviderError> {
        let ledger = transaction_ledger_version(tx_bytes)?;
        match self.builds().await? {
            Builds::Ledger8(builds) if ledger == LedgerVersion::V8 => {
                builds.balance_transaction(tx_bytes).await
            }
            Builds::Ledger9(builds) if ledger == LedgerVersion::V9 => {
                builds.balance_transaction(tx_bytes).await
            }
            builds => Err(midnight_types::WalletError::LedgerMismatch {
                expected: builds.ledger_version(),
                found: ledger,
            }
            .into()),
        }
    }

    /// Hand back the inputs a build reserved, because that build will never
    /// reach the chain. See [`WalletFacade::release`].
    ///
    /// A build reserves its inputs so a later one does not re-select them, so
    /// a transaction that is rejected at submit, or built and then abandoned,
    /// keeps its coins out of circulation until the TTL window elapses.
    /// Releasing frees them at once.
    ///
    /// Only for a transaction that cannot land. Releasing one still in flight
    /// lets a later build re-select the same inputs, and the loser is rejected
    /// on chain.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn release(&self, spent: &SpentInputs) -> Result<(), ProviderError> {
        self.attached()?.facade.release(spent).await;
        Ok(())
    }

    /// Submit a transaction that this provider's wallet built, and carry the
    /// reservation of its build on the returned handle.
    ///
    /// Use it for the [`TransferResult`] of a `.build()` on a transfer, of
    /// [`Self::balance_transaction`], or of a deploy. The inputs in
    /// `result.spent` then go back to the wallet in two cases:
    ///
    /// - The prepare fails: the connect, the metadata read, or the encoding of
    ///   the extrinsic. The transaction never left this process, whatever the
    ///   error, and the release completes before the error returns.
    /// - A wait on the [`PendingTx`] reads
    ///   [`SubmitError::Invalid`](crate::SubmitError::Invalid), the node's
    ///   definitive rejection. It arrives after this method returns, so the
    ///   reservation travels with the handle.
    ///
    /// Any other failure keeps the inputs reserved until their TTL passes,
    /// because the node can still hold the transaction. Use [`Self::submit`]
    /// for bytes that this wallet did not build. This is
    /// [`Self::prepare_reserved`], then [`PreparedTx::submit`](crate::PreparedTx::submit).
    pub async fn submit_reserved(
        &self,
        result: TransferResult,
    ) -> Result<PendingTx, ProviderError> {
        self.prepare_reserved(result).await?.submit().await
    }

    /// The attached wallet's shielded public keys. See
    /// `Wallet::shielded_public_keys` on the implementing wallet.
    ///
    /// Returns [`ProviderError::NoWallet`] if no wallet is attached.
    pub async fn shielded_public_keys(
        &self,
    ) -> Result<(CoinPublicKey, EncryptionPublicKey), ProviderError> {
        Ok(self.attached()?.facade.shielded_public_keys().await)
    }

    /// Get or create the node connection.
    ///
    /// Built once and cached for the provider's lifetime: the underlying
    /// websocket auto-reconnects with backoff, so a network drop needs no
    /// cache invalidation.
    ///
    /// The caller runs the connect inside the bound of its query (see
    /// [`QUERY_TIMEOUT`]), because the client retries a refused dial forever.
    /// A caller can drop the connect at any await. The connection then
    /// closes when its pending requests end.
    async fn get_or_connect(&self) -> Result<NodeConnection, ProviderError> {
        {
            let guard = self.conn.read().await;
            if let Some(ref conn) = *guard {
                return Ok(conn.clone());
            }
        }

        info!(url = %self.node_url, "Connecting to Midnight node");
        let reconnecting = ReconnectingRpcClient::builder()
            .build(&self.node_url)
            .await
            .map_err(|e| ProviderError::Rpc(e.to_string()))?;
        let rpc = RpcClient::new(reconnecting);
        // The runtime-aware client shares the same auto-reconnecting
        // transport.
        let (backend, mut driver) = CombinedBackend::builder()
            .build(rpc.clone())
            .await
            .map_err(|e| ProviderError::Rpc(format!("building the node backend: {e}")))?;
        let client = OnlineClient::<subxt::SubstrateConfig>::from_backend(Arc::new(backend))
            .await
            .map_err(|e| ProviderError::Rpc(format!("building the runtime client: {e}")))?;

        let mut guard = self.conn.write().await;
        let conn = guard.get_or_insert_with(|| {
            // Spawn the follow driver only for the kept connection: the
            // driver holds the client open until a hard error.
            tokio::spawn(async move {
                while let Some(res) = driver.next().await {
                    if let Err(e) = res {
                        debug!(error = %e, "chainHead follow error");
                    }
                }
            });
            NodeConnection { rpc, client }
        });
        Ok(conn.clone())
    }

    async fn call_runtime_api<R: subxt::ext::scale_decode::DecodeAsType>(
        &self,
        method: &str,
    ) -> Result<R, ProviderError> {
        query_within(QUERY_TIMEOUT, async {
            let conn = self.get_or_connect().await?;
            let payload =
                subxt::dynamic::runtime_api_call::<(), R>("MidnightRuntimeApi", method, ());
            conn.client
                .at_current_block()
                .await
                .map_err(|e| ProviderError::Rpc(format!("reading the finalized block: {e}")))?
                .runtime_apis()
                .call(payload)
                .await
                .map_err(|e| ProviderError::Rpc(format!("calling {method}: {e}")))
        })
        .await
    }
}

#[async_trait]
impl Provider for MidnightProvider {
    async fn get_contract_state(
        &self,
        address: &str,
        at: Option<BlockHash>,
    ) -> Result<Option<String>, ProviderError> {
        self.get_state_from_node(address, at).await
    }
}

/// The JSON-RPC error code of every `midnight_contractState` refusal (the
/// node's `INVALID_PARAMS_CODE`).
const CONTRACT_STATE_ERROR_CODE: i32 = -32602;

/// The `midnight_contractState` refusal for an address that holds no contract,
/// from node 2.0 on. Earlier nodes return an empty state instead.
const CONTRACT_NOT_PRESENT: &str = "Contract not present at the requested address";

/// Whether a `midnight_contractState` error says that no contract exists at
/// the address.
///
/// Only the node's own "not present" refusal qualifies. The node also answers
/// "Unable to get requested contract state" for a block it does not know or no
/// longer holds, so that refusal stays an error rather than an absent contract.
fn names_no_contract(error: &subxt::rpcs::Error) -> bool {
    matches!(
        error,
        subxt::rpcs::Error::User(subxt::rpcs::UserError { code, message, .. })
            if *code == CONTRACT_STATE_ERROR_CODE && message == CONTRACT_NOT_PRESENT
    )
}

#[cfg(test)]
mod names_no_contract_tests {
    use super::*;

    fn refusal(message: &str) -> subxt::rpcs::Error {
        subxt::rpcs::Error::User(subxt::rpcs::UserError {
            code: -32602,
            message: message.to_owned(),
            data: None,
        })
    }

    /// Mapping the other -32602 refusal to `None` reports a contract missing
    /// for a block that the node does not hold.
    #[test]
    fn only_the_not_present_refusal_names_no_contract() {
        assert!(names_no_contract(&refusal(
            "Contract not present at the requested address"
        )));
        assert!(!names_no_contract(&refusal(
            "Unable to get requested contract state"
        )));
    }
}

/// The node's block-header type under the chain's Substrate config; `number`
/// is the block height.
pub type NodeHeader = <subxt::SubstrateConfig as subxt::Config>::Header;

impl MidnightProvider {
    /// Get the current block number from the node (`chain_getHeader.number`).
    pub async fn get_block_number(&self) -> Result<u64, ProviderError> {
        query_within(QUERY_TIMEOUT, async {
            let conn = self.get_or_connect().await?;

            let header = legacy_rpc(&conn)
                .chain_get_header(None)
                .await
                .map_err(rpc_error("chain_getHeader"))?;

            debug!(header = ?header, "chain_getHeader response");

            header
                .map(|header| header.number)
                .ok_or_else(|| ProviderError::Rpc("chain_getHeader returned no header".to_string()))
        })
        .await
    }

    /// Get the hash of the node's best block (`chain_getBlockHash` with no
    /// height).
    ///
    /// A read given no block hash, such as
    /// [`get_state_from_node`](Self::get_state_from_node) with `None`, reads
    /// at the best block at the moment of that read. Pass this hash to such
    /// reads to keep two of them at one block.
    pub async fn best_block_hash(&self) -> Result<BlockHash, ProviderError> {
        query_within(QUERY_TIMEOUT, async {
            let conn = self.get_or_connect().await?;

            legacy_rpc(&conn)
                .chain_get_block_hash(None)
                .await
                .map_err(rpc_error("chain_getBlockHash"))?
                .ok_or_else(|| {
                    ProviderError::Rpc("chain_getBlockHash returned no best block".to_string())
                })
        })
        .await
    }

    /// Get the latest finalized block height (`archive_v1_finalizedHeight`).
    ///
    /// Finalized blocks cannot reorg (GRANDPA), so heights at or below this
    /// are safe for observers that must never see a block twice. Part of the
    /// `archive_v1` spec, the replacement for the legacy `chain_*` RPCs.
    pub async fn get_finalized_block_height(&self) -> Result<u64, ProviderError> {
        query_within(QUERY_TIMEOUT, async {
            let conn = self.get_or_connect().await?;

            archive_rpc(&conn)
                .archive_v1_finalized_height()
                .await
                .map(|height| height as u64)
                .map_err(rpc_error("archive_v1_finalizedHeight"))
        })
        .await
    }

    /// Get the hashes of the blocks at `height` (`archive_v1_hashByHeight`):
    /// exactly one for a height at or below the finalized height, empty when
    /// the chain has not reached `height`, and possibly several while
    /// unfinalized forks exist at it.
    ///
    /// A finalized height's hash pins historical reads such as
    /// [`get_state_from_node`](Self::get_state_from_node).
    pub async fn get_block_hashes_by_height(
        &self,
        height: u64,
    ) -> Result<Vec<BlockHash>, ProviderError> {
        query_within(QUERY_TIMEOUT, async {
            let conn = self.get_or_connect().await?;

            let height = usize::try_from(height).map_err(|_| {
                ProviderError::Rpc(format!("block height {height} overflows usize"))
            })?;
            archive_rpc(&conn)
                .archive_v1_hash_by_height(height)
                .await
                .map_err(rpc_error("archive_v1_hashByHeight"))
        })
        .await
    }

    /// Get the header of the block with `hash` (`archive_v1_header`), or
    /// `None` when the node does not know the hash. The SCALE-encoded
    /// response decodes into the config-derived [`NodeHeader`].
    pub async fn get_block_header(
        &self,
        hash: BlockHash,
    ) -> Result<Option<NodeHeader>, ProviderError> {
        query_within(QUERY_TIMEOUT, async {
            let conn = self.get_or_connect().await?;

            archive_rpc(&conn)
                .archive_v1_header(hash)
                .await
                .map_err(rpc_error("archive_v1_header"))
        })
        .await
    }

    /// Get the timestamp of the block with `hash`, as time since the Unix
    /// epoch.
    ///
    /// Reads `Timestamp::Now` in that block's state. Genesis sets no
    /// timestamp, so it reads as zero. Errors when the node does not know
    /// `hash`, or no longer holds the state at it.
    pub async fn get_block_timestamp(&self, hash: BlockHash) -> Result<Duration, ProviderError> {
        query_within(QUERY_TIMEOUT, async {
            let conn = self.get_or_connect().await?;

            let millis = conn
                .client
                .at_block(hash)
                .await
                .map_err(|e| ProviderError::Rpc(format!("reading block {hash:#x}: {e}")))?
                .storage()
                .fetch(subxt::dynamic::storage::<(), u64>("Timestamp", "Now"), ())
                .await
                .map_err(|e| {
                    ProviderError::Rpc(format!("reading Timestamp::Now at {hash:#x}: {e}"))
                })?
                .decode()
                .map_err(|e| {
                    ProviderError::Rpc(format!("decoding Timestamp::Now at {hash:#x}: {e}"))
                })?;
            Ok(Duration::from_millis(millis))
        })
        .await
    }

    /// The node's chain-spec display name (substrate `system_chain`), e.g.
    /// `"Midnight Devnet"`.
    ///
    /// This is a human-readable label, **not** the ledger network id. It is not
    /// interchangeable with [`Network`], and a label such as the one above is
    /// not even a valid network name. The address prefixes come from
    /// [`MidnightProvider::network`]. The network the node runs comes from
    /// [`MidnightProvider::ledger_network_id`].
    pub async fn system_chain(&self) -> Result<String, ProviderError> {
        query_within(QUERY_TIMEOUT, async {
            let conn = self.get_or_connect().await?;

            let chain: String = conn
                .rpc
                .request("system_chain", RpcParams::new())
                .await
                .map_err(rpc_error("system_chain"))?;

            debug!(chain = %chain, "system_chain response");

            Ok(chain)
        })
        .await
    }

    /// The network that the node's runtime reports, at the current finalized
    /// block.
    ///
    /// The node answers through its `MidnightRuntimeApi::get_network_id`
    /// runtime API. This is the network the chain runs, so it can differ from
    /// [`MidnightProvider::network`], which is the network the attached wallet
    /// derives addresses for. Before its replay, a pinned wallet sync (the
    /// default) compares this network with the one it syncs as, and fails
    /// with
    /// [`WalletError::NetworkMismatch`](midnight_types::WalletError::NetworkMismatch)
    /// when they differ. A sync with no network takes this one, and fails
    /// when there is none. An error here does not fail a sync that names its
    /// network.
    ///
    /// It needs no wallet and runs no resync. The id goes through the parse
    /// of a [`Network`]. Errors when the node cannot be reached, when its
    /// metadata describes no such API or another return type, or when the
    /// parse refuses the id, as it does the empty id of a node with no
    /// network id set.
    pub async fn ledger_network_id(&self) -> Result<Network, ProviderError> {
        let id = self.call_runtime_api::<String>("get_network_id").await?;
        id.parse()
            .map_err(|e| ProviderError::Rpc(format!("get_network_id: {e}")))
    }

    /// The [`Network`] this provider's wallet derives addresses for.
    ///
    /// The network the node runs comes from
    /// [`MidnightProvider::ledger_network_id`]. Errors if no wallet is
    /// attached.
    pub async fn network(&self) -> Result<Network, ProviderError> {
        Ok(self.attached()?.facade.network().await)
    }

    /// Best-effort health status of the node.
    ///
    /// After the connect, it queries `system_health` and `chain_getHeader` at
    /// the same time. Failures surface in the returned [`Health`] fields: a
    /// field is `None` when its query fails or gets no answer within 10 s,
    /// and `node_connected` is `false` when neither query answers.
    pub async fn health(&self) -> Health {
        let conn = match query_within(QUERY_TIMEOUT, self.get_or_connect()).await {
            Ok(conn) => conn,
            Err(err) => {
                warn!(url = %self.node_url, error = %err, "Failed to connect to Midnight node");
                return Health {
                    node_connected: false,
                    block_height: None,
                    peers: None,
                    is_syncing: None,
                };
            }
        };

        let (sys_health, header) = tokio::join!(
            query_within(QUERY_TIMEOUT, async {
                conn.rpc
                    .request::<serde_json::Value>("system_health", RpcParams::new())
                    .await
                    .map_err(|e| ProviderError::Rpc(e.to_string()))
            }),
            query_within(QUERY_TIMEOUT, async {
                legacy_rpc(&conn)
                    .chain_get_header(None)
                    .await
                    .map_err(|e| ProviderError::Rpc(e.to_string()))
            }),
        );
        let sys_health = sys_health
            .inspect_err(|e| warn!(error = %e, "system_health RPC call failed"))
            .ok();
        let header = header
            .inspect_err(|e| warn!(error = %e, "chain_getHeader RPC call failed"))
            .ok();

        debug!(health = ?sys_health, "system_health response");
        debug!(header = ?header, "chain_getHeader response");

        let peers = sys_health
            .as_ref()
            .and_then(|v| v.get("peers"))
            .and_then(|v| v.as_u64());
        let is_syncing = sys_health
            .as_ref()
            .and_then(|v| v.get("isSyncing"))
            .and_then(|v| v.as_bool());
        let block_height = header
            .as_ref()
            .and_then(Option::as_ref)
            .map(|header| header.number);

        Health {
            node_connected: sys_health.is_some() || header.is_some(),
            block_height,
            peers,
            is_syncing,
        }
    }

    /// Fetch full contract state via the node RPC (`midnight_contractState`),
    /// at `at_block_hash`, or at the node's best block when it is `None`.
    ///
    /// Returns the hex-encoded serialized contract state, or `None` if no
    /// contract exists at `address` in that block. This uses the standard
    /// node RPC that is available on all devnet nodes (unlike
    /// `midnight_queryContractState` which requires a custom node build).
    ///
    /// # Errors
    ///
    /// [`ProviderError::Rpc`] when the node cannot be reached or refuses the
    /// read, for example for a block it does not know or no longer holds.
    pub async fn get_state_from_node(
        &self,
        address: &str,
        at_block_hash: Option<BlockHash>,
    ) -> Result<Option<String>, ProviderError> {
        query_within(STATE_QUERY_TIMEOUT, async {
            let conn = self.get_or_connect().await?;
            match conn
                .rpc
                .request::<String>(
                    "midnight_contractState",
                    rpc_params![address, at_block_hash],
                )
                .await
            {
                Ok(hex_state) => Ok((!hex_state.is_empty()).then_some(hex_state)),
                Err(e) if names_no_contract(&e) => Ok(None),
                Err(e) => Err(ProviderError::Rpc(e.to_string())),
            }
        })
        .await
    }

    /// Query contract state with an optional block hash pin.
    ///
    /// When `at_block_hash` is `None`, the node returns state at the latest
    /// block. When set, the node returns state as of that specific block hash.
    pub(crate) async fn query_contract_state_at(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
        at_block_hash: Option<BlockHash>,
    ) -> Result<Vec<StateQueryResult>, ProviderError> {
        query_within(STATE_QUERY_TIMEOUT, async {
            let conn = self.get_or_connect().await?;
            conn.rpc
                .request(
                    "midnight_queryContractState",
                    rpc_params![address, queries, at_block_hash],
                )
                .await
                .map_err(|e| ProviderError::Rpc(e.to_string()))
        })
        .await
    }
}

/// The attached wallet's builds, on the generation its state is in. See
/// [`MidnightProvider::builds`].
#[derive(Debug)]
pub enum Builds<'a> {
    /// The builds of a wallet whose state is on ledger 8.
    Ledger8(ledger_8::Builds<'a>),
    /// The builds of a wallet whose state is on ledger 9.
    Ledger9(ledger_9::Builds<'a>),
}

impl Builds<'_> {
    /// The generation these builds build for.
    pub fn ledger_version(&self) -> LedgerVersion {
        match self {
            Self::Ledger8(_) => LedgerVersion::V8,
            Self::Ledger9(_) => LedgerVersion::V9,
        }
    }
}

/// The attached wallet, reached only through its API: as the facade, and as
/// the builds of each generation.
///
/// How its state is shared is the implementation's business: every method
/// on [`WalletFacade`] returns an owned value or covers one transition, so
/// nothing here holds a lock. Cloning an `Arc` is cheap and safe.
struct AttachedWallet {
    facade: Arc<dyn WalletFacade>,
    ledger_8: Arc<dyn midnight_wallet_facade::ledger_8::WalletBuilds>,
    ledger_9: Arc<dyn midnight_wallet_facade::ledger_9::WalletBuilds>,
}

/// The generation of a proven transaction, from its tag.
fn transaction_ledger_version(tx_bytes: &[u8]) -> Result<LedgerVersion, ProviderError> {
    LedgerVersion::of_transaction(tx_bytes)
        .map_err(|e| ProviderError::Transaction(format!("deserialize transaction: {e}")))
}

/// The inputs a build has reserved, released if the build does not finish.
///
/// The reservation is recorded before proving, so anything that ends a build
/// early has to hand the inputs back or they stay unusable until their TTL
/// elapses. Run the rest of the build through [`Self::keep_on_ok`], which
/// keeps the reservation when the build succeeds and awaits the release when
/// it fails. A caller can drop the build future (a `timeout`, a `select!`, an
/// aborted task) before the release runs. `Drop` cannot await, so under a
/// Tokio runtime it hands the release to the runtime. With no runtime, the
/// inputs wait for their TTL.
pub struct HeldInputs {
    wallet: Option<Arc<dyn WalletFacade>>,
    spent: SpentInputs,
}

impl HeldInputs {
    pub(crate) fn of(spent: SpentInputs, wallet: Option<Arc<dyn WalletFacade>>) -> Self {
        Self { wallet, spent }
    }

    /// The inputs this guard holds. A build reads them before
    /// [`Self::keep_on_ok`] when its result has to name them beside the
    /// inputs of a later reservation (see
    /// [`SpentInputs::merge`](midnight_types::SpentInputs::merge)).
    pub fn spent(&self) -> &SpentInputs {
        &self.spent
    }

    /// Run `work` and keep the inputs reserved only if it succeeds.
    ///
    /// `work` is the rest of the build. On `Ok` the build can reach the chain,
    /// so the reservation stands. On `Err` the release completes before the
    /// error returns, so a caller that observes the error also observes the
    /// inputs back.
    ///
    /// # Errors
    ///
    /// Returns the error of `work`, after the inputs go back to the wallet.
    ///
    /// # Cancellation
    ///
    /// Under a Tokio runtime, dropping the returned future while `work` runs
    /// releases the inputs in the background, as dropping `self` does. A
    /// release that has started runs to its end, even when the returned
    /// future is dropped during it.
    ///
    /// With no Tokio runtime, a drop releases nothing, and the release on
    /// `Err` runs inline. Inputs that a drop leaves reserved stay unusable
    /// until their TTL elapses.
    pub fn keep_on_ok<T, E>(
        mut self,
        work: impl Future<Output = Result<T, E>>,
    ) -> impl Future<Output = Result<T, E>> {
        // Boxed before the returned future exists: an `async fn` stores its
        // arguments, so a caller's future would carry the whole build inline.
        let work = Box::pin(work);
        async move {
            let result = work.await;
            // Disarm `Drop` on both paths: `Ok` keeps the reservation, and
            // `Err` releases it here.
            let wallet = self.wallet.take();
            let spent = std::mem::take(&mut self.spent);
            if result.is_err()
                && let Some(wallet) = wallet
            {
                hand_back(wallet, spent).await;
            }
            result
        }
    }

    /// Hand the inputs back now. They are back when this returns, as with an
    /// `Err` of [`Self::keep_on_ok`].
    pub(crate) async fn release(mut self) {
        let spent = std::mem::take(&mut self.spent);
        if let Some(wallet) = self.wallet.take() {
            hand_back(wallet, spent).await;
        }
    }

    /// Disarm the guard and keep the inputs reserved, for a transaction that
    /// the node can hold. The returned reservation lets a later definitive
    /// rejection hand them back. `None` when no wallet holds them.
    pub(crate) fn into_reservation(mut self) -> Option<crate::submit::Reservation> {
        let wallet = self.wallet.take()?;
        Some(crate::submit::Reservation::new(
            wallet,
            std::mem::take(&mut self.spent),
        ))
    }
}

/// Hand `spent` back to `wallet`, and return when the wallet has them.
///
/// Under a Tokio runtime the release runs as a task of its own, so a drop of
/// this future cannot cut it short. With no runtime, it runs inline.
async fn hand_back(wallet: Arc<dyn WalletFacade>, spent: SpentInputs) {
    if spent.is_empty() {
        return;
    }
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            let _ = handle
                .spawn(async move { wallet.release(&spent).await })
                .await;
        }
        Err(_) => wallet.release(&spent).await,
    }
}

impl std::fmt::Debug for HeldInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldInputs")
            .field("spent", &self.spent)
            .field("armed", &self.wallet.is_some())
            .finish_non_exhaustive()
    }
}

impl Drop for HeldInputs {
    fn drop(&mut self) {
        let Some(wallet) = self.wallet.take() else {
            return;
        };
        let spent = std::mem::take(&mut self.spent);
        if spent.is_empty() {
            return;
        }
        // With no runtime, nothing can run the release, so the inputs wait
        // for their TTL.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { wallet.release(&spent).await });
        }
    }
}

/// The node facts a chain pin asks for. A wallet sync asks them through
/// [`SyncSource`](midnight_types::chain_pin::SyncSource), and the provider's
/// own resync checks pins through the same answers.
///
/// A query that fails, or that gets no answer within its bound, gives
/// `None`.
#[async_trait]
impl midnight_types::chain_pin::ChainView for MidnightProvider {
    async fn block_hashes_at(&self, height: u64) -> Option<Vec<BlockHash>> {
        self.get_block_hashes_by_height(height).await.ok()
    }

    async fn finalized_height(&self) -> Option<u64> {
        self.get_finalized_block_height().await.ok()
    }
}

/// The node's network has the same bound as the `ChainView` answers of this
/// provider.
#[async_trait]
impl midnight_types::chain_pin::SyncSource for MidnightProvider {
    fn indexer_url(&self) -> &str {
        MidnightProvider::indexer_url(self)
    }

    async fn network(&self) -> Option<Network> {
        match self.ledger_network_id().await {
            Ok(network) => Some(network),
            Err(e) => {
                warn!(error = %e, "could not read the node's network");
                None
            }
        }
    }
}

/// The answer to a node `query`, or [`ProviderError::Rpc`] when the node
/// gives none within `bound`.
async fn query_within<T>(
    bound: Duration,
    query: impl Future<Output = Result<T, ProviderError>>,
) -> Result<T, ProviderError> {
    tokio::time::timeout(bound, query)
        .await
        .unwrap_or_else(|_| {
            Err(ProviderError::Rpc(format!(
                "the node gave no answer within {bound:?}"
            )))
        })
}

/// Typed view over a connection's raw client for the new JSON-RPC spec
/// family (`chainHead_v1` / `archive_v1`), with hash and header types
/// derived from the chain's Substrate config.
fn archive_rpc(conn: &NodeConnection) -> ChainHeadRpcMethods<RpcConfigFor<subxt::SubstrateConfig>> {
    ChainHeadRpcMethods::new(conn.rpc.clone())
}

/// Typed view over a connection's raw client for the legacy `chain_*` RPCs,
/// with hash and header types derived from the chain's Substrate config.
fn legacy_rpc(conn: &NodeConnection) -> LegacyRpcMethods<RpcConfigFor<subxt::SubstrateConfig>> {
    LegacyRpcMethods::new(conn.rpc.clone())
}

/// Log a failed node RPC `method` and wrap its error as [`ProviderError::Rpc`].
fn rpc_error(method: &'static str) -> impl FnOnce(subxt::rpcs::Error) -> ProviderError {
    move |e| {
        warn!(error = %e, "{method} failed");
        ProviderError::Rpc(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_indexer_client::testutil;
    use serde_json::{Value, json};
    use tokio::sync::oneshot;

    fn test_provider() -> MidnightProvider {
        MidnightProvider::new("ws://test", "http://test")
    }

    /// Merging nothing is a caller error, not an empty transaction.
    #[test]
    fn merge_transactions_rejects_empty() {
        let err = test_provider().merge_transactions(&[]).unwrap_err();
        assert!(
            matches!(err, ProviderError::Transaction(ref m) if m.contains("at least one")),
            "got {err:?}"
        );
    }

    /// Undecodable bytes surface as a typed `Transaction` error naming the
    /// failing step, not a panic inside the ledger deserializer.
    #[test]
    fn merge_transactions_rejects_invalid_bytes() {
        let err = test_provider()
            .merge_transactions(&[vec![0xFF; 8]])
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::Transaction(ref m) if m.contains("deserialize")),
            "got {err:?}"
        );
    }

    /// `balance_transaction` deserializes before touching the network, so
    /// garbage bytes fail fast with a typed error and no node access.
    #[tokio::test]
    async fn balance_transaction_rejects_invalid_bytes() {
        let err = test_provider()
            .balance_transaction(&[0xFF; 8])
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::Transaction(ref m) if m.contains("deserialize")),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn health_returns_disconnected_on_bad_urls() {
        let provider = MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1");
        let health = provider.health().await;
        assert!(!health.node_connected);
    }

    /// A mock node: a listener and the `ws://` URL that dials it.
    async fn bind_node() -> (tokio::net::TcpListener, String) {
        let (listener, url) = testutil::bind().await;
        (listener, url.replacen("http://", "ws://", 1))
    }

    /// Answer the JSON-RPC request `request` with `result`.
    async fn answer(ws: &mut testutil::ServerWs, request: &Value, result: Value) {
        let response = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
        testutil::send_json(ws, &response).await;
    }

    /// A provider whose node went down after the first connect. The client
    /// sees the close and dials again, and no dial succeeds after that.
    async fn provider_during_a_node_outage() -> MidnightProvider {
        let (listener, node_url) = bind_node().await;
        let (outage_tx, outage_rx) = oneshot::channel::<()>();
        let node = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = testutil::accept_ws(stream).await;
            let answer_connect = async {
                while let Some(request) = testutil::next_json(&mut ws).await {
                    match request["method"].as_str() {
                        Some("rpc_methods") => {
                            answer(&mut ws, &request, json!({"methods": []})).await;
                        }
                        Some("chain_getBlockHash") => {
                            let genesis = format!("0x{}", "00".repeat(32));
                            answer(&mut ws, &request, json!(genesis)).await;
                        }
                        _ => {}
                    }
                }
            };
            tokio::select! {
                () = answer_connect => {}
                _ = outage_rx => {}
            }
            drop(ws);
            // The client dials again only after it sees the close. From then
            // on, it keeps each query in its queue.
            let _reconnect = listener.accept().await.unwrap();
        });
        let provider = MidnightProvider::new(&node_url, "http://127.0.0.1:1");
        provider.get_or_connect().await.expect("the connect");
        outage_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), node)
            .await
            .expect("the client dials again after the close")
            .unwrap();
        provider
    }

    /// The `name` of a read and its outcome, checked by `expected`. The
    /// outcome is `None` when the read does not end within 30 s.
    async fn ended<T>(
        name: &'static str,
        read: impl Future<Output = T>,
        expected: impl FnOnce(T) -> Result<(), String>,
    ) -> (&'static str, Option<Result<(), String>>) {
        let outcome = tokio::time::timeout(Duration::from_secs(30), read).await;
        (name, outcome.ok().map(expected))
    }

    /// Whether a read failed as a read that gets no answer does.
    fn gave_no_answer<T>(read: Result<T, ProviderError>) -> Result<(), String> {
        match read {
            Err(ProviderError::Rpc(_)) => Ok(()),
            Err(other) => Err(format!("{other:?}")),
            Ok(_) => Err("Ok".to_owned()),
        }
    }

    /// Every node read ends during a node outage (see `QUERY_TIMEOUT`). It
    /// catches a read left outside its bound, a `health` that bounds only its
    /// connect, and a `prepare` whose metadata read times out as `Rpc`. The
    /// two contract state reads have a 60 s bound, so the test leaves them
    /// out. It takes about ten seconds: paused time does not work with real
    /// sockets.
    #[tokio::test]
    async fn every_node_read_ends_during_a_node_outage() {
        use midnight_types::chain_pin::ChainView;
        use subxt::ext::futures::{FutureExt, future::join_all};

        let provider = provider_during_a_node_outage().await;
        let hash = BlockHash::repeat_byte(1);
        let disconnected = Health {
            node_connected: false,
            block_height: None,
            peers: None,
            is_syncing: None,
        };

        let outcomes = join_all([
            ended("ledger_version", provider.ledger_version(), gave_no_answer).boxed_local(),
            ended(
                "get_block_number",
                provider.get_block_number(),
                gave_no_answer,
            )
            .boxed_local(),
            ended(
                "best_block_hash",
                provider.best_block_hash(),
                gave_no_answer,
            )
            .boxed_local(),
            ended(
                "get_finalized_block_height",
                provider.get_finalized_block_height(),
                gave_no_answer,
            )
            .boxed_local(),
            ended(
                "get_block_hashes_by_height",
                provider.get_block_hashes_by_height(1),
                gave_no_answer,
            )
            .boxed_local(),
            ended(
                "get_block_header",
                provider.get_block_header(hash),
                gave_no_answer,
            )
            .boxed_local(),
            ended(
                "get_block_timestamp",
                provider.get_block_timestamp(hash),
                gave_no_answer,
            )
            .boxed_local(),
            ended("system_chain", provider.system_chain(), gave_no_answer).boxed_local(),
            ended(
                "ledger_network_id",
                provider.ledger_network_id(),
                gave_no_answer,
            )
            .boxed_local(),
            ended("health", provider.health(), |health| {
                (health == disconnected)
                    .then_some(())
                    .ok_or_else(|| format!("{health:?}"))
            })
            .boxed_local(),
            ended("prepare", provider.prepare(&[0; 8]), |read| match read {
                Err(ProviderError::Submission(crate::SubmitError::NotSubmitted { .. })) => Ok(()),
                Err(other) => Err(format!("{other:?}")),
                Ok(_) => Err("Ok".to_owned()),
            })
            .boxed_local(),
            ended(
                "ChainView::finalized_height",
                ChainView::finalized_height(&provider),
                |height| height.map_or(Ok(()), |height| Err(format!("Some({height})"))),
            )
            .boxed_local(),
        ])
        .await;

        let unbounded: Vec<_> = outcomes
            .iter()
            .filter(|(_, outcome)| outcome.is_none())
            .map(|(name, _)| *name)
            .collect();
        assert!(
            unbounded.is_empty(),
            "these reads did not end within 30 s: {unbounded:?}"
        );
        let wrong: Vec<_> = outcomes
            .iter()
            .filter_map(|(name, outcome)| match outcome {
                Some(Err(got)) => Some((*name, got)),
                _ => None,
            })
            .collect();
        assert!(
            wrong.is_empty(),
            "these reads ended with the wrong outcome: {wrong:#?}"
        );
    }

    /// A chain query whose first connect stalls gives no answer within the
    /// bound. The node accepts the dial and never answers the genesis-hash
    /// query of the client build. A bound that starts after the connect waits
    /// for the client's 60 s request timeout. The test takes about ten
    /// seconds: paused time does not work with real sockets.
    #[tokio::test]
    async fn a_chain_query_whose_connect_stalls_gives_none() {
        use midnight_types::chain_pin::ChainView;

        let (listener, node_url) = bind_node().await;
        let (genesis_asked_tx, mut genesis_asked_rx) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = testutil::accept_ws(stream).await;
            let mut genesis_asked_tx = Some(genesis_asked_tx);
            while let Some(request) = testutil::next_json(&mut ws).await {
                match request["method"].as_str() {
                    Some("rpc_methods") => answer(&mut ws, &request, json!({"methods": []})).await,
                    Some("chain_getBlockHash") => {
                        if let Some(tx) = genesis_asked_tx.take() {
                            let _ = tx.send(());
                        }
                    }
                    _ => {}
                }
            }
        });
        let provider = MidnightProvider::new(&node_url, "http://127.0.0.1:1");

        let answer = tokio::time::timeout(
            Duration::from_secs(30),
            ChainView::finalized_height(&provider),
        )
        .await;

        assert!(
            genesis_asked_rx.try_recv().is_ok(),
            "the client build never asked for the genesis hash, so a failed dial can explain the answer"
        );
        assert_eq!(answer, Ok(None));
    }

    /// A connect that its caller drops in the middle of the runtime-client
    /// build closes its connection. The client starts its follow driver
    /// before it queries the genesis hash. A drop between the two leaves that
    /// driver to hold the connection open.
    #[tokio::test]
    async fn a_connect_dropped_during_the_build_closes_its_connection() {
        let (listener, node_url) = bind_node().await;
        let (genesis_asked_tx, genesis_asked_rx) = oneshot::channel();
        let (answer_genesis_tx, answer_genesis_rx) = oneshot::channel::<()>();
        let (closed_tx, closed_rx) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = testutil::accept_ws(stream).await;
            let genesis_request = loop {
                let request = testutil::next_json(&mut ws).await.expect("a request");
                match request["method"].as_str() {
                    Some("rpc_methods") => {
                        let methods = json!({"methods": ["chainHead_v1_follow"]});
                        answer(&mut ws, &request, methods).await;
                    }
                    Some("chainSpec_v1_genesisHash") => break request,
                    _ => {}
                }
            };
            genesis_asked_tx.send(()).unwrap();
            answer_genesis_rx.await.unwrap();
            let genesis = format!("0x{}", "00".repeat(32));
            answer(&mut ws, &genesis_request, json!(genesis)).await;
            let closed = tokio::time::timeout(Duration::from_secs(5), async {
                while testutil::next_json(&mut ws).await.is_some() {}
            })
            .await
            .is_ok();
            let _ = closed_tx.send(closed);
        });
        let provider = MidnightProvider::new(&node_url, "http://127.0.0.1:1");

        tokio::select! {
            connect = provider.get_or_connect() => {
                panic!("the connect ended before the genesis answer: {:?}", connect.err())
            }
            asked = genesis_asked_rx => asked.unwrap(),
        }
        // The pending genesis query keeps the connection open until it ends.
        answer_genesis_tx.send(()).unwrap();

        assert!(
            closed_rx.await.unwrap(),
            "the connection stayed open after its connect was dropped"
        );
    }
}
