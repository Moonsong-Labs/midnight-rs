# midnight-rs

**The Rust SDK for the [Midnight](https://midnight.network) blockchain.** Deploy Compact smart contracts, call circuits on-chain, manage shielded and unshielded wallets, and query the indexer, all from Rust.

> [!WARNING]
> This project is under active development. APIs may change without notice.

## Features

- **Deploy & call Compact smart contracts**: typed Rust bindings generated from the compiler's `analyzed-ir.sexp`, with on-chain circuit calls that take typed arguments and return typed values.
- **Per-contract private state**: pluggable `PrivateStateProvider` store with password-encrypted export/import; witnesses thread the state through circuit calls (see [`docs/private-state.md`](docs/private-state.md)).
- **Contract maintenance / governance**: deploy with a k-of-n maintenance committee, rotate verifier keys and replace the authority via externally-signed updates (see [`docs/contract-maintenance-governance.md`](docs/contract-maintenance-governance.md)).
- **Shielded & unshielded wallet**: zswap shielded coins, unshielded UTXOs, and Dust (the fee token), all synced in parallel.
- **Indexer & node clients**: a typed GraphQL client for the Midnight indexer plus node RPC over subxt.
- **Ledger 8 and ledger 9**: one build runs on chains of both generations, reads the generation from the chain, and carries a wallet across the hard fork (see [`docs/ledger-generations.md`](docs/ledger-generations.md)).

## Prerequisites

- Rust: [`rust-toolchain.toml`](rust-toolchain.toml) names the tested toolchain.
- Docker, to run the local devnet (node and indexer).
- Nix, to build the Compact compiler.

The SDK reads `compiler/analyzed-ir.sexp`, an artifact that only a fork of the Compact compiler writes. The `tools/compact-compiler` submodule pins that fork ([`RomarQ/compact`](https://github.com/RomarQ/compact)), and the `Makefile` builds it with Nix:

```bash
make build-compactc          # fetch + nix-build the pinned compactc
make compile-contracts       # recompile devnet/contracts/* with it
```

To make the `Makefile` use a different compactc binary, set `COMPACTC=<path>`. That binary must be a build of the same fork: the `Makefile` refuses a compactc that does not take `--analyzed-ir`.

To compile a contract for the SDK, pass `--analyzed-ir`. This command, run from the root of this repository, compiles the counter contract of the [Quick start](#quick-start) into your crate:

```bash
tools/compact-compiler/result/bin/compactc --analyzed-ir \
    devnet/contracts/counter/counter.compact path/to/your-crate/compiled/counter
```

The SDK reads these entries of the output directory:

- `compiler/analyzed-ir.sexp`: the file that the `contract!` macro reads.
- `keys/`: the prover and verifier keys of each circuit.
- `zkir/`: the circuit IR that the prover reads.

## Install

Add the SDK and tokio to the `Cargo.toml` of your crate:

```toml
[dependencies]
midnight-core = { git = "https://github.com/Moonsong-Labs/midnight-rs" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Then copy the `[patch.crates-io]` table at the end of the root [`Cargo.toml`](Cargo.toml) of this repository into the root manifest of your build. That manifest is the `Cargo.toml` of your crate, or the root `Cargo.toml` of your workspace when your crate is a workspace member. The ledger 9 crates ship only as git tags, and that table points each one at its tag. Cargo applies a `[patch]` table only from the root manifest of the build, so your crate does not get the table through the dependency. Without the table, Cargo stops with `failed to select a version for the requirement` on a ledger 9 crate.

`midnight-core` re-exports the SDK crates as the modules `provider`, `wallet`, `contract`, `indexer` and `crypto` (see [Crates](#crates)). The `contract!` macro needs the `contract` feature of `midnight-core`, which is on by default.

The first build clones these git sources:

- midnight-rs, with its `tools/compact-compiler` submodule (the compiler fork).
- [midnight-node](https://github.com/midnightntwrk/midnight-node), for the ledger helpers.
- [midnight-ledger](https://github.com/midnightntwrk/midnight-ledger), through the patch table.
- [polkadot-sdk](https://github.com/paritytech/polkadot-sdk), for `sp-storage`.

## Quick start

```rust
use midnight_core::{LocalWallet, MidnightProvider, Network, Seed, Wallet};

mod counter {
    midnight_core::contract!("compiled/counter/compiler/analyzed-ir.sexp");
}

const NODE_URL: &str = "ws://localhost:9944";
const INDEXER_URL: &str = "http://localhost:8088";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seed = Seed::from_hex(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )?;
    // The wallet syncs on its own (zswap + dust + unshielded subscriptions
    // against the indexer) and is then attached to the provider. `pinned_to`
    // is the chain-reset guard: the wallet's cursors are counts, so without a
    // pin a recreated chain resumes cleanly and serves the old chain's balance.
    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?;
    let wallet = Wallet::sync(provider.indexer_url(), seed, Network::Undeployed)
        .pinned_to(&provider)
        .await?;
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    // Deploy: the builder is awaitable directly via `IntoFuture`.
    // `.with_zk_config` takes a custom `ZkConfigProvider`, or the compiler's
    // output directory, which holds `keys/` and `zkir/`. A relative path
    // resolves against the working directory, not the crate root.
    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(concat!(env!("CARGO_MANIFEST_DIR"), "/compiled/counter"))
        .await?;

    println!("deployed at {}", contract.address());
    println!("round = {}", contract.ledger().await?.round()?);

    // Call a circuit on-chain. `circuits()` defaults to no witnesses; add
    // `.with_witnesses(&w)` for stateful witnesses. A call returns a
    // `CallOutcome`: `.value` is the circuit's typed return value, and the
    // other fields identify the transaction that carried the call.
    let returned: u64 = contract.circuits().increment().await?.value;
    println!("increment returned {returned}");
    println!("round = {}", contract.ledger().await?.round()?);

    // Typed arguments are supported for on-chain calls.
    let returned: u16 = contract.circuits().increment_by(5).await?.value;
    println!("increment_by(5) returned {returned}");
    println!("round = {}", contract.ledger().await?.round()?);

    Ok(())
}
```

The `contract!` macro checks `analyzed-ir.sexp` before it generates anything. It rejects a compiler or language version outside the supported families, [`SUPPORTED_COMPILER_VERSION_FAMILIES`](crates/compact/codegen/src/types.rs) and [`SUPPORTED_LANGUAGE_VERSION_FAMILIES`](crates/compact/codegen/src/types.rs), with a compile error. The error names the offending version and explains how to proceed: recompile the contract with a supported Compact compiler, or widen the supported range in `compact-codegen`.

See [`examples/`](examples) for complete working examples. They run against a local devnet (node + indexer). Run `make dev-up` from the repo root to start it, or run `docker compose -f devnet/docker-compose.yml up -d` directly. `make e2e` starts the devnet, runs the examples in the `EXAMPLES` list of the [`Makefile`](Makefile), and stops the devnet.

## Connecting to an existing contract

Given a contract address (from an earlier deploy, or another process), reconnect with
`Contract::at`. It's synchronous and makes no network calls; the returned handle fetches
fresh state per call, exactly like the one `deploy` hands back:

```rust,ignore
let contract = counter::Contract::at(&provider, &address)
    .with_zk_config(concat!(env!("CARGO_MANIFEST_DIR"), "/compiled/counter"))
    .build();

let returned: u64 = contract.circuits().increment().await?.value;
println!("increment returned {returned}");
println!("round = {}", contract.ledger().await?.round()?);
```

## Wallet

The provider holds a wallet through `WalletFacade` and the `WalletBuilds` of each ledger generation. `midnight-wallet`'s `Wallet`, attached as `LocalWallet`, is the local implementation, tracking shielded coins, unshielded UTXOs, and Dust (the fee token).
`Wallet::sync` above runs all three subscriptions in parallel and can persist progress to disk. Balance queries,
transfers, Dust registration, and submission helpers all hang off `MidnightProvider`:

```rust,ignore
let balance = provider.balance().await.expect("wallet attached");
let pending = provider.transfer_unshielded(midnight_core::wallet::NIGHT, 100, &recipient).await?;
pending.wait_finalized().await?;
```

See [`docs/wallet.md`](docs/wallet.md) for sync, balances, transfers, Dust registration, persistence layout,
and pending-spend reservations. The [`examples/wallet-sync`](examples/wallet-sync) crate is a runnable
end-to-end walkthrough.

## Observing inclusion explicitly

The simple `.await?` path above submits, waits for the best block, checks that the chain applied the deploy there, then waits for the indexer. One deadline, set with `with_deploy_timeout` (60 s by default), bounds the two waits together. If you want to observe both `Best` and `Finalized` block hashes, use `.send().await?`:

```rust,ignore
let pending = counter::Contract::deploy(&provider)
    .with_initial_state(counter::LedgerInitialState::default())
    .with_zk_config(concat!(env!("CARGO_MANIFEST_DIR"), "/compiled/counter"))
    .send().await?;
println!("ext: {}", pending.extrinsic_hash_hex());
let (best, pending)      = pending.wait_best().await?;
let (finalized, pending) = pending.wait_finalized().await?;
let contract             = pending.into_contract().await?;
```

`wait_best` / `wait_finalized` consume `self` and return it back so callers re-bind through each
step without `let mut`. Cancelling either future is safe but does not retract the transaction
from the mempool; see [`PendingTx`](crates/midnight-provider/src/submit.rs) for details.

Each wait fails with `ContractError::TransactionFailed` when the chain did not apply the deploy. With no wait before it, `into_contract` waits for the best block itself. When the deadline passes first, it fails with `ContractError::DeployTimeout`. Its `in_block` tells you to query the transaction before you deploy again, or to connect with `Contract::at`.

A raw `PendingTx` wait on a transaction that landed but did not apply fails with `ProviderError::NotApplied` (see below). Other failed waits surface `ProviderError::Submission` carrying a typed `SubmitError`: match its variants (`Invalid` is a definitive rejection, safe to rebuild and resubmit; `Dropped` / `NodeError` mean the tx may still land, so resubmitting risks a double spend; `WatchStream` is transport trouble; `VerdictFetch` means the tx landed but its events couldn't be decoded, so don't resubmit, re-query the chain) instead of parsing error text. See [`SubmitError`](crates/midnight-provider/src/submit.rs) for the full variant set, including the pre-watch `NotSubmitted` / `SubmitRpc` cases.

A completed `wait_best` / `wait_finalized` means the extrinsic carrying your transaction reached a block and the chain applied the transaction. The wait reads the verdict from the events the Midnight pallet emits for the transaction, so no indexer is involved: `Success` means every phase applied, `PartialSuccess` means the guaranteed phase committed and at least one fallible segment did not, and `Failure` means the dispatch errored and nothing applied at all. Only `Success` returns `Ok`. For the other two, the wait fails with `ProviderError::NotApplied`, whose `NotApplied` holds the `TxInBlock` with the verdict. Match the verdict there when you need to branch on the outcome. The verdict of `wait_best` is provisional: a reorg can drop the block, and the transaction can land again with another verdict. `wait_finalized` gives the final verdict. An `Err` consumes the handle, so when the final verdict matters, call `wait_finalized` in place of `wait_best`. The events name the transaction but not which segment failed; for a transaction with more than one fallible segment, `provider.get_transactions(TransactionOffset::hash(not_applied.0.transaction_hash.to_string()))` reads the indexer's per-segment breakdown. See [`docs/midnight-js-comparison.md`](docs/midnight-js-comparison.md) for the two-phase model.

## Crates

| Crate | Description |
|---|---|
| `midnight-core` | The crate to depend on: re-exports the SDK as the modules `provider`, `wallet`, `contract`, `indexer` and `crypto`, and the names of the quick start at its root |
| `midnight-provider` | `Provider` trait + `MidnightProvider` (indexer + node RPC + wallet ownership) |
| `midnight-contract` | Typed contract interactions: deploy, call, query, prove, submit |
| `midnight-wallet` | `Wallet` state machine: sync, balances, transfers, dust, address derivation |
| `midnight-private-state` | `PrivateStateProvider` store for per-contract private state + signing keys, with encrypted export/import |
| `compact-bindgen` | `contract!` macro: generates typed bindings from `analyzed-ir.sexp` |
| `midnight-indexer-client` | Typed GraphQL client for the Midnight indexer API |
| `midnight-crypto` | Facade re-exporting `midnight-base-crypto`, `midnight-curves`, `midnight-transient-crypto` as namespaced modules |
| `midnight-helpers` | A `ledger_8` and a `ledger_9` module over the upstream node helpers (the single pinning point for them), and the items both generations share |

## Development

The `Makefile` wraps the workflow; the CI in [`.github/workflows/ci.yml`](.github/workflows/ci.yml) calls the same targets.

```bash
make ci              # the local CI gates (see the ci target in the Makefile)
make test            # cargo test --workspace
make dev-up          # start the local devnet (node + indexer)
make test-e2e        # devnet integration tests
make examples        # run the example crates against the devnet
make conformance     # circuit interpreter vs @midnight-ntwrk/compact-runtime goldens
```

The conformance suite ([`tests/conformance`](tests/conformance)) cross-checks the Rust circuit interpreter against the canonical TypeScript Compact runtime over a corpus of compiled contracts; `make conformance-regen` (Node 22+) regenerates the goldens and CI fails when they drift.

Run `make` (no args) for the full list.

### Stack size in a debug build

Building and proving a transaction runs deep. At `opt-level = 0` a single deploy against the local devnet needs between 1.5 and 1.75 MiB of stack. `libtest` gives each test thread 2 MiB, so an unoptimized test has little room left.

Every public entry point that builds, proves, or resyncs hands back a boxed future, so a caller's own future stays a few hundred bytes instead of tens of kilobytes. Raise `RUST_MIN_STACK` (`RUST_MIN_STACK=16777216`) if a test of your own still runs out.
