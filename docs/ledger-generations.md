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

Every encoded ledger value starts with a tag. A value whose encoding changed at the fork carries a different tag in each generation. The SDK reads the generation from these tags: the ledger parameters the indexer serves on the tip block, each ledger event, and each transaction. There is nothing to configure.

```rust
let chain = provider.ledger_version().await?; // LedgerVersion::V8 or LedgerVersion::V9
let state = wallet.ledger_version();          // the generation of the wallet's state
```

The wallet returns `WalletError::UnknownLedger` for data of a generation this build does not link.

## What a DApp writes once

Most of the API is the same on both generations:

- the provider's transfers (`transfer_shielded`, `transfer_unshielded`, `shielded_swap`, `register_dust`), `balance`, `submit`, `merge_transactions` and `balance_transaction`
- `Wallet::sync` and `LocalWallet`
- the generated contract bindings (`Contract::deploy`, `circuits()`, `ledger()`), `Contract::call_with`, and contract maintenance

Its vocabulary lives in `midnight-types`, one type for both generations: `ShieldedTokenType`, `UnshieldedTokenType`, `Nullifier`, `Nonce`, `CoinInfo`, `CoinPublicKey`, `EncryptionPublicKey`, `ShieldedRecipient`, `ContractAddress`, `DustNullifier`, `ChainParameters` and `LedgerVersion`. The provider, wallet and contract crates re-export it. A DApp that uses only this surface runs on either chain with no code for a generation.

The Compact side (the interpreter, the typed state and the generated bindings) reads ledger 9's types on both chains. The two generations share the Impact ops and the state encoding, and `midnight_typed_state::decode_contract_state` converts the state a ledger 8 chain serves.

## What is per generation

A few low-level surfaces hand out or take the upstream types of one generation, so they name it.

### Builds

`MidnightProvider::builds` resyncs the attached wallet, then hands out the builds of the generation its state is in:

```rust
use midnight_provider::Builds;

match provider.builds().await? {
    Builds::Ledger8(builds) => {
        let context = builds.build_context().await?; // ledger 8 types
    }
    Builds::Ledger9(builds) => {
        let context = builds.build_context().await?; // ledger 9 types
    }
}
```

Each `Builds` has `build_context`, `execution_context`, `add_funding`, `build_funded`, `prepare_shielded_inputs`, `balance_transaction` and `proof_provider`. Their types come from `midnight_helpers::ledger_8` or `midnight_helpers::ledger_9`. The contract crate builds its calls, deploys and maintenance updates on them.

### A hand-built shielded offer

A deploy can carry a shielded offer that the caller builds. `ShieldedOffer::Ledger8` and `ShieldedOffer::Ledger9` take the types in `midnight_contract::ledger_8` and `midnight_contract::ledger_9`, and a match on `builds()` picks the generation:

```rust
use midnight_contract::{Contract, ShieldedOffer};
use midnight_provider::{Builds, Network};

macro_rules! offer_in {
    ($ledger:ident) => {{
        use midnight_contract::$ledger as l;
        use l::IntoLedger;
        let token_type = token_type.into_ledger();
        let input = l::InputInfo {
            origin: (&seed).into_ledger(),
            token_type,
            value: 1,
            nullifier: None,
        };
        let output: l::OutputInfo<l::ShieldedWallet<l::DefaultDB>> = l::OutputInfo {
            destination: l::shielded_destination(&recipient, Network::Undeployed)?,
            token_type,
            value: 1,
        };
        l::OfferInfo {
            inputs: vec![Box::new(input)],
            outputs: vec![Box::new(output)],
            transients: vec![],
        }
    }};
}

let offer = match provider.builds().await? {
    Builds::Ledger8(_) => ShieldedOffer::Ledger8(offer_in!(ledger_8)),
    Builds::Ledger9(_) => ShieldedOffer::Ledger9(offer_in!(ledger_9)),
};
let contract = Contract::deploy(&provider)
    .with_initial_state(state)
    .with_zk_config("compiled")
    .with_shielded_offer(offer)
    .await?;
```

The deploy fails with `WalletError::LedgerMismatch` when the offer's generation is not the wallet's.

### A custom wallet

`with_wallet` takes a wallet that implements `WalletFacade` and the `WalletBuilds` trait of each generation: `midnight_provider::ledger_8::WalletBuilds` and `midnight_provider::ledger_9::WalletBuilds`. The provider uses the `WalletBuilds` of the generation the wallet reports in `WalletFacade::ledger_version`. `LocalWallet` implements all three. `crates/midnight-provider/tests/substituting_the_wallet.rs` has a stub wallet that implements them.

### Proof backends

A `ProofProvider` proves the transactions of one generation. `with_proof_provider` takes `ProofProviders`, which holds one prover per generation:

```rust
use std::sync::Arc;
use midnight_provider::{ProofProviders, RemoteProofServer};

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
- An attached wallet crosses at its next resync. Every build resyncs first, and the provider's `rescan_shielded` and `watch_for_coin` resync first when they must.

What the crossing keeps and what it drops:

- The shielded coins carry across, because the zswap state keeps its encoding.
- The unshielded UTXOs carry across, but their Dust registration flags reset, because the fork wiped every registration.
- The Dust state starts empty, because the fork emptied the chain's Dust state. The replay skips the ledger 8 Dust events.
- The pending reservations of ledger 8 drop, because a transaction built for ledger 8 cannot land on ledger 9.
- A contract keeps its state. A call to a contract deployed before the fork works after it.

### Register Dust again

After the fork, no NIGHT generates Dust, so the wallet cannot pay a fee. Register each NIGHT UTXO again. `DustBalance::unregistered_night_utxos` counts the UTXOs left, and each `register_dust` call registers one. A registration pays its own fee from the NIGHT it spends, so a wallet with no Dust can register.

```rust
use std::time::Duration;

let mut left = provider.balance().await?.dust.unregistered_night_utxos;
while left > 0 {
    provider.register_dust(None).await?.wait_finalized().await?;
    // The indexer serves the registration a moment after the node finalizes it.
    while provider.balance().await?.dust.unregistered_night_utxos == left {
        tokio::time::sleep(Duration::from_secs(1)).await;
        provider.resync_wallet().await?;
    }
    left = provider.balance().await?.dust.unregistered_night_utxos;
}
```

Dust accrues over time, so wait until the balance covers a fee before the first fee-paying transaction. NIGHT that arrives after a registration generates Dust with no further call. A wallet with one registered UTXO holds one Dust UTXO, so its second fee-paying build fails with "insufficient DUST" until the indexer serves the first one's Dust change. See [`dust-and-fees.md`](dust-and-fees.md).

### Snapshots

`metadata.json` and `pending.json` name the ledger generation that wrote them (`ledger_version`). A file that names none reads as ledger 8, the only generation an older build wrote.

A snapshot of a later generation than the chain runs belongs to a chain that was replaced. The sync returns `WalletError::LedgerRegression { path, snapshot, chain }` and leaves the snapshot alone. Remove `path` and sync again.

### Errors

- `WalletError::LedgerMismatch { expected, found }`: data of one generation met a wallet, or a value, of another. A shielded offer, a prepared maintenance update or a transaction to balance of the other generation returns it. So does a build between a fork and the wallet's next resync.
- `WalletError::LedgerRegression`: the snapshot is of a later generation than the chain. See above.
- `WalletError::UnknownLedger`: data of a generation this build does not link.

## Testing both generations

- `make dev-up` runs a ledger 8 devnet, and `make dev-up DEVNET_LEDGER=9` a ledger 9 one. Both use ports 9944 and 8088, so run one at a time. `make test-e2e` runs the E2E suite against the devnet that runs. CI runs the suite once per generation.
- `make fork-up fork-test fork-down` runs `crates/midnight-contract/tests/fork_crossing.rs` on a chain that starts on ledger 8. The test forks the chain to ledger 9 in the middle, with `make fork-upgrade`, so every run needs a new `fork-up`. It checks the three crossings, the Dust registrations, a call to a contract deployed before the fork, and a transfer of each kind.
