# midnight-rs

> [!WARNING]
> This project is under active development and is **not production ready**. APIs may change without notice.

Rust SDK for the [Midnight](https://midnight.network) blockchain. Deploy contracts, call circuits on-chain, query state -- all from Rust.

## Prerequisites

Circuit execution and transaction building require a **forked Compact compiler** ([`RomarQ/compact`](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions)) that extends `contract-info.json` with circuit IR.

```bash
git clone https://github.com/RomarQ/compact.git && cd compact
git checkout feat/contract-info-extensions
nix --extra-experimental-features "nix-command flakes" build .#compactc

./result/bin/compactc my_contract.compact compiled/my_contract
```

## Quick start

```rust
use midnight_provider::MidnightProvider;

mod counter {
    midnight_bindgen::contract!("compiled/contract-info.json");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = MidnightProvider::new("ws://localhost:9944", "http://localhost:8088")?
        .with_wallet("0000000000000000000000000000000000000000000000000000000000000001");

    // Deploy
    let mut contract = counter::Contract::deploy()
        .provider(&provider)
        .initial_state(counter::LedgerInitialState { round: 0 })
        .zk_keys("compiled")
        .deploy()
        .await?;

    println!("deployed at {}", contract.address());
    println!("round = {}", contract.ledger().round()?);

    // Call circuit on-chain
    contract.circuits().increment().await?;
    println!("round = {}", contract.ledger().round()?);

    Ok(())
}
```

See [`examples/counter`](examples/counter) for a complete working example with Docker setup.

## Crates

| Crate | Description |
|---|---|
| `midnight-core` | Meta-crate, re-exports all sub-crates |
| `midnight-provider` | `Provider` trait + `MidnightProvider` (indexer + node RPC) |
| `midnight-contract` | Typed contract interactions: deploy, call, query, prove, submit |
| `midnight-indexer-client` | Typed GraphQL client for the Midnight indexer API |

## Development

```bash
cargo check --workspace
cargo test --workspace
```

## License

MIT OR Apache-2.0
