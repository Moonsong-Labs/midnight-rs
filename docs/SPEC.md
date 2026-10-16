# midnight-rs — Architecture

Rust SDK for the Midnight blockchain. Covers the full contract lifecycle (deploy, query, call circuits, prove, submit) plus wallet management for the chain's three asset legs (zswap-shielded coins, Dust fee tokens, unshielded UTXOs).

## Workspace

All crates live under `crates/`.

```
midnight-core                    meta-crate; re-exports the public API
  ├── midnight-contract          contract lifecycle (deploy / call / prove / submit)
  │     ├── contract.rs          Contract<P>, DeployBuilder, ConnectBuilder, PendingDeploy
  │     ├── call.rs              circuit-call tx builder, shielded inputs, caller change
  │     ├── deploy.rs            deploy tx builder
  │     ├── maintenance.rs       verifier-key rotation, authority replacement
  │     ├── state.rs             state fetch (node RPC) + deserialization
  │     ├── zk_config.rs         ZkConfigProvider: where prover/verifier keys come from
  │     ├── offer.rs             ShieldedOffer: coins and payments a deploy carries
  │     ├── ledger_8 / ledger_9  (private) each generation's half of a call, deploy and
  │     │                        maintenance update, and the shielded outputs and change
  │     │                        that calls and deploys share
  │     └── interpreter / runtime  re-exports of compact-interpreter and compact-runtime
  │
  ├── midnight-provider          network entrypoint; wallet reached only through the facade
  │     ├── MidnightProvider     Provider impl; transfer_*, register_dust, resync, submit
  │     ├── ledger_8 / ledger_9  Builds: the attached wallet's builds on one generation
  │     ├── ProofProviders       one ProofProvider per generation
  │     ├── remote_prover        RemoteProofServer (ProofProvider over an HTTP proof server)
  │     ├── submit               PendingTx, PreparedTx, TxInBlock, Verdict, NotApplied
  │     └── (deps) midnight-types, midnight-wallet-facade, subxt (node RPC)
  │
  ├── midnight-types       implementation-free vocabulary and toolkit; a function of
  │     │                        midnight-helpers + the indexer client + H256 + BIP-32/BIP-39
  │     ├── ledger_version.rs    LedgerVersion, read from the tag of chain data
  │     ├── coin.rs              token types, nullifiers, coins, keys: one type for both
  │     ├── transfer.rs          TransferRequest, SpentInputs, TransferResult
  │     ├── ledger_8 / ledger_9  TransferBuilder + PreparedTransfer + prove, PreparedInput,
  │     │                        and the conversions to and from that generation's types
  │     ├── balance.rs           WalletBalance / DustBalance / ShieldedBalance
  │     ├── sync.rs              TrackedUtxo, SyncCursors
  │     ├── hd.rs                Seed and its addresses, mnemonic, Role and the BIP-32 role keys
  │     ├── address.rs           parse_shielded_recipient
  │     ├── network.rs           Network: the bech32 HRP suffix, typed
  │     ├── chain_pin.rs         pin a snapshot to a finalized block, to catch a chain swap; SyncSource
  │     └── error.rs             WalletError
  │
  ├── midnight-wallet-facade     the WalletFacade trait, IntoWalletFacade (what
  │                              with_wallet takes), and per generation the
  │                              WalletBuilds trait and ReservedBuild, in
  │                              midnight-types's vocabulary; nothing else
  │
  ├── midnight-wallet            the local implementation; depends on the two crates above
  │     ├── local.rs             LocalWallet: the facade over a locally-owned Wallet
  │     ├── sync.rs              Wallet::from_seed / from_mnemonic / from_mnemonic_with_passphrase or
  │     │                        WalletBuilder::new → WalletBuilder → sync(source), pinned by default
  │     ├── wallet.rs            Wallet { seed, state of the chain's generation }, crossing forks
  │     ├── ledger_8 / ledger_9  the state, balance and pending reservations of one generation
  │     └── storage.rs           generation-based atomic persistence
  │
  ├── midnight-private-state     per-contract private state + maintenance signing keys,
  │                              with password-encrypted export/import
  │
  ├── midnight-owner-only        0700 directories, 0600 files on Unix, for the wallet and private-state stores
  │
  ├── midnight-indexer-client    GraphQL client + subscriptions for the wallet sync
  │
  ├── compact-bindgen           `contract!` macro entry point
  │     ├── compact-bindgen-macro      proc-macro → compact-codegen
  │     └── midnight-typed-state    accessors, nav, lazy::StateQueryProvider
  │
  ├── compact-codegen            Compact IR types + Rust codegen
  ├── compact-analyzed-ir        reader for the compiler's analyzed-ir.sexp artifact
  ├── compact-interpreter        tree-walking interpreter for the analyzed IR
  ├── compact-runtime            runtime values, witnesses, execution results
  │
  ├── midnight-crypto            facade over base-crypto / transient-crypto / curves
  │
  └── midnight-helpers           a ledger_8 and a ledger_9 module over the upstream node
                                 helpers, and the items both generations share
                                 (single pinning point for the upstream deps)
```

## Core types at a glance

| Type | Crate | Role |
|---|---|---|
| `MidnightProvider` | provider | Network entry. Holds node URL, the indexer URL that the wallet sync reads, wallet (`Arc<dyn WalletFacade>`), proof backend. |
| `Provider` trait | provider | Read-only chain interface; blanket-impl'd for `&T`, `Arc<T>`, `Box<T>`. |
| `Wallet` | wallet | The synced state itself. It runs the indexer replays of a sync, a resync and a rescan. The provider reaches it through `WalletFacade`. |
| `WalletFacade` | wallet-facade | The wallet's API, in `midnight-types`'s vocabulary. Both `midnight-wallet` and `midnight-provider` depend on it; neither the facade nor the provider depends on an implementation. |
| `LocalWallet` / `WalletBuilder` | wallet | The facade implemented over a locally-owned `Wallet`, and the builder (`Wallet::from_seed`, `Wallet::from_mnemonic`, `Wallet::from_mnemonic_with_passphrase`, `WalletBuilder::new`) that syncs one from an indexer. |
| `Contract<P>` | contract | Stateless, immutable handle. Holds address + provider; fetches fresh state per call. |
| `DeployBuilder<'_, P>` / `ConnectBuilder<P>` | contract | Typestate builders; `DeployBuilder` is `IntoFuture`. |
| `PendingTx` / `TxInBlock` | provider | Watch handle over `submit_and_watch`; `wait_best` / `wait_finalized`. A wait returns the `TxInBlock` only when the chain applied the transaction; a landed transaction that did not apply fails with `ProviderError::NotApplied`, and other failures carry a typed `SubmitError`. |
| `PendingDeploy<P>` | contract | Same as `PendingTx` for deploys, plus `into_contract()`. A wait fails with `TransactionFailed` when the deploy did not apply. `into_contract()` waits for the best block itself when no wait ran, then reads the contract from the node at the block that applied the deploy. One deadline bounds the wait and the read. |
| `ProofProvider` | helpers | Proof backend trait, one per ledger generation. |
| `ProofProviders` | provider | One `ProofProvider` per generation. Set on the provider with `with_proof_provider`. `ProofProviders::local()` (in-process) is the default. |
| `RemoteProofServer` | provider | `ProofProvider` of both generations that delegates to an HTTP proof server (`/check` + `/prove`). |
| `LedgerVersion` | types | A ledger generation (`V8`, `V9`), read from the tag of the chain's data. |
| `Builds` | provider | The attached wallet's builds on the generation its state is in: `Builds::Ledger8` or `Builds::Ledger9`, from `MidnightProvider::builds`. |

## Provider ↔ Wallet model

The wallet owns the seed, secret keys, synced zswap / dust / unshielded state, ledger parameters, the latest `BlockContext`, and a `PendingReservations` set, all in the ledger generation its chain runs. It exposes accessors. A build reaches its state, and reserves its inputs, through the facade. The only I/O it drives is the replay phase of a sync, a resync or a shielded rescan, and the network and chain-pin questions to a node. A sync reads its indexer and its node from its `SyncSource`, such as the provider. The wallet keeps that indexer for later replays, and the provider answers a resync's pin questions.

Each of those three splits into plan → run → commit, so the replay runs with the wallet free: the plan is snapshotted under a read lock, the replay touches nothing, and the commit takes a write lock. `LocalWallet` composes the three; `Wallet::resync` and `Wallet::rescan_shielded` compose them for a wallet nobody shares.

`MidnightProvider` reaches the wallet through `Arc<dyn WalletFacade>` and never names an implementation. The local one is built on its own and attached:

```
let provider = MidnightProvider::new(node_url, indexer_url)

Wallet::from_mnemonic(phrase)?                      // midnight-wallet; or from_seed(hex),
                                                    // from_mnemonic_with_passphrase(phrase, passphrase), WalletBuilder::new(seed)
      .with_network(Network::Preprod)               // optional: the node's network without it
      .with_storage(dir)                            // optional persistence
      .unpinned()                                   // optional: skip the network check and the chain-reset guard
      .sync(&provider).await                        // one-shot sync → Wallet; the source is any SyncSource
    or .sync_with_progress(&provider).await         // streaming progress

provider
  .with_wallet(wallet)                              // a synced Wallet, or a WalletFacade that also
                                                    // implements each generation's WalletBuilds
  .with_proof_provider(provers)                     // optional, ProofProviders::local() by default
  .ledger_version().await          → the chain's generation, from the node's finalized block
  .resync_wallet().await                            // incremental refresh
  .watch_for_coin(coin).await                       // claim a coin with no usable ciphertext
  .forget_coin(coin).await                          // drop a registration that matched nothing
  .rescan_shielded().await                          // replay the shielded stream from event zero
  .builds().await                  → Builds::Ledger8(b) | Builds::Ledger9(b) (resyncs first)
      b.build_context().await      → Arc<BuildContext> (evicts expired pending)
      b.execution_context().await  → the half a circuit runs against, with no funding view
      b.add_funding(&ctx) / b.build_funded(tx_info) / b.prepare_shielded_inputs(..)
  .transfer_shielded / transfer_unshielded / shielded_swap / register_dust
  .prepare(tx_bytes).await         → PreparedTx (hashes known, not submitted or validated)
  .submit(tx_bytes).await          → PendingTx
  .merge_transactions(&[..])       → one transaction from several proven ones
  .balance_transaction(bytes).await → fund someone else's fee-less transaction
  .balance() / .dust_synced() / .parameters() / .unshielded_utxos() / .sync_cursors()
  .release(&spent)                 // hand back a build's reserved inputs
  .health().await                  → node reachability
```

`transfer_shielded`, `transfer_unshielded` and a generated contract call also offer `.without_dust()`, which stops at a fee-less `DustlessTransaction`. That is the multi-party flow: the contributors build fee-less, one payer merges the halves with `merge_transactions`, covers the fee with `balance_transaction`, then submits. `shielded_swap` is always fee-less, because an unbalanced half cannot fund itself, so it has no `.without_dust()` step. See [`docs/wallet.md`](wallet.md).

`with_network` accepts a `Network` variant or a `&str` / `String` (via `impl Into<Network>`). See [`docs/wallet.md`](wallet.md) for the typed and string forms.

`WalletBuilder::sync` runs three concurrent indexer subscriptions (zswap ledger events, dust ledger events, unshielded transactions) and returns once all three have caught up. Each subscription keeps its socket alive with a client ping after idle and a hard idle timeout, and transient transport failures reconnect with bounded exponential backoff, resuming from the last applied cursor (`IndexerError::is_retryable` distinguishes retryable from fatal; a per-connection dedupe keeps re-delivered events from being double-applied). The latest-block query that each sync and resync makes retries on the same bound. State is persisted under `{base}/{network}/{sha256(unshielded_address)}/` as `metadata.json` + `zswap-N.bin` + `dust_wallet-N.bin` + `pending.json`, with generation-based atomic writes (binary files first, atomic metadata rename, then old-generation cleanup). The sync persists only with `.with_storage(base)`, and `Wallet::default_storage_dir()` gives `~/.midnight/wallets`. `metadata.json` and `pending.json` also name the ledger generation whose types wrote them (`ledger_version`, ledger 8 when absent).

`PendingReservations` records the spends of builds that the chain has not yet confirmed. Every build reserves its dust spends, unshielded UTXOs and shielded nullifiers before it proves, under the same hold of the wallet that selected them. Thus a second build in this process cannot pick the same input.

A `transfer_*` build reserves its inputs through `prepare_transfer`. A deploy with no shielded offer and a maintenance update reserve theirs through `prepare_funded`. A sponsor's build reserves the Dust for the fee it pays through `prepare_fees`. A call, or a deploy with a shielded offer, reserves its pinned coins through `spend_shielded`. It then reserves the Dust for its own fee through `prepare_fees`.

Reservations clear when event replay (sync or resync) observes the corresponding confirmed spends: a dust batch clears when any of its spend nullifiers appears in a `DustSpendProcessed` event, an unshielded reservation when its `(intent_hash, output_index)` key appears as a spent UTXO. TTL expiry (`evict_expired`, which every funded build runs through the wallet's `add_funding`) remains as a backstop for transactions that never confirm.

`watch_for_coin` covers the shielded coin a wallet owns but cannot discover, because its output carries no ciphertext the wallet can read. It records the coin's commitment (`ZswapLocalState::watch_for`) and then replays `zswapLedgerEvents` from event zero, since a replay that meets an unclaimable output collapses that Merkle leaf and a resync resumes from its cursor. The replay rebuilds `zswap_state` and its cursor only; dust, unshielded, parameters, and pending reservations are left alone. It re-registers every coin the wallet already holds first, because the ledger consumes a registration when it claims it and a coin recovered by an earlier registration has no ciphertext to be re-found by. `commit_resync` carries registrations across for the same reason. `forget_coin` drops a registration whose rebuilt `CoinInfo` matched no output, which would otherwise ride along on every replay. See [`docs/wallet.md`](wallet.md#recovering-a-coin-the-wallet-cannot-discover).

## Ledger generations

One build runs on a ledger 8 chain and on a ledger 9 chain. The SDK reads the generation from the chain's data: the encoding tag of the tip block's ledger parameters, of each ledger event and of each transaction. Nothing configures it.

Most of the API is the same on both generations: the transfers, `balance`, `submit`, the generated contract bindings, a deploy's shielded offer and maintenance. Its vocabulary (token types, nullifiers, coins, keys, `ChainParameters`) lives in `midnight-types`, one type for both generations. The per-generation layer is for low-level builds. `MidnightProvider::builds` hands out the `Builds` of the wallet's generation, and a custom wallet implements `WalletBuilds` once per generation.

Each crate compiles one per-ledger source twice, as its `ledger_8` and `ledger_9` modules, against that generation's helpers. The Compact side reads ledger 9's types on both chains. The two generations share the Impact ops and the state encoding, and `decode_contract_state` converts a ledger 8 state.

A wallet crosses a hard fork on a sync, on a resume from its snapshot, and at the next resync of an attached wallet. See [`docs/ledger-generations.md`](ledger-generations.md).

## Data flows

### Query state

One node RPC, `midnight_contractState`, serves the whole state. `MidnightProvider` exposes it as `get_state_from_node`, and its `Provider::get_contract_state` forwards to it:

```
Provider.get_contract_state(address, at)              → hex string   // at = None: the best block
MidnightProvider.get_state_from_node(address, at)     → hex string
  → hex decode → decode_contract_state → ContractState<InMemoryDB>
```

Generated bindings expose this as `contract.ledger().await?`, which calls `midnight_contractState` over node RPC and returns a sync `Ledger` struct with typed field accessors. `contract.ledger_query()` (custom node builds only) routes per-field reads through `midnight_queryContractState` via the `StateQueryProvider` bridge in `midnight-provider`.

### Deploy

```
Contract::deploy(&provider)                              // DeployBuilder<'_, P>
  .with_initial_state(LedgerInitialState::default())
  .with_zk_config("compiled")
  [.with_deploy_timeout(...)]

  .await                                                 // IntoFuture: send + into_contract
    │
    └─ .send().await   →  PendingDeploy<P>               // explicit form
         ├─ .wait_best().await        → (TxInBlock, PendingDeploy)   // TransactionFailed if not applied
         ├─ .wait_finalized().await   → (TxInBlock, PendingDeploy)   // TransactionFailed if not applied
         └─ .into_contract().await    → Contract<P>
```

Internally:

```
with_zk_config(initial_state, zk_config)      // load *.verifier files into state.operations
  ↓
deploy_funded(state, provider, shielded_offer)
  ├─ provider.builds().await                  // resync wallet, pick its generation's Builds
  ├─ builds.execution_context().await         // build that generation's context
  ├─ builds.proof_provider()                  // that generation's prover (ProofProviders)
  ├─ build deploy intent (state re-encoded for the generation)
  ├─ no offer: builds.build_funded(tx_info).await  → DeployResult { address, tx_bytes }
  │   ├─ one transition: add the funding view, balance the fee with mock
  │   │  proofs (speculative_spend loop), record what it drew
  │   └─ prove the balanced tx once, for real, with the wallet free
  └─ an offer:                                → DeployResult { address, tx_bytes }
      ├─ builds.add_funding(context), then builds.prepare_shielded_inputs:
      │  the wallet spends and reserves the offer's coins
      ├─ refuse a token the coins do not cover (ShieldedOfferShortfall)
      ├─ guaranteed offer: the coins, the payments, one change output per token
      ├─ build_no_validate: prove with no fee
      └─ builds.balance_transaction: the Dust fee in its own intent
  ↓
provider.submit(tx_bytes).await               → PendingTx
  ↓
into_contract
  ├─ one deadline: deploy_timeout from the start of into_contract
  ├─ the inclusion of the last wait, or wait_best under the deadline
  │    (not applied → TransactionFailed; deadline passes → DeployTimeout { in_block: None })
  └─ deployed_state(provider, address, in_block, remaining, timeout)
       └─ read midnight_contractState at in_block's block, for the time left of the deadline
          (absent → NotFound; deadline passes → DeployTimeout { in_block: Some(..) })
  ↓
Contract<P>   // stateless handle, no cached state
```

### Connect to an existing contract

```
Contract::at(&provider, address)              // ConnectBuilder<P>
  .with_zk_config("compiled")
  [.at_block(node_block_hash)]                // pin every read to one block
  .build()                                    // synchronous, no network calls
  → Contract<P>
```

### Call a circuit (on-chain)

```
contract.circuits().increment_by(5).await
  ↓
read the state and the block time at one block (per call):
  state_at_block(address, at_block)           // the at_block pin, else best_block_hash();
                                              // the state through midnight_contractState,
                                              // the time from Timestamp::Now, in whole seconds
  ↓
provider.shielded_public_keys()               // the wallet's coin public key, which ownPublicKey() returns
  ↓
interpreter::execute(ir, program, state, args,
                     Env { witnesses, private_state, address, block_time, coin_public_key })
  → ExecutionResult { state, reads, gather_ops, communication_outputs, result }
  ↓
build verify-ops:
  gather_ops.iter().map(|op| op.clone().translate(|()| reads.next()))
  → filter empty Idx/Ins
  → Vec<Op<ResultModeVerify, InMemoryDB>>
  ↓
provider.builds() → the Builds of the wallet's generation (resyncs first)
  ↓
builds.execution_context()
  ↓
partition_transcripts([PreTranscript { context, program: verify_ops, comm_comm: None }],
                      the chain's ledger parameters from the context)
  // context: the block time and the contract balance that execute ran with
  → (guaranteed_transcripts, fallible_transcripts)
  ↓
cross InMemoryDB → DefaultDB boundary (serialize round-trip into the generation's types)
  ↓
CallAction holding typed transcripts + AlignedValue inputs/outputs
  ↓
builds.add_funding(&context)                                   // the payer joins here
  ↓
builds.prepare_shielded_inputs(..)                             // only if the call pins coins
  └─ spends them into this context and reserves them, as one transition
  ↓
  → StandardTransactionInfo → build_no_validate                // fee-less, even when self-funded
  ↓
if pay_fees: builds.balance_transaction(bytes)
  └─ the wallet draws the Dust and reserves it as one transition (prepare_fees),
     then the fee is proved on its own and merged in at its own intent segment,
     so the circuit proof is not redone
  ↓
prepared.submit().await → PendingTx → wait_finalized (bounded by DEFAULT_TX_FINALIZE_TIMEOUT)
  └─ settle: Ok confirms the snapshot; NotApplied marks it failed → TransactionFailed
  ↓
decode typed return value from ExecutionResult.result → caller
```

`Contract<P>` is not mutated; the new state is discarded because the next call will fetch fresh state anyway.

### Transfer (shielded / unshielded / swap / register dust)

Each of the four provider methods is a *sync constructor* that returns a builder type (`ShieldedTransfer<'a>`, `UnshieldedTransfer<'a>`, `ShieldedSwap<'a>`, `DustRegistration<'a>`). The builder defers all work until awaited or `.build()` is called:

```
provider.transfer_shielded(token_type, amount, recipient)       // bech32 address; no work yet
        .transfer_unshielded(token_type, amount, recipient)
        .shielded_swap(give_token, give_amount, receive_token, receive_amount)
        .register_dust(utxo_ctime)

  ↓ .await? (or .build().await? for the no-submit escape hatch)

provider.builds()                    // resync_wallet, then the wallet's generation
  ↓
ledger_N::WalletBuilds::prepare_transfer(request, proof_provider)   // one hold of the wallet
  build_context_inner (execution context + add_funding, which evicts expired pending)
  TransferBuilder::prepare(request)
    └─ select inputs from wallet's local state
    └─ balance Dust fees (speculative_spend loop, mock proofs only)
    └─ prepare_no_validate
  reserve_pending(dust_batches, spent_unshielded_inputs, shielded, reserved_at)
  → ReservedBuild
  ↓
(wallet released)    PreparedTransfer::prove       // the only real proving
  → TransferResult { tx_bytes, ledger_version, spent, fee_speck }
  ↓
(.await path only)   provider.submit(tx_bytes).await → PendingTx
```

`.await` returns `PendingTx`; the caller then chooses `wait_best` / `wait_finalized`. `.build().await` stops before submitting and returns `TransferResult`, which the caller can submit (or route) themselves. Reservations clear during the next sync/resync, when event replay observes the confirmed spends, or get evicted on TTL expiry the next time a funded build runs `add_funding`.

## Transaction submission

One auto-reconnecting websocket carries everything the node serves: raw Substrate and `midnight_*` RPCs through subxt's `RpcClient`, and the calls that need runtime metadata (submission, and storage reads such as `MidnightProvider::get_block_timestamp`) through the `OnlineClient` built on the same transport. The connection is opened on first use and cached for the provider's lifetime.

`MidnightProvider::submit` wraps `tx_bytes` as an unsigned `Midnight::send_mn_transaction` extrinsic, calls `submit_and_watch`, and hands back:

- `PendingTx` — owns the watch stream.
  - `extrinsic_hash() → H256`, `extrinsic_hash_hex() → String`
  - `transaction_hash() → TransactionHash`: the ledger's own identity for the tx
  - `wait_best(self) → Result<(TxInBlock, Self), _>`: consumes & returns self; its verdict is provisional
  - `wait_finalized(self) → Result<(TxInBlock, Self), _>`: same; may be called without prior `wait_best`; its verdict is final
  - both return `Ok` only for `Success`, and `ProviderError::NotApplied` for any other verdict
- `TxInBlock { block_hash, extrinsic_hash, transaction_hash, verdict }`
- `NotApplied(TxInBlock)`: a transaction that landed but did not apply
- `Verdict`: `Success` (`TxApplied`), `PartialSuccess` (`TxPartialSuccess`: the guaranteed phase committed, a fallible segment did not), or `Failure`.

Both `wait_*` methods return `self` so callers re-bind without `let mut`. Cancelling a future is safe but does not retract the extrinsic from the mempool. Failures with no verdict surface as `ProviderError::Submission(SubmitError)`; the variant tells the caller whether resubmitting is safe (`Invalid`: definitive rejection; `NotSubmitted`: never left the process) or risks a double spend (`Dropped` / `NodeError`: the tx may still land) or is a wait/decode issue that leaves the tx in flight (`WatchStream`: transport-only; `VerdictFetch`: landed but events undecodable; re-query the chain rather than resubmit). `SubmitRpc` splits on the underlying failure (clean refusal is safe; transport mid-call is ambiguous).

`MidnightProvider::prepare` stops one step earlier: it builds the unsigned `send_mn_transaction` extrinsic locally from the node's metadata and returns a `PreparedTx` whose extrinsic hash is already known, so a caller can durably record state keyed by that hash before the transaction reaches the mempool. The metadata checks only the call's shape, so the node validates the transaction only at submit. `PreparedTx::submit` then hands back the same `PendingTx`.

A build that reserved inputs carries the reservation on its `PendingTx`, so a terminal rejection arriving long after the builder returned still hands them back.

## Block pinning

A contract handle pins its reads with `Contract::at(..).at_block(hash)`, and the hash is a node block hash. Both paths that honour it are node RPCs: `midnight_contractState` for a whole `ContractState`, and `midnight_queryContractState` for a lazy per-field read. Neither accepts a height, so a caller holding a height resolves it to a hash first (`MidnightProvider::get_block_hashes_by_height`). A circuit call on a pinned handle also reads the block time (`Timestamp::Now`) at the pin, so its clock checks run at the time of that block.

`Provider::get_contract_state` reads through `midnight_contractState` too, and takes the same optional hash. The generated `Ledger::from_provider(provider, address)` constructor passes none, so it reads at the best block.

## External dependencies

| Crate | Source | Purpose |
|---|---|---|
| `midnight-ledger` 8.1.2 (+ its `midnight-zswap`, `midnight-onchain-*`) | `RomarQ/midnight-ledger`, branch `rq/midnight-rs-ledger-8` | Ledger 8: transaction types, VM, proving |
| `midnight-ledger-v9` (+ its companion crates, and the `midnight-transient-crypto` 2.2 that ledger 8 also uses) | `RomarQ/midnight-ledger`, branch `rq/midnight-rs-ledger-9` | Ledger 9, the on-chain runtime the Compact side reads on both chains, and the crypto of both |
| `midnight-serialize`, `midnight-base-crypto`, `midnight-storage` | one instance for both generations, from the ledger 9 branch | The tagged codec, `HashOutput`, `AlignedValue`, `Sp` |
| `midnight-node-ledger-helpers`, `midnight-ledger-unsafe-helpers` | `RomarQ/midnight-node`, branch `rq/midnight-rs-toolkit-2.1.0-rc.3` | A `ledger_8` and a `ledger_9` module each: the type surface, and the transaction and wallet builders |
| `subxt` | crates.io | Substrate RPC (including `midnight_contractState` and `midnight_queryContractState`), extrinsic submission, watch streams, reconnecting client |
| `tokio-tungstenite` | crates.io | Indexer WebSocket subscriptions |

Ledger 9 ships only as git tags, and upstream's ledger crates and helpers name each other by crates.io version, which a git consumer can only redirect with a `[patch.crates-io]` table. The forks name every ledger crate by `path` or by revision, so a crate that depends on midnight-rs needs no table. The root `Cargo.toml` pins each fork by revision. The helpers must build against the same ledger 9 set, so the branches move together, in the order that the root `Cargo.toml` gives.

## Documentation index

| Document | What it covers |
|---|---|
| `aligned-value-navigation.md` | `AlignedValue` internals and state tree structure |
| `compact-adt-state-mapping.md` | Compact storage kinds → `StateValue` mapping |
| `compact-natives.md` | The Compact native functions the interpreter implements |
| `tagged-serialization.md` | midnight-ledger's tagged serialization format |
| `dust-and-fees.md` | Dust token model, fee balancing, generation transitions |
| `intents-and-zswap-mechanics.md` | Intent structure, zswap shielded transfer mechanics |
| `wallet.md` | Wallet usage: sync, balances, transfers, Dust registration, persistence |
| `tokens.md` | Token model: shielded vs unshielded ledgers, NIGHT, DUST, the zero-id pitfall |
| `private-state.md` | Per-contract private state store, witnesses, encrypted export/import |
| `contract-maintenance-governance.md` | k-of-n maintenance committees, verifier-key rotation, authority replacement |
| `ledger-generations.md` | Ledger 8 and ledger 9 from one build: detection, the per-generation API, crossing a hard fork |
| `midnight-js-comparison.md` | Mapping to midnight-js concepts; guaranteed/fallible phase model |

## Not yet implemented

| Feature | Notes |
|---|---|
| State change subscriptions | WebSocket subscription support for contract state updates |
| Lazy query batching | `query_contract_state` already takes a batch, but each generated `ledger_query()` accessor sends one query of its own |
| Production proving | `ProofProvider::prove` returns `Transaction`, not `Result`, so a backend signals failure by panicking. The SDK catches that unwind and reports `WalletError::Proving` rather than losing the task, but the trait has no error channel and this is not mainnet-ready |
| A remote signer | The facade lets a different local wallet stand in, but a build still needs `seed`. A browser extension or an HSM needs `prepare_transfer` to return something unsigned plus a separate signing step |
