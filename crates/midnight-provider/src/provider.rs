use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use subxt::OnlineClient;
use subxt::backend::CombinedBackend;
use subxt::config::RpcConfigFor;
use subxt::ext::futures::StreamExt;
use subxt::rpcs::client::reconnecting_rpc_client::RpcClient as ReconnectingRpcClient;
use subxt::rpcs::client::{RpcClient, rpc_params};
use subxt::rpcs::{ChainHeadRpcMethods, LegacyRpcMethods};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::{PendingTx, Prover, ProviderError, submit};
use midnight_helpers::LedgerParameters;
use midnight_helpers::midnight_serialize::tagged_deserialize;
use midnight_private_state::PrivateStateProvider;
use midnight_types::build::{self, MaybeWallet, Reservation};
use midnight_types::{BlockHash, CoinPublicKey, MidnightWallet, Network, TxEnvelope};

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

/// A Midnight node client, backed by a node WebSocket connection for direct
/// RPC communication. With a wallet, it also fills, funds, proves, signs and
/// sends transactions.
///
/// `W` is the wallet that [`Self::with_wallet`] attached, or `()` with none.
/// The provider calls the wallet only through [`MidnightWallet`]: it fills a
/// transaction from node queries, the wallet funds it, the provider proves
/// it, the wallet signs it, and the provider sends it.
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
/// [`Self::prepare`] and [`Self::send_tx_envelope`] bound their connect and
/// their metadata read to 10 s. The SDK sets no deadline on the submit call or
/// on the wait for a sent transaction (see [`PendingTx`]).
///
/// An archive node is a node whose state pruning and block pruning are both in
/// an archive mode. Only an archive node serves the `archive_v1` methods, and
/// another node refuses them as unknown methods.
pub struct MidnightProvider<W = ()> {
    node_url: String,
    wallet: W,
    pub(crate) prover: Prover,
    /// Optional store for per-contract private state and maintenance signing
    /// keys. Set with [`Self::with_private_state`]; absent for contracts whose
    /// witnesses are stateless.
    private_state: Option<Arc<dyn PrivateStateProvider>>,
    conn: Arc<RwLock<Option<NodeConnection>>>,
}

impl<W: std::fmt::Debug> std::fmt::Debug for MidnightProvider<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidnightProvider")
            .field("node_url", &self.node_url)
            .field("wallet", &self.wallet)
            .field("prover", &self.prover)
            .field("private_state_attached", &self.private_state.is_some())
            .finish_non_exhaustive()
    }
}

impl MidnightProvider {
    /// Create a provider from the node's WebSocket URL.
    ///
    /// The node connection is **not** established here; it is deferred to
    /// the first call that requires it.
    ///
    /// A wallet is built on its own and attached with
    /// [`Self::with_wallet`]. With the local implementation from
    /// `midnight-wallet`:
    /// ```rust,no_run
    /// # use midnight_provider::MidnightProvider;
    /// # use midnight_wallet::{Seed, Wallet};
    /// # async fn f(phrase: &str) -> anyhow::Result<()> {
    /// # const NODE_URL: &str = "ws://localhost:9944";
    /// # const INDEXER_URL: &str = "http://localhost:8088";
    /// let provider = MidnightProvider::new(NODE_URL);
    /// let wallet = Wallet::from_seed(Seed::from_mnemonic(phrase)?)
    ///     .sync(&provider, INDEXER_URL)
    ///     .await?;
    /// let provider = provider.with_wallet(&wallet);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(node_url: &str) -> Self {
        Self {
            node_url: node_url.to_string(),
            wallet: (),
            prover: Prover::local(),
            private_state: None,
            conn: Arc::new(RwLock::new(None)),
        }
    }

    /// Attach `wallet`, which funds and signs the transactions of this
    /// provider.
    ///
    /// The provider calls `wallet` only through [`MidnightWallet`]. A synced
    /// `Wallet` from `midnight-wallet` implements it, and so do a `&Wallet`
    /// and an `Arc<Wallet>`. Attach a borrow to keep using the wallet beside
    /// the provider:
    ///
    /// ```rust,no_run
    /// use midnight_provider::MidnightProvider;
    /// use midnight_wallet::{Seed, Wallet};
    ///
    /// # const INDEXER_URL: &str = "http://localhost:8088";
    /// # async fn example(provider: MidnightProvider, seed: Seed) -> Result<(), Box<dyn std::error::Error>> {
    /// let wallet = Wallet::from_seed(seed).sync(&provider, INDEXER_URL).await?;
    /// let provider = provider.with_wallet(&wallet);
    /// let balance = wallet.balance().await;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// A read of the wallet returns the state of its last sync. Each funding
    /// syncs the wallet first. To keep the reads fresh, attach an
    /// `Arc<Wallet>` and start `Wallet::sync_in_background` on it, with this
    /// provider in an `Arc` as its chain.
    pub fn with_wallet<W: MidnightWallet>(self, wallet: W) -> MidnightProvider<W> {
        let MidnightProvider {
            node_url,
            wallet: (),
            prover,
            private_state,
            conn,
        } = self;
        MidnightProvider {
            node_url,
            wallet,
            prover,
            private_state,
            conn,
        }
    }
}

impl<W> MidnightProvider<W> {
    /// Prove every transaction of this provider with `prover`: each
    /// [`TransactionRequest`](midnight_types::TransactionRequest), and the
    /// deploys, calls and maintenance updates of a `Contract` on it. The
    /// default is [`Prover::local`].
    ///
    /// ```rust,no_run
    /// # const NODE_URL: &str = "ws://localhost:9944";
    /// use midnight_provider::{MidnightProvider, Prover};
    ///
    /// let provider =
    ///     MidnightProvider::new(NODE_URL).with_prover(Prover::remote("http://localhost:6300"));
    /// ```
    pub fn with_prover(mut self, prover: Prover) -> Self {
        self.prover = prover;
        self
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
    /// use std::sync::Arc;
    /// use midnight_provider::FsPrivateStateProvider;
    ///
    /// let store = Arc::new(FsPrivateStateProvider::with_default_dir(wallet_address).unwrap());
    /// let provider = MidnightProvider::new(NODE_URL).with_private_state(store);
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

    /// Make the extrinsic of a signed transaction, so its hashes are known.
    /// Send nothing.
    ///
    /// The returned [`PreparedTx`](crate::PreparedTx) holds the reservation
    /// that `tx` carries. Its extrinsic hash is already known, so a caller can
    /// durably record state keyed by that hash *before* the transaction
    /// reaches the mempool, and then [send](crate::PreparedTx::send) it. The
    /// extrinsic comes from the node's metadata, which checks only the call's
    /// shape. The node validates the transaction only at the send.
    ///
    /// The inputs that the wallet reserved go back to it when the prepare
    /// fails, before the error returns, and when the `PreparedTx` drops before
    /// its send. Its send moves the reservation to the [`PendingTx`], which
    /// hands the inputs back when [`PendingTx::get_receipt`] reads
    /// [`SubmitError::Invalid`](crate::SubmitError::Invalid).
    ///
    /// Send the transaction only through the returned `PreparedTx`. Bytes
    /// that you send another way can land after a drop of the `PreparedTx`
    /// hands their inputs back, and a later fill can then spend the same
    /// inputs.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Transaction`] when `tx` pays no fee: it came from a
    /// request made `without_fee` or from a swap half, and the node refuses
    /// it. Hand its bytes ([`TxEnvelope::into_bytes`]) to the party that pays
    /// its fee. [`ProviderError::Rpc`] when the connect or the metadata read
    /// fails or gets no answer within 10 s, and when the node's metadata has no
    /// matching call.
    pub async fn prepare(&self, tx: TxEnvelope) -> Result<submit::PreparedTx, ProviderError> {
        let (bytes, reservation) = build::into_parts(tx).map_err(|_fee_less| fee_less_refusal())?;
        self.prepare_parts(bytes, reservation).await
    }

    /// [`Self::prepare`] for the parts of a transaction that pays its fee.
    async fn prepare_parts(
        &self,
        bytes: Vec<u8>,
        reservation: Reservation,
    ) -> Result<submit::PreparedTx, ProviderError> {
        // A prepare sends nothing to the node, so on every error the
        // reservation drops here and hands the inputs back.
        let conn = query_within(QUERY_TIMEOUT, self.get_or_connect()).await?;
        submit::prepare_bytes(&conn.client, bytes, reservation).await
    }

    /// Send a signed transaction, and carry the reservation that `tx` carries
    /// on the returned handle.
    ///
    /// The inputs that the wallet reserved go back to it in two cases:
    ///
    /// - The prepare fails: the connect, the metadata read, or the encoding of
    ///   the extrinsic. The transaction never left this process, whatever the
    ///   error, and the release completes before the error returns.
    /// - [`PendingTx::get_receipt`] reads
    ///   [`SubmitError::Invalid`](crate::SubmitError::Invalid), the node's
    ///   definitive rejection. It arrives after this method returns, so the
    ///   reservation travels with the handle.
    ///
    /// Any other failure keeps the inputs reserved until their TTL passes,
    /// because the node can still hold the transaction. This is
    /// [`Self::prepare`], then [`PreparedTx::send`](crate::PreparedTx::send).
    ///
    /// # Errors
    ///
    /// The errors of [`Self::prepare`], a refusal of a transaction that pays
    /// no fee among them, and of [`PreparedTx::send`](crate::PreparedTx::send).
    pub async fn send_tx_envelope(&self, tx: TxEnvelope) -> Result<PendingTx, ProviderError> {
        self.prepare(tx).await?.send().await
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

impl<W: MidnightWallet> MidnightProvider<W> {
    /// The wallet that this provider received in [`MidnightProvider::with_wallet`].
    pub fn wallet(&self) -> &W {
        &self.wallet
    }
}

impl<W: MaybeWallet> MidnightProvider<W> {
    /// The coin public key of the wallet, which `ownPublicKey()` returns, or
    /// `None` when the provider holds no wallet.
    #[doc(hidden)]
    pub fn coin_public_key(&self) -> Option<CoinPublicKey> {
        self.wallet.coin_public_key()
    }
}

pub(crate) fn fee_less_refusal() -> ProviderError {
    ProviderError::Transaction(
        "the transaction pays no fee, so the node refuses it; hand the bytes of its fill \
         (`TxEnvelope::into_bytes`) to the party that pays its fee"
            .into(),
    )
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

impl<W> MidnightProvider<W> {
    /// Get the hash of the node's best block (`chain_getBlockHash` with no
    /// height).
    ///
    /// A read given no block hash, such as
    /// [`get_contract_state`](Self::get_contract_state) with `None`, reads
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
    ///
    /// It needs an archive node (see [`MidnightProvider`]). On another node, it
    /// returns [`ProviderError::Rpc`].
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
    /// [`get_contract_state`](Self::get_contract_state).
    ///
    /// It needs an archive node (see [`MidnightProvider`]). On another node, it
    /// returns [`ProviderError::Rpc`].
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
    /// `None` when the node does not know the hash.
    ///
    /// It needs an archive node (see [`MidnightProvider`]). On another node, it
    /// returns [`ProviderError::Rpc`].
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

    /// The network that the node's runtime reports at the current finalized
    /// block, from its `MidnightRuntimeApi::get_network_id` runtime API.
    ///
    /// A node with no network id set reports an empty id, which the parse of
    /// a [`Network`] refuses.
    pub(crate) async fn ledger_network_id(&self) -> Result<Network, ProviderError> {
        let id = self.call_runtime_api::<String>("get_network_id").await?;
        id.parse()
            .map_err(|e| ProviderError::Rpc(format!("get_network_id: {e}")))
    }

    /// The ledger parameters that the node's runtime holds at the current
    /// finalized block, from its `MidnightRuntimeApi::get_ledger_parameters`
    /// runtime API.
    pub(crate) async fn ledger_parameters(&self) -> Result<LedgerParameters, ProviderError> {
        let encoded = self
            .call_runtime_api::<Result<Vec<u8>, subxt::dynamic::Value>>("get_ledger_parameters")
            .await?
            .map_err(|e| ProviderError::Rpc(format!("get_ledger_parameters: {e}")))?;
        tagged_deserialize(&mut &encoded[..])
            .map_err(|e| ProviderError::Rpc(format!("decoding get_ledger_parameters: {e}")))
    }

    /// Fetch full contract state via the node RPC (`midnight_contractState`),
    /// at `at_block_hash`, or at the node's best block when it is `None`.
    ///
    /// A reorg can replace a best block, so pass a finalized block hash for
    /// state that cannot change.
    ///
    /// Returns the hex-encoded serialized contract state, or `None` if no
    /// contract exists at `address` in that block.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Rpc`] when the node cannot be reached or refuses the
    /// read, for example for a block it does not know or no longer holds.
    pub async fn get_contract_state(
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
}

/// The node facts a chain pin asks for. A wallet sync asks them through
/// [`SyncSource`](midnight_types::chain_pin::SyncSource), and the funding of
/// the provider's wallet checks pins through the same answers.
///
/// A query that fails, or that gets no answer within its bound, gives
/// `None`.
#[async_trait]
impl<W: Send + Sync> midnight_types::chain_pin::ChainView for MidnightProvider<W> {
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
impl<W: Send + Sync> midnight_types::chain_pin::SyncSource for MidnightProvider<W> {
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

/// Typed view over a connection's raw client for the legacy RPCs, with hash
/// and header types derived from the chain's Substrate config.
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

    /// A wallet that this test never reaches.
    #[derive(Debug)]
    struct UnreachedWallet;

    #[async_trait]
    impl MidnightWallet for UnreachedWallet {
        fn shielded_recipient(&self) -> midnight_types::ShieldedRecipient {
            unreachable!("the test fails before the wallet")
        }

        async fn fund_transaction(
            &self,
            _chain: &dyn midnight_types::chain_pin::ChainView,
            _tx: midnight_types::FilledTx,
        ) -> Result<midnight_types::FundedTx, midnight_types::WalletError> {
            unreachable!("the test fails before the wallet")
        }

        async fn sign_transaction(
            &self,
            _tx: midnight_types::ProvenTx,
        ) -> Result<TxEnvelope, midnight_types::WalletError> {
            unreachable!("the test fails before the wallet")
        }
    }

    /// A sponsor decodes its bytes before the fill queries the node, so
    /// garbage bytes fail fast with a typed error and no node access.
    #[tokio::test]
    async fn a_sponsor_of_bytes_that_are_no_transaction_fails_before_the_node() {
        let err = MidnightProvider::new("ws://test")
            .with_wallet(UnreachedWallet)
            .fill(midnight_types::TransactionRequest::sponsor(vec![0xFF; 8]))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                ProviderError::Wallet(midnight_types::WalletError::Transfer(ref m))
                    if m.contains("deserialize")
            ),
            "got {err:?}"
        );
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
        let provider = MidnightProvider::new(&node_url);
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

    /// A reservation of nothing, for a transaction that no wallet funded.
    fn unreserved() -> Reservation {
        Reservation::new(midnight_types::build::SpentInputs::default(), |_| {})
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
    /// catches a read left outside its bound. The contract state read has a
    /// 60 s bound, so the test leaves it out. It takes about ten seconds:
    /// paused time does not work with real sockets.
    #[tokio::test]
    async fn every_node_read_ends_during_a_node_outage() {
        use midnight_types::chain_pin::ChainView;
        use subxt::ext::futures::{FutureExt, future::join_all};

        let provider = provider_during_a_node_outage().await;
        let hash = BlockHash::repeat_byte(1);

        let outcomes = join_all([
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
            ended(
                "ledger_network_id",
                provider.ledger_network_id(),
                gave_no_answer,
            )
            .boxed_local(),
            ended(
                "prepare",
                provider.prepare_parts(vec![0; 8], unreserved()),
                gave_no_answer,
            )
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
        let provider = MidnightProvider::new(&node_url);

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
        let provider = MidnightProvider::new(&node_url);

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
