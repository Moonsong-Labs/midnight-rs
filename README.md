# midnight-rs

> [!WARNING]
> This project is under active development and is **not production ready**. APIs may change without notice.

Rust SDK for the [Midnight](https://midnight.network) blockchain. Deploy contracts, call circuits, query state, build and prove transactions — all from Rust.

## Prerequisites

Circuit execution and transaction building require a **forked Compact compiler** ([`RomarQ/compact`](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions)) that extends `contract-info.json` with circuit IR and helper function bodies. State querying works with the standard compiler.

```bash
git clone https://github.com/RomarQ/compact.git && cd compact
git checkout feat/contract-info-extensions
nix build .#compactc
./result/bin/compactc --skip-zk my_contract.compact /tmp/compiled/my_contract
```

## Usage

### Connect and query state

```rust
use midnight_core::{MidnightProvider, Contract, Provider};

mod counter {
    midnight_core::midnight_bindgen::contract!(
        "compiled/counter/compiler/contract-info.json"
    );
}

let provider = MidnightProvider::new("ws://localhost:9944", "http://localhost:8088")?;

let contract = Contract::<_, counter::Ledger>::new("0xaabb...", &provider);
let ledger = contract.ledger().await?;
println!("round: {}", ledger.round()?);
```

### Call a circuit

```rust
// Generated method — no IR loading, no interpreter imports needed
let state = midnight_contract::fetch_state(&provider, "0xaabb...").await?;
let ledger = counter::Ledger::new(state);
let updated = ledger.call_increment()?;
```

### Deploy a contract

```rust
use midnight_contract::{deploy_funded, format_address};

let (address, tx_bytes, _) = deploy_funded(&initial_state, "undeployed").await?;
println!("deployed at: {}", format_address(&address));
```

### Submit to the network

```rust
use midnight_contract::submit;

let hash = submit("ws://localhost:9944", &tx_bytes).await?;
println!("submitted: {hash}");
```

### Prove and submit (full pipeline)

```rust
use midnight_contract::prove_circuit;

let (proven_bytes, new_state) = prove_circuit(
    &provider, "0xaabb...", &ir, "increment", "compiled/counter"
).await?;

submit("ws://localhost:9944", &proven_bytes).await?;
```

## Crates

| Crate | Description |
|---|---|
| `midnight-core` | Meta-crate, re-exports all sub-crates |
| `midnight-provider` | `Provider` trait + `MidnightProvider` (indexer + node RPC) |
| `midnight-contract` | Typed contract interactions: query, call, deploy, prove, submit |
| `midnight-indexer-client` | Typed GraphQL client for the Midnight indexer API |

## Development

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings

# With compiled contracts (requires the compiler fork)
MIDNIGHT_COMPILED_DIR=/tmp/compiled cargo test -p midnight-contract

# With proving infrastructure
MIDNIGHT_LEDGER_TEST_STATIC_DIR=/tmp/empty-keys cargo test -p midnight-contract
```

## License

MIT OR Apache-2.0
