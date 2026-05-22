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
use midnight_provider::{MidnightProvider, WalletSeed};

mod counter {
    midnight_bindgen::contract!("compiled/contract-info.json");
}

const NODE_URL: &str = "ws://localhost:9944";
const INDEXER_URL: &str = "http://localhost:8088";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seed = WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )?;
    // The provider owns the URLs and drives the wallet sync (zswap + dust +
    // unshielded subscriptions against its own indexer).
    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?
        .sync_wallet(seed, "undeployed", None)
        .await?;

    // Deploy — the builder is awaitable directly via `IntoFuture`.
    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_keys("compiled")
        .await?;

    println!("deployed at {}", contract.address());
    println!("round = {}", contract.ledger().await?.round()?);

    // Call a circuit on-chain. Witnesses are provided once per call chain;
    // circuits with typed return values hand them back to the caller.
    let returned: u64 = contract.circuits(&NoWitnesses).increment().await?;
    println!("returned = {returned}");
    println!("round = {}", contract.ledger().await?.round()?);

    // Typed arguments are supported for on-chain calls.
    let returned: u16 = contract.circuits(&NoWitnesses).increment_by(5).await?;

    // Reference an existing contract (synchronous, no network calls).
    let address = contract.address().to_string();
    let _contract = counter::Contract::at(&provider, &address)
        .with_zk_keys("compiled")
        .build();
    println!("returned = {returned}");

    Ok(())
}
```

See [`examples/counter`](examples/counter) for a complete working example with Docker setup.

## Wallet

The provider owns a typed `Wallet` that tracks shielded coins, unshielded UTXOs, and Dust (the fee token).
`sync_wallet` above runs all three subscriptions in parallel and persists progress to disk. Balance queries,
transfers, Dust registration, and submission helpers all hang off `MidnightProvider`:

```rust,ignore
let balance = provider.balance().await.expect("wallet attached");
let result  = provider.transfer_unshielded(midnight_wallet::NIGHT, 100, &recipient).await?;
let pending = provider.submit(&result.tx_bytes).await?;
let (_, _)  = pending.wait_best().await?;
```

See [`docs/wallet.md`](docs/wallet.md) for sync, balances, transfers, Dust registration, persistence layout,
and pending-spend reservations. The [`examples/wallet-sync`](examples/wallet-sync) crate is a runnable
end-to-end walkthrough.

## Observing inclusion explicitly

The simple `.await?` path above submits, waits for the best block, then waits for the indexer.
If you want to observe both `Best` and `Finalized` block hashes, use `.send().await?`:

```rust,ignore
let pending = counter::Contract::deploy(&provider)
    .with_initial_state(counter::LedgerInitialState::default())
    .with_zk_keys("compiled")
    .send().await?;
println!("ext: {}", pending.extrinsic_hash_hex());
let (best, pending)      = pending.wait_best().await?;
let (finalized, pending) = pending.wait_finalized().await?;
let contract             = pending.into_contract().await?;
```

`wait_best` / `wait_finalized` consume `self` and return it back so callers re-bind through each
step without `let mut`. Cancelling either future is safe but does not retract the transaction
from the mempool; see [`PendingTx`](crates/midnight-contract/src/call.rs) for details.

## Crates

| Crate | Description |
|---|---|
| `midnight-core` | Meta-crate, re-exports all sub-crates |
| `midnight-provider` | `Provider` trait + `MidnightProvider` (indexer + node RPC) |
| `midnight-contract` | Typed contract interactions: deploy, call, query, prove, submit |
| `midnight-wallet` | Validated `Wallet` handle: seed validation, address derivation |
| `midnight-bindgen` | `contract!` macro: generates typed bindings from `contract-info.json` |
| `midnight-indexer-client` | Typed GraphQL client for the Midnight indexer API |

## Development

```bash
cargo check --workspace
cargo test --workspace
```

## License

MIT OR Apache-2.0
