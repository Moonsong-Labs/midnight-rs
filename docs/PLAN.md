# midnight-rs Implementation Plan

> **For Claude:** REQUIRED: Use core-engineering:subagent-driven-development (if subagents available) or core-engineering:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a 4-crate Rust SDK (`midnight-indexer-client`, `midnight-provider`, `midnight-contract`, `midnight-core`) for read-only interaction with the Midnight blockchain.

**Architecture:** Layered crate workspace following alloy-rs patterns. Bottom layer is a typed GraphQL client for the Midnight indexer. Middle layer is a `Provider` trait abstracting indexer + Substrate node RPC (via `subxt`). Top layer is a generic `Contract<P, L>` struct that fetches and deserializes state using midnight-bindgen's generated `Ledger` types.

**Tech Stack:** Rust (edition 2024), `reqwest` (HTTP), `subxt` 0.50 (Substrate RPC), `serde`/`serde_json`, `thiserror`, `async-trait`, `tracing`, `tokio` (runtime), midnight-ledger crates (rev `371b8012`), midnight-bindgen (local path).

**Spec:** `docs/SPEC.md` (same directory as this plan)

**Reference implementation:** The `IndexerClient`, types, and queries in `mcs-connector-midnight` at `~/Projects/Moonsong/Midnight/mcs-compact-sandbox/engine/crates/connector-midnight/src/indexer/` are the source material for `midnight-indexer-client`. The `MidnightConnector` in that same crate's `lib.rs` shows the subxt RPC pattern to reuse in `MidnightProvider`.

**Important naming note:** The `midnight-bindgen::contract!` macro accepts only a path argument (not a name). It always generates a struct called `Ledger`. Use the module name as the namespace: `gateway::Ledger`, not `Gateway`. Examples throughout this plan use `Contract::<_, gateway::Ledger>`.

**Method renames from reference:** When porting from mcs-connector-midnight, apply these renames:
- `get_contract_state_raw` → `get_contract_state` (the `ContractStateView`-returning version is dropped)
- `get_contract_state_at_block_height` → `get_contract_state_at_height`
- `get_contract_action_at_block_height` → `get_contract_action_at_height`

**serde annotations:** The spec type snippets omit `#[serde(default)]` for brevity. When implementing, **every `Option<T>` field must have `#[serde(default)]`** — the GraphQL response frequently omits optional fields. Copy the annotations from the reference implementation.

---

## File Structure

```
midnight-rs/
├── Cargo.toml                                          # workspace root
├── docs/
│   ├── SPEC.md
│   └── PLAN.md                                         # this file
├── crates/
│   ├── midnight-indexer-client/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                                  # re-exports
│   │       ├── client.rs                               # IndexerClient struct + methods
│   │       ├── error.rs                                # IndexerError enum
│   │       ├── queries.rs                              # GraphQL query constants
│   │       └── types.rs                                # Block, Transaction, ContractAction, etc.
│   ├── midnight-provider/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                                  # re-exports + Provider trait
│   │       ├── error.rs                                # ProviderError enum
│   │       ├── provider.rs                             # MidnightProvider struct + Provider impl
│   │       └── types.rs                                # Health struct
│   ├── midnight-contract/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                                  # re-exports
│   │       ├── contract.rs                             # Contract<P, L> struct
│   │       └── error.rs                                # ContractError enum
│   └── midnight-core/
│       ├── Cargo.toml
│       └── src/
│           └── lib.rs                                  # feature-gated re-exports
└── README.md
```

---

## Task 1: Workspace Scaffold

Set up the Cargo workspace with all 4 crate skeletons so everything compiles from the start.

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/midnight-indexer-client/Cargo.toml`
- Create: `crates/midnight-indexer-client/src/lib.rs`
- Create: `crates/midnight-provider/Cargo.toml`
- Create: `crates/midnight-provider/src/lib.rs`
- Create: `crates/midnight-contract/Cargo.toml`
- Create: `crates/midnight-contract/src/lib.rs`
- Create: `crates/midnight-core/Cargo.toml`
- Create: `crates/midnight-core/src/lib.rs`

- [ ] **Step 1: Create workspace root Cargo.toml**

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
edition = "2024"
license = "MIT OR Apache-2.0"
rust-version = "1.85"

[workspace.dependencies]
# Internal crates
midnight-indexer-client = { path = "crates/midnight-indexer-client" }
midnight-provider = { path = "crates/midnight-provider" }
midnight-contract = { path = "crates/midnight-contract" }
midnight-core = { path = "crates/midnight-core" }

# External
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
async-trait = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
subxt = "0.50"

# midnight-bindgen (local path — adjust if published)
midnight-bindgen = { path = "../compact-rust-codegen/crates/midnight-bindgen" }
```

- [ ] **Step 2: Create midnight-indexer-client crate skeleton**

`crates/midnight-indexer-client/Cargo.toml`:
```toml
[package]
name = "midnight-indexer-client"
version = "0.1.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
description = "Typed GraphQL client for the Midnight indexer API"

[dependencies]
reqwest = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

`crates/midnight-indexer-client/src/lib.rs`:
```rust
// Placeholder — will be populated in Task 2.
```

- [ ] **Step 3: Create midnight-provider crate skeleton**

`crates/midnight-provider/Cargo.toml`:
```toml
[package]
name = "midnight-provider"
version = "0.1.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
description = "Provider trait and MidnightProvider for Midnight network interaction"

[dependencies]
midnight-indexer-client = { workspace = true }
subxt = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
tokio = { workspace = true }
serde_json = { workspace = true }

```

`crates/midnight-provider/src/lib.rs`:
```rust
// Placeholder — will be populated in Task 3.
```

- [ ] **Step 4: Create midnight-contract crate skeleton**

`crates/midnight-contract/Cargo.toml`:
```toml
[package]
name = "midnight-contract"
version = "0.1.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
description = "Typed contract state access for Midnight smart contracts"

[dependencies]
midnight-provider = { workspace = true }
midnight-bindgen = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

`crates/midnight-contract/src/lib.rs`:
```rust
// Placeholder — will be populated in Task 4.
```

- [ ] **Step 5: Create midnight-core crate skeleton**

`crates/midnight-core/Cargo.toml`:
```toml
[package]
name = "midnight-core"
version = "0.1.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
description = "Meta-crate for the midnight-rs SDK — re-exports all sub-crates"

[features]
default = ["provider", "contract", "indexer"]
provider = ["dep:midnight-provider"]
contract = ["dep:midnight-contract", "dep:midnight-bindgen"]
indexer = ["dep:midnight-indexer-client"]

[dependencies]
midnight-indexer-client = { workspace = true, optional = true }
midnight-provider = { workspace = true, optional = true }
midnight-contract = { workspace = true, optional = true }
midnight-bindgen = { workspace = true, optional = true }
```

`crates/midnight-core/src/lib.rs`:
```rust
// Placeholder — will be populated in Task 5.
```

- [ ] **Step 6: Verify workspace compiles**

Run: `cargo check --workspace`
Expected: clean compilation with no errors.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/
git commit -m "scaffold: initialize Cargo workspace with 4 crate skeletons"
```

---

## Task 2: midnight-indexer-client — Types and Error

Implement all response types and the error enum. These are self-contained data types with no logic beyond serde deserialization and convenience accessors.

**Files:**
- Create: `crates/midnight-indexer-client/src/error.rs`
- Create: `crates/midnight-indexer-client/src/types.rs`
- Modify: `crates/midnight-indexer-client/src/lib.rs`

**Reference:** Copy the type definitions from the spec's "Response Types" section. These are modeled on the existing types in `~/Projects/Moonsong/Midnight/mcs-compact-sandbox/engine/crates/connector-midnight/src/indexer/types.rs`.

- [ ] **Step 1: Write tests for type deserialization**

Add to the bottom of `crates/midnight-indexer-client/src/types.rs` (or a separate test module). These test that the serde `#[serde(tag = "__typename")]` discriminated union works correctly with real-ish indexer JSON:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_block() {
        let json = r#"{
            "hash": "abc123",
            "height": 42,
            "protocolVersion": 1,
            "timestamp": 1700000000,
            "author": "validator1"
        }"#;
        let block: Block = serde_json::from_str(json).unwrap();
        assert_eq!(block.hash, "abc123");
        assert_eq!(block.height, 42);
        assert_eq!(block.protocol_version, Some(1));
        assert_eq!(block.timestamp, Some(1700000000));
    }

    #[test]
    fn deserialize_regular_transaction() {
        let json = r#"{
            "__typename": "RegularTransaction",
            "id": 1,
            "hash": "tx_hash_1",
            "protocolVersion": 1,
            "identifiers": ["id1", "id2"],
            "transactionResult": {
                "status": "SUCCESS",
                "segments": [{"id": 0, "success": true}]
            }
        }"#;
        let tx: Transaction = serde_json::from_str(json).unwrap();
        assert_eq!(tx.id(), 1);
        assert_eq!(tx.hash(), "tx_hash_1");
        match &tx {
            Transaction::RegularTransaction(rt) => {
                assert_eq!(rt.identifiers.as_ref().unwrap().len(), 2);
                assert_eq!(
                    rt.transaction_result.as_ref().unwrap().status,
                    TransactionResultStatus::Success
                );
            }
            _ => panic!("expected RegularTransaction"),
        }
    }

    #[test]
    fn deserialize_system_transaction() {
        let json = r#"{
            "__typename": "SystemTransaction",
            "id": 2,
            "hash": "sys_hash_1",
            "protocolVersion": 1
        }"#;
        let tx: Transaction = serde_json::from_str(json).unwrap();
        assert_eq!(tx.id(), 2);
        assert!(matches!(tx, Transaction::SystemTransaction(_)));
    }

    #[test]
    fn deserialize_contract_call_action() {
        let json = r#"{
            "__typename": "ContractCall",
            "address": "contract_addr",
            "state": "deadbeef",
            "entryPoint": "withdraw",
            "zswapState": "cafe"
        }"#;
        let action: ContractAction = serde_json::from_str(json).unwrap();
        assert_eq!(action.address(), "contract_addr");
        assert_eq!(action.state(), "deadbeef");
        assert_eq!(action.zswap_state(), Some("cafe"));
        match &action {
            ContractAction::ContractCall(c) => {
                assert_eq!(c.entry_point.as_deref(), Some("withdraw"));
            }
            _ => panic!("expected ContractCall"),
        }
    }

    #[test]
    fn deserialize_contract_deploy_action() {
        let json = r#"{
            "__typename": "ContractDeploy",
            "address": "deploy_addr",
            "state": "aabb",
            "unshieldedBalances": [
                {"tokenType": "0x01", "amount": "1000"}
            ]
        }"#;
        let action: ContractAction = serde_json::from_str(json).unwrap();
        assert!(matches!(action, ContractAction::ContractDeploy(_)));
        assert_eq!(action.unshielded_balances().len(), 1);
        assert_eq!(action.unshielded_balances()[0].token_type, "0x01");
    }

    #[test]
    fn deserialize_graphql_error() {
        let json = r#"{"message": "something went wrong"}"#;
        let err: GraphQLError = serde_json::from_str(json).unwrap();
        assert_eq!(err.to_string(), "something went wrong");
    }

    #[test]
    fn transaction_accessor_methods() {
        let tx = Transaction::RegularTransaction(RegularTransaction {
            id: 5,
            hash: "h".into(),
            protocol_version: None,
            raw: None,
            identifiers: None,
            merkle_tree_root: None,
            start_index: None,
            end_index: None,
            fees: None,
            transaction_result: None,
            block: Some(Block {
                hash: "bh".into(),
                height: 10,
                protocol_version: None,
                timestamp: None,
                author: None,
                transactions: None,
                ledger_parameters: None,
            }),
            contract_actions: Some(vec![]),
            unshielded_created_outputs: None,
            unshielded_spent_outputs: None,
        });
        assert_eq!(tx.id(), 5);
        assert_eq!(tx.hash(), "h");
        assert_eq!(tx.block().unwrap().height, 10);
        assert_eq!(tx.contract_actions().len(), 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p midnight-indexer-client`
Expected: compilation errors — types don't exist yet.

- [ ] **Step 3: Implement error.rs**

Create `crates/midnight-indexer-client/src/error.rs` with the full `IndexerError` enum per spec:

```rust
use crate::types::GraphQLError;

fn format_graphql_errors(errors: &[GraphQLError]) -> String {
    errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

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

- [ ] **Step 4: Implement types.rs**

Create `crates/midnight-indexer-client/src/types.rs` with all response types from the spec. This is a direct port of `~/Projects/Moonsong/Midnight/mcs-compact-sandbox/engine/crates/connector-midnight/src/indexer/types.rs` with these changes:
- Add `Serialize` derive to all public types (the original only has `Deserialize`)
- Add `PartialEq, Eq` derives to all types
- Keep all `#[serde(default)]` annotations on optional fields
- Keep `#[serde(tag = "__typename")]` on `Transaction` and `ContractAction` enums
- Keep `#[serde(rename_all = ...)]` on all structs

Include all types: `GraphQLResponse` (pub(crate)), `GraphQLError`, `Block`, `Transaction`, `RegularTransaction`, `SystemTransaction`, `TransactionFees`, `TransactionResult`, `TransactionResultStatus`, `Segment`, `ContractAction`, `ContractDeploy`, `ContractCall`, `ContractUpdate`, `ContractBalance`, `UnshieldedUtxo`, `BlockQueryData` (pub(crate)), `ContractActionQueryData` (pub(crate)), `TransactionsQueryData` (pub(crate)).

Include the accessor methods on `Transaction` (`id`, `hash`, `block`, `contract_actions`) and `ContractAction` (`address`, `state`, `zswap_state`, `unshielded_balances`) per spec.

- [ ] **Step 5: Wire up lib.rs**

Update `crates/midnight-indexer-client/src/lib.rs`:

```rust
mod error;
pub mod types;

pub use error::IndexerError;
pub use types::*;
```

Note: `client` and `queries` modules are not wired yet — they don't exist.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p midnight-indexer-client`
Expected: all type deserialization tests pass.

- [ ] **Step 7: Run clippy**

Run: `cargo clippy -p midnight-indexer-client -- -D warnings`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/midnight-indexer-client/
git commit -m "feat(indexer-client): add response types and error enum"
```

---

## Task 3: midnight-indexer-client — Queries and Client

Implement the GraphQL query constants and the `IndexerClient` struct with all methods.

**Files:**
- Create: `crates/midnight-indexer-client/src/queries.rs`
- Create: `crates/midnight-indexer-client/src/client.rs`
- Modify: `crates/midnight-indexer-client/src/lib.rs`

**Reference:** Port queries from `~/Projects/Moonsong/Midnight/mcs-compact-sandbox/engine/crates/connector-midnight/src/indexer/queries.rs` and client from `~/Projects/Moonsong/Midnight/mcs-compact-sandbox/engine/crates/connector-midnight/src/indexer/client.rs`.

- [ ] **Step 1: Write unit tests for IndexerClient**

Add `#[cfg(test)] mod tests` at the bottom of `crates/midnight-indexer-client/src/client.rs`. Tests for URL construction (no network needed):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_construction_bare_host() {
        let client = IndexerClient::new("http://localhost:8088").unwrap();
        assert_eq!(client.url(), "http://localhost:8088/api/v3/graphql");
    }

    #[test]
    fn url_construction_with_trailing_slash() {
        let client = IndexerClient::new("http://localhost:8088/").unwrap();
        assert_eq!(client.url(), "http://localhost:8088/api/v3/graphql");
    }

    #[test]
    fn url_construction_full_path() {
        let client = IndexerClient::new("http://localhost:8088/api/v3/graphql").unwrap();
        assert_eq!(client.url(), "http://localhost:8088/api/v3/graphql");
    }

    #[test]
    fn url_construction_https() {
        let client = IndexerClient::new("https://indexer.midnight.network").unwrap();
        assert_eq!(
            client.url(),
            "https://indexer.midnight.network/api/v3/graphql"
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p midnight-indexer-client`
Expected: compilation errors — `IndexerClient` doesn't exist yet.

- [ ] **Step 3: Implement queries.rs**

Create `crates/midnight-indexer-client/src/queries.rs`. Direct port of the 6 query constants from `mcs-connector-midnight/src/indexer/queries.rs`:

- `BLOCK_QUERY`
- `BLOCK_WITH_TRANSACTIONS_QUERY`
- `CONTRACT_STATE_QUERY`
- `CONTRACT_ACTION_QUERY`
- `LATEST_CONTRACT_BLOCK_HEIGHT_QUERY`
- `TRANSACTIONS_QUERY`

Copy them verbatim — the GraphQL strings are identical.

- [ ] **Step 4: Implement client.rs**

Create `crates/midnight-indexer-client/src/client.rs`. Port from `mcs-connector-midnight/src/indexer/client.rs` with these changes:

1. **`new()` returns `Result<Self, IndexerError>`** instead of panicking. Use `IndexerError::Config` for builder failures.
2. **Add configurable timeout** — accept `Duration` or use default 30s. Use a builder pattern or just provide `IndexerClient::with_timeout(base_url, timeout)` as a second constructor.
3. **Remove `crate::state::ContractStateView` dependency** — the `get_contract_state` method returns `Option<String>` (raw hex), not a deserialized view. Drop the `get_contract_state` method that returned `ContractStateView` — only keep the raw hex version.
4. **All method signatures match the spec's Public API section.**

The internal `execute` method is identical to the existing one: POST JSON to the GraphQL endpoint, deserialize `GraphQLResponse<R>`, check for errors, return `data`.

Each public method constructs the variables JSON and calls `execute` with the appropriate query constant and response wrapper type.

- [ ] **Step 5: Update lib.rs to wire all modules**

```rust
mod client;
mod error;
pub mod queries;
pub mod types;

pub use client::IndexerClient;
pub use error::IndexerError;
pub use types::*;
```

- [ ] **Step 6: Run all tests**

Run: `cargo test -p midnight-indexer-client`
Expected: all tests pass (URL construction + type deserialization).

Note: also add a test that verifies `IndexerClient::new` with an invalid reqwest configuration returns `IndexerError::Config` (if applicable — reqwest's default builder rarely fails, but this ensures the error path exists).

- [ ] **Step 7: Run clippy**

Run: `cargo clippy -p midnight-indexer-client -- -D warnings`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/midnight-indexer-client/
git commit -m "feat(indexer-client): add GraphQL queries and IndexerClient"
```

---

## Task 4: midnight-provider — Error, Types, and Provider Trait

Implement the `ProviderError`, `Health` struct, and `Provider` trait with blanket impls. No concrete implementation yet — just the trait definition.

**Files:**
- Create: `crates/midnight-provider/src/error.rs`
- Create: `crates/midnight-provider/src/types.rs`
- Modify: `crates/midnight-provider/src/lib.rs`

- [ ] **Step 1: Write a compile-test for blanket impls**

In `crates/midnight-provider/src/lib.rs`, add a test that verifies the blanket impls work:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // A dummy provider for compile-testing blanket impls.
    struct DummyProvider;

    #[async_trait::async_trait]
    impl Provider for DummyProvider {
        async fn get_block_number(&self) -> Result<i64, ProviderError> { Ok(0) }
        async fn get_network_id(&self) -> Result<String, ProviderError> { Ok("test".into()) }
        async fn get_block(&self) -> Result<Option<midnight_indexer_client::Block>, ProviderError> { Ok(None) }
        async fn get_block_by_height(&self, _h: i64) -> Result<Option<midnight_indexer_client::Block>, ProviderError> { Ok(None) }
        async fn get_block_by_hash(&self, _h: &str) -> Result<Option<midnight_indexer_client::Block>, ProviderError> { Ok(None) }
        async fn get_block_with_transactions(&self, _h: i64) -> Result<Option<midnight_indexer_client::Block>, ProviderError> { Ok(None) }
        async fn get_contract_state(&self, _a: &str) -> Result<Option<String>, ProviderError> { Ok(None) }
        async fn get_contract_state_at_height(&self, _a: &str, _h: i64) -> Result<Option<String>, ProviderError> { Ok(None) }
        async fn get_contract_state_at_block_hash(&self, _a: &str, _h: &str) -> Result<Option<String>, ProviderError> { Ok(None) }
        async fn get_contract_state_at_tx_hash(&self, _a: &str, _h: &str) -> Result<Option<String>, ProviderError> { Ok(None) }
        async fn get_contract_action(&self, _a: &str) -> Result<Option<midnight_indexer_client::ContractAction>, ProviderError> { Ok(None) }
        async fn get_contract_action_at_height(&self, _a: &str, _h: i64) -> Result<Option<midnight_indexer_client::ContractAction>, ProviderError> { Ok(None) }
        async fn get_latest_contract_block_height(&self, _a: &str) -> Result<Option<i64>, ProviderError> { Ok(None) }
        async fn get_transactions_by_hash(&self, _h: &str) -> Result<Vec<midnight_indexer_client::Transaction>, ProviderError> { Ok(vec![]) }
        async fn get_transactions_by_identifier(&self, _i: &str) -> Result<Vec<midnight_indexer_client::Transaction>, ProviderError> { Ok(vec![]) }
        async fn health(&self) -> Result<Health, ProviderError> { Ok(Health { node_connected: true, indexer_connected: true, block_height: None, peers: None, is_syncing: None }) }
    }

    #[tokio::test]
    async fn blanket_impl_ref() {
        let p = DummyProvider;
        let r: &dyn Provider = &p;
        assert_eq!(r.get_block_number().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn blanket_impl_arc() {
        let p = Arc::new(DummyProvider);
        assert_eq!(p.get_block_number().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn blanket_impl_box() {
        let p: Box<dyn Provider> = Box::new(DummyProvider);
        assert_eq!(p.get_block_number().await.unwrap(), 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p midnight-provider`
Expected: compilation errors — trait doesn't exist yet.

- [ ] **Step 3: Implement error.rs**

```rust
use midnight_indexer_client::IndexerError;

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

- [ ] **Step 4: Implement types.rs**

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

- [ ] **Step 5: Implement lib.rs with Provider trait and blanket impls**

Write `crates/midnight-provider/src/lib.rs` with:
1. The full `Provider` trait (all 16 methods from the spec)
2. Blanket impls for `&T`, `Arc<T>`, `Box<T>` where `T: Provider + ?Sized`
3. Module declarations and re-exports

Each blanket impl delegates every method to the inner `Provider`. For `&T`, delegate to `(*self).method()`. For `Arc<T>` and `Box<T>`, delegate to `(**self).method()`.

Re-exports:
```rust
pub use error::ProviderError;
pub use types::Health;

// Re-export indexer types so consumers of midnight-provider don't need
// a separate dependency on midnight-indexer-client for response types.
pub use midnight_indexer_client::{
    self as indexer, Block, ContractAction, ContractBalance, ContractCall, ContractDeploy,
    ContractUpdate, IndexerClient, IndexerError, RegularTransaction, Segment, SystemTransaction,
    Transaction, TransactionFees, TransactionResult, TransactionResultStatus, UnshieldedUtxo,
};
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p midnight-provider`
Expected: all 3 blanket impl tests pass.

- [ ] **Step 7: Run clippy**

Run: `cargo clippy -p midnight-provider -- -D warnings`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/midnight-provider/
git commit -m "feat(provider): add Provider trait, error types, and blanket impls"
```

---

## Task 5: midnight-provider — MidnightProvider Implementation

Implement the concrete `MidnightProvider` struct that wires the indexer client and subxt RPC client together.

**Files:**
- Create: `crates/midnight-provider/src/provider.rs`
- Modify: `crates/midnight-provider/src/lib.rs`

**Reference:** The RPC connection pattern (lazy connect, reconnect on failure) is directly ported from `MidnightConnector` in `~/Projects/Moonsong/Midnight/mcs-compact-sandbox/engine/crates/connector-midnight/src/lib.rs`.

- [ ] **Step 1: Write tests for MidnightProvider**

In `crates/midnight-provider/src/provider.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_provider() {
        let provider = MidnightProvider::new("ws://localhost:9944", "http://localhost:8088").unwrap();
        assert_eq!(provider.indexer().url(), "http://localhost:8088/api/v3/graphql");
    }

    #[tokio::test]
    async fn health_returns_disconnected_on_bad_urls() {
        let provider = MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1").unwrap();
        let health = provider.health().await.unwrap();
        assert!(!health.node_connected);
        assert!(!health.indexer_connected);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p midnight-provider`
Expected: compilation errors — `MidnightProvider` doesn't exist yet.

- [ ] **Step 3: Implement provider.rs**

Create `crates/midnight-provider/src/provider.rs`:

```rust
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use subxt::rpcs::client::{RpcClient, RpcParams};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::{Health, Provider, ProviderError};
use midnight_indexer_client::{
    Block, ContractAction, IndexerClient, Transaction,
};

const RPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct MidnightProvider {
    indexer: IndexerClient,
    node_url: String,
    rpc: Arc<RwLock<Option<RpcClient>>>,
}
```

Implement:
1. `MidnightProvider::new(node_url, indexer_url) -> Result<Self, ProviderError>` — creates `IndexerClient::new(indexer_url)?` (maps `IndexerError` to `ProviderError::Indexer`), stores `node_url`.
2. `MidnightProvider::indexer(&self) -> &IndexerClient`
3. Private `get_or_connect(&self) -> Result<RpcClient, ProviderError>` — lazy connect with timeout, same pattern as `MidnightConnector::get_or_connect`.
4. Private `clear_connection(&self)` — clears cached RPC client.
5. `impl Provider for MidnightProvider` — delegates:
   - Block/tx/contract queries → `self.indexer.method()` with error mapping via `?` (IndexerError → ProviderError::Indexer via `From`).
   - `get_block_number` → `self.get_or_connect()` then `rpc.request("chain_getHeader", ...)`, parse hex height from response JSON (same as `MidnightConnector::health_check`). The header returns hex-encoded `u64`; cast to `i64` via `as i64` (safe — Substrate block numbers fit in `u32`).
   - `get_network_id` → `self.get_or_connect()` then `rpc.request("system_chain", ...)`.
   - `health` → try RPC `system_health` + `chain_getHeader` for node status, `self.indexer.health_check()` for indexer status. On RPC failure, return `Health { node_connected: false, ... }` (do not error — health is best-effort).

- [ ] **Step 4: Update lib.rs to export MidnightProvider**

Add to `crates/midnight-provider/src/lib.rs`:

```rust
mod provider;
pub use provider::MidnightProvider;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p midnight-provider`
Expected: all tests pass (creation + health check with bad URLs).

- [ ] **Step 6: Run clippy**

Run: `cargo clippy -p midnight-provider -- -D warnings`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/midnight-provider/
git commit -m "feat(provider): implement MidnightProvider with indexer + subxt RPC"
```

---

## Task 6: midnight-bindgen — Add FromHex Trait

Add the `FromHex` trait to `midnight-bindgen-runtime` and emit `FromHex` impls from the `contract!` macro. This is a change to the **external** midnight-bindgen project (at `~/Projects/project-ideas/compact-rust-codegen/`), not to midnight-rs itself.

**Files:**
- Modify: `~/Projects/project-ideas/compact-rust-codegen/crates/midnight-bindgen-runtime/src/lib.rs`
- Create: `~/Projects/project-ideas/compact-rust-codegen/crates/midnight-bindgen-runtime/src/from_hex.rs`
- Modify: `~/Projects/project-ideas/compact-rust-codegen/crates/compact-codegen/src/rust_emitter.rs`

- [ ] **Step 1: Write test for FromHex trait**

In `~/Projects/project-ideas/compact-rust-codegen/crates/midnight-bindgen-runtime/src/from_hex.rs`:

```rust
use crate::StateError;

/// Trait for types that can be deserialized from hex-encoded Midnight contract state.
///
/// Implemented by `midnight-bindgen`'s generated `Ledger` structs.
pub trait FromHex: Sized {
    fn from_hex(hex_state: &str) -> Result<Self, StateError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyLedger;

    impl FromHex for DummyLedger {
        fn from_hex(_hex: &str) -> Result<Self, StateError> {
            Ok(DummyLedger)
        }
    }

    #[test]
    fn from_hex_trait_compiles() {
        let _ = DummyLedger::from_hex("deadbeef").unwrap();
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ~/Projects/project-ideas/compact-rust-codegen && cargo test -p midnight-bindgen-runtime`
Expected: compilation error — `from_hex` module doesn't exist.

- [ ] **Step 3: Add from_hex module to runtime**

In `~/Projects/project-ideas/compact-rust-codegen/crates/midnight-bindgen-runtime/src/lib.rs`, add:

```rust
mod from_hex;
pub use from_hex::FromHex;
```

- [ ] **Step 4: Run tests**

Run: `cd ~/Projects/project-ideas/compact-rust-codegen && cargo test -p midnight-bindgen-runtime`
Expected: passes.

- [ ] **Step 5: Update rust_emitter.rs to emit FromHex impl**

In `~/Projects/project-ideas/compact-rust-codegen/crates/compact-codegen/src/rust_emitter.rs`, the `emit_ledger_wrapper` function is called from two codegen paths with different crate prefixes:
- `generate_bindings()` (proc macro path) uses `midnight_bindgen::*`
- `generate_lib_rs()` (standalone crate) uses `midnight_bindgen_runtime::*`

Add a `crate_prefix: &str` parameter to `emit_ledger_wrapper` (or add a separate `emit_from_hex_impl` function). After the `Ledger` struct and `impl` block, emit:

```rust
impl {crate_prefix}::FromHex for Ledger {
    fn from_hex(hex_state: &str) -> Result<Self, {crate_prefix}::StateError> {
        Self::from_hex(hex_state)
    }
}
```

Where `{crate_prefix}` is `midnight_bindgen` in the proc macro path and `midnight_bindgen_runtime` in the standalone path. Update both call sites of `emit_ledger_wrapper` to pass the correct prefix.

- [ ] **Step 6: Run full test suite**

Run: `cd ~/Projects/project-ideas/compact-rust-codegen && cargo test --workspace`
Expected: all tests pass. The integration tests that use `contract!` macro should now generate `FromHex` impls.

- [ ] **Step 7: Run clippy**

Run: `cd ~/Projects/project-ideas/compact-rust-codegen && cargo clippy --workspace -- -D warnings`
Expected: no warnings.

- [ ] **Step 8: Commit (in midnight-bindgen repo)**

```bash
cd ~/Projects/project-ideas/compact-rust-codegen
git add -A
git commit -m "feat: add FromHex trait and emit impl from contract! macro"
```

---

## Task 7: midnight-contract — Contract Struct and Error

Implement the `Contract<P, L>` struct with `ledger()` and historical read methods.

**Files:**
- Create: `crates/midnight-contract/src/error.rs`
- Create: `crates/midnight-contract/src/contract.rs`
- Modify: `crates/midnight-contract/src/lib.rs`

- [ ] **Step 1: Write tests for Contract**

In `crates/midnight-contract/src/contract.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use midnight_provider::{Health, Provider, ProviderError};
    use midnight_indexer_client::{Block, ContractAction, Transaction};

    // A mock provider that returns a fixed hex string for contract state.
    struct MockProvider {
        state_hex: Option<String>,
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn get_block_number(&self) -> Result<i64, ProviderError> { Ok(0) }
        async fn get_network_id(&self) -> Result<String, ProviderError> { Ok("mock".into()) }
        async fn get_block(&self) -> Result<Option<Block>, ProviderError> { Ok(None) }
        async fn get_block_by_height(&self, _: i64) -> Result<Option<Block>, ProviderError> { Ok(None) }
        async fn get_block_by_hash(&self, _: &str) -> Result<Option<Block>, ProviderError> { Ok(None) }
        async fn get_block_with_transactions(&self, _: i64) -> Result<Option<Block>, ProviderError> { Ok(None) }
        async fn get_contract_state(&self, _: &str) -> Result<Option<String>, ProviderError> {
            Ok(self.state_hex.clone())
        }
        async fn get_contract_state_at_height(&self, _: &str, _: i64) -> Result<Option<String>, ProviderError> {
            Ok(self.state_hex.clone())
        }
        async fn get_contract_state_at_block_hash(&self, _: &str, _: &str) -> Result<Option<String>, ProviderError> {
            Ok(self.state_hex.clone())
        }
        async fn get_contract_state_at_tx_hash(&self, _: &str, _: &str) -> Result<Option<String>, ProviderError> {
            Ok(self.state_hex.clone())
        }
        async fn get_contract_action(&self, _: &str) -> Result<Option<ContractAction>, ProviderError> { Ok(None) }
        async fn get_contract_action_at_height(&self, _: &str, _: i64) -> Result<Option<ContractAction>, ProviderError> { Ok(None) }
        async fn get_latest_contract_block_height(&self, _: &str) -> Result<Option<i64>, ProviderError> { Ok(None) }
        async fn get_transactions_by_hash(&self, _: &str) -> Result<Vec<Transaction>, ProviderError> { Ok(vec![]) }
        async fn get_transactions_by_identifier(&self, _: &str) -> Result<Vec<Transaction>, ProviderError> { Ok(vec![]) }
        async fn health(&self) -> Result<Health, ProviderError> {
            Ok(Health { node_connected: false, indexer_connected: false, block_height: None, peers: None, is_syncing: None })
        }
    }

    // A trivial FromHex impl for testing.
    struct FakeLedger(String);

    impl midnight_bindgen::FromHex for FakeLedger {
        fn from_hex(hex_state: &str) -> Result<Self, midnight_bindgen::StateError> {
            Ok(FakeLedger(hex_state.to_string()))
        }
    }

    #[tokio::test]
    async fn ledger_returns_deserialized_state() {
        let provider = MockProvider { state_hex: Some("deadbeef".into()) };
        let contract: Contract<_, FakeLedger> = Contract::new("addr1", provider);
        let ledger = contract.ledger().await.unwrap();
        assert_eq!(ledger.0, "deadbeef");
    }

    #[tokio::test]
    async fn ledger_returns_not_found_when_no_state() {
        let provider = MockProvider { state_hex: None };
        let contract: Contract<_, FakeLedger> = Contract::new("addr1", provider);
        let err = contract.ledger().await.unwrap_err();
        assert!(matches!(err, ContractError::NotFound(_)));
    }

    #[tokio::test]
    async fn ledger_at_height_works() {
        let provider = MockProvider { state_hex: Some("cafe".into()) };
        let contract: Contract<_, FakeLedger> = Contract::new("addr1", provider);
        let ledger = contract.ledger_at_height(100).await.unwrap();
        assert_eq!(ledger.0, "cafe");
    }

    #[tokio::test]
    async fn ledger_at_block_hash_works() {
        let provider = MockProvider { state_hex: Some("babe".into()) };
        let contract: Contract<_, FakeLedger> = Contract::new("addr1", provider);
        let ledger = contract.ledger_at_block_hash("abc").await.unwrap();
        assert_eq!(ledger.0, "babe");
    }

    #[tokio::test]
    async fn ledger_at_tx_hash_works() {
        let provider = MockProvider { state_hex: Some("face".into()) };
        let contract: Contract<_, FakeLedger> = Contract::new("addr1", provider);
        let ledger = contract.ledger_at_tx_hash("txhash").await.unwrap();
        assert_eq!(ledger.0, "face");
    }

    #[tokio::test]
    async fn contract_by_ref_provider() {
        let provider = MockProvider { state_hex: Some("abab".into()) };
        let contract: Contract<&MockProvider, FakeLedger> = Contract::new("addr1", &provider);
        let ledger = contract.ledger().await.unwrap();
        assert_eq!(ledger.0, "abab");
    }

    #[test]
    fn address_and_provider_accessors() {
        let provider = MockProvider { state_hex: None };
        let contract: Contract<_, FakeLedger> = Contract::new("myaddr", provider);
        assert_eq!(contract.address(), "myaddr");
        let _ = contract.provider(); // just verify it compiles
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p midnight-contract`
Expected: compilation errors — `Contract` doesn't exist yet.

- [ ] **Step 3: Implement error.rs**

```rust
use midnight_provider::ProviderError;

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

- [ ] **Step 4: Implement contract.rs**

```rust
use std::marker::PhantomData;

use midnight_bindgen::FromHex;
use midnight_provider::Provider;

use crate::error::ContractError;

pub struct Contract<P, L> {
    address: String,
    provider: P,
    _ledger: PhantomData<L>,
}

impl<P: Provider, L: FromHex> Contract<P, L> {
    pub fn new(address: &str, provider: P) -> Self {
        Self {
            address: address.to_string(),
            provider,
            _ledger: PhantomData,
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }

    pub async fn ledger(&self) -> Result<L, ContractError> {
        let hex = self
            .provider
            .get_contract_state(&self.address)
            .await?
            .ok_or_else(|| ContractError::NotFound(self.address.clone()))?;
        Ok(L::from_hex(&hex)?)
    }

    pub async fn ledger_at_height(&self, height: i64) -> Result<L, ContractError> {
        let hex = self
            .provider
            .get_contract_state_at_height(&self.address, height)
            .await?
            .ok_or_else(|| ContractError::NotFound(self.address.clone()))?;
        Ok(L::from_hex(&hex)?)
    }

    pub async fn ledger_at_block_hash(&self, hash: &str) -> Result<L, ContractError> {
        let hex = self
            .provider
            .get_contract_state_at_block_hash(&self.address, hash)
            .await?
            .ok_or_else(|| ContractError::NotFound(self.address.clone()))?;
        Ok(L::from_hex(&hex)?)
    }

    pub async fn ledger_at_tx_hash(&self, tx_hash: &str) -> Result<L, ContractError> {
        let hex = self
            .provider
            .get_contract_state_at_tx_hash(&self.address, tx_hash)
            .await?
            .ok_or_else(|| ContractError::NotFound(self.address.clone()))?;
        Ok(L::from_hex(&hex)?)
    }
}
```

- [ ] **Step 5: Wire up lib.rs**

```rust
mod contract;
mod error;

pub use contract::Contract;
pub use error::ContractError;

// Re-export FromHex so consumers can use it without depending on midnight-bindgen directly.
pub use midnight_bindgen::FromHex;
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p midnight-contract`
Expected: all 7 contract tests pass.

- [ ] **Step 7: Run clippy**

Run: `cargo clippy -p midnight-contract -- -D warnings`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/midnight-contract/
git commit -m "feat(contract): implement Contract<P, L> with typed ledger access"
```

---

## Task 8: midnight-core — Meta-crate Re-exports

Wire up the meta-crate with feature-gated re-exports.

**Files:**
- Modify: `crates/midnight-core/src/lib.rs`

- [ ] **Step 1: Write compile tests**

In `crates/midnight-core/src/lib.rs`, add:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn reexports_provider_types() {
        // These should all resolve via midnight-core re-exports.
        let _: fn() -> Result<Option<crate::Block>, crate::ProviderError>;
        let _: fn() -> Result<Option<crate::Transaction>, crate::IndexerError>;
    }

    #[test]
    #[cfg(feature = "contract")]
    fn reexports_contract_types() {
        use crate::{Contract, ContractError};
        let _: fn() -> Result<(), ContractError>;
        // Contract is generic, just check the name resolves.
        let _ = std::any::type_name::<Contract<(), ()>>();
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p midnight-core`
Expected: compilation errors — re-exports don't exist yet.

- [ ] **Step 3: Implement lib.rs**

```rust
//! Meta-crate for the midnight-rs SDK.
//!
//! Re-exports all sub-crates for convenience. Use feature flags to opt out
//! of crates you don't need.

// Re-export sub-crates as modules.
#[cfg(feature = "indexer")]
pub use midnight_indexer_client as indexer;

#[cfg(feature = "provider")]
pub use midnight_provider as provider;

#[cfg(feature = "contract")]
pub use midnight_contract as contract;

// Re-export key provider types at top level.
#[cfg(feature = "provider")]
pub use midnight_provider::{Health, MidnightProvider, Provider, ProviderError};

// Re-export key indexer types at top level.
#[cfg(feature = "indexer")]
pub use midnight_indexer_client::{
    Block, ContractAction, ContractBalance, ContractCall, ContractDeploy, ContractUpdate,
    IndexerClient, IndexerError, RegularTransaction, Segment, SystemTransaction, Transaction,
    TransactionFees, TransactionResult, TransactionResultStatus, UnshieldedUtxo,
};

// Re-export contract types (gated behind "contract" feature).
#[cfg(feature = "contract")]
pub use midnight_contract::{Contract, ContractError, FromHex};

// Re-export midnight-bindgen for the contract! macro (gated behind "contract" feature).
#[cfg(feature = "contract")]
pub use midnight_bindgen;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p midnight-core`
Expected: all tests pass.

- [ ] **Step 5: Run clippy on entire workspace**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings across all 4 crates.

- [ ] **Step 6: Commit**

```bash
git add crates/midnight-core/
git commit -m "feat(core): add meta-crate with feature-gated re-exports"
```

---

## Task 9: Integration Tests (Devnet)

Add integration tests gated behind environment variables, matching the pattern from mcs-connector-midnight.

**Files:**
- Create: `crates/midnight-indexer-client/tests/devnet.rs`
- Create: `crates/midnight-provider/tests/devnet.rs`

These tests only run when `MIDNIGHT_INDEXER_URL` (and optionally `MIDNIGHT_NODE_URL`, `MIDNIGHT_CONTRACT_ADDRESS`) are set.

- [ ] **Step 1: Create indexer-client devnet tests**

Create `crates/midnight-indexer-client/tests/devnet.rs`:

```rust
//! Integration tests against a running Midnight devnet indexer.
//! Skipped unless MIDNIGHT_INDEXER_URL is set.

use midnight_indexer_client::IndexerClient;

fn client() -> Option<IndexerClient> {
    std::env::var("MIDNIGHT_INDEXER_URL")
        .ok()
        .map(|url| IndexerClient::new(&url).expect("valid URL"))
}

macro_rules! require_indexer {
    () => {
        match client() {
            Some(c) => c,
            None => {
                eprintln!("skipping: MIDNIGHT_INDEXER_URL not set");
                return;
            }
        }
    };
}

#[tokio::test]
async fn health_check() {
    let client = require_indexer!();
    assert!(client.health_check().await);
}

#[tokio::test]
async fn get_latest_block() {
    let client = require_indexer!();
    let block = client.get_latest_block().await.unwrap();
    let block = block.expect("devnet should have blocks");
    assert!(block.height > 0);
    assert!(!block.hash.is_empty());
}

#[tokio::test]
async fn get_block_by_height() {
    let client = require_indexer!();
    let block = client.get_block_by_height(1).await.unwrap();
    let block = block.expect("block 1 should exist");
    assert_eq!(block.height, 1);
}

#[tokio::test]
async fn get_block_by_hash() {
    let client = require_indexer!();
    let latest = client.get_latest_block().await.unwrap().unwrap();
    let by_hash = client.get_block_by_hash(&latest.hash).await.unwrap().unwrap();
    assert_eq!(by_hash.height, latest.height);
}

#[tokio::test]
async fn get_block_with_transactions() {
    let client = require_indexer!();
    let block = client.get_block_with_transactions(1).await.unwrap().unwrap();
    assert_eq!(block.height, 1);
    assert!(block.transactions.is_some());
}

#[tokio::test]
async fn get_nonexistent_block() {
    let client = require_indexer!();
    let block = client.get_block_by_height(999_999_999).await.unwrap();
    assert!(block.is_none());
}

#[tokio::test]
async fn get_transactions_by_nonexistent_hash() {
    let client = require_indexer!();
    let txs = client
        .get_transactions_by_hash("0000000000000000000000000000000000000000000000000000000000000000")
        .await
        .unwrap();
    assert!(txs.is_empty());
}
```

- [ ] **Step 2: Create provider devnet tests**

Create `crates/midnight-provider/tests/devnet.rs`:

```rust
//! Integration tests against a running Midnight devnet.
//! Skipped unless MIDNIGHT_INDEXER_URL and MIDNIGHT_NODE_URL are set.

use midnight_provider::{MidnightProvider, Provider};

fn provider() -> Option<MidnightProvider> {
    let indexer_url = std::env::var("MIDNIGHT_INDEXER_URL").ok()?;
    let node_url = std::env::var("MIDNIGHT_NODE_URL").ok()?;
    Some(MidnightProvider::new(&node_url, &indexer_url).expect("valid URLs"))
}

fn contract_address() -> Option<String> {
    if let Ok(addr) = std::env::var("MIDNIGHT_CONTRACT_ADDRESS") {
        return Some(addr);
    }
    if let Ok(path) = std::env::var("MIDNIGHT_CONTRACT_ADDRESS_FILE") {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            let addr = contents.trim().to_string();
            if !addr.is_empty() {
                return Some(addr);
            }
        }
    }
    None
}

macro_rules! require_provider {
    () => {
        match provider() {
            Some(p) => p,
            None => {
                eprintln!("skipping: MIDNIGHT_INDEXER_URL or MIDNIGHT_NODE_URL not set");
                return;
            }
        }
    };
}

macro_rules! require_contract {
    () => {{
        let p = require_provider!();
        match contract_address() {
            Some(addr) => (p, addr),
            None => {
                eprintln!("skipping: MIDNIGHT_CONTRACT_ADDRESS not set");
                return;
            }
        }
    }};
}

#[tokio::test]
async fn health_check() {
    let p = require_provider!();
    let health = p.health().await.unwrap();
    assert!(health.node_connected);
    assert!(health.indexer_connected);
    assert!(health.block_height.unwrap() > 0);
    eprintln!("health: {health:?}");
}

#[tokio::test]
async fn get_block_number() {
    let p = require_provider!();
    let height = p.get_block_number().await.unwrap();
    assert!(height > 0);
}

#[tokio::test]
async fn get_block() {
    let p = require_provider!();
    let block = p.get_block().await.unwrap().unwrap();
    assert!(block.height > 0);
}

#[tokio::test]
async fn get_contract_state() {
    let (p, addr) = require_contract!();
    let hex = p.get_contract_state(&addr).await.unwrap();
    assert!(hex.is_some(), "deployed contract should have state");
    eprintln!("contract state: {} hex chars", hex.unwrap().len());
}

#[tokio::test]
async fn get_contract_action() {
    let (p, addr) = require_contract!();
    let action = p.get_contract_action(&addr).await.unwrap();
    assert!(action.is_some());
    let action = action.unwrap();
    assert_eq!(action.address(), addr);
}
```

- [ ] **Step 3: Verify tests compile**

Run: `cargo test --workspace --no-run`
Expected: compiles successfully. Tests will be skipped at runtime without env vars.

- [ ] **Step 4: Run tests (without devnet — should skip)**

Run: `cargo test --workspace`
Expected: devnet tests print "skipping: MIDNIGHT_INDEXER_URL not set" and pass. Unit tests all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/midnight-indexer-client/tests/ crates/midnight-provider/tests/
git commit -m "test: add devnet integration tests for indexer-client and provider"
```

---

## Task 10: README and Final Checks

Add a README with usage examples and run final verification.

**Files:**
- Create: `README.md`

- [ ] **Step 1: Create README.md**

Write a concise README with:
- One-paragraph description
- Crate table (name, description, crates.io badge placeholder)
- Quick start example (the end-to-end example from the spec)
- Feature flags table
- Development section (how to build, test, run devnet tests)
- License (MIT OR Apache-2.0)

- [ ] **Step 2: Run full workspace check**

Run: `cargo check --workspace`
Expected: clean.

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

Run: `cargo test --workspace`
Expected: all unit tests pass, devnet tests skip gracefully.

Run: `cargo doc --workspace --no-deps`
Expected: docs generate without warnings.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: add README with usage examples and development guide"
```

---

## Execution Notes

**Task dependency order:** Tasks 1→2→3 (indexer-client), then 4→5 (provider), then 6 (bindgen change, external repo), then 7 (contract), then 8 (core), then 9→10 (tests + docs).

Tasks 6 (bindgen) is in a separate repo (`~/Projects/project-ideas/compact-rust-codegen/`). It must be completed before Task 7 because `midnight-contract` depends on `FromHex` from `midnight-bindgen`.

**Parallelizable tasks:** Tasks 2+3 and 4+5 could run in parallel since `midnight-indexer-client` and `midnight-provider` (trait definition only) don't depend on each other. However, Task 5 (MidnightProvider impl) depends on Task 3 (IndexerClient) being complete.

**Environment for devnet tests (Task 9):**
```bash
export MIDNIGHT_INDEXER_URL=http://localhost:8088
export MIDNIGHT_NODE_URL=ws://localhost:9944
export MIDNIGHT_CONTRACT_ADDRESS=<deployed gateway address>
```
Start devnet with `make dev-up-midnight` from the mcs-compact-sandbox repo.
