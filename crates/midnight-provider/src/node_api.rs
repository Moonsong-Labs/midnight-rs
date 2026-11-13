use std::time::Duration;

use async_trait::async_trait;
use midnight_types::BlockHash;

use crate::ProviderError;

/// The node's block-header type under the chain's Substrate config; `number`
/// is the block height.
pub type NodeHeader = <subxt::SubstrateConfig as subxt::Config>::Header;

/// The node reads of a Midnight client: blocks, block time and contract state.
///
/// [`MidnightProvider`](crate::MidnightProvider) implements it over its node
/// connection. Bring the trait into scope to call the reads:
///
/// ```no_run
/// use midnight_provider::{MidnightProvider, NodeApi};
///
/// # async fn run() -> Result<(), midnight_provider::ProviderError> {
/// let provider = MidnightProvider::new("ws://localhost:9944");
/// let height = provider.get_finalized_block_height().await?;
/// let hashes = provider.get_block_hashes_by_height(height).await?;
/// # Ok(())
/// # }
/// ```
///
/// Each read of `MidnightProvider` ends within a bound that includes its
/// connect: 60 s for [`get_contract_state`](Self::get_contract_state), and
/// 10 s for every other read. A read that the node does not answer in that
/// time returns [`ProviderError::Rpc`].
///
/// An archive node is a node whose state pruning and block pruning are both in
/// an archive mode. Only an archive node serves the `archive_v1` methods, and
/// another node refuses them as unknown methods.
#[async_trait]
pub trait NodeApi: Send + Sync {
    /// Get the hash of the node's best block (`chain_getBlockHash` with no
    /// height).
    ///
    /// A read given no block hash, such as
    /// [`get_contract_state`](Self::get_contract_state) with `None`, reads
    /// at the best block at the moment of that read. Pass this hash to such
    /// reads to keep two of them at one block.
    async fn best_block_hash(&self) -> Result<BlockHash, ProviderError>;

    /// Get the latest finalized block height (`archive_v1_finalizedHeight`).
    ///
    /// Finalized blocks cannot reorg (GRANDPA), so heights at or below this
    /// are safe for observers that must never see a block twice. Part of the
    /// `archive_v1` spec, the replacement for the legacy `chain_*` RPCs.
    ///
    /// It needs an archive node (see [`NodeApi`]). On another node, it
    /// returns [`ProviderError::Rpc`].
    async fn get_finalized_block_height(&self) -> Result<u64, ProviderError>;

    /// Get the hashes of the blocks at `height` (`archive_v1_hashByHeight`):
    /// exactly one for a height at or below the finalized height, empty when
    /// the chain has not reached `height`, and possibly several while
    /// unfinalized forks exist at it.
    ///
    /// A finalized height's hash pins historical reads such as
    /// [`get_contract_state`](Self::get_contract_state).
    ///
    /// It needs an archive node (see [`NodeApi`]). On another node, it
    /// returns [`ProviderError::Rpc`].
    async fn get_block_hashes_by_height(
        &self,
        height: u64,
    ) -> Result<Vec<BlockHash>, ProviderError>;

    /// Get the header of the block with `hash` (`archive_v1_header`), or
    /// `None` when the node does not know the hash.
    ///
    /// It needs an archive node (see [`NodeApi`]). On another node, it
    /// returns [`ProviderError::Rpc`].
    async fn get_block_header(&self, hash: BlockHash) -> Result<Option<NodeHeader>, ProviderError>;

    /// Get the timestamp of the block with `hash`, as time since the Unix
    /// epoch.
    ///
    /// Reads `Timestamp::Now` in that block's state. Genesis sets no
    /// timestamp, so it reads as zero. Errors when the node does not know
    /// `hash`, or no longer holds the state at it.
    async fn get_block_timestamp(&self, hash: BlockHash) -> Result<Duration, ProviderError>;

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
    async fn get_contract_state(
        &self,
        address: &str,
        at_block_hash: Option<BlockHash>,
    ) -> Result<Option<String>, ProviderError>;
}
