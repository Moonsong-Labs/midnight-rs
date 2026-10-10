# midnight-rs

**The Rust SDK for the [Midnight](https://midnight.network) blockchain.** Deploy Compact smart contracts, call circuits on-chain, manage shielded and unshielded wallets, and query the indexer, all from Rust.

> [!WARNING]
> This project is under active development. APIs may change without notice.

## Features

- **Deploy and call Compact smart contracts**: typed Rust bindings generated from the compiled contract. A circuit call takes typed arguments and returns a typed value.
- **Per-contract private state**: a pluggable `PrivateStateProvider` store with password-encrypted export and import. Witnesses read and update the state during circuit calls (see [`docs/private-state.md`](docs/private-state.md)).
- **Contract maintenance**: deploy with a k-of-n maintenance committee, then rotate verifier keys or replace the authority with signed updates (see [`docs/contract-maintenance-governance.md`](docs/contract-maintenance-governance.md)).
- **Shielded and unshielded wallet**: shielded coins, unshielded UTXOs, and Dust (the fee token).
- **Indexer and node clients**: a typed GraphQL client for the Midnight indexer, and node RPC over subxt.
- **Ledger 8 and ledger 9**: one build runs on chains of both generations and carries a wallet across the hard fork (see [`docs/ledger-generations.md`](docs/ledger-generations.md)).

## Compile a contract

The SDK reads `compiler/analyzed-ir.sexp`. Only a fork of the Compact compiler ([`RomarQ/compact`](https://github.com/RomarQ/compact)) writes this file, when it runs with `--analyzed-ir`. The command below runs the public image of the fork at `COMPACT_REV`, the commit that the [`Makefile`](Makefile) names.

Run these commands from the root of your crate. They compile the counter contract of the [Quick start](#quick-start):

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

The container sees only the working directory, so keep the source and the output directory under it.

## Install

Add the SDK, tokio and anyhow to the `Cargo.toml` of your crate:

```toml
[dependencies]
midnight-core = { git = "https://github.com/Moonsong-Labs/midnight-rs" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
anyhow = "1"
```

## Quick start

The Quick start runs against a local devnet (node and indexer). Run `make dev-up` from the root of a clone of this repository to start it. Run `make dev-down` to stop it.

```rust
use midnight_core::{LocalWallet, MidnightProvider, Network, Seed, Wallet};

mod counter {
    midnight_core::contract!("compiled/counter/compiler/analyzed-ir.sexp");
}

const NODE_URL: &str = "ws://localhost:9944";
const INDEXER_URL: &str = "http://localhost:8088";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // The dev seed, funded at genesis on the local devnet.
    let seed = Seed::from_hex(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )?;
    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?;
    let wallet = Wallet::sync(&provider, seed, Network::Undeployed).await?;
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    // `with_zk_config` resolves a relative path against the working directory.
    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(concat!(env!("CARGO_MANIFEST_DIR"), "/compiled/counter"))
        .await?;

    println!("deployed at {}", contract.address());
    println!("round = {}", contract.ledger().await?.round()?);

    // `.value` is the typed return value of the circuit.
    let returned: u64 = contract.circuits().increment().await?.value;
    println!("increment returned {returned}");
    println!("round = {}", contract.ledger().await?.round()?);

    let returned: u16 = contract.circuits().increment_by(5).await?.value;
    println!("increment_by(5) returned {returned}");
    println!("round = {}", contract.ledger().await?.round()?);

    Ok(())
}
```

The crates in [`examples/`](examples) run against the same devnet.

## Connecting to an existing contract

`Contract::at` connects to a deployed contract by its address. It makes no network call. The handle reads fresh state at each call, the same as the handle that `deploy` returns:

```rust,ignore
let contract = counter::Contract::at(&provider, &address)
    .with_zk_config(concat!(env!("CARGO_MANIFEST_DIR"), "/compiled/counter"))
    .build();

let returned: u64 = contract.circuits().increment().await?.value;
println!("increment returned {returned}");
println!("round = {}", contract.ledger().await?.round()?);
```

## Wallet

After `with_wallet`, the provider gives balances, transfers and Dust registration:

```rust,ignore
let balance = provider.balance().await?;
let pending = provider.transfer_unshielded(midnight_core::wallet::NIGHT, 100, &recipient).await?;
pending.wait_finalized().await?;
```

[`docs/wallet.md`](docs/wallet.md) covers sync, balances, transfers, Dust registration and persistence. [`examples/wallet-sync`](examples/wallet-sync) is a runnable walkthrough.

## Observing inclusion explicitly

The `.await` of a deploy waits for the best block, then for the indexer to show the contract. `with_deploy_timeout` sets one deadline for the two waits (60 s by default). To see each step, call `.send()`:

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

On this path, the deadline covers only `into_contract`. `wait_best` and `wait_finalized` have no deadline.

A wait fails with `ContractError::TransactionFailed` when the chain did not apply the deploy. When the deadline passes, `into_contract` fails with `ContractError::DeployTimeout`, and the error message says what to do. [`docs/wallet.md`](docs/wallet.md#submission-and-waiting) lists the submission errors and what to do after each one.

## Crates

| Crate | Description |
|---|---|
| `midnight-core` | The crate to depend on. It re-exports the SDK as the modules `provider`, `wallet`, `contract`, `indexer` and `crypto` |
| `midnight-provider` | `Provider` trait and `MidnightProvider`: indexer, node RPC and the attached wallet |
| `midnight-contract` | Typed contract interactions: deploy, call, query, prove, submit |
| `midnight-wallet` | `Wallet` state machine: sync, balances, transfers, Dust, address derivation |
| `midnight-private-state` | `PrivateStateProvider` store for per-contract private state and signing keys, with encrypted export and import |
| `compact-bindgen` | `contract!` macro: generates typed bindings from `analyzed-ir.sexp` |
| `midnight-indexer-client` | Typed GraphQL client for the Midnight indexer API |
| `midnight-crypto` | Re-exports `midnight-base-crypto`, `midnight-curves` and `midnight-transient-crypto` as modules |
| `midnight-helpers` | The upstream node helpers, as a `ledger_8` and a `ledger_9` module |

## Development

The `Makefile` wraps the workflow, and [CI](.github/workflows/ci.yml) calls the same targets. Run `make` for the full list.

```bash
make ci              # the local CI gates
make test            # cargo test --workspace
make dev-up          # start the local devnet (node + indexer)
make test-e2e        # devnet integration tests
make examples        # run the examples of the EXAMPLES list against the devnet
make conformance     # check the circuit interpreter against the TypeScript Compact runtime
```
