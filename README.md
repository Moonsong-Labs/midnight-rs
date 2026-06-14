# midnight-rs

**The Rust SDK for the [Midnight](https://midnight.network) blockchain.** Deploy Compact smart contracts, call circuits on-chain, manage shielded and unshielded wallets, and query the indexer, all from Rust.

> [!WARNING]
> This project is under active development and is **not production ready**. APIs may change without notice.

## Features

- **Deploy & call Compact smart contracts**: typed Rust bindings generated from `contract-info.json`, with on-chain circuit calls that take typed arguments and return typed values.
- **Per-contract private state**: pluggable `PrivateStateProvider` store with password-encrypted export/import; witnesses thread the state through circuit calls (see [`docs/private-state.md`](docs/private-state.md)).
- **Contract maintenance / governance**: deploy with a k-of-n maintenance committee, rotate verifier keys and replace the authority via externally-signed updates (see [`docs/contract-maintenance-governance.md`](docs/contract-maintenance-governance.md)).
- **Shielded & unshielded wallet**: zswap shielded coins, unshielded UTXOs, and Dust (the fee token), all synced in parallel.
- **Indexer & node clients**: a typed GraphQL client for the Midnight indexer plus node RPC over subxt.

## Prerequisites

Circuit execution and transaction building require a **forked Compact compiler** ([`RomarQ/compact`](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions)) that extends `contract-info.json` with circuit IR. It's pinned as a git submodule and built via Nix; the `Makefile` wraps the fetch + build:

```bash
make build-compactc          # fetch + nix-build the pinned compactc
make compile-contracts       # recompile devnet/contracts/* with it
```

Override with `COMPACTC=<path>` to use a system-installed binary instead. To invoke the built compiler directly:

```bash
tools/compact-compiler/result/bin/compactc my_contract.compact compiled/my_contract
```

## Quick start

```rust
use midnight_provider::{MidnightProvider, Network, Seed};

mod counter {
    midnight_bindgen::contract!("compiled/contract-info.json");
}

const NODE_URL: &str = "ws://localhost:9944";
const INDEXER_URL: &str = "http://localhost:8088";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seed = Seed::from_hex(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )?;
    // The provider owns the URLs and drives the wallet sync (zswap + dust +
    // unshielded subscriptions against its own indexer).
    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?
        .sync_wallet(seed, Network::Undeployed)
        .await?;

    // Deploy — the builder is awaitable directly via `IntoFuture`.
    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_keys("compiled")
        .await?;

    println!("deployed at {}", contract.address());
    println!("round = {}", contract.ledger().await?.round()?);

    // Call a circuit on-chain. `circuits()` defaults to no witnesses; add
    // `.with_witnesses(&w)` for stateful witnesses. Circuits with typed return
    // values hand them back to the caller.
    let returned: u64 = contract.circuits().increment().await?;
    println!("increment returned {returned}");
    println!("round = {}", contract.ledger().await?.round()?);

    // Typed arguments are supported for on-chain calls.
    let returned: u16 = contract.circuits().increment_by(5).await?;
    println!("increment_by(5) returned {returned}");
    println!("round = {}", contract.ledger().await?.round()?);

    // Reconnect from just the address (synchronous, no network calls) — what a
    // second process would do — and call through the fresh handle.
    let address = contract.address().to_string();
    let reconnected = counter::Contract::at(&provider, &address)
        .with_zk_keys("compiled")
        .build();
    let returned: u64 = reconnected.circuits().increment().await?;
    println!("after reconnect, increment returned {returned}");

    Ok(())
}
```

The `contract!` macro validates `contract-info.json` before generating anything and rejects compiler or language versions outside the supported families (currently compiler 0.30.x/0.31.x, language 0.22.x/0.23.x) with a compile error. The error names the offending version and explains how to proceed: recompile the contract with a supported Compact compiler, or widen the supported range in `compact-codegen`.

See [`examples/`](examples) for complete working examples. They run against a local
devnet (node + indexer) — `make dev-up` from the repo root starts it (or
`docker compose -f devnet/docker-compose.yml up -d` directly), and `make e2e`
spins the devnet up, runs every example end-to-end, and tears it down.

## Wallet

The provider owns a typed `Wallet` that tracks shielded coins, unshielded UTXOs, and Dust (the fee token).
`sync_wallet` above runs all three subscriptions in parallel and persists progress to disk. Balance queries,
transfers, Dust registration, and submission helpers all hang off `MidnightProvider`:

```rust,ignore
let balance = provider.balance().await.expect("wallet attached");
let pending = provider.transfer_unshielded(midnight_wallet::NIGHT, 100, &recipient).await?;
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
from the mempool; see [`PendingTx`](crates/midnight-provider/src/submit.rs) for details.

Failed waits surface `ProviderError::Submission` carrying a typed `SubmitError`: match its variants (`Invalid` is a definitive rejection, safe to rebuild and resubmit; `Dropped` / `NodeError` mean the tx may still land, so resubmitting risks a double spend; `WatchStream` is transport trouble; `VerdictFetch` means the tx landed but its events couldn't be decoded, so don't resubmit, re-query the chain) instead of parsing error text. See [`SubmitError`](crates/midnight-provider/src/submit.rs) for the full variant set, including the pre-watch `NotSubmitted` / `SubmitRpc` cases.

Inclusion in a block confirms the **guaranteed phase** passed but says nothing about whether the fallible phase (contract calls, verifier-key updates) actually succeeded. For that, call `provider.wait_transaction_result(&extrinsic_hash, timeout, poll_interval).await?` after `wait_best`. It returns `TxResultWait::Found(TransactionResult { status, segments })` once the indexer surfaces the chain's verdict, or `TxResultWait::TimedOut` if it didn't within the deadline (the result may still surface later; a lagging indexer and a tx that never landed are indistinguishable). See [`docs/midnight-js-comparison.md`](docs/midnight-js-comparison.md) for the guaranteed/fallible phase model.

## Crates

| Crate | Description |
|---|---|
| `midnight-core` | Meta-crate, re-exports all sub-crates |
| `midnight-provider` | `Provider` trait + `MidnightProvider` (indexer + node RPC + wallet ownership) |
| `midnight-contract` | Typed contract interactions: deploy, call, query, prove, submit |
| `midnight-wallet` | `Wallet` state machine: sync, balances, transfers, dust, address derivation |
| `midnight-private-state` | `PrivateStateProvider` store for per-contract private state + signing keys, with encrypted export/import |
| `midnight-bindgen` | `contract!` macro: generates typed bindings from `contract-info.json` |
| `midnight-indexer-client` | Typed GraphQL client for the Midnight indexer API |
| `midnight-crypto` | Facade re-exporting `midnight-base-crypto`, `midnight-curves`, `midnight-transient-crypto` as namespaced modules |
| `midnight-helpers` | Facade over `midnight-node-ledger-helpers` (single pinning point for `LedgerContext`, `DustSpend`, etc.) |

## Development

The `Makefile` wraps the workflow; the CI in [`.github/workflows/ci.yml`](.github/workflows/ci.yml) calls the same targets.

```bash
make ci              # the full CI gate: fmt-check + clippy -D warnings + check + test
make test            # cargo test --workspace
make dev-up          # start the local devnet (node + indexer)
make test-e2e        # devnet integration tests
make examples        # run the example crates against the devnet
```

Run `make` (no args) for the full list.
