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
- Docker, to run the local devnet (node and indexer) and the Compact compiler.
- Nix and Node, only to regenerate the conformance goldens: Nix builds the Compact runtime that the conformance driver runs.

## Compile a contract

The SDK reads `compiler/analyzed-ir.sexp`. Only a fork of the Compact compiler, [`RomarQ/compact`](https://github.com/RomarQ/compact), writes that file, when it runs with `--analyzed-ir`. At each push to the [`midnight-rs`](https://github.com/RomarQ/compact/tree/midnight-rs) branch of the fork, a workflow publishes the new head commit as the image `ghcr.io/romarq/compactc:<commit>`. The image is public, so a pull needs no login. `COMPACT_REV` in the [`Makefile`](Makefile) names the commit that this repository tests, and the commands below use its image.

These commands, run from the root of your crate, compile the counter contract of the [Quick start](#quick-start) into it:

```bash
cp path/to/midnight-rs/devnet/contracts/counter/counter.compact .
mkdir -p "$HOME/.cache/midnight/zk-params"
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$PWD:$PWD" -w "$PWD" \
  -v "$HOME/.cache/midnight/zk-params:/zk-params" -e MIDNIGHT_PP=/zk-params \
  --entrypoint compactc \
  ghcr.io/romarq/compactc:fa2181fbc6dac2135defdb4f55ce10d8332185d5 \
  --analyzed-ir counter.compact compiled/counter
```

Each part of the `docker run` command has a reason:

- `--user "$(id -u):$(id -g)"`: you own the output files.
- `-v "$PWD:$PWD" -w "$PWD"`: the container sees only this directory, at the same path. Keep the source and the output directory under it.
- `-v "$HOME/.cache/midnight/zk-params:/zk-params" -e MIDNIGHT_PP=/zk-params`: key generation reads the public parameters from `MIDNIGHT_PP`, and downloads each parameter that is missing. The SDK prover uses the same directory by default. Create the directory before the run, because Docker creates a missing directory as root on Linux.
- `--entrypoint compactc`: the entrypoint of the image is `bash -c`. This flag runs compactc in its place, so the arguments after the image go to compactc.
- `ghcr.io/romarq/compactc:fa2181fbc6dac2135defdb4f55ce10d8332185d5`: the image of `COMPACT_REV`.
- `--analyzed-ir counter.compact compiled/counter`: `--analyzed-ir` writes `compiler/analyzed-ir.sexp` into the output directory `compiled/counter`.

With `--skip-zk`, compactc writes no keys, and the parameter cache is not necessary. From the root of this repository, `make compile-contracts` runs the same image on each contract in [`devnet/contracts`](devnet/contracts).

The SDK reads these entries of the output directory:

- `compiler/analyzed-ir.sexp`: the file that the `contract!` macro reads.
- `keys/`: the prover and verifier keys of each circuit.
- `zkir/`: the circuit IR that the prover reads.

## Install

Add the SDK, tokio and anyhow to the `Cargo.toml` of your crate. The `main` of the [Quick start](#quick-start) returns `anyhow::Result<()>`, so a failure prints its message and then each cause.

```toml
[dependencies]
midnight-core = { git = "https://github.com/Moonsong-Labs/midnight-rs" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
anyhow = "1"
```

`midnight-core` re-exports the SDK crates as the modules `provider`, `wallet`, `contract`, `indexer` and `crypto` (see [Crates](#crates)). The `contract!` macro needs the `contract` feature of `midnight-core`, which is on by default.

The first build clones these git sources:

- midnight-rs, for the SDK crates.
- [RomarQ/midnight-node](https://github.com/RomarQ/midnight-node), a fork of midnight-node, for the ledger helpers.
- [RomarQ/midnight-ledger](https://github.com/RomarQ/midnight-ledger), a fork of midnight-ledger, for the ledger 8 and ledger 9 crates.

## Quick start

```rust
use midnight_core::{LocalWallet, MidnightProvider, Network, Seed, Wallet};

mod counter {
    midnight_core::contract!("compiled/counter/compiler/analyzed-ir.sexp");
}

const NODE_URL: &str = "ws://localhost:9944";
const INDEXER_URL: &str = "http://localhost:8088";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let seed = Seed::from_hex(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )?;
    // The wallet syncs on its own (zswap + dust + unshielded subscriptions
    // against the provider's indexer) and is then attached to the provider.
    // The sync pins the wallet to the chain by default, so after a chain
    // reset its next resync fails with `ChainMismatch` instead of serving the
    // old chain's balance.
    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?;
    let wallet = Wallet::sync(&provider, seed, Network::Undeployed).await?;
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

The `contract!` macro checks `analyzed-ir.sexp` before it generates anything. Each midnight-rs release reads the output of one compiler `major.minor`, [`SUPPORTED_COMPILER_FAMILY`](crates/compact/codegen/src/types.rs), which is the compactc of the image at `COMPACT_REV`. The macro rejects an artifact from any other compiler version with a compile error that names the version. Recompile the contract with the image of [Compile a contract](#compile-a-contract), or use a midnight-rs release that supports that compiler.

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
