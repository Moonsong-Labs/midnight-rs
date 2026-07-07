# midnight-rs — Architecture

Rust SDK for the Midnight blockchain. Covers the full contract lifecycle (deploy, query, call circuits, prove, submit) plus wallet management for the chain's three asset legs (zswap-shielded coins, Dust fee tokens, unshielded UTXOs).

## Workspace

All crates live under `crates/`.

```
midnight-core                    meta-crate; re-exports the public API
  ├── midnight-contract          contract lifecycle (deploy / call / prove / submit)
  │     ├── interpreter          circuit IR execution + WitnessProvider trait
  │     ├── call                 tx builders, state fetch, address utils, default timeouts
  │     ├── contract             Contract<P>, DeployBuilder, ConnectBuilder, PendingDeploy
  │     └── prover               Prover { Local, Remote(url) }
  │
  ├── midnight-provider          network entrypoint; owns wallet + node connection + indexer
  │     ├── MidnightProvider     Provider impl; sync_wallet, transfer_*, register_dust, submit
  │     ├── submit               PendingTx, TxInBlock, submit_bytes
  │     └── (deps) midnight-indexer-client (GraphQL), subxt + jsonrpsee (node RPC)
  │
  ├── midnight-wallet            pure state machine, no I/O
  │     ├── state.rs             Wallet { seed, secret keys, zswap + dust + unshielded state }
  │     ├── transfer.rs          TransferBuilder { shielded, unshielded, register_dust }
  │     ├── balance.rs           WalletBalance / DustBalance / ShieldedBalance / UnshieldedUtxoInfo
  │     ├── pending.rs           PendingReservations — in-flight spend tracking with TTL
  │     ├── address.rs           derive_shielded / derive_unshielded
  │     └── storage.rs           generation-based atomic persistence
  │
  ├── midnight-indexer-client    typed GraphQL client + subscriptions
  │
  ├── midnight-bindgen           `contract!` macro entry point
  │     ├── midnight-bindgen-macro      proc-macro → compact-codegen
  │     └── midnight-bindgen-runtime    accessors, nav, lazy::StateQueryProvider
  │
  ├── compact-codegen            Compact IR types + Rust codegen
  │
  └── midnight-helpers           thin re-export facade over midnight-node-ledger-helpers
                                 (single pinning point for the upstream dep)
```

## Core types at a glance

| Type | Crate | Role |
|---|---|---|
| `MidnightProvider` | provider | Network entry. Holds node URL, indexer client, wallet (`Arc<RwLock<Wallet>>`), proof backend. |
| `Provider` trait | provider | Read-only chain interface; blanket-impl'd for `&T`, `Arc<T>`, `Box<T>`. |
| `Wallet` | wallet | Pure state machine. All I/O is driven by `MidnightProvider`. |
| `Contract<P>` | contract | Stateless, immutable handle. Holds address + provider; fetches fresh state per call. |
| `DeployBuilder<P>` / `ConnectBuilder<P>` | contract | Typestate builders; `DeployBuilder` is `IntoFuture`. |
| `PendingTx` / `TxInBlock` | provider | Watch handle over `submit_and_watch`; `wait_best` / `wait_finalized`. Failures carry a typed `SubmitError`. |
| `PendingDeploy<P>` | contract | Same as `PendingTx` for deploys, plus `into_contract()` to wait for indexer. |
| `Prover` | contract | `Local` (in-process) or `Remote(url)` (HTTP proof server). |

## Provider ↔ Wallet model

The wallet owns the seed, secret keys, synced zswap / dust / unshielded state, ledger parameters, the latest `BlockContext`, and a `PendingReservations` set. It exposes accessors and `set_*` / `reserve_pending` mutators; aside from `sync_inner` / `resync` (driven by the provider), it does no I/O itself.

`MidnightProvider` owns the wallet behind `Arc<RwLock<Wallet>>` and is the only place that drives network I/O for it:

```
MidnightProvider::new(node_url, indexer_url)
  .sync_wallet(seed, Network::Preprod)
      .with_storage(dir)                            // optional persistence
      .await                                        // one-shot sync
    or .stream()                                    // streaming progress
  .with_wallet(wallet)                              // attach an existing one
  .resync_wallet().await                            // incremental refresh
  .build_context().await           → Arc<LedgerContext> (resyncs + evicts expired pending)
  .transfer_shielded / transfer_unshielded / register_dust
  .submit(tx_bytes).await          → PendingTx
  .balance() / .dust_synced() / .seed() / .wallet() / .wallet_mut()
```

The `network` argument accepts both `Network` enum variants and `&str` / `String` (via `impl Into<Network>`). See [`docs/wallet.md`](wallet.md) for the typed-vs-string ergonomics.

`sync_wallet` runs three concurrent indexer subscriptions (zswap ledger events, dust ledger events, unshielded transactions) and returns once all three have caught up. Each subscription keeps its socket alive with a client ping after idle and a hard idle timeout, and transient transport failures reconnect with bounded exponential backoff, resuming from the last applied cursor (`IndexerError::is_retryable` distinguishes retryable from fatal; a per-connection dedupe keeps re-delivered events from being double-applied). State is persisted under `~/.midnight/wallets/{network}/{sha256(seed)[..16]}/` as `metadata.json` + `zswap-N.bin` + `dust_wallet-N.bin` + `pending.json`, with generation-based atomic writes (binary files first, atomic metadata rename, then old-generation cleanup).

`PendingReservations` records spends that have been built but not yet confirmed on-chain. Each `transfer_*` call reserves dust spends + unshielded UTXOs against the wallet immediately after building, so a subsequent build can't pick the same coin. Reservations clear when event replay (sync or resync) observes the corresponding confirmed spends: a dust batch clears when any of its spend nullifiers appears in a `DustSpendProcessed` event, an unshielded reservation when its `(intent_hash, output_index)` key appears as a spent UTXO. TTL expiry (`evict_expired`, called from `build_context_inner`) remains as a backstop for transactions that never confirm.

## Data flows

### Query state

Two paths, both surfaced through `MidnightProvider`:

```
indexer (GraphQL):
  Provider.get_contract_state(address, offset) → hex string
  deserialize_state(hex) → ContractState<InMemoryDB>

node RPC (preferred for latest / hash-pinned):
  MidnightProvider.get_state_from_node(address, at_block_hash) → hex string
  → deserialize_state → ContractState<InMemoryDB>
```

Generated bindings expose this as `contract.ledger().await?`, which calls `midnight_contractState` over node RPC and returns a sync `Ledger` struct with typed field accessors. `contract.ledger_query()` (custom node builds only) routes per-field reads through `midnight_queryContractState` via the `StateQueryProvider` bridge in `midnight-provider`.

### Deploy

```
Contract::deploy(&provider)                              // DeployBuilder<P>
  .with_initial_state(LedgerInitialState::default())
  .with_zk_keys("compiled")
  [.with_prover(...) .with_deploy_timeout(...) .with_deploy_poll_interval(...)]

  .await                                                 // IntoFuture: send + wait_best + into_contract
    │
    └─ .send().await   →  PendingDeploy<P>               // explicit form
         ├─ .wait_best().await        → (TxInBlock, PendingDeploy)
         ├─ .wait_finalized().await   → (TxInBlock, PendingDeploy)
         └─ .into_contract().await    → Contract<P>
```

Internally:

```
with_zk_keys(initial_state, keys_dir)         // load *.verifier files into state.operations
  ↓
deploy_funded(state, provider, keys_dir, prover)
  ├─ provider.build_context().await           // resync wallet, build LedgerContext
  ├─ build deploy intent, balance Dust fees   // speculative_spend loop, mock then real proofs
  └─ build_no_validate                         → DeployResult { address, tx_bytes }
  ↓
provider.submit(tx_bytes).await               → PendingTx
  ↓ (IntoFuture path) wait_best
wait_for_deployment(provider, address, timeout, poll_interval)
  └─ poll indexer until the contract appears
  ↓
Contract<P>   // stateless handle, no cached state
```

### Connect to an existing contract

```
Contract::at(&provider, address)              // ConnectBuilder<P>
  .with_zk_keys("compiled")
  [.at_block(BlockRef::Hash | BlockRef::Height | BlockRef::Finalized) .with_prover(...)]
  .build()                                    // synchronous, no network calls
  → Contract<P>
```

### Call a circuit (on-chain)

```
contract.circuits().increment_by(5).await
  ↓
fetch fresh state (per-call):
  at_block = Some(Hash(h))   → fetch_state_from_node(address, Some(h))
  at_block = Some(Height(n)) → fetch_state_at(address, ContractActionOffset::block_height)
  at_block = Some(Finalized) → fetch_state_from_node(address, Some(chain_getFinalizedHead))
  at_block = None            → fetch_state_from_node(address, None)
  ↓
interpreter::execute_with(ir, state, args, witnesses, helpers, structs[, enums])
  → ExecutionResult { state, reads, gather_ops, communication_outputs, result }
  ↓
build verify-ops:
  gather_ops.iter().map(|op| op.clone().translate(|()| reads.next()))
  → filter empty Idx/Ins
  → Vec<Op<ResultModeVerify, InMemoryDB>>
  ↓
partition_transcripts([PreTranscript { context, program: verify_ops, comm_comm: None }],
                      INITIAL_PARAMETERS)
  → (guaranteed_transcripts, fallible_transcripts)
  ↓
cross InMemoryDB → DefaultDB boundary (serialize round-trip)
  ↓
provider.build_context() → CallAction holding typed transcripts + AlignedValue inputs/outputs
  → StandardTransactionInfo → pay_fees_no_validate → prove_tx_no_validate → tx_bytes
  ↓
provider.submit(tx_bytes).await?.wait_best().await?            // best-block inclusion
wait_for_contract_update(provider, address, height_before, …)  // indexer caught up
  ↓
decode typed return value from ExecutionResult.result → caller
```

`Contract<P>` is not mutated; the new state is discarded because the next call will fetch fresh state anyway.

### Transfer (shielded / unshielded / register dust)

Each of the three provider methods is a *sync constructor* that returns a builder type (`ShieldedTransfer<'a>`, `UnshieldedTransfer<'a>`, `DustRegistration<'a>`). The builder defers all work until awaited or `.build()` is called:

```
provider.transfer_shielded(token_type, amount, recipient)       // bech32 address; no work yet
        .transfer_unshielded(token_type, amount, recipient)
        .register_dust(utxo_ctime)

  ↓ .await? (or .build().await? for the no-submit escape hatch)

(under wallet write lock)
resync_wallet → build_context_inner (also evicts expired pending)
  ↓
TransferBuilder::new(wallet, context, proof_provider)
  .shielded / .unshielded / .register_dust
  └─ select inputs from wallet's local state
  └─ balance Dust fees (speculative_spend loop, mock proofs → real proofs)
  └─ build_no_validate
  → TransferResult { tx_bytes, dust_batches, spent_unshielded_inputs }
  ↓
wallet.reserve_pending(dust_batches, spent_unshielded_inputs, reserved_at)
  ↓
(.await path only)   provider.submit(tx_bytes).await → PendingTx
```

`.await` returns `PendingTx`; the caller then chooses `wait_best` / `wait_finalized`. `.build().await` stops after the reserve step and returns `TransferResult`, which the caller can submit (or route) themselves. Reservations clear during the next sync/resync, when event replay observes the confirmed spends, or get evicted on TTL expiry the next time `build_context_inner` runs.

## Transaction submission

`MidnightProvider::submit` connects (or reuses the cached jsonrpsee `WsClient`), wraps `tx_bytes` as an unsigned `Midnight::send_mn_transaction` extrinsic, calls `submit_and_watch`, and hands back:

- `PendingTx` — owns the watch stream.
  - `extrinsic_hash() → [u8; 32]`, `extrinsic_hash_hex() → String`
  - `wait_best(self) → Result<(TxInBlock, Self), _>` — consumes & returns self
  - `wait_finalized(self) → Result<(TxInBlock, Self), _>` — same; may be called without prior `wait_best`
- `TxInBlock { block_hash, extrinsic_hash }`

Both `wait_*` methods return `self` so callers re-bind without `let mut`. Cancelling a future is safe but does not retract the extrinsic from the mempool. Failures surface as `ProviderError::Submission(SubmitError)`; the variant tells the caller whether resubmitting is safe (`Invalid`: definitive rejection; `NotSubmitted`: never left the process) or risks a double spend (`Dropped` / `NodeError`: the tx may still land) or is a wait/decode issue that leaves the tx in flight (`WatchStream`: transport-only; `VerdictFetch`: landed but events undecodable; re-query the chain rather than resubmit). `SubmitRpc` splits on the underlying failure (clean refusal is safe; transport mid-call is ambiguous).

## Block pinning

`BlockRef::Hash(_)` works for both circuit-call state fetches (node RPC) and lazy ledger queries (also node RPC). `BlockRef::Height(_)` works only for circuit-call state fetches via the indexer's `ContractActionOffset`; lazy ledger queries fall back to latest because `midnight_queryContractState` only accepts a block hash. `BlockRef::Finalized` works everywhere `Hash` does: `Contract::resolved_block_hash` resolves it to the finalized head's hash via `chain_getFinalizedHead` at query time (for a lazy `ledger_query()` handle, once when the handle is built). Use `Hash` for fully consistent block-pinned access, `Finalized` for reorg-safe reads that follow the chain.

## External dependencies

| Crate | Source | Purpose |
|---|---|---|
| `midnight-ledger` (+ `midnight-zswap`, `midnight-onchain-*`, `midnight-serialize`, `midnight-transient-crypto`, `midnight-storage-core`) | `midnightntwrk/midnight-ledger` git tags (rc.1), patched in via `[patch.crates-io]` | Transaction types, VM, proving, crypto |
| `midnight-node-ledger-helpers` | `RomarQ/midnight-node` (forked) | `DustWallet`, `LedgerContext`, `WalletSeed`, sync infra |
| `midnight-rpc-api` | `RomarQ/midnight-node` (forked) | Typed client for `midnight_contractState` + `midnight_queryContractState` RPCs |
| `subxt` | crates.io | Substrate RPC, extrinsic submission, watch streams |
| `jsonrpsee` | crates.io | WebSocket RPC client (shared between subxt and typed midnight RPC) |
| `tokio-tungstenite` | crates.io | Indexer WebSocket subscriptions |

The published `=8.1.0-rc.1` ledger versions don't exist on crates.io; the workspace mirrors the helpers crate's git redirects in `[patch.crates-io]` so resolver paths unify with our direct deps.

## Documentation index

| Document | What it covers |
|---|---|
| `aligned-value-navigation.md` | `AlignedValue` internals and state tree structure |
| `compact-adt-state-mapping.md` | Compact storage kinds → `StateValue` mapping |
| `tagged-serialization.md` | midnight-ledger's tagged serialization format |
| `dust-and-fees.md` | Dust token model, fee balancing, generation transitions |
| `intents-and-zswap-mechanics.md` | Intent structure, zswap shielded transfer mechanics |
| `wallet.md` | Wallet usage: sync, balances, transfers, Dust registration, persistence |
| `tokens.md` | Token model: shielded vs unshielded ledgers, NIGHT, DUST, the zero-id pitfall |
| `private-state.md` | Per-contract private state store, witnesses, encrypted export/import |
| `contract-maintenance-governance.md` | k-of-n maintenance committees, verifier-key rotation, authority replacement |
| `midnight-js-comparison.md` | Mapping to midnight-js concepts; guaranteed/fallible phase model |

## Not yet implemented

| Feature | Notes |
|---|---|
| State change subscriptions | WebSocket subscription support for contract state updates |
| Lazy query batching | Each `ledger_query()` accessor still issues its own RPC |
| Production proving | Uses `test-utilities` proving paths; not mainnet-ready |
