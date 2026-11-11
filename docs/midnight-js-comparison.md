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

**midnight-rs** bundles most of these into one [`MidnightProvider`](../crates/midnight-provider/src/provider.rs). The name is the same as a midnight-js interface but the scope is much wider: our `MidnightProvider<W>` owns the node WebSocket connection, a `Prover`, an optional private-state store, and the submission path, and it holds the wallet `W` that it gets. It calls the wallet only through the `MidnightWallet` trait, which funds and signs. The wallet keeps the indexer URL that it synced from.

| midnight-js abstraction       | midnight-rs equivalent                                                                              |
| ----------------------------- | --------------------------------------------------------------------------------------------------- |
| `MidnightProvider.submitTx`   | `MidnightProvider::send_tx_envelope`, or `send_transaction` for a `TransactionRequest`              |
| `PublicDataProvider`          | The `Provider` trait (`get_contract_state`) and `MidnightProvider`'s `get_*` reads |
| `WalletProvider`              | The `MidnightWallet` trait, which `Wallet` implements (`fund_transaction` funds, `sign_transaction` signs), plus `merge_transactions` / `TransactionRequest::sponsor` (see "Combining and balancing transactions") |
| `ProofProvider`               | `Prover`, set with `with_prover`: `Prover::local()` by default, or `Prover::remote(url)` |
| `ZkConfigProvider`            | The `ZkConfigProvider` trait, set with `with_zk_config`, where a path gives the default `FsZkConfigProvider` |
| `PrivateStateProvider`        | `midnight-private-state` crate (`FsPrivateStateProvider`); threaded through witnesses via `WitnessContext` — see below |
| `LoggerProvider`              | The `tracing` crate facade — implicit, not a provider                                               |

Neither shape is right or wrong. The TS split lets you swap a remote prover, a browser-wallet-based balancer, or an HTTP-fed `ZkConfigProvider` without touching the rest. Our bundled shape is shorter to set up and statically typed end-to-end at the cost of less swappability. The proof backend (`with_prover`) and the zk artifact source (`with_zk_config`) are pluggable.

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

### midnight-rs: combined `fill` then `send`

```
provider.fill(TransactionRequest::unshielded(...)).await
    │
    └─ internally:
       1. fill: the network, the ledger parameters and the TTL from node queries
       2. MidnightWallet::fund_transaction: resync the wallet against the
          indexer, select inputs from local state, balance Dust fees
          (speculative_spend loop), and reserve them
       3. prove (with the provider's prover: in process by default, or
          Prover::remote set with with_prover)
       4. MidnightWallet::sign_transaction → TxEnvelope

provider.send_tx_envelope(envelope).await → PendingTx
pending.get_receipt().await             → TransactionReceipt
```

The "balance" step runs inside the wallet's funding because it iterates between balancing and mock-proving until the Dust fee is right. The real proof follows in the provider, with the wallet free. For **your own** transaction there is no standalone `proveTx` you call separately.

What you *can* do is stop before submission and get the signed transaction back: `fill` returns the `TxEnvelope` without a send, and `TxEnvelope::into_bytes` gives its proven bytes. A contract call's `contract.circuits().<circuit>().without_fee().await` (equivalently `Contract::without_fee_call_with`) returns the call with no fee, for a sponsor. That is the hook the multi-party flows in the next section build on.

The one-shot paths (`provider.send_transaction(request).await` and `Contract::deploy(...).send().await`) collapse the fill, the proof and the submit into one call. The wait is a call on the pending handle that they return.

## Combining and balancing transactions (multi-party)

Several parties can contribute to one transaction (atomic swaps, or "one wallet pays the fees for another's transaction"). midnight-js expresses this with two wallet-SDK primitives; we now mirror both, with one gap.

### Merging proven transactions

**midnight-js:** `Transaction.merge(other)` combines two already-proven, each-self-funded transactions into one, unioning their Zswap offers and intents and summing the binding randomness. This is the symmetric case: each party already paid its own fees.

**midnight-rs:** the free function [`merge_transactions(&[bytes, ...])`](../crates/midnight-provider/src/pipeline.rs) does the same, deserializing proven `FinalizedTransaction`s and folding them with the ledger's `Transaction::merge`. Feed it the bytes of `TxEnvelope::into_bytes`:

```rust,ignore
let mine = contract.circuits().withdraw(coin).without_fee().await?.into_bytes();   // proven, not sent
let merged = merge_transactions(&[mine, counterparty_bytes])?;
provider.send_transaction(TransactionRequest::sponsor(merged)).await?;
```

**Segment constraint.** `Transaction::merge` rejects two inputs that both carry an intent at the same segment. A contract call and an unshielded (UTXO) transfer put their action at the fallible segment (1), so at most one input may carry a segment-1 intent. A fee-less *shielded* transfer carries no intent (pure Zswap) and merges freely. So the practical multi-party shape is "one party pays": the contributors fill fee-less (`without_fee`), one party sponsors with `TransactionRequest::sponsor` (next section), and its fee intent rides a segment that the merge leaves free.

One subtlety that shapes both SDKs: a bare proven *offer* cannot be merged, only a whole transaction carries the binding randomness the merge needs, and proving discards the per-input randomness a loose offer would require. So the artifacts exchanged are always full transactions, not offers.

### Balancing someone else's transaction (one party pays the fees)

**midnight-js:** `walletProvider.balanceTx(tx, ttl?)` is a single primitive that takes a transaction carrying only its *effects* and makes it submittable: the calling wallet adds inputs to cover any token deficit **and** pays the Dust fees from its own coins. Whoever calls it is the payer. In a two-party swap, Party 1 builds an unbalanced transaction and Party 2 balances it with `balanceTx`.

**midnight-rs:** the same work is split across two calls, matching the two roles:

- [`TransactionRequest::without_fee`](../crates/midnight-types/src/request.rs), the Party-1 side. A request with the fee turned off fills a proven, token-balanced transaction that pays no Dust. Dust is the general fee token, so `without_fee` is not shielded-specific: it applies to a shielded or an unshielded transfer, and a generated contract-call builder has its own `.without_fee()`. The result is a `TxEnvelope` that a send refuses (it is not valid alone); hand its bytes (`into_bytes`) to the payer. A contract call's `.without_fee()` refuses a call whose witnesses change the private state, with `ContractError::UnrecordedPrivateState`, because it records no snapshot. midnight-js returns the next private state from a build-only call (`createUnprovenCallTx` gives `private.nextPrivateState`), and midnight-rs refuses for now.
- [`TransactionRequest::sponsor(bytes)`](../crates/midnight-types/src/request.rs), the Party-2 (payer) side. Takes the other party's proven, fee-less transaction and pays its Dust fees from *this* wallet. `send_transaction` sends the completed transaction, and hands the Dust back when the node never gets it or rejects it. The fill draws Dust for the fee estimate and prices each round with a mock proof, until the Dust covers the (growing) fee. It then proves the fee intent once and merges it in. It is the same balancing loop a transfer runs, but against a finished external transaction and leaving its proofs untouched.

```rust,ignore
// Party 1 (owns the coin, pays nothing): a fee-less transaction.
let partial = alice
    .fill(TransactionRequest::shielded(token, 5, alice_recipient).without_fee())
    .await?
    .into_bytes();
// Party 2 (pays all the fees):
bob.send_transaction(TransactionRequest::sponsor(partial)).await?;
```

**The remaining gap.** midnight-js's `balanceTx` also covers a **token deficit** from the balancing wallet: the balancer can supply its *own* tokens, not just fees, in a *single* unbalanced transaction. Our `TransactionRequest::sponsor` is **fee-only**: a leftover shielded-token deficit is rejected with a clear error. This matters only for the asymmetric shape where one wallet both provides the missing tokens and pays the fees for another's transaction; the native two-party swap (next section) no longer needs it, because the two mirrored halves cancel their tokens on merge and only the Dust fee is left to cover. Supplying the balancer's own coins for the asymmetric case is the tracked follow-up.

### Native two-party token swaps

**midnight-js:** the wallet-SDK's `initSwap` builds one side of a cross-token swap: a proven, fee-less transaction that gives one token and takes another, net unbalanced. Two mirrored halves merge into a balanced swap a sponsor funds and submits.

**midnight-rs:** [`TransactionRequest::shielded_swap(give, give_amount, receive, receive_amount)`](../crates/midnight-types/src/request.rs) is the equivalent. Each party fills its half: a fee-less Zswap offer that spends `give_amount` of `give` and creates an output for `receive_amount` of `receive` payable to itself, with give-side change returned. The half is net unbalanced (`+give`, `-receive`) and so inherently fee-less. The two halves are exact mirrors, so merging them cancels both tokens into a balanced, fee-less transaction whose only shortfall is Dust, which `TransactionRequest::sponsor` covers:

```rust,ignore
// Party A: give dx of X, receive dy of Y (to A's own wallet).
let a_half = alice.fill(TransactionRequest::shielded_swap(token_x, dx, token_y, dy)).await?;
// Party B: the exact mirror.
let b_half = bob.fill(TransactionRequest::shielded_swap(token_y, dy, token_x, dx)).await?;

// A sponsor (either party or a third party) combines and pays the fees:
let swap = merge_transactions(&[a_half.into_bytes(), b_half.into_bytes()])?;
sponsor.send_transaction(TransactionRequest::sponsor(swap)).await?;
```

Because each half carries only a guaranteed Zswap offer and no intent, two halves merge without the segment-1 collision above, and the sponsor's fee intent rides its own segment. The two halves must carry exactly mirrored `(token, amount)` pairs or the merge won't balance; the builder can't enforce the counterparty's side. See [`examples/shielded-swap`](../examples/shielded-swap) for the end-to-end flow.

### Spending your own coin in a call, and coin discovery

Two smaller pieces round out the parity:

- **Naming a coin.** midnight-js's balancer auto-selects coins by amount, and so does the wallet for a call: it covers each shielded deficit of the call, such as a `receiveShielded(coin)`, with coins it selects, and returns the change. `Wallet::spendable_shielded_coins()` enumerates the wallet's coins with their nonces and nullifiers, and `contract.circuits().<circuit>().with_shielded_inputs([coin])` names exact, nullifier-pinned coins that the wallet spends first.
- **Coin discovery.** midnight-js returns the coins that a call creates for the caller as `newCoins`, in the private data of the call result. Our analogue is `with_coin_encryption_keys` on a call: it attaches a discovery ciphertext to circuit-created outputs so the recipient finds them through normal sync (no `watchFor`). Different mechanism, same goal.

## Guaranteed vs Fallible transaction phases

A Midnight transaction has **two phases** that execute in order. midnight-js's governance code documents the model explicitly; the same rules apply to transactions built by this SDK.

**Guaranteed phase.** Runs first. Covers fee payment, Zswap input/output validity, signature checks. If anything in this phase fails, the transaction applies nothing. Normally the node rejects it at submission and the extrinsic never reaches a block, so there is no on-chain record at all. The exception is a dispatch that errors: the extrinsic still lands in a block, the pallet emits `System::ExtrinsicFailed` rather than an outcome event, and the SDK reports that as `Verdict::Failure`.

**Fallible phase.** Runs second. Covers contract calls, verifier-key updates, and other state mutations that the chain wants to record even on failure. If the fallible phase fails, the transaction **is included in a block** but its `TransactionResult` is `PartialSuccess` (or `Failure`) rather than `SucceedEntirely`. Fees are still paid; the partial mutation is **not** applied.

Practical consequences for SDK callers:

- `pending.get_receipt().await` returning successfully means the extrinsic carrying the transaction is in a finalized block, and the chain applied the transaction there. A transaction that landed but did not apply fails the wait with `ProviderError::NotApplied`.
- A contract call can land on-chain and still do nothing useful. `Contract::call_with` reports that as `ContractError::TransactionFailed`, and on the `send_transaction` path `get_receipt` reports it as `ProviderError::NotApplied`. An `Ok` from either means the call applied.
- For multi-step intents (e.g. shielded offer + contract call), one segment can succeed while another fails. The chain records this as `PartialSuccess`.

`get_receipt` returns [`TransactionReceipt`](../crates/midnight-provider/src/submit.rs), which carries the block hash, the extrinsic hash, the Midnight transaction hash, and the chain's own verdict. The verdict comes from the events the pallet emits for the transaction, read off the block the SDK is already waiting on, so separating "the extrinsic is in a block" from "the transaction applied" needs no indexer and no second call. Only `Success` reaches the caller as `Ok`. For `PartialSuccess` and `Failure`, the wait fails with `ProviderError::NotApplied`, so `?` stops on a transaction that did not apply:

```rust,ignore
let pending = provider
    .send_transaction(TransactionRequest::unshielded(NIGHT, 100, recipient))
    .await?;
pending.get_receipt().await?;
```

A caller that branches on the outcome matches the result of the wait. The `NotApplied` holds the `TransactionReceipt`, so its `verdict` tells the two failures apart:

```rust,ignore
match pending.get_receipt().await {
    Ok(receipt) => { /* applied */ }
    Err(ProviderError::NotApplied(not_applied)) => match not_applied.0.verdict {
        Verdict::PartialSuccess => { /* guaranteed phase committed, a fallible segment did not */ }
        Verdict::Failure        => { /* the dispatch errored: the extrinsic is on chain, the
                                        transaction applied nothing, and no fees were taken */ }
        Verdict::Success        => unreachable!("the wait returns Success as Ok"),
    },
    Err(other) => { /* no verdict: see `SubmitError` below */ }
}
```

What the events do not carry is which segment failed. Every transaction the SDK builds holds one fallible segment, so `PartialSuccess` is unambiguous there. A merged multi-party transaction (`merge_transactions`, two `shielded_swap` halves) holds several, and the SDK does not report which of them failed.

When `get_receipt` fails with no verdict, the error is `ProviderError::Submission` carrying a typed [`SubmitError`](../crates/midnight-provider/src/submit.rs) instead of a string to parse. The variants encode the retry semantics: `Invalid` is a definitive node rejection (safe to rebuild and resubmit), `Dropped` and `NodeError` are not (the tx may still be re-included; resubmitting the same inputs risks a double spend), and `WatchStream` means the watch subscription itself broke while the tx's fate stayed unknown.

## Contract events

A Compact circuit emits an event (MIP-0002) with `emit`. midnight-js returns the events of a call as `CallResultPublic.logEvents`, in emission order across the whole call tree. Each `LogEvent` carries the address of the contract that emitted it, and `ContractLog.decodeAll` decodes the payloads.

midnight-rs returns the events as `logs` on `CallOutcome` (from `PendingCall::get_receipt` or `Contract::call_with`) and on `Simulated` (from `.simulate()`). The interpreter puts them on `ExecutionResult::logs`. Each item is the upstream `VersionedLogItem`, re-exported with its `LogEventType` from `midnight_contract::runtime`. It holds a version, an event type and the payload as a raw `StateValue`. Read the payload with the cell helpers of `midnight_typed_state`, such as `cell_as`.

The differences:

- An item carries no address. The interpreter refuses a cross-contract call, so every event comes from the called contract, and the caller has its address.
- The SDK has no typed decoder like `ContractLog.decodeAll`.
- The local run decodes each event as the ledger 9 VM does, so the SDK returns the item that the chain logs.

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

**midnight-rs** abstracts them behind its own `ZkConfigProvider` trait, which `with_zk_config` takes. A path gives the default `FsZkConfigProvider`, which reads the `keys/` and `zkir/` subdirectories of that path. To serve the artifacts from an embedded bundle, memory or a remote service, implement the trait and pass an `Arc` of it.

Before a call runs its circuit, midnight-rs compares the circuit's local verifier key with the key in the contract state. It refuses a difference with `ContractError::VerifierKeyMismatch`, before any proof. midnight-js checks every circuit when `findDeployedContract` connects. For a ledger 9 artifact, a failed check throws `ContractTypeError`. `submitCallTx` refuses a missing operation or an operation with no verifier key before the proof. It does not compare the keys.

## Contract maintenance / governance

The chain supports three governance operations on a deployed contract:

- **Insert verifier key.** Add a verifier key for a new circuit ID, or rotate the key for an existing one. Needed when you add circuits after deployment or rotate ZK setup.
- **Remove verifier key.** Mark a circuit as no longer callable.
- **Replace contract maintenance authority.** Change which signing key controls future maintenance updates.

midnight-js: see `packages/contracts/src/governance/` (`submit-insert-vk-tx.ts`, `submit-remove-vk-tx.ts`, `submit-replace-authority-tx.ts`).

midnight-rs: exposed via `Contract::at(addr).maintenance()`, which returns a `ContractMaintenance` builder. Chain `insert_verifier_key`, `remove_verifier_key`, and `replace_authority` (applied in order, atomically, in one signed update), then `prepare()` for a `PreparedMaintenance`. Signing is caller-controlled: read `data_to_sign()`, attach signatures with `sign(committee_index, key)` / `add_signature(...)`, then call `send()` to submit, or `build()` for the proven bytes without a submit. The initial authority is set at deploy via `DeployBuilder::with_maintenance_authority`, and `Contract::maintenance_authority()` reads the current committee. The SDK never stores a key. See [`contract-maintenance-governance.md`](./contract-maintenance-governance.md).

## Per-contract private state

Some contracts use witnesses that are themselves stateful (counters, secret balances, etc.). midnight-js gives every contract a `privateStateId` and stores the corresponding private state — plus the contract's signing key — in a local keychain via `PrivateStateProvider`. The `level-private-state-provider` package backs this with LevelDB, encrypts the private state at rest with a password from `privateStoragePasswordProvider`, and supports encrypted export / import.

midnight-rs: the `midnight-private-state` crate has a `PrivateStateProvider` trait plus a filesystem default (`FsPrivateStateProvider`) that stores opaque per-contract private-state blobs and signing keys, with password-encrypted (Argon2id + AES-256-GCM) export/import. Unlike midnight-js, it stores the private state in plaintext at rest, and only the export is encrypted. Attach it with `MidnightProvider::with_private_state(...)`. See [`private-state.md`](./private-state.md).

Private state is also **threaded** through witness execution, matching midnight-js. `WitnessProvider::call_witness(ctx, name, args)` takes a `&mut WitnessContext` carrying the mutable private state (the analogue of midnight-js's `(ctx, ...args) => [newPS, result]`). When a `PrivateStateProvider` is attached, a circuit call loads the contract's state before execution and threads it through the witnesses. It records the state after the call as a `Pending` snapshot before the submit. It then settles the snapshot by the chain's verdict (see [`private-state.md`](./private-state.md)). So stateful-witness contracts work across calls without the caller managing storage. Private state is keyed by contract address: a Compact contract has exactly one private-state type shared by all its witnesses, so there is one blob per contract (the caller packs every private variable into it), rather than midnight-js's separate per-app `privateStateId`.

## Deploy and the constructor

Both SDKs run the contract's Compact constructor locally when they deploy, and deploy the state that it leaves. midnight-js runs `contractExec.initialize(initialPrivateState, ...args)` from `deployContract` (`packages/contracts/src/unproven-deploy-tx.ts`). midnight-rs runs the constructor's analyzed IR with the interpreter, from the generated `Contract::deploy`. Both run it with the wallet's coin public key for `ownPublicKey()` and the wall-clock time. midnight-js runs it at the zero contract address, `dummyContractAddress()`, so `kernel.self()` there reads zeros ([LFDT-Minokawa/compact#117](https://github.com/LFDT-Minokawa/compact/issues/117)). midnight-rs runs it with no address, and refuses a constructor that reads `kernel.self()`.

| midnight-js `deployContract` option | midnight-rs generated `DeployBuilder` |
| --- | --- |
| `args` | `with_constructor_args(..)`, typed from the constructor's parameters. It exists only for a constructor with parameters. |
| `initialPrivateState` | `with_initial_private_state(state)`, after `with_witnesses(&w)`. They exist only for a contract with witnesses. |
| `privateStateId` | none: the journal keys the private state by the contract address (see [Per-contract private state](#per-contract-private-state)) |
| none | `with_initial_state(state)`: deploy `state` as it is and skip the constructor |

The differences:

- midnight-js stores the private state with `privateStateProvider.set` after the deploy finalizes, and only when the caller gives a `privateStateId`. Without one, it stores nothing, also when the constructor's witnesses leave a private state. midnight-rs records the private state as a `Pending` snapshot before the submit, as for a call. It refuses a private state to record when no store is attached.
- midnight-js drops the ledger effects of the constructor, and puts its Zswap outputs into the guaranteed offer of the deploy (`zswapStateToOffer`). midnight-rs refuses a constructor with a ledger effect, a Zswap input or a Zswap output, with `ContractError::Construction`, before any proof.
- compact-js gives the contract a maintenance authority with one key from a sampled signing key. midnight-rs keeps the empty committee unless the caller sets one with `with_maintenance_authority`.

## Feature gaps

These are real Midnight chain features that midnight-js exposes and this SDK does not (yet). Listed roughly in order of how often they come up.

### Wallet observability

midnight-js's `PublicDataProvider` exposes RxJS `Observable` streams backed by indexer subscriptions, such as `contractStateObservable` and `unshieldedBalancesObservable`. It also has promises that wait for one record, such as `watchForContractState` and `watchForTxData`. midnight-rs has no contract-state or contract-balance stream. `WalletBuilder::sync_with_progress` reports the progress of the sync that builds a wallet as `SyncProgress` events. After that sync, `Wallet::sync_in_background` returns a `BackgroundSync`. Its `wait_until` waits until a condition on the balance holds. Its `last_error` gives the error of the last pass. For a transaction that this SDK submits, `PendingTx::get_receipt` takes the place of `watchForTxData`.

### Cross-contract calls

midnight-js runs a circuit that calls another contract: `CrossContractConfig` and the `contractModuleProvider` resolve each callee to its compiled module. midnight-rs generates bindings for such a circuit, but the interpreter refuses the call with `InterpreterError::Unsupported`.

## When to use which SDK

- **midnight-js** — browser dApps, Node.js services, anywhere you want to plug a browser wallet, swap a remote prover, or fetch keys over HTTP. The provider split makes this natural.
- **midnight-rs** — Rust services and CLIs, embedded / signing-server use cases, anywhere a typed `?`-everywhere experience matters more than runtime swappability.

The two SDKs target the same chain and read outputs of the same compiler run (via our [forked Compact compiler](../README.md#compile-a-contract)), so the same contract can be deployed from one and called from the other. The exception is a circuit that calls another contract, which midnight-rs refuses (see "Cross-contract calls" above).
