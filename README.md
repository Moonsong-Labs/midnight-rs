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
use midnight_contract::interpreter::NoWitnesses;
use midnight_provider::MidnightProvider;

mod counter {
    midnight_bindgen::contract!("compiled/contract-info.json");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = MidnightProvider::new("ws://localhost:9944", "http://localhost:8088")?
        .with_wallet("0000000000000000000000000000000000000000000000000000000000000001");

    // Deploy — the builder is awaitable directly via `IntoFuture`.
    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_keys("compiled")
        .await?;

    println!("deployed at {}", contract.address());
    println!("round = {}", contract.ledger().round().await?);

    // Call a circuit on-chain. Witnesses are provided once per call chain;
    // circuits with typed return values hand them back to the caller.
    let returned: u64 = contract.circuits(&NoWitnesses).increment().await?;
    println!("returned = {returned}");
    println!("round = {}", contract.ledger().round().await?);

    // Reconnect to the same contract from a fresh handle.
    let address = contract.address().to_string();
    let contract = counter::Contract::connect(&provider, &address)
        .with_zk_keys("compiled")
        .await?;

    // Typed arguments are supported for on-chain calls.
    let returned: u16 = contract.circuits(&NoWitnesses).increment_by(5).await?;
    println!("returned = {returned}");

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
