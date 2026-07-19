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
| `WalletProvider`              | `MidnightProvider`'s attached `Wallet` (sync, balances, transfers), plus `merge_transactions` / `balance_transaction` (see "Combining and balancing transactions") |
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
       4. prove (via the provider's proof backend — LocalProofServer by default, or a RemoteProofServer set with with_proof_provider)
       5. tagged-serialize → TransferResult { tx_bytes, ... }

provider.submit(&tx_bytes).await       → PendingTx
pending.wait_best().await              → TxInBlock + PendingTx
pending.wait_finalized().await         → TxInBlock
```

The "prove" and "balance" steps live together inside `build` because they're interleaved: `speculative_spend` iterates between balancing and (mock-)proving until the Dust fee is right. For **your own** transaction they stay fused: there is no standalone `proveTx` you call separately.

What you *can* now do is stop before submission and get the proven bytes back: `.build()` on a transfer, or `contract.circuits().<circuit>().build().await` (equivalently `Contract::build_call_with`) for a contract call, returns a proven `Vec<u8>` without submitting. That is the hook the multi-party flows in the next section build on, and it closes most of the "you can't separate building from submitting" gap this section used to describe.

Both the one-shot path (`provider.transfer_*.await`) and `Contract::deploy(...).await` collapse the whole pipeline into one.

## Combining and balancing transactions (multi-party)

Several parties can contribute to one transaction (atomic swaps, or "one wallet pays the fees for another's transaction"). midnight-js expresses this with two wallet-SDK primitives; we now mirror both, with one gap.

### Merging proven transactions

**midnight-js:** `Transaction.merge(other)` combines two already-proven, each-self-funded transactions into one, unioning their Zswap offers and intents and summing the binding randomness. This is the symmetric case: each party already paid its own fees.

**midnight-rs:** [`MidnightProvider::merge_transactions(&[bytes, ...])`](../crates/midnight-provider/src/provider.rs) does the same, deserializing proven `FinalizedTransaction`s and folding them with the ledger's `Transaction::merge`. Feed it the bytes from `.build()`:

```rust,ignore
let mine = contract.circuits().withdraw(coin).build().await?;   // proven, not submitted
let merged = provider.merge_transactions(&[mine, counterparty_bytes])?;
provider.submit(&merged).await?;
```

**Segment constraint.** `Transaction::merge` rejects two inputs that both carry an intent at the same segment. A self-funded build attaches its Dust-fee intent at the fallible segment (1), and a contract call and an unshielded (UTXO) transfer put their action there too, so you cannot merge two self-funded transactions: they collide at segment 1. A Dustless *shielded* transfer carries no intent (pure Zswap) and merges freely. So the practical multi-party shape is "one party pays": the contributors build fee-less (`.without_dust()`), one party sponsors via `balance_transaction` (next section), and its fee intent rides a distinct segment. The `.build()`-then-`merge` example above works only when at most one side carries a segment-1 intent.

One subtlety that shapes both SDKs: a bare proven *offer* cannot be merged, only a whole transaction carries the binding randomness the merge needs, and proving discards the per-input randomness a loose offer would require. So the artifacts exchanged are always full transactions, not offers.

### Balancing someone else's transaction (one party pays the fees)

**midnight-js:** `walletProvider.balanceTransaction(tx, newCoins)` is a single primitive that takes a transaction carrying only its *effects* and makes it submittable: the calling wallet adds inputs to cover any token deficit **and** pays the Dust fees from its own coins. Whoever calls it is the payer. In a two-party swap, Party 1 builds an unbalanced transaction and Party 2 `balanceTransaction`s it.

**midnight-rs:** the same work is split across two calls, matching the two roles:

- [`transfer_shielded(token, amount, recipient).without_dust()`](../crates/midnight-provider/src/transfer.rs), the Party-1 side. Any builder with fees turned off: a proven, token-balanced but **Dustless** transaction (no Dust). Dust is the general fee token, so `.without_dust()` is not shielded-specific: it is one method on the `DustlessBuilder` trait that yields a `DustlessTransaction`. It is implemented on the shielded-transfer builder, the unshielded-transfer builder, and generated contract-call builders (`contract.circuits().foo().without_dust()`). A `DustlessTransaction` has no submit path (it is not valid alone); hand its bytes to the payer.
- [`MidnightProvider::balance_transaction(bytes)`](../crates/midnight-provider/src/provider.rs), the Party-2 (payer) side. Takes the other party's proven, fee-less transaction and pays its Dust fees from *this* wallet, returning a completed transaction to `submit`. It draws dust for the fee estimate, proves a fee-only transaction, merges it in, and iterates until the (growing) fee is covered. It is the same balancing loop the `build` path runs, but against a finished external transaction and leaving its proofs untouched.

```rust,ignore
use midnight_provider::DustlessBuilder; // brings `.without_dust()` into scope

// Party 1 (owns the coin, pays nothing): a Dustless transaction.
let partial = alice.transfer_shielded(token, 5, &alice_addr).without_dust().await?;
// Party 2 (pays all the fees):
let complete = bob.balance_transaction(partial.as_bytes()).await?;
bob.submit(&complete).await?;
```

**The one gap.** midnight-js's `balanceTransaction` also covers a **token deficit** from the balancing wallet: the balancer can supply its *own* tokens (e.g. tokenY in a swap), not just fees. Our `balance_transaction` is **fee-only**: a shielded-token deficit is rejected with a clear error. Covering it (adding the payer's own coins) is the tracked follow-up. So today we fully cover the *fee-sponsorship* shape; a swap where the balancer also provides tokens needs the caller to arrange the token side itself (each party balances its own half, then `merge_transactions`).

### Spending your own coin in a call, and coin discovery

Two smaller pieces round out the parity:

- **Naming a coin.** midnight-js's balancer auto-selects coins by amount. But a circuit like `receiveShielded(coin)` re-commits a *specific* coin (its exact nonce), so amount-based selection can't express it. [`MidnightProvider::spendable_shielded_coins()`](../crates/midnight-provider/src/provider.rs) enumerates the wallet's coins with their nonces and nullifiers, and `contract.circuits().<circuit>().with_shielded_inputs([coin])` attaches an exact, nullifier-pinned coin to the call. This is more explicit than midnight-js's auto-balance, by necessity.
- **Coin discovery.** midnight-js's `balanceTransaction` takes a `newCoins` argument so the wallet tracks coins the transaction creates for it. Our analogue is `with_coin_encryption_keys` on a call: it attaches a discovery ciphertext to circuit-created outputs so the recipient finds them through normal sync (no `watchFor`). Different mechanism, same goal.

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
match provider
    .wait_transaction_result(&in_block.extrinsic_hash, Duration::from_secs(30), Duration::from_secs(1))
    .await?
{
    TxResultWait::Found(r) => match r.status {
        TransactionResultStatus::Success        => { /* applied */ }
        TransactionResultStatus::PartialSuccess => { /* check r.segments */ }
        TransactionResultStatus::Failure        => { /* rolled back */ }
    },
    TxResultWait::TimedOut => { /* indexer didn't catch up in time; the result may still surface */ }
}
```

It polls the indexer's `transaction_result` field until either it surfaces (`TxResultWait::Found`) or the timeout elapses (`TxResultWait::TimedOut`). `TimedOut` is provisional, not a verdict: the indexer cannot positively report "this tx never landed" (absence from its index also covers plain indexer lag), so the result may still appear on a later poll. There's no equivalent in midnight-js's `FinalizedTxData` to "indexer hasn't caught up yet": the JS pipeline waits indefinitely.

When `wait_best` / `wait_finalized` themselves fail, the error is `ProviderError::Submission` carrying a typed [`SubmitError`](../crates/midnight-provider/src/submit.rs) instead of a string to parse. The variants encode the retry semantics: `Invalid` is a definitive node rejection (safe to rebuild and resubmit), `Dropped` and `NodeError` are not (the tx may still be re-included; resubmitting the same inputs risks a double spend), and `WatchStream` means the watch subscription itself broke while the tx's fate stayed unknown.

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
