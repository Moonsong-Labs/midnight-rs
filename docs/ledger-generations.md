# Ledger generations

A Midnight chain runs one generation of the ledger at a time: a transaction format and the rules the chain applies to it. A chain changes generation only at a hard fork. midnight-rs runs on ledger 8 and on ledger 9 from one build, and it reads the generation from the chain.

## Which networks run which

| Network | Generation |
| --- | --- |
| mainnet, preprod, preview | ledger 8, until their hard fork |
| the public devnet | ledger 9, since its hard fork on 2026-09-28 |
| stagenet | ledger 9 |
| a local devnet | ledger 8 from `make dev-up`, ledger 9 from `make dev-up DEVNET_LEDGER=9` |

## How the SDK picks the generation

Every encoded ledger value starts with a tag. A value whose encoding changed at the fork carries a different tag in each generation. The SDK reads the generation from these tags: the ledger parameters of a block, each ledger event, and each transaction. `provider.ledger_version()` reads the parameters of the node's finalized block, and the wallet sync reads those of the indexer's latest block. There is nothing to configure.

```rust
let chain = provider.ledger_version().await?; // LedgerVersion::V8 or LedgerVersion::V9
let state = wallet.ledger_version();          // the generation of the wallet's state
```

The wallet returns `WalletError::UnknownLedger` for data of a generation this build does not link.

## What a DApp writes once

Most of the API is the same on both generations:

- the provider's transfers (`transfer_shielded`, `transfer_unshielded`, `shielded_swap`, `register_dust`), `balance`, `submit`, `merge_transactions`, `balance_transaction` and `balance_transaction_with_tokens`
- the wallet sync (the `WalletBuilder` that `Wallet::from_seed`, `Wallet::from_mnemonic`, `Wallet::from_mnemonic_with_passphrase` and `WalletBuilder::new` start) and `LocalWallet`
- the generated contract bindings (`Contract::deploy`, `circuits()`, `ledger()`), `Contract::call_with`, a deploy's `ShieldedOffer`, and contract maintenance

Its vocabulary lives in `midnight-types`, one type for both generations: `ShieldedTokenType`, `UnshieldedTokenType`, `Nullifier`, `Nonce`, `CoinInfo`, `CoinPublicKey`, `EncryptionPublicKey`, `ShieldedRecipient`, `ContractAddress`, `DustNullifier`, `ChainParameters` and `LedgerVersion`. The provider, wallet and contract crates re-export it. A DApp that uses only this surface runs on either chain with no code for a generation.

The Compact side (the interpreter, the typed state and the generated bindings) reads ledger 9's types on both chains. The two generations share the Impact ops and the state encoding, and `midnight_typed_state::decode_contract_state` converts the state a ledger 8 chain serves.

### A shielded offer on a deploy

A deploy can carry a shielded offer. A `ShieldedOffer` holds coins from `spendable_shielded_coins` and the `ShieldedPayment`s to make from them. The offer has no ledger generation of its own, so the same code deploys on either chain:

```rust
use midnight_core::Contract;
use midnight_core::contract::{ShieldedOffer, ShieldedPayment, parse_shielded_recipient};

let coin = provider
    .spendable_shielded_coins()
    .await?
    .into_iter()
    .find(|coin| coin.token_type == token_type && coin.value >= 10)
    .ok_or_else(|| std::io::Error::other("no coin of the token holds 10"))?;
let offer = ShieldedOffer {
    coins: vec![coin],
    outputs: vec![ShieldedPayment {
        recipient: parse_shielded_recipient(address, provider.network().await?)?,
        token_type,
        value: 10,
    }],
};
let contract = Contract::deploy(&provider)
    .with_initial_state(state)
    .with_zk_config("compiled")
    .with_shielded_offer(offer)
    .await?;
```

`DeployBuilder::with_shielded_offer` says how the deploy spends the coins, returns the change and pays the fee. The deploy builder of a `contract!` binding has the same method.

## What is per generation

A few low-level surfaces hand out or take the upstream types of one generation, so they name it.

### Builds

`MidnightProvider::builds` resyncs the attached wallet, then hands out the builds of the generation its state is in:

```rust
use midnight_core::provider::Builds;

match provider.builds().await? {
    Builds::Ledger8(builds) => {
        let context = builds.build_context().await?; // ledger 8 types
    }
    Builds::Ledger9(builds) => {
        let context = builds.build_context().await?; // ledger 9 types
    }
}
```

Each `Builds` has `build_context`, `execution_context`, `add_funding`, `build_funded`, `prepare_shielded_inputs`, `balance_transaction`, `balance_transaction_with_tokens` and `proof_provider`. Their types come from `midnight_helpers::ledger_8` or `midnight_helpers::ledger_9`. The contract crate builds its calls, deploys and maintenance updates on them.

### A custom wallet

`with_wallet` takes a wallet that implements `WalletFacade` and the `WalletBuilds` trait of each generation: `midnight_core::provider::ledger_8::WalletBuilds` and `midnight_core::provider::ledger_9::WalletBuilds`. The provider uses the `WalletBuilds` of the generation the wallet reports in `WalletFacade::ledger_version`. `LocalWallet` implements all three, and a synced `Wallet` goes in behind a `LocalWallet`. `crates/midnight-provider/tests/substituting_the_wallet.rs` has a stub wallet that implements them.

### Proof backends

A `ProofProvider` proves the transactions of one generation. `with_proof_provider` takes `ProofProviders`, which holds one prover per generation:

```rust
use std::sync::Arc;
use midnight_core::provider::{ProofProviders, RemoteProofServer};

// A backend that implements both generations' ProofProvider serves both.
let provider = provider.with_proof_provider(Arc::new(RemoteProofServer::new(proof_url)));

// Or set one prover per generation.
let provider = provider.with_proof_provider(
    ProofProviders::local().with_ledger_9(Arc::new(RemoteProofServer::new(proof_url))),
);
```

`ProofProviders::local()` is the default, and proves both generations in process. A remote proof server must support the chain's generation.

### Maintenance updates

A prepared maintenance update belongs to one generation, which `PreparedMaintenance::ledger_version` names. Its signatures are valid on that generation only. See [`contract-maintenance-governance.md`](contract-maintenance-governance.md).

## Crossing a hard fork

When the chain moves from ledger 8 to ledger 9, a wallet crosses with it:

- A sync from genesis replays the ledger 8 history, then continues on ledger 9.
- A wallet stored before the fork resumes from its snapshot and crosses on the way.
- An attached wallet crosses at its next resync. Every build resyncs first. A rescan or a coin watch, through the provider or through a handle on the wallet, resyncs first when the wallet's state is in the earlier generation.

What the crossing keeps and what it drops:

- The shielded coins carry across, because the zswap state keeps its encoding.
- The unshielded UTXOs carry across, but their Dust registration flags reset, because the fork wiped every registration.
- The Dust state starts empty, because the fork emptied the chain's Dust state. The replay skips the ledger 8 Dust events.
- The pending reservations of ledger 8 drop, because a transaction built for ledger 8 cannot land on ledger 9.
- A contract keeps its state. A call to a contract deployed before the fork works after it.

### Register Dust again

After the fork, no NIGHT generates Dust, so the wallet cannot pay a fee. Register each NIGHT UTXO again. `DustBalance::unregistered_night_utxos` counts the UTXOs left, and each `register_dust` call registers one. A registration pays its own fee from the NIGHT it spends, so a wallet with no Dust can register.

```rust
use std::time::{Duration, Instant};

// A balance read does not resync, and the resync crosses the fork.
provider.resync_wallet().await?;
let mut left = provider.balance().await?.dust.unregistered_night_utxos;
while left > 0 {
    provider.register_dust(None).await?.wait_finalized().await?;
    // The indexer serves the registration a moment after the node finalizes it.
    let deadline = Instant::now() + Duration::from_secs(60);
    while provider.balance().await?.dust.unregistered_night_utxos == left {
        if Instant::now() >= deadline {
            return Err("the indexer did not serve the Dust registration".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        provider.resync_wallet().await?;
    }
    left = provider.balance().await?.dust.unregistered_night_utxos;
}
```

Dust accrues over time, so wait until the balance covers a fee before the first fee-paying transaction. NIGHT that arrives after a registration generates Dust with no further call. A wallet with one registered UTXO has one Dust UTXO. Its second fee-paying build fails with `WalletError::InsufficientDust` until a resync sees the Dust spend of the first one.

To wait for that resync, call `BackgroundSync::wait_until` on the background sync of the wallet, with `|b| b.dust.spendable_speck == b.dust.balance_speck`. See also [`dust-and-fees.md`](dust-and-fees.md#dust-shortfalls).

### Snapshots

`metadata.json` and `pending.json` name the ledger generation that wrote them (`ledger_version`). A file that names none reads as ledger 8, the only generation an older build wrote.

A snapshot of a later generation than the chain runs belongs to a chain that was replaced. The sync returns `WalletError::LedgerRegression { path, snapshot, chain }` and leaves the snapshot alone. Remove `path` and sync again.

### Errors

- `WalletError::LedgerMismatch { expected, found }`: data of one generation met a wallet, or a value, of another. A prepared maintenance update or a transaction to balance of the other generation returns it. So does a build between a fork and the wallet's next resync.
- `WalletError::LedgerRegression`: the snapshot is of a later generation than the chain. See above.
- `WalletError::UnknownLedger`: data of a generation this build does not link.

## Testing both generations

- `make dev-up` runs a ledger 8 devnet, and `make dev-up DEVNET_LEDGER=9` a ledger 9 one. Both use ports 9944 and 8088, so run one at a time. `make test-e2e` runs the E2E suite against the devnet that runs. CI runs the suite once per generation.
- `make fork-up fork-test fork-down` runs `crates/midnight-contract/tests/fork_crossing.rs` on a chain that starts on ledger 8. The test forks the chain to ledger 9 in the middle, with `make fork-upgrade`, so every run needs a new `fork-up`. It checks the three crossings, the Dust registrations, a call to a contract deployed before the fork, and a transfer of each kind.
