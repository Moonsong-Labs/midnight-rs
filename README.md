# midnight-rs

**The Rust SDK for the [Midnight](https://midnight.network) blockchain.** Deploy Compact smart contracts, call circuits on-chain, and manage shielded and unshielded wallets, all from Rust.

> [!WARNING]
> This project is under active development. APIs may change without notice.

## Features

- **Shielded and unshielded wallet**: shielded coins, unshielded UTXOs, and Dust (the fee token). A wallet can register its NIGHT to generate Dust for another wallet's Dust address.
- **Swaps and sponsored fees**: two-party shielded swaps, and one party paying the Dust fees of another party's transactions.
- **Node and indexer clients**: node RPC over subxt for contract state and submission, and a GraphQL client for the indexer that the wallet sync reads.
- **Ledger 9**: the SDK runs on chains that run ledger 9. On a chain that forked from ledger 8 to ledger 9, a sync from genesis also finds the shielded coins that the wallet received before the fork.
- **Deploy and call Compact smart contracts**: typed Rust bindings generated from the compiled contract. A circuit call takes typed arguments, and returns a typed value and the events that the circuit emits.
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

The Quick start runs against a local devnet (node, indexer and proof server) in Docker. Run `make dev-up` from the root of a clone of this repository to start it. Run `make dev-down` to stop it.

```rust
use midnight_core::provider::TransactionRequest;
use midnight_core::{MidnightProvider, NIGHT, STARS_PER_NIGHT, Seed, Wallet};

const NODE_URL: &str = "ws://localhost:9944";
const INDEXER_URL: &str = "http://localhost:8088";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = MidnightProvider::new(NODE_URL);
    // The dev seed, funded at genesis on the local devnet.
    let seed = Seed::from_hex("0000000000000000000000000000000000000000000000000000000000000001")?;
    let wallet = Wallet::from_seed(seed).sync(&provider, INDEXER_URL).await?;
    let provider = provider.with_wallet(&wallet);

    let balance = wallet.balance().await;
    for utxo in &balance.unshielded {
        println!("unshielded {utxo}");
    }
    for coin in &balance.shielded.coins {
        println!("shielded {coin}");
    }

    let recipient = Seed::generate();

    // NIGHT is the native token. Amounts are in STAR, its smallest unit.
    let request =
        TransactionRequest::unshielded(NIGHT, STARS_PER_NIGHT, recipient.unshielded_recipient());
    let pending = provider.send_transaction(request).await?;
    println!("unshielded transfer {}", pending.transaction_hash());
    pending.get_receipt().await?;

    let token = balance.shielded.coins[0].token_type;
    let request = TransactionRequest::shielded(token, 1, recipient.shielded_recipient());
    let pending = provider.send_transaction(request).await?;
    println!("shielded transfer {}", pending.transaction_hash());
    pending.get_receipt().await?;

    Ok(())
}
```

[`docs/wallet.md`](docs/wallet.md) covers sync, balances, transfers, Dust registration and persistence.

The crates in [`examples/`](examples) run against the same devnet. Run one from the root of the clone with `make run-<name>`, for example `make run-shielded-transfer`.

To sync a wallet against another network that runs ledger 9, give [`examples/wallet-sync`](examples/wallet-sync) the URLs of its node and indexer. Its README lists the environment variables.

## Proof backends

A `Prover` proves the transactions that the provider builds. By default, the provider proves in its own process (`Prover::local`). To prove on a proof server, give `Prover::remote` to `with_prover`:

```rust,ignore
use midnight_core::provider::Prover;

let provider = provider.with_prover(Prover::remote(&proof_url));
```

The local prover downloads the public parameters on its first proofs (see `Prover::local`).

## Swaps and sponsored fees

A shielded swap is one transaction that two parties fill in halves, each with its own provider. Each half gives one token and receives the other, so the two halves must mirror each other. A sponsor, either party or a third party, merges the halves, pays the Dust fee and sends the result:

```rust,ignore
use midnight_core::provider::{TransactionRequest, merge_transactions};

let a_half = alice.fill(TransactionRequest::shielded_swap(token_x, dx, token_y, dy)).await?;
let b_half = bob.fill(TransactionRequest::shielded_swap(token_y, dy, token_x, dx)).await?;
let swap = merge_transactions(&[a_half.into_bytes(), b_half.into_bytes()])?;
sponsor.send_transaction(TransactionRequest::sponsor(swap)).await?.get_receipt().await?;
```

A party with no Dust fills a transfer with `TransactionRequest::without_fee`, or a contract call with the call builder's `.without_fee()`, and hands the bytes of the result to the sponsor. [`examples/shielded-swap`](examples/shielded-swap) and [`examples/combine-and-sponsor`](examples/combine-and-sponsor) run both flows on the devnet.

## Compile a contract

The SDK reads `compiler/analyzed-ir.sexp`. Only a fork of the Compact compiler ([`RomarQ/compact`](https://github.com/RomarQ/compact)) writes this file, when it runs with `--analyzed-ir`.

Run these commands from the root of your crate. They compile the counter contract that [Deploy and call a contract](#deploy-and-call-a-contract) uses:

```bash
curl -O https://raw.githubusercontent.com/Moonsong-Labs/midnight-rs/main/devnet/contracts/counter/counter.compact
docker run --rm -v "$PWD:/src" -w /src ghcr.io/romarq/compactc:536ef6dd63f15f9103ea70aa4c5ed19432c80c0c \
  "compactc --analyzed-ir counter.compact compiled/counter"
```

## Deploy and call a contract

The `contract!` macro generates typed bindings from the compiled contract. It also emits `counter::ZK_CONFIG_DIR`, the directory that `with_zk_config` reads. The code after the module runs in `main`, with the `provider` of the [Quick start](#quick-start):

```rust,ignore
mod counter {
    midnight_core::contract!("compiled/counter/compiler/analyzed-ir.sexp");
}

let contract = counter::Contract::deploy(&provider)
    .with_zk_config(counter::ZK_CONFIG_DIR)
    .send()
    .await?
    .into_contract()
    .await?;

println!("deployed at {}", contract.address());
println!("round = {}", contract.ledger().await?.round()?);

// `send` submits the call. `get_receipt` waits for finality and returns
// the outcome, whose `.value` is the typed return value of the circuit.
let returned: u64 = contract.circuits().increment().send().await?.get_receipt().await?.value;
println!("increment returned {returned}");

let returned: u16 = contract.circuits().increment_by(5).send().await?.get_receipt().await?.value;
println!("increment_by(5) returned {returned}");
```

## Deploy a contract with a constructor

The deploy runs the contract's Compact constructor and deploys the state that it leaves. The generated builder takes what the constructor needs:

- `with_constructor_args(..)`, only for a constructor with parameters. It takes the same Rust types as the circuit methods.
- `with_witnesses(&w)` and `with_initial_private_state(state)`, only for a contract with witnesses. The witnesses answer the constructor's witness calls and start from the initial private state.
- `with_initial_state(state)` deploys `state` as it is and skips the constructor.

The [`owned-counter`](devnet/contracts/owned-counter) contract keeps the owner's secret in its private state, and its constructor takes the first round. Bind it with `contract!` as above, then deploy it:

```rust,ignore
use midnight_core::provider::FsPrivateStateProvider;

struct OwnerKey;

impl owned_counter::Witnesses for OwnerKey {
    type PrivateState = [u8; 32];

    fn owner_secret(&self, secret: &mut [u8; 32]) -> Result<Bytes<32>, owned_counter::WitnessError> {
        Ok(Bytes(*secret))
    }
}

// The deploy records the private state, so the provider needs a store. Take
// `address` from `wallet.unshielded_address()`.
let store = FsPrivateStateProvider::with_default_dir(&address).expect("a home directory");
let provider = provider.with_private_state(Arc::new(store));

let contract = owned_counter::Contract::deploy(&provider)
    .with_witnesses(&OwnerKey)
    .with_initial_private_state(secret)
    .with_constructor_args(5)
    .with_zk_config(owned_counter::ZK_CONFIG_DIR)
    .send()
    .await?
    .into_contract()
    .await?;
```

[`docs/private-state.md`](docs/private-state.md#threading-through-a-deploy) covers the private state of a deploy.

## Deploy with a shielded offer

A deploy can carry a shielded offer. A `ShieldedOffer` holds coins from `Wallet::spendable_shielded_coins` and the `ShieldedPayment`s to make from them:

```rust,ignore
use midnight_core::Contract;
use midnight_core::contract::{ShieldedOffer, ShieldedPayment, parse_shielded_recipient};

let coin = wallet
    .spendable_shielded_coins()
    .await
    .into_iter()
    .find(|coin| coin.token_type == token_type && coin.value >= 10)
    .ok_or_else(|| std::io::Error::other("no coin of the token holds 10"))?;
let offer = ShieldedOffer {
    coins: vec![coin],
    outputs: vec![ShieldedPayment {
        recipient: parse_shielded_recipient(address, wallet.network())?,
        token_type,
        value: 10,
    }],
};
let contract = Contract::deploy(&provider)
    .with_initial_state(state)
    .with_zk_config("compiled")
    .with_shielded_offer(offer)
    .send()
    .await?
    .into_contract()
    .await?;
```

`DeployBuilder::with_shielded_offer` says how the deploy spends the coins, returns the change and pays the fee. The deploy builder of a `contract!` binding has the same method.

## Connecting to an existing contract

`Contract::at` connects to a deployed contract by the address that `contract.address()` returns:

```rust,ignore
let contract = counter::Contract::at(&provider, address)
    .with_zk_config(counter::ZK_CONFIG_DIR)
    .build();

let returned: u64 = contract.circuits().increment().send().await?.get_receipt().await?.value;
println!("increment returned {returned}");
println!("round = {}", contract.ledger().await?.round()?);
```

## Observing inclusion explicitly

`send` submits a deploy and returns a `PendingDeploy`. By itself, `into_contract` waits only for the best block. To wait for finality, call `get_receipt` on the `PendingDeploy` before `into_contract`:

```rust,ignore
let pending = counter::Contract::deploy(&provider)
    .with_zk_config(counter::ZK_CONFIG_DIR)
    .send().await?;
println!("ext: {:x}", pending.extrinsic_hash());
let (receipt, pending) = pending.get_receipt().await?;
let contract           = pending.into_contract().await?;
```

[`docs/wallet.md`](docs/wallet.md#submission-and-waiting) lists the submission errors and what to do after each one.

## Crates

| Crate | Description |
|---|---|
| `midnight-core` | The crate to depend on. It re-exports the SDK as the modules `provider`, `wallet`, `contract` and `crypto` |
| `midnight-provider` | `MidnightProvider`: node RPC, and the transactions that the attached `MidnightWallet` funds and signs |
| `midnight-wallet` | `Wallet` state machine: sync, balances, funding and signing as a `MidnightWallet`, Dust, address derivation |
| `midnight-indexer-client` | GraphQL client for the Midnight indexer API |
| `midnight-crypto` | Re-exports `midnight-base-crypto`, `midnight-curves` and `midnight-transient-crypto` as modules |
| `midnight-contract` | Typed contract interactions: deploy, call, query, prove, submit |
| `compact-bindgen` | `contract!` macro: generates typed bindings from `analyzed-ir.sexp` |
| `midnight-private-state` | `PrivateStateProvider` store for per-contract private state and signing keys, with encrypted export and import |

## Development

The `Makefile` wraps the workflow. Run `make` for the full list.

```bash
make ci              # the local CI gates
make test            # cargo test --workspace
make dev-up          # start the local devnet (node, indexer and proof server)
make test-e2e        # devnet integration tests
make examples        # run the examples of the EXAMPLES list against the devnet
make conformance     # check the circuit interpreter against the TypeScript Compact runtime
```
