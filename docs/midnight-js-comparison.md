# How this SDK relates to midnight-js

[`midnight-js`](https://github.com/midnightntwrk/midnight-js) is the TypeScript reference implementation maintained by the Midnight Foundation. This document maps its abstractions onto ours, explains the deliberate differences, and notes the parts of Midnight that midnight-js exposes but this SDK does not (yet).

Read this if you are coming from midnight-js, or if you want to know which Midnight mechanics actually exist on the chain — independent of any particular SDK.

## Two different shapes for the same chain

**midnight-js** is built as a set of small, pluggable provider interfaces. Each concern is a separate package and a separate interface, and a "real" application wires them together:

```ts
ContractProviders = {
  publicDataProvider:   PublicDataProvider     // indexer + node RPC reads
  privateStateProvider: PrivateStateProvider   // local persistent per-contract state + signing keys
  walletProvider:       WalletProvider         // balances txs, holds spending keys
  midnightProvider:     MidnightProvider       // submits txs (NOTHING else)
  proofProvider:        ProofProvider          // proves unproven txs
  zkConfigProvider:     ZkConfigProvider       // fetches prover/verifier/zkir per circuit
  logger?:              LoggerProvider
}
```

**midnight-rs** bundles all of these into one [`MidnightProvider`](../crates/midnight-provider/src/provider.rs). The name is the same as a midnight-js interface but the scope is much wider — our `MidnightProvider` owns the indexer client, the node WebSocket connection, the wallet (with persisted state), an optional proof provider, and the submission path.

| midnight-js abstraction       | midnight-rs equivalent                                                                              |
| ----------------------------- | --------------------------------------------------------------------------------------------------- |
| `MidnightProvider.submitTx`   | `MidnightProvider::submit`                                                                          |
| `PublicDataProvider`          | `MidnightProvider`'s `indexer()` accessor + the `Provider` trait reads                              |
| `WalletProvider`              | `MidnightProvider`'s attached `Wallet` (sync, balances, transfers)                                  |
| `ProofProvider`               | `Prover` enum (`Local` / `Remote`) + the `ProofProvider` trait from `midnight-helpers`              |
| `ZkConfigProvider`            | Implicit — keys are read from a path passed to `.with_zk_keys("compiled")` (no trait abstraction)   |
| `PrivateStateProvider`        | `midnight-private-state` crate (`FsPrivateStateProvider`); threaded through witnesses via `WitnessContext` — see below |
| `LoggerProvider`              | The `tracing` crate facade — implicit, not a provider                                               |

Neither shape is right or wrong. The TS split lets you swap a remote prover, a browser-wallet-based balancer, or an HTTP-fed `ZkConfigProvider` without touching the rest. Our bundled shape is shorter to set up and statically typed end-to-end at the cost of less swappability — only the proof backend is currently abstracted (`with_proof_provider`).

## Transaction lifecycle

The two SDKs agree on what *needs* to happen to land a transaction; they slice the steps differently.

### midnight-js: pipeline as four explicit provider calls

```
proofProvider.proveTx(unproven)        → proven           (ZK proofs)
walletProvider.balanceTx(proven)       → finalized        (Dust fees, binding randomness)
midnightProvider.submitTx(finalized)   → txId
publicDataProvider.watchForTxData(id)  → FinalizedTxData  (waits indefinitely)
```

Each step is a separate provider, so you can plug in a remote prover, a browser wallet, etc.

### midnight-rs: combined `build` then `submit`

```
provider.transfer_unshielded(...).build().await
    │
    └─ internally:
       1. resync wallet against indexer
       2. select inputs from local state
       3. balance Dust fees (speculative_spend loop)
       4. prove (via the configured Prover — Local or Remote)
       5. tagged-serialize → TransferResult { tx_bytes, ... }

provider.submit(&tx_bytes).await       → PendingTx
pending.wait_best().await              → TxInBlock + PendingTx
pending.wait_finalized().await         → TxInBlock
```

The "prove" and "balance" steps live together inside `build` because they're interleaved — `speculative_spend` iterates between balancing and (mock-)proving until the Dust fee is right. Splitting them as two free-standing operations the way midnight-js does requires the `WalletProvider`-shaped abstraction we don't have today.

Both the one-shot path (`provider.transfer_*.await`) and `Contract::deploy(...).await` collapse the four steps into one.

## Guaranteed vs Fallible transaction phases

A Midnight transaction has **two phases** that execute in order. midnight-js's governance code documents the model explicitly; the same rules apply to transactions built by this SDK.

**Guaranteed phase.** Runs first. Covers fee payment, Zswap input/output validity, signature checks. If anything in this phase fails, the transaction is **rejected by the node and not included in a block** — there is no on-chain record at all.

**Fallible phase.** Runs second. Covers contract calls, verifier-key updates, and other state mutations that the chain wants to record even on failure. If the fallible phase fails, the transaction **is included in a block** but its `TransactionResult` is `PartialSuccess` (or `Failure`) rather than `SucceedEntirely`. Fees are still paid; the partial mutation is **not** applied.

Practical consequences for SDK callers:

- `pending.wait_best().await` returning successfully means the *guaranteed* phase passed and the tx is in a best block. It does **not** mean the contract call succeeded.
- A contract call can land on-chain and still have done nothing useful. Read the contract's state after `wait_finalized` to confirm the round counter (or whatever your circuit mutates) actually moved.
- For multi-step intents (e.g. shielded offer + contract call), one segment can succeed while another fails. The chain records this as `PartialSuccess`.

`wait_best` / `wait_finalized` return [`TxInBlock`](../crates/midnight-provider/src/submit.rs) — block hash + extrinsic hash — and nothing about the chain-side outcome. To distinguish "in a block" from "in a block AND succeeded entirely", call [`MidnightProvider::wait_transaction_result`](../crates/midnight-provider/src/provider.rs) after `wait_best`:

```rust,ignore
let pending = provider.transfer_unshielded(NIGHT, 100, &recipient).await?;
let (in_block, _) = pending.wait_best().await?;
let result = provider
    .wait_transaction_result(&in_block.extrinsic_hash, Duration::from_secs(30), Duration::from_secs(1))
    .await?;
match result.as_ref().map(|r| &r.status) {
    Some(TransactionResultStatus::Success)        => { /* applied */ }
    Some(TransactionResultStatus::PartialSuccess) => { /* check result.segments */ }
    Some(TransactionResultStatus::Failure)        => { /* rolled back */ }
    None                                          => { /* indexer didn't catch up in time */ }
}
```

It polls the indexer's `transaction_result` field until either it surfaces or the timeout elapses. There's no equivalent in midnight-js's `FinalizedTxData` to "indexer hasn't caught up yet" — the JS pipeline waits indefinitely.

## ZK artifacts

A Compact circuit compiles to three files per circuit name `<C>`:

| File                 | Purpose                                                       | Consumer                                |
| -------------------- | ------------------------------------------------------------- | --------------------------------------- |
| `keys/<C>.prover`    | Prover key — generates the ZK proof for a transcript          | Local / remote prover                   |
| `keys/<C>.verifier`  | Verifier key — embedded in `ContractState.operations` on deploy. The node uses it to verify proofs. | `ContractState` (on-chain) + node verifier |
| `zkir/<C>.bzkir`     | Zero-knowledge intermediate representation — the circuit's arithmetic layout | Prover                                  |

**midnight-js** abstracts all three behind a `ZkConfigProvider` trait. Implementations include:

- `node-zk-config-provider` — reads from filesystem (server-side Node)
- `fetch-zk-config-provider` — fetches over HTTP (browser, embedded clients)
- `dapp-connector-proof-provider` — proxies to a browser wallet's KeyMaterialProvider

This lets a browser app or a no-filesystem environment ship the same SDK as a server-side app.

**midnight-rs** reads keys directly from disk via `.with_zk_keys(path)`. The path is expected to contain `keys/` and `zkir/` subdirectories. There is no abstraction yet, so embedded / HTTP / browser key sources require dropping down to lower-level APIs.

## Feature gaps

These are real Midnight chain features that midnight-js exposes and this SDK does not (yet). Listed roughly in order of how often they come up.

### Contract maintenance / governance

The chain supports three governance operations on a deployed contract:

- **Insert verifier key.** Add a verifier key for a new circuit ID, or rotate the key for an existing one. Needed when you add circuits after deployment or rotate ZK setup.
- **Remove verifier key.** Mark a circuit as no longer callable.
- **Replace contract maintenance authority.** Change which signing key controls future maintenance updates.

midnight-js: see `packages/contracts/src/governance/` (`submit-insert-vk-tx.ts`, `submit-remove-vk-tx.ts`, `submit-replace-authority-tx.ts`).

midnight-rs: now exposed via `Contract::at(addr).maintenance()`, which returns a `ContractMaintenance` builder. Chain `insert_verifier_key`, `remove_verifier_key`, and `replace_authority` (applied in order, atomically, in one signed update), then `prepare()` for a `PreparedMaintenance`. Signing is caller-controlled: read `data_to_sign()`, attach signatures with `sign(committee_index, key)` / `add_signature(...)`, then await (or `build()`) to submit. The initial authority is set at deploy via `DeployBuilder::with_maintenance_authority`, and `Contract::maintenance_authority()` reads the current committee. The SDK never stores a key. See [`contract-maintenance-governance.md`](./contract-maintenance-governance.md).

### Per-contract private state

Some contracts use witnesses that are themselves stateful (counters, secret balances, etc.). midnight-js gives every contract a `privateStateId` and stores the corresponding private state — plus the contract's signing key — in a local keychain via `PrivateStateProvider`. The `level-private-state-provider` package backs this with LevelDB and supports encrypted export / import.

midnight-rs: now exposed end-to-end via the `midnight-private-state` crate — a `PrivateStateProvider` trait plus a filesystem default (`FsPrivateStateProvider`) that stores opaque per-contract private-state blobs and signing keys, with password-encrypted (Argon2id + AES-256-GCM) export/import. Attach it with `MidnightProvider::with_private_state(...)`. See [`private-state.md`](./private-state.md).

Private state is also **threaded** through witness execution, matching midnight-js. `WitnessProvider::call_witness(ctx, name, args)` takes a `&mut WitnessContext` carrying the mutable private state (the analogue of midnight-js's `(ctx, ...args) => [newPS, result]`). When a `PrivateStateProvider` is attached, a circuit call loads the contract's state before execution, threads it through the witnesses, and persists the updated state after the tx lands — so stateful-witness contracts work across calls without the caller managing storage. Private state is keyed by contract address: a Compact contract has exactly one private-state type shared by all its witnesses, so there is one blob per contract (the caller packs every private variable into it), rather than midnight-js's separate per-app `privateStateId`.

### `ZkConfigProvider` abstraction

As described above: keys are filesystem-only today.

### Wallet observability

midnight-js's `PublicDataProvider` exposes RxJS `Observable` streams (`watchForContractState`, `watchForTxData`, etc.) backed by indexer subscriptions. midnight-rs has the subscription client (`midnight-indexer-client`) but does not surface streams to user code beyond the `SyncProgress` channel during initial sync.

## When to use which SDK

- **midnight-js** — browser dApps, Node.js services, anywhere you want to plug a browser wallet, swap a remote prover, or fetch keys over HTTP. The provider split makes this natural.
- **midnight-rs** — Rust services and CLIs, embedded / signing-server use cases, anywhere a typed `?`-everywhere experience matters more than runtime swappability.

The two SDKs target the same chain and the same `contract-info.json` artifacts (via our [forked Compact compiler](../README.md#prerequisites)), so the same contract can be deployed from one and called from the other.
