# midnight-rs — Rust SDK for the Midnight Network

A general-purpose Rust SDK for interacting with the Midnight blockchain, following the layered crate architecture of [alloy-rs](https://github.com/alloy-rs/alloy).

## Goals

1. Provide a typed, ergonomic Rust interface for reading Midnight contract state
2. Abstract over the two data sources (indexer GraphQL API + Substrate node RPC)
3. Integrate with [midnight-bindgen](https://github.com/RomarQ/midnight-rust-bindgen) for generated typed contract access
4. Be usable by any Rust application — not coupled to MCS or any specific consumer
5. Follow alloy-rs patterns: layered crates, `Provider` trait, generated `Contract` struct

## Non-Goals (Initial Scope)

- Transaction construction / proof generation (requires deep midnight-ledger integration)
- Signer abstraction / FROST threshold signing
- Custom transport layer (subxt handles node RPC transport)
- Event streaming / subscriptions
- Wallet management

## Architecture

### Crate Dependency Graph

```
midnight-core (meta-crate, re-exports all public API)
  ├── midnight-contract
  │     ├── midnight-provider
  │     │     ├── midnight-indexer-client
  │     │     └── subxt (external)
  │     └── midnight-bindgen (external, for codegen)
  └── (re-exports)
```

### Workspace Layout

```
midnight-rs/
├── Cargo.toml              # workspace root
├── docs/
│   └── SPEC.md             # this file
├── crates/
│   ├── midnight-core/      # meta-crate
│   ├── midnight-contract/  # generated Contract<P> wrapper
│   ├── midnight-provider/  # Provider trait + MidnightProvider impl
│   └── midnight-indexer-client/  # typed GraphQL client
└── README.md
```

---

## Crate: `midnight-indexer-client`

Typed Rust client for the Midnight indexer's GraphQL API. This is a standalone crate with no dependency on midnight-provider or midnight-contract.

### Purpose

The Midnight indexer exposes a GraphQL API (typically at `{base_url}/api/v3/graphql`) for querying blocks, transactions, and contract state. This crate provides a typed client that handles query construction, HTTP transport, and response deserialization.

### Dependencies

- `reqwest` — HTTP client
- `serde`, `serde_json` — serialization
- `thiserror` — error types
- `tracing` — structured logging

### Public API

```rust
pub struct IndexerClient { /* ... */ }

impl IndexerClient {
    /// Create a new client. Appends `/api/v3/graphql` if not present.
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(base_url: &str) -> Result<Self, IndexerError>;

    /// URL the client is connected to.
    pub fn url(&self) -> &str;

    // -- Blocks --

    /// Fetch the latest block.
    pub async fn get_latest_block(&self) -> Result<Option<Block>, IndexerError>;

    /// Fetch a block by height.
    pub async fn get_block_by_height(&self, height: i64) -> Result<Option<Block>, IndexerError>;

    /// Fetch a block by hash.
    pub async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Block>, IndexerError>;

    /// Fetch a block with its transactions.
    pub async fn get_block_with_transactions(&self, height: i64) -> Result<Option<Block>, IndexerError>;

    // -- Contract State --

    /// Fetch raw hex-encoded contract state at the latest block.
    pub async fn get_contract_state(&self, address: &str) -> Result<Option<String>, IndexerError>;

    /// Fetch raw contract state at a specific block height.
    pub async fn get_contract_state_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<String>, IndexerError>;

    /// Fetch raw contract state at a specific block hash.
    pub async fn get_contract_state_at_block_hash(
        &self,
        address: &str,
        hash: &str,
    ) -> Result<Option<String>, IndexerError>;

    /// Fetch raw contract state at a specific transaction hash.
    pub async fn get_contract_state_at_tx_hash(
        &self,
        address: &str,
        tx_hash: &str,
    ) -> Result<Option<String>, IndexerError>;

    // -- Contract Actions --

    /// Fetch the latest contract action (state + metadata).
    pub async fn get_contract_action(
        &self,
        address: &str,
    ) -> Result<Option<ContractAction>, IndexerError>;

    /// Fetch contract action at a specific block height.
    pub async fn get_contract_action_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<ContractAction>, IndexerError>;

    /// Fetch the block height of the latest transaction touching a contract.
    pub async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, IndexerError>;

    // -- Transactions --

    /// Fetch transactions by hash.
    pub async fn get_transactions_by_hash(
        &self,
        hash: &str,
    ) -> Result<Vec<Transaction>, IndexerError>;

    /// Fetch transactions by identifier.
    pub async fn get_transactions_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Vec<Transaction>, IndexerError>;

    // -- Health --

    /// Returns true if the indexer is reachable and has blocks.
    pub async fn health_check(&self) -> bool;
}
```

### Error Type

```rust
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("HTTP client configuration error: {0}")]
    Config(String),

    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("GraphQL errors: {}", format_graphql_errors(.0))]
    GraphQL(Vec<GraphQLError>),

    #[error("deserialization error: {0}")]
    Deserialization(String),

    #[error("missing response data")]
    MissingData,
}
```

### Response Types

All response types derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`.
Numeric fields use `i64` throughout to match the GraphQL schema.

#### Blocks

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub hash: String,
    pub height: i64,
    #[serde(default)]
    pub protocol_version: Option<i64>,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub transactions: Option<Vec<Transaction>>,
    #[serde(default)]
    pub ledger_parameters: Option<String>,
}
```

#### Transactions (Discriminated Union)

The indexer returns transactions tagged by `__typename`. We model this as a Rust enum:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "__typename")]
pub enum Transaction {
    RegularTransaction(RegularTransaction),
    SystemTransaction(SystemTransaction),
}

impl Transaction {
    pub fn id(&self) -> i64;
    pub fn hash(&self) -> &str;
    pub fn block(&self) -> Option<&Block>;
    pub fn contract_actions(&self) -> &[ContractAction];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegularTransaction {
    pub id: i64,
    pub hash: String,
    pub protocol_version: Option<i64>,
    pub raw: Option<String>,
    pub identifiers: Option<Vec<String>>,
    pub merkle_tree_root: Option<String>,
    pub start_index: Option<i64>,
    pub end_index: Option<i64>,
    pub fees: Option<TransactionFees>,
    pub transaction_result: Option<TransactionResult>,
    pub block: Option<Block>,
    pub contract_actions: Option<Vec<ContractAction>>,
    pub unshielded_created_outputs: Option<Vec<UnshieldedUtxo>>,
    pub unshielded_spent_outputs: Option<Vec<UnshieldedUtxo>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemTransaction {
    pub id: i64,
    pub hash: String,
    pub protocol_version: Option<i64>,
    pub raw: Option<String>,
    pub block: Option<Block>,
    pub contract_actions: Option<Vec<ContractAction>>,
    pub unshielded_created_outputs: Option<Vec<UnshieldedUtxo>>,
    pub unshielded_spent_outputs: Option<Vec<UnshieldedUtxo>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionFees {
    pub paid_fees: String,
    pub estimated_fees: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionResult {
    pub status: TransactionResultStatus,
    pub segments: Option<Vec<Segment>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TransactionResultStatus {
    Success,
    PartialSuccess,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub id: i64,
    pub success: bool,
}
```

#### Contract Actions (Discriminated Union)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "__typename")]
pub enum ContractAction {
    ContractDeploy(ContractDeploy),
    ContractCall(ContractCall),
    ContractUpdate(ContractUpdate),
}

impl ContractAction {
    pub fn address(&self) -> &str;
    pub fn state(&self) -> &str;
    pub fn zswap_state(&self) -> Option<&str>;
    pub fn unshielded_balances(&self) -> &[ContractBalance];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractDeploy {
    pub address: String,
    pub state: String,
    pub zswap_state: Option<String>,
    pub unshielded_balances: Option<Vec<ContractBalance>>,
    pub transaction: Option<Box<Transaction>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractCall {
    pub address: String,
    pub state: String,
    pub zswap_state: Option<String>,
    pub entry_point: Option<String>,
    pub deploy: Option<Box<ContractDeploy>>,
    pub unshielded_balances: Option<Vec<ContractBalance>>,
    pub transaction: Option<Box<Transaction>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractUpdate {
    pub address: String,
    pub state: String,
    pub zswap_state: Option<String>,
    pub unshielded_balances: Option<Vec<ContractBalance>>,
    pub transaction: Option<Box<Transaction>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractBalance {
    pub token_type: String,
    pub amount: String,
}
```

#### Unshielded UTXOs

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnshieldedUtxo {
    pub owner: String,
    pub token_type: String,
    pub value: String,
    pub intent_hash: Option<String>,
    pub output_index: Option<i64>,
    pub ctime: Option<i64>,
    pub registered_for_dust_generation: Option<bool>,
}
```

#### GraphQL Internal Types

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphQLError {
    pub message: String,
}

impl std::fmt::Display for GraphQLError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Internal wrapper for GraphQL response envelope.
#[derive(Debug, Deserialize)]
pub(crate) struct GraphQLResponse<T> {
    pub data: Option<T>,
    pub errors: Option<Vec<GraphQLError>>,
}
```

### GraphQL Queries

The crate embeds GraphQL query strings as constants (not a code-generated GraphQL client). Queries cover:

- `BlockQuery` — fetch block by optional offset (hash or height)
- `BlockWithTransactionsQuery` — block + embedded transactions
- `ContractStateQuery` — hex-encoded contract state with optional offset
- `ContractActionQuery` — full contract action with zswap state and balances
- `LatestContractBlockHeightQuery` — block height of latest contract interaction
- `TransactionsQuery` — transactions by hash or identifier

### Design Notes

- Extracted and generalized from the `IndexerClient` in `mcs-connector-midnight`
- Returns raw hex strings for contract state — deserialization is the consumer's job (via midnight-bindgen). The hex string is the `contractAction.state` field from the indexer, which is `midnight_serialize::tagged_serialize`-encoded `ContractState<InMemoryDB>`.
- All queries support optional offsets (block height, block hash, tx hash) for historical reads
- Timeout is configurable (default 30s)
- Retries are the consumer's responsibility. The client does not retry failed requests.
- No pagination support in v1. Transaction queries return all results. Pagination can be added later if needed.

---

## Crate: `midnight-provider`

Unified read interface over the Midnight indexer and Substrate node. Analogous to alloy's `Provider` trait.

### Purpose

Applications should not need to know whether data comes from the indexer or the node RPC. The provider abstracts both behind a single trait, and `MidnightProvider` is the concrete implementation that wires them together.

### Dependencies

- `midnight-indexer-client` — indexer GraphQL
- `subxt` — Substrate node RPC (specifically `subxt::rpcs::client::RpcClient`)
- `async-trait` — async trait methods
- `thiserror` — error types
- `tracing` — structured logging

### Provider Trait

```rust
/// Read-only interface to the Midnight network.
///
/// Analogous to alloy's `Provider` trait. Currently covers state reads
/// and chain queries. Transaction submission will be added in a future
/// version alongside proving infrastructure.
///
/// Automatically implemented for `&T`, `Arc<T>`, and `Box<T>` where `T: Provider`.
#[async_trait]
pub trait Provider: Send + Sync {
    // -- Chain info --

    /// Get the current block number from the node.
    async fn get_block_number(&self) -> Result<i64, ProviderError>;

    /// Get the chain's network ID.
    async fn get_network_id(&self) -> Result<String, ProviderError>;

    // -- Blocks --

    /// Get the latest block from the indexer.
    async fn get_block(&self) -> Result<Option<Block>, ProviderError>;

    /// Get a block by height.
    async fn get_block_by_height(&self, height: i64) -> Result<Option<Block>, ProviderError>;

    /// Get a block by hash.
    async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Block>, ProviderError>;

    /// Get a block with its transactions.
    async fn get_block_with_transactions(
        &self,
        height: i64,
    ) -> Result<Option<Block>, ProviderError>;

    // -- Contract state --

    /// Fetch the latest hex-encoded contract state.
    async fn get_contract_state(
        &self,
        address: &str,
    ) -> Result<Option<String>, ProviderError>;

    /// Fetch hex-encoded contract state at a specific block height.
    async fn get_contract_state_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<String>, ProviderError>;

    /// Fetch hex-encoded contract state at a specific block hash.
    async fn get_contract_state_at_block_hash(
        &self,
        address: &str,
        hash: &str,
    ) -> Result<Option<String>, ProviderError>;

    /// Fetch hex-encoded contract state at a specific transaction hash.
    async fn get_contract_state_at_tx_hash(
        &self,
        address: &str,
        tx_hash: &str,
    ) -> Result<Option<String>, ProviderError>;

    // -- Contract actions --

    /// Fetch the latest contract action.
    async fn get_contract_action(
        &self,
        address: &str,
    ) -> Result<Option<ContractAction>, ProviderError>;

    /// Fetch contract action at a specific block height.
    async fn get_contract_action_at_height(
        &self,
        address: &str,
        height: i64,
    ) -> Result<Option<ContractAction>, ProviderError>;

    /// Fetch the block height of the latest transaction touching a contract.
    async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, ProviderError>;

    // -- Transactions --

    /// Fetch transactions by hash.
    async fn get_transactions_by_hash(
        &self,
        hash: &str,
    ) -> Result<Vec<Transaction>, ProviderError>;

    /// Fetch transactions by identifier.
    async fn get_transactions_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Vec<Transaction>, ProviderError>;

    // -- Health --

    /// Check connectivity to both the node and indexer.
    async fn health(&self) -> Result<Health, ProviderError>;
}
```

### Blanket Implementations

The `Provider` trait is automatically implemented for reference and smart pointer types:

```rust
#[async_trait]
impl<T: Provider + ?Sized> Provider for &T { /* delegates to (*self) */ }

#[async_trait]
impl<T: Provider + ?Sized> Provider for Arc<T> { /* delegates to (**self) */ }

#[async_trait]
impl<T: Provider + ?Sized> Provider for Box<T> { /* delegates to (**self) */ }
```

This allows `Contract::new(addr, &provider)` to work without moving the provider.

### MidnightProvider (Concrete Implementation)

```rust
pub struct MidnightProvider {
    indexer: IndexerClient,
    node_url: String,
    rpc: Arc<RwLock<Option<RpcClient>>>,  // subxt::rpcs::client::RpcClient
}

impl MidnightProvider {
    /// Create a provider from node WebSocket URL and indexer HTTP URL.
    ///
    /// # Example
    /// ```rust
    /// let provider = MidnightProvider::new(
    ///     "ws://localhost:9944",
    ///     "http://localhost:8088",
    /// )?;
    /// ```
    pub fn new(node_url: &str, indexer_url: &str) -> Result<Self, ProviderError>;

    /// Access the underlying indexer client directly.
    pub fn indexer(&self) -> &IndexerClient;
}

impl Provider for MidnightProvider { /* ... */ }
```

### Error Type

```rust
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("indexer error: {0}")]
    Indexer(#[from] IndexerError),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("RPC connection timed out")]
    RpcTimeout,
}
```

### Health Type

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub node_connected: bool,
    pub indexer_connected: bool,
    pub block_height: Option<i64>,
    pub peers: Option<u64>,
    pub is_syncing: Option<bool>,
}
```

### Design Notes

- No `Network` generic for now — Midnight is a single network. If multi-network support is needed later (e.g., testnet vs mainnet with different types), the trait can be parameterized.
- Node RPC uses lazy connection with reconnect: connects on first use, clears cached client on failure so the next call reconnects.
- The `Provider` trait is intentionally read-only. Transaction submission (`send_transaction`) will be added alongside `midnight-tx` / `midnight-prover` crates.
- `subxt` is used directly for Substrate RPC — no custom transport wrapper. `subxt` handles WebSocket/HTTP transport, typed RPC calls, and extrinsic encoding internally.
- Re-exports `Block`, `Transaction`, `ContractAction` and all response types from `midnight-indexer-client` (single source of truth for response types).
- `subxt::rpcs::client::RpcClient` is `Send + Sync` (since subxt 0.34+), so `MidnightProvider` satisfies the `Provider: Send + Sync` bound.
- All block heights use `i64` consistently, matching the GraphQL schema. Substrate block numbers fit in `u32`, so no truncation risk.

---

## Crate: `midnight-contract`

Generated contract wrapper that combines the `Provider` with midnight-bindgen's `Ledger` for typed state access. Analogous to alloy's `ContractInstance`.

### Purpose

Given a contract address and a provider, fetch and deserialize the contract's on-chain state into the typed `Ledger` struct generated by `midnight-bindgen::contract!`.

### Dependencies

- `midnight-provider` — the `Provider` trait
- `midnight-bindgen` — re-exported for `contract!` macro + runtime types
- `thiserror` — error types

### The `FromHex` Trait

Defined in `midnight-bindgen` (not in midnight-contract) to avoid circular dependencies. The `contract!` macro emits a `FromHex` impl for each generated `Ledger`:

```rust
// In midnight-bindgen-runtime:
pub trait FromHex: Sized {
    fn from_hex(hex_state: &str) -> Result<Self, StateError>;
}

// Generated by contract! macro for each contract:
impl FromHex for Ledger {
    fn from_hex(hex_state: &str) -> Result<Self, StateError> {
        // delegates to existing Ledger::from_hex
    }
}
```

### Contract Struct

```rust
/// A deployed contract instance bound to a provider.
///
/// Generic over `P: Provider` so it works with any provider implementation
/// (owned, borrowed, `Arc`, etc. via blanket impls).
///
/// # Example
/// ```rust
/// use midnight_contract::Contract;
/// use midnight_provider::MidnightProvider;
///
/// // Ledger type generated by midnight-bindgen
/// midnight_bindgen::contract!(
///     Gateway,
///     "path/to/contract-info.json"
/// );
///
/// let provider = MidnightProvider::new(
///     "ws://localhost:9944",
///     "http://localhost:8088",
/// )?;
///
/// let contract = Contract::<_, Gateway>::new(
///     "0x1234...abcd",
///     &provider,
/// );
///
/// // Fetch and deserialize the ledger state
/// let ledger = contract.ledger().await?;
/// let threshold = ledger.threshold()?;
/// ```
pub struct Contract<P, L> {
    address: String,
    provider: P,
    _ledger: PhantomData<L>,
}

impl<P: Provider, L: FromHex> Contract<P, L> {
    /// Create a new contract instance.
    pub fn new(address: &str, provider: P) -> Self;

    /// The contract's on-chain address.
    pub fn address(&self) -> &str;

    /// Reference to the provider.
    pub fn provider(&self) -> &P;

    /// Fetch the current ledger state, deserialized into the generated type.
    pub async fn ledger(&self) -> Result<L, ContractError>;

    /// Fetch the ledger state at a specific block height.
    pub async fn ledger_at_height(&self, height: i64) -> Result<L, ContractError>;

    /// Fetch the ledger state at a specific block hash.
    pub async fn ledger_at_block_hash(&self, hash: &str) -> Result<L, ContractError>;

    /// Fetch the ledger state at a specific transaction hash.
    pub async fn ledger_at_tx_hash(&self, tx_hash: &str) -> Result<L, ContractError>;
}
```

### Error Type

```rust
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("contract not found at address {0}")]
    NotFound(String),

    #[error("state deserialization error: {0}")]
    State(#[from] midnight_bindgen::StateError),
}
```

### Design Notes

- `Contract` is generic over both `P: Provider` and `L: FromHex`, so it works with any provider implementation and any contract's generated ledger type.
- `FromHex` lives in `midnight-bindgen` to avoid circular dependencies. The `contract!` macro already generates `Ledger::from_hex()`, so adding the trait impl is minimal.
- No circuit call methods yet — `contract.call("withdraw", args).send().await` is future scope alongside `midnight-tx`.
- The pattern mirrors alloy's `ContractInstance<P, N>` but without the `Network` generic and without ABI-level function dispatch (Midnight contracts use circuits, not ABI functions).
- Historical reads by block hash and tx hash are supported alongside block height, matching the full indexer query surface.

---

## Crate: `midnight-core`

Meta-crate that re-exports the public API from all sub-crates, providing a single dependency for consumers.

### Purpose

Convenience crate so users can `use midnight_core::*` instead of depending on each sub-crate individually. Analogous to alloy's `alloy` meta-crate.

### Dependencies

All other midnight-rs crates, re-exported. Feature-gated so consumers can opt out.

### Public API

```rust
// Re-export sub-crates as modules
pub use midnight_indexer_client as indexer;
pub use midnight_provider as provider;

#[cfg(feature = "contract")]
pub use midnight_contract as contract;

// Re-export key types at the top level for convenience
pub use midnight_provider::{Provider, MidnightProvider, ProviderError, Health};
pub use midnight_indexer_client::{
    IndexerClient, IndexerError, Block, Transaction, ContractAction,
    RegularTransaction, SystemTransaction, ContractDeploy, ContractCall, ContractUpdate,
};

#[cfg(feature = "contract")]
pub use midnight_contract::{Contract, ContractError};

// Re-export midnight-bindgen only when contract feature is enabled
#[cfg(feature = "contract")]
pub use midnight_bindgen;
```

### Feature Flags

```toml
[features]
default = ["provider", "contract", "indexer"]
provider = ["dep:midnight-provider"]
contract = ["dep:midnight-contract", "dep:midnight-bindgen"]
indexer = ["dep:midnight-indexer-client"]
```

### Design Notes

- Thin re-export layer, no logic of its own.
- Feature flags allow consumers to opt out of crates they don't need.
- `midnight-bindgen` is re-exported only behind the `contract` feature flag, so consumers who only want the indexer client don't pull in the proc-macro and midnight-ledger dependencies.

---

## End-to-End Usage Example

```rust
use midnight_core::{MidnightProvider, Contract};

// Generate typed bindings for the gateway contract
midnight_core::midnight_bindgen::contract!(
    Gateway,
    "contracts/gateway/src/compiled/gateway/compiler/contract-info.json"
);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connect to the network
    let provider = MidnightProvider::new(
        "ws://localhost:9944",    // node RPC
        "http://localhost:8088",  // indexer
    )?;

    // Check health
    let health = provider.health().await?;
    println!("node: {}, indexer: {}", health.node_connected, health.indexer_connected);

    // Read contract state
    let gateway = Contract::<_, Gateway>::new("0x1234...abcd", &provider);
    let ledger = gateway.ledger().await?;

    println!("threshold: {}", ledger.threshold()?);
    println!("signing_fee: {}", ledger.signing_fee()?);
    println!("operations: {:?}", ledger.operations());

    // Historical state at a specific block
    let old_ledger = gateway.ledger_at_height(100).await?;
    println!("old threshold: {}", old_ledger.threshold()?);

    // Direct indexer access for lower-level queries
    let block = provider.get_block().await?;
    println!("latest block: {:?}", block);

    Ok(())
}
```

---

## Migration Path from mcs-connector-midnight

The `IndexerClient` in `mcs-connector-midnight` will be replaced by `midnight-indexer-client`. The `MidnightConnector` will use `MidnightProvider` internally. The devnet integration tests will use `Contract<_, Gateway>` instead of manual `ContractStateView` + index-based access.

| Before (mcs-connector-midnight) | After (midnight-rs) |
|---|---|
| `IndexerClient::new(url)` | `IndexerClient::new(url)?` (now returns `Result`) |
| `client.get_contract_state_raw(addr)` | `provider.get_contract_state(addr)` |
| `ContractStateView::from_hex(&hex)` | `contract.ledger().await?` |
| `view.get_cell_u64(0)` | `ledger.threshold()` |

---

## Testing Strategy

### Unit Tests

- `midnight-indexer-client`: Test URL construction, response deserialization from fixture JSON, error handling for malformed responses.
- `midnight-provider`: Test `Provider` trait blanket impls compile. Test `MidnightProvider` health check with bad URLs returns appropriate errors.
- `midnight-contract`: Test `Contract::ledger()` with a mock `Provider` that returns fixture hex state.

### Integration Tests (Devnet)

Gated behind `MIDNIGHT_INDEXER_URL` and `MIDNIGHT_NODE_URL` environment variables (same pattern as mcs-connector-midnight):

- `midnight-indexer-client`: Full query surface against running devnet indexer.
- `midnight-provider`: Health check, block queries, contract state reads against devnet.
- `midnight-contract`: End-to-end typed contract state read against deployed gateway contract.

---

## Future Crates (Out of Scope)

These crates are planned but not part of the initial implementation:

| Crate | Purpose | Blocked on |
|---|---|---|
| `midnight-tx` | Transaction construction (Intent, ContractCall, partition) | Deep midnight-ledger integration |
| `midnight-prover` | Proof generation (local + proof-server HTTP client) | midnight-tx |
| `midnight-signer` | Signing abstraction (local keys, FROST threshold) | midnight-tx |
| `midnight-primitives` | Low-level types extracted from midnight-ledger re-exports | Stabilization |
