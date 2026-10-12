# midnight-rs

**The Rust SDK for the [Midnight](https://midnight.network) blockchain.** Deploy Compact smart contracts, call circuits on-chain, and manage shielded and unshielded wallets, all from Rust.

> [!WARNING]
> This project is under active development. APIs may change without notice.

## Features

- **Shielded and unshielded wallet**: shielded coins, unshielded UTXOs, and Dust (the fee token).
- **Swaps and sponsored fees**: two-party shielded swaps, and one party paying the Dust fees of another party's transactions.
- **Node and indexer clients**: node RPC over subxt for contract state and submission, and a GraphQL client for the indexer that the wallet sync reads.
- **Ledger 8 and ledger 9**: one build runs on chains of both generations and carries a wallet across the hard fork (see [`docs/ledger-generations.md`](docs/ledger-generations.md)).
- **Deploy and call Compact smart contracts**: typed Rust bindings generated from the compiled contract. A circuit call takes typed arguments and returns a typed value.
- **Per-contract private state**: a pluggable `PrivateStateProvider` store with password-encrypted export and import. Witnesses read and update the state during circuit calls (see [`docs/private-state.md`](docs/private-state.md)).
- **Contract maintenance**: deploy with a k-of-n maintenance committee, then rotate verifier keys or replace the authority with signed updates (see [`docs/contract-maintenance-governance.md`](docs/contract-maintenance-governance.md)).

## Install

Add the SDK, tokio and anyhow to the `Cargo.toml` of your crate:

```toml
[dependencies]
midnight-core = { git = "https://github.com/Moonsong-Labs/midnight-rs" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
anyhow = "1"
```

## Quick start

The Quick start runs against a local devnet (node and indexer) in Docker. Run `make dev-up` from the root of a clone of this repository to start it. Run `make dev-down` to stop it.

```rust
use midnight_core::provider::{NIGHT, STARS_PER_NIGHT};
use midnight_core::{LocalWallet, MidnightProvider, Network, Seed, Wallet};

const NODE_URL: &str = "ws://localhost:9944";
const INDEXER_URL: &str = "http://localhost:8088";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // The dev seed, funded at genesis on the local devnet.
    let seed = Seed::from_hex("0000000000000000000000000000000000000000000000000000000000000001")?;
    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL);
    let wallet = Wallet::sync(&provider, seed, Network::Undeployed).await?;
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let balance = provider.balance().await?;
    for utxo in &balance.unshielded {
        println!("unshielded {utxo}");
    }
    for coin in &balance.shielded.coins {
        println!("shielded {coin}");
    }

    let recipient = Seed::generate();
    let unshielded = recipient.unshielded_address(Network::Undeployed);
    let shielded = recipient.shielded_address(Network::Undeployed);

    // NIGHT is the native token. Amounts are in STAR, its smallest unit.
    let pending = provider
        .transfer_unshielded(NIGHT, STARS_PER_NIGHT, &unshielded)
        .await?;
    println!("unshielded transfer {}", pending.extrinsic_hash_hex());
    pending.wait_finalized().await?;

    let token = balance.shielded.coins[0].token_type;
    let pending = provider.transfer_shielded(token, 1, &shielded).await?;
    println!("shielded transfer {}", pending.extrinsic_hash_hex());
    pending.wait_finalized().await?;

    Ok(())
}
```

[`docs/wallet.md`](docs/wallet.md) covers sync, balances, transfers, Dust registration and persistence.

The crates in [`examples/`](examples) run against the same devnet. Run one from the root of the clone with `make run-<name>`, for example `make run-shielded-transfer`.

Preprod, the public test network, needs no Docker. [`examples/wallet-sync`](examples/wallet-sync) gives its URLs and faucet, and registers Dust for the fees. The faucet funds only unshielded addresses.

## Swaps and sponsored fees

A shielded swap is one transaction that two parties build in halves, each with its own provider. Each half gives one token and receives the other, so the two halves must mirror each other. A sponsor, either party or a third party, merges the halves, pays the Dust fee and submits:

```rust,ignore
let a_half = alice.shielded_swap(token_x, dx, token_y, dy).await?;
let b_half = bob.shielded_swap(token_y, dy, token_x, dx).await?;
let merged = sponsor.merge_transactions(&[a_half.into_bytes(), b_half.into_bytes()])?;
let funded = sponsor.balance_transaction(&merged).await?;
sponsor.submit(&funded).await?;
```

A party with no Dust builds a transfer or a contract call with `.without_dust()`, from `use midnight_core::provider::DustlessBuilder;`. A sponsor pays its Dust fee with `balance_transaction` and submits it, after `merge_transactions` when there are several. [`examples/shielded-swap`](examples/shielded-swap) and [`examples/combine-and-sponsor`](examples/combine-and-sponsor) run both flows on the devnet.

## Compile a contract

The SDK reads `compiler/analyzed-ir.sexp`. Only a fork of the Compact compiler ([`RomarQ/compact`](https://github.com/RomarQ/compact)) writes this file, when it runs with `--analyzed-ir`. The command below runs the public image of the fork at `COMPACT_REV`, the commit that the [`Makefile`](Makefile) names.

Run these commands from the root of your crate. They compile the counter contract that [Deploy and call a contract](#deploy-and-call-a-contract) uses:

```bash
curl -O https://raw.githubusercontent.com/Moonsong-Labs/midnight-rs/main/devnet/contracts/counter/counter.compact
docker run --rm -v "$PWD:/src" -w /src ghcr.io/romarq/compactc:fa2181fbc6dac2135defdb4f55ce10d8332185d5 \
  "compactc --analyzed-ir counter.compact compiled/counter"
```

The container sees only the working directory, at `/src`. Give the source and the output directory as relative paths under it.

## Deploy and call a contract

The `contract!` macro generates typed bindings from the compiled contract. The code after the module runs in `main`, with the `provider` of the [Quick start](#quick-start):

```rust,ignore
mod counter {
    midnight_core::contract!("compiled/counter/compiler/analyzed-ir.sexp");
}

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

let returned: u16 = contract.circuits().increment_by(5).await?.value;
println!("increment_by(5) returned {returned}");
```

## Connecting to an existing contract

`Contract::at` connects to a deployed contract by the address that `contract.address()` returns. It makes no network call. The handle reads fresh state at each call, the same as the handle that `deploy` returns:

```rust,ignore
let contract = counter::Contract::at(&provider, &address)
    .with_zk_config(concat!(env!("CARGO_MANIFEST_DIR"), "/compiled/counter"))
    .build();

let returned: u64 = contract.circuits().increment().await?.value;
println!("increment returned {returned}");
println!("round = {}", contract.ledger().await?.round()?);
```

## Observing inclusion explicitly

The `.await` of a deploy waits for the best block, then reads the contract from the node at that block. `with_deploy_timeout` sets one deadline for the wait and the read (60 s by default). To see each step, call `.send()`:

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
| `midnight-core` | The crate to depend on. It re-exports the SDK as the modules `provider`, `wallet`, `contract` and `crypto` |
| `midnight-provider` | `Provider` trait and `MidnightProvider`: node RPC and the attached wallet |
| `midnight-wallet` | `Wallet` state machine: sync, balances, transfers, Dust, address derivation |
| `midnight-indexer-client` | GraphQL client for the Midnight indexer API, which the wallet sync uses |
| `midnight-crypto` | Re-exports `midnight-base-crypto`, `midnight-curves` and `midnight-transient-crypto` as modules |
| `midnight-contract` | Typed contract interactions: deploy, call, query, prove, submit |
| `compact-bindgen` | `contract!` macro: generates typed bindings from `analyzed-ir.sexp` |
| `midnight-private-state` | `PrivateStateProvider` store for per-contract private state and signing keys, with encrypted export and import |

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
