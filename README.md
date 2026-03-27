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

## Crates

| Crate | Description |
|---|---|
| `midnight-core` | Meta-crate, re-exports all sub-crates |
| `midnight-provider` | `Provider` trait + `MidnightProvider` (indexer + node RPC) |
| `midnight-contract` | Typed contract interactions: query, call, deploy, prove |
| `midnight-indexer-client` | Typed GraphQL client for the Midnight indexer API |

## Usage

### Connect to the network

```rust
use midnight_core::{MidnightProvider, Provider};

let provider = MidnightProvider::new(
    "ws://localhost:9944",    // node RPC
    "http://localhost:8088",  // indexer
)?;

let health = provider.health().await?;
println!("node: {}, indexer: {}", health.node_connected, health.indexer_connected);
```

### Query contract state

The `contract!` macro generates a typed `Ledger` struct from `contract-info.json`. Each ledger field becomes a typed accessor method.

```rust
use midnight_core::{Contract, Provider};

mod counter {
    midnight_core::midnight_bindgen::contract!(
        "compiled/counter/compiler/contract-info.json"
    );
}

let contract = Contract::<_, counter::Ledger>::new("0xaabb...", &provider);
let ledger = contract.ledger().await?;
println!("round: {}", ledger.round()?);

// Historical state
let old = contract.ledger_at_height(1000).await?;
```

### Call a circuit

```rust
use midnight_contract::call;

// Fetch current state, execute the circuit, generate ZK proofs, get ready-to-submit bytes
let (proven_bytes, new_state) = call::prove_circuit(
    &provider,
    "0xaabb...",            // contract address
    &ir,                    // circuit IR
    "increment",            // circuit name
    "compiled/counter",     // compiler output directory (contains keys/)
).await?;
```

For circuits that take arguments or need private state (witnesses):

```rust
let (proven_bytes, new_state) = call::prove_circuit_with(
    &provider,
    "0xaabb...",
    &ir,
    "set",
    "compiled/tiny",
    &[("v", Value::Integer(42))],   // circuit arguments
    &my_witness_provider,            // private state callbacks
    &helpers,                        // helper functions from contract-info.json
).await?;
```

### Submit a transaction

```rust
use subxt::{OnlineClient, SubstrateConfig};

let client = OnlineClient::<SubstrateConfig>::from_insecure_url("ws://localhost:9944").await?;
let tx = subxt::dynamic::tx(
    "Midnight", "send_mn_transaction",
    vec![subxt::dynamic::Value::from_bytes(&proven_bytes)],
);
client.tx().await?.create_unsigned(&tx)?.submit().await?;
```

### Deploy a contract

```rust
use midnight_contract::call;

let (address, deploy_bytes) = call::deploy_with_provider(&provider, &initial_state).await?;
println!("deployed at: {address}");
// Submit deploy_bytes to the node (same as above)
```

## Development

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check

# With compiled contracts (requires the compiler fork)
MIDNIGHT_COMPILED_DIR=/tmp/compiled cargo test -p midnight-contract

# E2E with running node
MIDNIGHT_NODE_URL=ws://localhost:9944 \
MIDNIGHT_INDEXER_URL=http://localhost:8088 \
cargo test --workspace
```

## License

MIT OR Apache-2.0
