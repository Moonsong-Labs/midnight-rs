# midnight-rs

> [!WARNING]
> This project is under active development and is **not production ready**. APIs may change without notice.

Rust SDK for the Midnight blockchain following [alloy-rs](https://github.com/alloy-rs/alloy) patterns.
Read-only for now: contract state access, block and transaction queries.
General-purpose — not tied to any specific consumer.

## Crates

| Crate | Description |
|---|---|
| `midnight-core` | Meta-crate, re-exports all sub-crates |
| `midnight-provider` | `Provider` trait + `MidnightProvider` (indexer + node RPC) |
| `midnight-contract` | `Contract<P, L>` for typed contract state access |
| `midnight-indexer-client` | Typed GraphQL client for the Midnight indexer API |

## Quick Start

```rust
use midnight_core::{MidnightProvider, Contract, Provider};

// Generate typed bindings (via midnight-bindgen)
mod gateway {
    midnight_core::midnight_bindgen::contract!(
        "path/to/contract-info.json"
    );
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = MidnightProvider::new("ws://localhost:9944", "http://localhost:8088")?;

    let health = provider.health().await?;
    println!("connected: node={}, indexer={}", health.node_connected, health.indexer_connected);

    let contract = Contract::<_, gateway::Ledger>::new("contract_address", &provider);
    let ledger = contract.ledger().await?;

    println!("threshold: {}", ledger.threshold()?);
    println!("operations: {:?}", ledger.operations());

    Ok(())
}
```

## Feature Flags

| Flag | Default | Description |
|---|---|---|
| `indexer` | yes | Include `midnight-indexer-client` |
| `provider` | yes | Include `midnight-provider` |
| `contract` | yes | Include `midnight-contract` + `midnight-bindgen` |

## Development

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings

# Integration tests (requires running devnet)
MIDNIGHT_INDEXER_URL=http://localhost:8088 \
MIDNIGHT_NODE_URL=ws://localhost:9944 \
MIDNIGHT_CONTRACT_ADDRESS=<address> \
cargo test --workspace
```

## License

MIT OR Apache-2.0
