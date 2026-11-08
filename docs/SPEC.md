# midnight-rs — Architecture

Rust SDK for the Midnight blockchain. Covers the full contract lifecycle (deploy, query, call circuits, prove, submit) plus wallet management for the chain's three asset legs (zswap-shielded coins, Dust fee tokens, unshielded UTXOs).

## Workspace

All crates live under `crates/`.

```
midnight-core                    meta-crate; re-exports the public API
  ├── midnight-contract          contract lifecycle (deploy / call / prove / submit)
  │     ├── contract.rs          Contract<P>, DeployBuilder, ConnectBuilder, PendingDeploy
  │     ├── constructor.rs       Constructor: the constructor run of a deploy
  │     ├── call.rs              circuit-call tx builder, shielded inputs, caller change
  │     ├── deploy.rs            deploy tx builder
  │     ├── maintenance.rs       verifier-key rotation, authority replacement
  │     ├── state.rs             state fetch (node RPC) + deserialization
  │     ├── zk_config.rs         ZkConfigProvider: where prover/verifier keys come from
  │     ├── offer.rs             ShieldedOffer: coins and payments a deploy carries
  │     ├── ledger/              (private) the ledger 9 build of a call, deploy and
  │     │     │                  maintenance update
  │     │     ├── call.rs        a circuit call's transaction: transcripts, offers, proof, funding
  │     │     ├── deploy.rs      a deploy's transaction
  │     │     ├── maintenance.rs a maintenance update's transaction
  │     │     ├── resolver.rs    the resolver that contract calls prove with
  │     │     └── state.rs       a circuit's operation, from the verifier key of its artifact
  │     └── interpreter / runtime  re-exports of compact-interpreter and compact-runtime
  │
  ├── midnight-provider          network entrypoint; wallet reached only through the facade
  │     ├── MidnightProvider     Provider impl; transfer_*, register_dust, resync, submit
  │     ├── builds.rs            Builds: the attached wallet's builds
  │     ├── remote_prover.rs     RemoteProofServer (ProofProvider over an HTTP proof server)
  │     ├── proving.rs           RemoteProofServer's ProofProvider impl and its proof server client
  │     ├── submit.rs            PendingTx, PreparedTx, TxInBlock, Verdict, NotApplied
  │     └── (deps) midnight-types, midnight-wallet-facade, subxt (node RPC)
  │
  ├── midnight-types       implementation-free vocabulary and toolkit; a function of
  │     │                        midnight-helpers + H256 + BIP-32/BIP-39
  │     ├── coin.rs              token types, nullifiers, coins, keys
  │     ├── transfer.rs          TransferRequest, SpentInputs, TransferResult
  │     ├── builder.rs           TransferBuilder + PreparedTransfer + prove, BuildInputs
  │     ├── inputs.rs            PreparedInput: the shielded inputs that the wallet spends
  │     ├── convert.rs           the conversions to and from the ledger's types
  │     ├── balance.rs           WalletBalance / DustBalance / ShieldedBalance
  │     ├── sync.rs              TrackedUtxo, SyncCursors
  │     ├── hd.rs                Seed and its addresses, mnemonic, Role and the BIP-32 role keys
  │     ├── address.rs           parse_shielded_recipient, parse_unshielded_recipient,
  │     │                        parse_dust_address
  │     ├── network.rs           Network: the bech32 HRP suffix, typed
  │     ├── chain_pin.rs         pin a snapshot to a finalized block, to catch a chain swap; SyncSource
  │     └── error.rs             WalletError
  │
  ├── midnight-wallet-facade     the WalletFacade trait, IntoWalletFacade (what
  │                              with_wallet takes), and in builds.rs the
  │                              WalletBuilds trait and ReservedBuild, in
  │                              midnight-types's vocabulary; nothing else
  │
  ├── midnight-wallet            the local implementation; depends on the two crates above
  │     ├── local.rs             LocalWallet: the facade over a locally-owned Wallet
  │     ├── background.rs        BackgroundSync: LocalWallet::sync_in_background resyncs until the handle drops
  │     ├── sync.rs              Wallet::from_seed / from_mnemonic / from_mnemonic_with_passphrase or
  │     │                        WalletBuilder::new → WalletBuilder → sync(source), pinned by default
  │     ├── wallet.rs            Wallet { state }
  │     ├── state.rs             the synced state, and the plan and commit of a resync or a rescan
  │     ├── balance.rs           the balances and the spendable shielded coins
  │     ├── pending.rs           pending reservations
  │     ├── builds.rs            LocalWallet's WalletBuilds impl
  │     ├── transfer.rs          the state as the BuildInputs of a transfer
  │     ├── snapshot.rs          the state files, under the layout of storage.rs
  │     ├── (private)            the reader of the shielded events from before the fork
  │     │                        to ledger 9, in ledger 8 types
  │     └── storage.rs           generation-based atomic persistence
  │
  ├── midnight-private-state     per-contract private state + maintenance signing keys,
  │                              with password-encrypted export/import
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
  └── midnight-helpers           the ledger 9 surface of the upstream node helpers at its
                                 root, plus a ledger_8 module and fork::old_to_new_ser for
                                 the wallet's reader of the shielded events before the fork
                                 (single pinning point for the upstream deps)
```

## Core types at a glance

| Type | Crate | Role |
|---|---|---|
| `MidnightProvider` | provider | Network entry. Holds node URL, the attached wallet (`Arc<dyn WalletFacade>`), the proof backends and the optional private-state store. |
| `Provider` trait | provider | Read-only chain interface; blanket-impl'd for `&T`, `Arc<T>`, `Box<T>`. |
| `Wallet` | wallet | The synced state itself. It runs the indexer replays of a sync, a resync and a rescan. The provider reaches it through `WalletFacade`. |
| `WalletFacade` | wallet-facade | The wallet's API, in `midnight-types`'s vocabulary. Both `midnight-wallet` and `midnight-provider` depend on it; neither the facade nor the provider depends on an implementation. |
| `BackgroundSync` | wallet | The handle of the task that `LocalWallet::sync_in_background` starts. The task resyncs the wallet after each pause. Dropping the handle stops the sync. `last_error()` names the error of the last pass. `wait_until(done)` waits until `done` holds for the balance. It checks at the call and after each pass. |
| `LocalWallet` / `WalletBuilder` | wallet | The facade implemented over a locally-owned `Wallet`, and the builder (`Wallet::from_seed`, `Wallet::from_mnemonic`, `Wallet::from_mnemonic_with_passphrase`, `WalletBuilder::new`) that syncs one from an indexer. |
| `Contract<P>` | contract | Stateless, immutable handle. Holds address + provider; fetches fresh state per call. |
| `DeployBuilder<'_, P>` / `ConnectBuilder<P>` | contract | Typestate builders; `DeployBuilder::send` submits the deploy and returns a `PendingDeploy`. |
| `PendingTx` / `TxInBlock` | provider | Watch handle over `submit_and_watch`; `wait_best` / `wait_finalized`. A wait returns the `TxInBlock` only when the chain applied the transaction; a landed transaction that did not apply fails with `ProviderError::NotApplied`, and other failures carry a typed `SubmitError`. |
| `PendingCall<T>` | contract | A submitted circuit call and its decoded result. `wait_finalized()` waits with no SDK deadline and settles the private-state snapshot by the verdict. A drop leaves the snapshot `Pending`. |
| `Constructor` | contract | A contract's constructor as an `ir::Circuit`, the declarations it calls and the state it starts from. The generated `Contract::deploy` sets it, and the deploy runs it before any proof. |
| `PendingDeploy<P>` | contract | Same as `PendingTx` for deploys, plus `into_contract()`. A wait fails with `TransactionFailed` when the deploy did not apply. `into_contract()` waits for the best block itself when no wait ran, then reads the contract from the node at the block that applied the deploy. One deadline bounds the wait and the read. `wait_finalized` settles the deploy's private-state snapshot as a call's wait does, and a best-block wait leaves it `Pending`. |
| `ProofProvider` | helpers | Proof backend trait (`midnight_helpers::ProofProvider`). Set on the provider as an `Arc<dyn ProofProvider<DefaultDB>>` with `with_proof_provider`. The in-process prover is the default. |
| `RemoteProofServer` | provider | `ProofProvider` that delegates to an HTTP proof server (`/check` + `/prove`). |
| `Builds` | provider | The attached wallet's builds (`midnight_provider::Builds`), from `MidnightProvider::builds`. |

## Provider ↔ Wallet model

The wallet owns the seed, secret keys, synced zswap / dust / unshielded state, ledger parameters, the latest `BlockContext`, and a `PendingReservations` set. It exposes accessors. A build reaches its state, and reserves its inputs, through the facade. The only I/O it drives is the replay phase of a sync, a resync or a shielded rescan, and the network and chain-pin queries to a node. Every build resyncs the wallet first, and when asked, `LocalWallet::sync_in_background` resyncs it in a background task. A sync replays from the indexer at the `indexer_url` it gets. It asks its `SyncSource`, such as the provider, for the network and the chain pin. The wallet keeps that indexer URL for later replays, and the provider answers a resync's pin queries.

Each of those three splits into plan → run → commit, so the replay runs with the wallet free: the plan is snapshotted under a read lock, the replay touches nothing, and the commit takes a write lock. `LocalWallet` composes the three under a mutex of its own, which serializes its resyncs and coin watches. Under one hold of that mutex, a coin watch registers and then rescans. A `Wallet` has no resync of its own: to resync one outside a provider, wrap it in `LocalWallet`.

`MidnightProvider` reaches the wallet through `Arc<dyn WalletFacade>` and never names an implementation. The local one is built on its own and attached:

```
let provider = MidnightProvider::new(node_url)

Wallet::from_mnemonic(phrase)?                      // midnight-wallet; or from_seed(hex),
                                                    // from_mnemonic_with_passphrase(phrase, passphrase), WalletBuilder::new(seed)
      .with_network(Network::Preprod)               // optional: the node's network without it
      .with_storage(dir)                            // optional persistence
      .unpinned()                                   // optional: skip the network check and the chain-reset guard
      .sync(&provider, indexer_url).await           // one-shot sync → Wallet; the source is any SyncSource
    or .sync_with_progress(&provider, indexer_url).await // streaming progress

provider
  .with_wallet(wallet)                              // a synced Wallet, or a WalletFacade that also
                                                    // implements WalletBuilds
  .with_proof_provider(prover)                      // optional, the in-process prover by default
  .resync_wallet().await                            // incremental refresh
  .watch_for_coin(coin).await                       // claim a coin with no usable ciphertext: register, then rescan
  .builds().await                  → Builds (resyncs first)
      b.build_context().await      → Arc<BuildContext> (evicts expired pending)
      b.execution_context().await  → the half a circuit runs against, with no funding view
      b.add_funding(&ctx) / b.build_funded(tx_info) / b.prepare_shielded_inputs(..)
  .transfer_shielded / transfer_unshielded / shielded_swap / register_dust
  .prepare(tx_bytes).await         → PreparedTx (hashes known, not submitted or validated)
  .prepare_reserved(result).await  → PreparedTx that holds the reservation of the build
  .submit(tx_bytes).await          → PendingTx
  .submit_reserved(result).await   → PendingTx that carries the reservation of the build
  .merge_transactions(&[..])       → one transaction from several proven ones
  .balance_transaction(bytes).await → TransferResult: someone else's fee-less transaction, funded
  .balance() / .spendable_shielded_coins()
  .release(&spent)                 // hand back a build's reserved inputs
  .health().await                  → node reachability

Arc<LocalWallet>                                    // a handle on the attached wallet
  .sync_in_background(provider, pause) → BackgroundSync (resyncs after each pause until dropped)
      sync.last_error()            → the error of the last pass, None after a commit
      sync.wait_until(done).await  // wait until done(&WalletBalance) holds: at the call and after each pass
  .watch_for_coins(coins, &provider).await          // the provider's coin watch, on the handle
```

Each node read of the provider, its connect included, ends within 10 s (60 s for a contract state read), and a read with no answer returns `ProviderError::Rpc`.

`transfer_shielded`, `transfer_unshielded` and a generated contract call also offer `.without_dust()`, which stops at a fee-less `DustlessTransaction`. That is the multi-party flow: the contributors build fee-less, one payer merges the halves with `merge_transactions`, covers the fee with `balance_transaction`, then submits with `submit_reserved`. `shielded_swap` is always fee-less, because an unbalanced half cannot fund itself, so it has no `.without_dust()` step. See [`docs/wallet.md`](wallet.md). A contract call's `.build()` and `.without_dust()` record no private state, so they fail with `ContractError::UnrecordedPrivateState` when the call's witnesses change it. The refusal comes after the local run and before the build reserves an input or makes a proof.

`with_network` takes a `Network` (via `impl Into<Network>`). Parse a name with `.parse::<Network>()?`, which refuses a name that is not safe in an address or in the storage path. See [`docs/wallet.md`](wallet.md#address-derivation) for the rule.

`WalletBuilder::sync` replays three indexer subscriptions (zswap ledger events, dust ledger events, unshielded transactions): the zswap and unshielded streams at the same time, then the Dust stream. It returns once all three have caught up. A resync replays all three at the same time. Each subscription keeps its socket alive with a client ping after idle and a hard idle timeout, and transient transport failures reconnect with bounded exponential backoff, resuming from the last applied cursor (a per-connection dedupe keeps re-delivered events from being double-applied). The latest-block query that each sync and resync makes retries on the same bound. A sync that gives up on the indexer returns `WalletError::Indexer`, whose `retryable` field tells whether a later sync can succeed. State is persisted under `{base}/{network}/{sha256(unshielded_address)}/` as `metadata.json` + `zswap-N.bin` + `dust_wallet-N.bin` + `pending.json`, with generation-based atomic writes (binary files first, atomic metadata rename, then old-generation cleanup). The sync persists only with `.with_storage(base)`, and `Wallet::default_storage_dir()` gives `~/.midnight/wallets`. `metadata.json` and `pending.json` also name the ledger generation whose types wrote them (`ledger_version`, ledger 8 when absent). The sync refuses a snapshot whose `metadata.json` names ledger 8 with `WalletError::Ledger8Snapshot`.

`PendingReservations` records the spends of builds that the chain has not yet confirmed. Every build reserves its dust spends, unshielded UTXOs and shielded nullifiers before it proves, under the same hold of the wallet that selected them. Thus a second build in this process cannot pick the same input. A reservation reaches `pending.json` only when its proof succeeds (`WalletFacade::persist_reservation`), so a process that stops during the proof leaves the next process no reservation for those inputs.

A `transfer_*` build reserves its inputs through `prepare_transfer`. A deploy with no shielded offer and a maintenance update reserve theirs through `prepare_funded`. A sponsor's build reserves the Dust for the fee it pays through `prepare_fees`. A call, or a deploy with a shielded offer, reserves its pinned coins through `spend_shielded`. It then reserves the Dust for its own fee through `prepare_fees`.

Reservations clear when event replay (sync or resync) observes the corresponding confirmed spends: a dust batch clears when any of its spend nullifiers appears in a `DustSpendProcessed` event, an unshielded reservation when its `(intent_hash, output_index)` key appears as a spent UTXO. TTL expiry (`evict_expired`, which every funded build runs through the wallet's `add_funding`) remains as a backstop for transactions that never confirm. Each reservation expires at the earliest TTL of the intents that its build made, which `.with_ttl(..)` on a transfer builder makes shorter than the chain's `global_ttl`, and the wallet compares it with the time of the chain's latest block.

`watch_for_coin` covers the shielded coin a wallet owns but cannot discover, because its output carries no ciphertext the wallet can read. It records the coin's commitment (`ZswapLocalState::watch_for`) and then replays `zswapLedgerEvents` from event zero, since a replay that meets an unclaimable output collapses that Merkle leaf and a resync resumes from its cursor. The wallet records and replays under one hold of its sync lock. The replay rebuilds `zswap_state` and its cursor only; dust, unshielded, parameters, and pending reservations are left alone. It re-registers every coin the wallet already holds first, because the ledger consumes a registration when it claims it and a coin recovered by an earlier registration has no ciphertext to be re-found by. A resync keeps each registration that its replay did not claim, because the replay starts from the state that holds them. A registration whose rebuilt `CoinInfo` matches no output rides along on every replay. See [`docs/wallet.md`](wallet.md#recovering-a-coin-the-wallet-cannot-discover).

## Ledger 9

The SDK runs on chains that run ledger 9. The wallet reads the encoding tag of the tip block's ledger parameters and of each ledger event. A tag that this build does not read fails the sync with `WalletError::UnknownLedger`, which names the tag, so a chain that runs ledger 8 fails the first sync. A full contract state read at a block before the fork (`at_block`) fails with `ContractError::StateFetch`, which names the tag `contract-state[v6]`. The lazy field queries read `impact-state-value[v2]`, which has the same tag before and after the fork.

A chain that forked from ledger 8 to ledger 9, such as mainnet, keeps its history from before the fork. The indexer serves those events in the ledger 8 encoding (`event[v9]`), in the same id sequence as the ledger 9 events (`event[v14]`). So the wallet keeps one private reader for the shielded events before the fork:

- A sync or a rescan from event zero reads them with ledger 8 types (`midnight_helpers::ledger_8`), so the wallet keeps the shielded coins that it received before the fork.
- At the first ledger 9 event, or at the end of a stream with no ledger 9 event, the reader converts the state with `midnight_helpers::fork::old_to_new_ser`. The sync then continues in ledger 9.
- The Dust replay skips the ledger 8 Dust events, because the fork emptied the chain's Dust state. The unshielded leg reads JSON in every generation.

A snapshot that a ledger 8 wallet wrote fails the sync with `WalletError::Ledger8Snapshot`, which names the directory to remove. See [`docs/wallet.md`](wallet.md#persistence).

A contract deployed before the fork keeps its `verifier-key[v6]` keys, and ledger 9 verifies a proof against them. The call's key check accepts `verifier-key[v6]` and `verifier-key[v7]`.

## Data flows

### Query state

One node RPC, `midnight_contractState`, serves the whole state. `MidnightProvider` exposes it as `get_state_from_node`, and its `Provider::get_contract_state` forwards to it:

```
Provider.get_contract_state(address, at)              → hex string   // at = None: the best block
MidnightProvider.get_state_from_node(address, at)     → hex string
  → hex decode → tagged_deserialize → ContractState<InMemoryDB>
```

Generated bindings expose this as `contract.ledger().await?`, which calls `midnight_contractState` over node RPC and returns a sync `Ledger` struct with typed field accessors. `contract.ledger_query()` (custom node builds only) routes per-field reads through `midnight_queryContractState` via the `StateQueryProvider` bridge in `midnight-provider`.

### Deploy

```
Contract::deploy(&provider)                              // DeployBuilder<'_, P, Wp>; sets the constructor,
                                                         // or the default state for a contract with none
  [.with_constructor_args(...)]                          // typed; only for a constructor with parameters
  [.with_witnesses(&w)]                                  // only for a contract with witnesses
  [.with_initial_private_state(...)]                     // W::PrivateState; after with_witnesses
  [.with_initial_state(state)]                           // deploy `state` as it is, skip the constructor
  .with_zk_config("compiled")
  [.with_deploy_timeout(...)]

  .send().await   →  PendingDeploy<P>
    ├─ .wait_best().await        → (TxInBlock, PendingDeploy)   // TransactionFailed if not applied
    ├─ .wait_finalized().await   → (TxInBlock, PendingDeploy)   // TransactionFailed if not applied
    └─ .into_contract().await    → Contract<P>
```

Internally:

```
the state: initial_state as it is, or else the constructor's run (neither → Construction)
  ├─ Constructor::check_args                  // a missing argument → Construction
  ├─ provider.shielded_recipient().coin_public_key // ownPublicKey() returns the wallet's coin key
  ├─ Constructor::run with no contract address and at the wall-clock time, with the
  │  witnesses and the initial private state (Interpreter on a failure or a kernel.self())
  └─ refuse a ledger effect or a Zswap input or output (Construction)
  ↓
refuse a private state that is not empty when no store is attached (Construction)
  ↓
populate_verifier_keys(state, zk_config)      // load *.verifier files into state.operations,
                                              // on a blocking thread
  ↓
deploy_funded(state, provider, shielded_offer)
  ├─ ContractState::new(data, ..)             // charged keys of the data, which the ledger rechecks
  ├─ provider.builds().await                  // resync wallet, then its Builds
  ├─ builds.execution_context().await         // build the context
  ├─ builds.proof_provider()                  // the provider's prover
  ├─ build deploy intent
  ├─ no offer: builds.build_funded(tx_info).await  → DeployResult { address, transaction }
  │   ├─ one transition: add the funding view, balance the fee with mock
  │   │  proofs (speculative_spend loop), record what it drew
  │   └─ prove the balanced tx once, for real, with the wallet free
  └─ an offer:                                → DeployResult { address, transaction }
      ├─ builds.add_funding(context), then builds.prepare_shielded_inputs:
      │  the wallet spends and reserves the offer's coins
      ├─ refuse a token the coins do not cover (ShieldedOfferShortfall)
      ├─ guaranteed offer: the coins, the payments, one change output per token
      ├─ prepare_no_validate, then PreparedTransfer::prove: prove with no fee
      ├─ builds.balance_transaction: the Dust fee in its own intent
      └─ the result's spent: the fee Dust, merged with the offer's coins
  ↓
provider.prepare_reserved(transaction).await  → PreparedTx (holds the reservation, knows the extrinsic hash)
  ↓
append_pending(address, extrinsic_hash, None, private_state)   // only for a private state that is not empty
  (fails → release the reservation, PendingSnapshotFailed)
  ↓
prepared.submit().await                       → PendingTx (carries the reservation)
  (fails → SubmissionWait; the snapshot stays Pending)
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
contract.circuits().increment_by(5)                      // IncrementByCallBuilder

  .send().await   →  PendingCall<T>                      // Contract::send_call_with
    ├─ .transaction_hash() / .extrinsic_hash()
    └─ .wait_finalized().await   → CallOutcome<T>        // no SDK deadline
                                                         // value, hashes, and the logs of the run
```

Internally:

```
read the state and the block time at one block (per call):
  state_at_block(address, at_block)           // the at_block pin, else best_block_hash();
                                              // the state through midnight_contractState,
                                              // the time from Timestamp::Now, in whole seconds
  ↓
zk_config.verifier_key(circuit)               // on a blocking thread
  └─ check it against the circuit's operation in the state, before any proof:
       no operation → VerifierKeyMismatch { reason: Missing }, no key → Keyless,
       another key → Mismatched
  ↓
provider.shielded_recipient().coin_public_key // the wallet's coin public key, which ownPublicKey() returns
  ↓
interpreter::execute(ir, program, state, args,
                     Env { witnesses, private_state, address, block_time, coin_public_key })
  → ExecutionResult { state, reads, gather_ops, communication_outputs, result, logs }
  ↓
build verify-ops:
  gather_ops.iter().map(|op| op.clone().translate(|()| reads.next()))
  → filter empty Idx/Ins
  → Vec<Op<ResultModeVerify, InMemoryDB>>
  ↓
provider.builds() → the Builds of the wallet (resyncs first)
  ↓
builds.execution_context()
  ↓
partition_transcripts([PreTranscript { context, program: verify_ops, comm_comm: None }],
                      the chain's ledger parameters from the context)
  // context: the block time and the contract balance that execute ran with
  → (guaranteed_transcripts, fallible_transcripts)
  ↓
resolver::register(address, circuit, key, zk_config) → the key location   // the key the check passed
  ↓
CallAction holding typed transcripts + AlignedValue inputs/outputs
  ↓
builds.add_funding(&context)                                   // the payer joins here
  ↓
builds.prepare_shielded_inputs(..)                             // only if the call pins coins
  └─ spends them into this context and reserves them, as one transition
  ↓
  → StandardTransactionInfo → prepare_no_validate              // fee-less, even when the call pays a fee
  → PreparedTransfer::prove
  ↓
if pay_fees: builds.balance_transaction(bytes)
  └─ the wallet draws the Dust and reserves it as one transition (prepare_fees),
     then the fee is proved on its own and merged in at its own intent segment,
     so the circuit proof is not redone.
  ↓
TransferResult: the merged bytes, and the pinned coins and the fee Dust in spent
  ↓
provider.prepare_reserved(result) → PreparedTx            // the extrinsic hash, before the submit
  ↓
decode the typed return value from ExecutionResult.result
  └─ an error: PreparedTx::release hands the inputs back
  ↓
append the Pending private-state snapshot, if the witnesses changed the state
  └─ an error: PreparedTx::release, then PendingSnapshotFailed
  ↓
prepared.submit().await → PendingTx → PendingCall<T>      // .send() returns here
  └─ SubmitRpc: SubmissionWait, the snapshot stays Pending, and the inputs stay reserved
  ↓
PendingCall::wait_finalized → PendingTx::wait_finalized   // no SDK deadline; Contract::call_with bounds the PendingTx wait by DEFAULT_TX_FINALIZE_TIMEOUT
  ↓
settle the result of the wait:
  ├─ Ok: confirm the snapshot, if the call recorded one
  └─ NotApplied (PartialSuccess or Failure): mark_failed that snapshot, then TransactionFailed
  ↓
CallOutcome<T> → caller
```

`Contract<P>` is not mutated; the new state is discarded because the next call will fetch fresh state anyway.

### Transfer (shielded / unshielded / swap / register dust)

Each of the four provider methods is a *sync constructor* that returns a builder type (`ShieldedTransfer<'a>`, `UnshieldedTransfer<'a>`, `ShieldedSwap<'a>`, `DustRegistration<'a>`). The builder defers all work until the caller calls `.send()` or `.build()`, or awaits a `ShieldedSwap`, which builds a `DustlessTransaction` and submits nothing:

```
provider.transfer_shielded(token_type, amount, recipient)       // a typed recipient; no work yet
        .transfer_unshielded(token_type, amount, recipient)
        .shielded_swap(give_token, give_amount, receive_token, receive_amount)
        .register_dust(utxo_ctime)                              // .to(dust_key) sends the Dust to another wallet

  ↓ .send().await? (or .build().await? for the no-submit escape hatch)

provider.builds()                    // resync_wallet, then the wallet's builds
  ↓
WalletBuilds::prepare_transfer(request, proof_provider)   // one hold of the wallet
  build_context_inner (execution context + add_funding, which evicts expired pending)
  TransferBuilder::prepare(request)
    └─ select inputs from wallet's local state
    └─ balance Dust fees (speculative_spend loop, mock proofs only)
    └─ prepare_no_validate
  reserve_pending(dust_batches, spent_unshielded_inputs, shielded, reserved_at, expires_at)
  → ReservedBuild
  ↓
(wallet released)    PreparedTransfer::prove       // the only real proving
  → TransferResult { tx_bytes, spent, fee_speck }
  WalletFacade::persist_reservation(spent)          // the reservation reaches pending.json
  ↓
(.send() path only)  provider.submit_reserved(result).await → PendingTx
```

`.send()` returns `PendingTx`; the caller then chooses `wait_best` / `wait_finalized`. `.build().await` stops before submitting and returns `TransferResult`, which the caller can submit with `submit_reserved`. Reservations clear during the next sync/resync, when event replay observes the confirmed spends, or get evicted on TTL expiry the next time a funded build runs `add_funding`.

## Transaction submission

One auto-reconnecting websocket carries everything the node serves: raw Substrate and `midnight_*` RPCs through subxt's `RpcClient`, and the calls that need runtime metadata (submission, and storage reads such as `MidnightProvider::get_block_timestamp`) through the `OnlineClient` built on the same transport. The connection is opened on first use and cached for the provider's lifetime.

`MidnightProvider::submit` wraps `tx_bytes` as an unsigned `Midnight::send_mn_transaction` extrinsic, calls `submit_and_watch`, and hands back:

- `PendingTx` — owns the watch stream.
  - `extrinsic_hash() → ExtrinsicHash`, `extrinsic_hash_hex() → String`
  - `transaction_hash() → TransactionHash`: the ledger's own identity for the tx
  - `wait_best(self) → Result<(TxInBlock, Self), _>`: consumes & returns self; its verdict is provisional
  - `wait_finalized(self) → Result<(TxInBlock, Self), _>`: same; may be called without prior `wait_best`; its verdict is final
  - both return `Ok` only for `Success`, and `ProviderError::NotApplied` for any other verdict
- `TxInBlock { block_hash, extrinsic_hash, transaction_hash, verdict }`
- `NotApplied(TxInBlock)`: a transaction that landed but did not apply
- `Verdict`: `Success` (`TxApplied`), `PartialSuccess` (`TxPartialSuccess`: the guaranteed phase committed, a fallible segment did not), or `Failure`.

Both `wait_*` methods return `self` so callers re-bind without `let mut`. Cancelling a future is safe but does not retract the extrinsic from the mempool. Failures with no verdict surface as `ProviderError::Submission(SubmitError)`; the variant tells the caller whether resubmitting is safe (`Invalid`: definitive rejection; `NotSubmitted`: never left the process) or risks a double spend (`Dropped` / `NodeError`: the tx may still land) or is a wait/decode issue that leaves the tx in flight (`WatchStream`: transport-only; `VerdictFetch`: landed but events undecodable; re-query the chain rather than resubmit). `SubmitRpc` does not say whether the node holds the tx, so a resubmit after it also risks a double spend.

`MidnightProvider::prepare` stops one step earlier: it builds the unsigned `send_mn_transaction` extrinsic locally from the node's metadata and returns a `PreparedTx` whose extrinsic hash is already known, so a caller can durably record state keyed by that hash before the transaction reaches the mempool. The metadata checks only the call's shape, so the node validates the transaction only at submit. `PreparedTx::submit` then hands back the same `PendingTx`.

`MidnightProvider::prepare_reserved` prepares a `TransferResult`, and the `PreparedTx` holds the reservation of its build. A failed prepare hands the inputs back before it returns. A `PreparedTx` that drops before its submit hands them back too, and `PreparedTx::release` does the same and waits for it. `PreparedTx::submit` moves the reservation to the `PendingTx`, so a wait that reads `Invalid` hands the inputs back. A failed submit keeps them reserved.

`MidnightProvider::submit_reserved` submits a `TransferResult` and carries the reservation of its build on the `PendingTx`. A failed prepare (the connect, the metadata read or the encoding) hands the inputs back before it returns, whatever the error, because nothing reached the node. A wait that reads `Invalid`, which can arrive long after the builder returned, hands them back too. Any other failure keeps them reserved until their TTL. The transfers, `register_dust` and a maintenance update submit this way, and so does a caller with the result of a `.build()` or of `balance_transaction`. A contract call and a deploy prepare with `prepare_reserved`, because each records its private-state snapshot between the prepare and the submit. So an error before the submit, or an `Invalid` at the wait, hands their inputs back. The build-only calls, `.build()` and `.without_dust()` of a call builder, return bytes that do not name their inputs. Those inputs stay reserved until a sync sees the chain spend them, or until their TTL passes.

## Block pinning

A contract handle pins its reads with `Contract::at(..).at_block(hash)`, and the hash is a node block hash. Both paths that honour it are node RPCs: `midnight_contractState` for a whole `ContractState`, and `midnight_queryContractState` for a lazy per-field read. Neither accepts a height, so a caller holding a height resolves it to a hash first (`MidnightProvider::get_block_hashes_by_height`). A circuit call on a pinned handle also reads the block time (`Timestamp::Now`) at the pin, so its clock checks run at the time of that block.

`Provider::get_contract_state` reads through `midnight_contractState` too, and takes the same optional hash. The generated `Ledger::from_provider(provider, address)` constructor passes none, so it reads at the best block.

## External dependencies

| Crate | Source | Purpose |
|---|---|---|
| `midnight-ledger-v9` (+ its companion crates, and the `midnight-transient-crypto` 2.2 it also uses) | `RomarQ/midnight-ledger`, branch `rq/midnight-rs-ledger-9` | Ledger 9: transaction types, VM, proving, and the on-chain runtime the Compact side reads |
| `midnight-ledger` 8.1.2 (+ its `midnight-zswap`, `midnight-onchain-*`) | `RomarQ/midnight-ledger`, branch `rq/midnight-rs-ledger-8` | Ledger 8 types, which the wallet's reader of the shielded events before the fork uses, and which the helpers link |
| `midnight-serialize`, `midnight-base-crypto`, `midnight-storage` | one instance, from the ledger 9 branch | The tagged codec, `HashOutput`, `AlignedValue`, `Sp` |
| `midnight-ledger-unsafe-helpers` (+ the `midnight-node-ledger-helpers` it builds on) | `RomarQ/midnight-node`, branch `rq/midnight-rs-toolkit-2.1.0-rc.3` | A `ledger_9` module: the type surface, and the transaction and wallet builders. A `ledger_8` module for the wallet's reader |
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
| `dust-and-fees.md` | Dust token model, fee balancing, Dust after the fork to ledger 9 |
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
| Cross-contract calls | Bindings generate for a circuit that calls another contract, but the interpreter refuses the call with `InterpreterError::Unsupported` |
| Lazy query batching | `query_contract_state` already takes a batch, but each generated `ledger_query()` accessor sends one query of its own |
| Production proving | `ProofProvider::prove` returns `Transaction`, not `Result`, so a backend signals failure by panicking. The SDK catches that unwind and reports `WalletError::Proving` rather than losing the task, but the trait has no error channel and this is not mainnet-ready |
| A remote signer | The facade lets a different local wallet stand in, and it asks for no seed. But a build signs inside `prepare_transfer`, so a browser extension or an HSM needs it to return something unsigned plus a separate signing step |
