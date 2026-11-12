# midnight-rs — Architecture

Rust SDK for the Midnight blockchain. Covers the full contract lifecycle (deploy, query, call circuits, prove, submit) plus wallet management for the chain's three asset legs (zswap-shielded coins, Dust fee tokens, unshielded UTXOs).

## Workspace

All crates live under `crates/`.

```
midnight-core                    meta-crate; re-exports the public API
  ├── midnight-contract          contract lifecycle (deploy / call / prove / submit)
  │     ├── contract.rs          Contract<P>, DeployBuilder, ConnectBuilder, PendingDeploy
  │     ├── constructor.rs       Constructor: the constructor run of a deploy
  │     ├── call.rs              circuit-call tx builder, the coins a call pins
  │     ├── deploy.rs            deploy tx builder
  │     ├── maintenance.rs       verifier-key rotation, authority replacement
  │     ├── state.rs             state fetch (node RPC) + deserialization
  │     ├── zk_config.rs         ZkConfigProvider: where prover/verifier keys come from
  │     ├── offer.rs             ShieldedOffer: coins and payments a deploy carries
  │     ├── ledger/              (private) the ledger 9 build of a call, deploy and
  │     │     │                  maintenance update
  │     │     ├── call.rs        a circuit call's transaction: transcripts, offers, proof, the wallet's cover
  │     │     ├── deploy.rs      a deploy's transaction
  │     │     ├── maintenance.rs a maintenance update's transaction
  │     │     ├── resolver.rs    the resolver that contract calls prove with
  │     │     └── state.rs       a circuit's operation, from the verifier key of its artifact
  │     └── interpreter / runtime  re-exports of compact-interpreter and compact-runtime
  │
  ├── midnight-provider          network entrypoint; wallet reached only through MidnightWallet
  │     ├── MidnightProvider<W>  prepare, send_tx_envelope, the node reads
  │     ├── pipeline.rs          send_transaction, fill: fill from node queries, then fund, prove
  │     │                        and sign; merge_transactions
  │     ├── prover.rs            Prover: local, remote, or custom (test-util)
  │     ├── remote_prover.rs     the HTTP proof server client of Prover::remote
  │     ├── proving.rs           its ProofProvider impl
  │     ├── submit.rs            PendingTx, PreparedTx, TransactionReceipt, Verdict, NotApplied
  │     └── (deps) midnight-types, subxt (node RPC)
  │
  ├── midnight-types       implementation-free vocabulary and toolkit; a function of
  │     │                        midnight-helpers + H256 + BIP-32/BIP-39
  │     ├── wallet.rs            MidnightWallet: the trait a provider funds and signs through
  │     ├── stage.rs             FilledTx, FundedTx, ProvenTx, TxEnvelope
  │     ├── request.rs           TransactionRequest: what a provider fills
  │     ├── build.rs             (hidden) the toolkit the SDK crates share: Reservation,
  │     │                        Fill, fill, ContractPart, MaybeWallet, and the re-exports below
  │     ├── coin.rs              token types, nullifiers, coins, keys
  │     ├── transfer.rs          SpentInputs
  │     ├── sign.rs              SignatureRequest, ProvenTx::{signature_requests, add_signature}
  │     ├── builder.rs           TransferBuilder + PreparedTransfer + prove, Funding
  │     ├── inputs.rs            PreparedInput: the shielded inputs that the wallet spends
  │     ├── convert.rs           the conversions to and from the ledger's types
  │     ├── balance.rs           WalletBalance / DustBalance / ShieldedBalance
  │     ├── sync.rs              TrackedUtxo
  │     ├── hd.rs                Seed and its addresses, mnemonic, Role and the BIP-32 role keys
  │     ├── address.rs           parse_shielded_recipient, parse_unshielded_recipient,
  │     │                        parse_dust_address
  │     ├── network.rs           Network: the bech32 HRP suffix, typed
  │     ├── chain_pin.rs         pin a snapshot to a finalized block, to catch a chain swap; SyncSource
  │     └── error.rs             WalletError
  │
  ├── midnight-wallet            the local implementation; depends on midnight-types
  │     ├── local.rs             Wallet: the identity, and the synced state behind its own locks
  │     ├── background.rs        BackgroundSync: Wallet::sync_in_background syncs until the handle drops
  │     ├── sync.rs              Wallet::from_seed(seed) → WalletBuilder → sync(source), pinned
  │     ├── wallet.rs            WalletState { state }, and the sync that builds it
  │     ├── state.rs             the synced state, and the plan and commit of a resync or a rescan
  │     ├── balance.rs           the balances and the spendable shielded coins
  │     ├── pending.rs           pending reservations, in a book behind a std mutex
  │     ├── builds.rs            Wallet's MidnightWallet impl
  │     ├── keys.rs              RoleKeys: the keys that a sync derives from the seed
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
  │     └── midnight-typed-state    accessors, nav
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
| `MidnightProvider<W>` | provider | Network entry. Holds node URL, the attached wallet `W` (or `()` with none), the proof backends and the optional private-state store. It calls the wallet only through `MidnightWallet`. |
| `Wallet` | wallet | One synced wallet, not `Clone`. It locks its own state, so a `&Wallet` serves every call and an `Arc<Wallet>` shares it. It runs the indexer replays of a sync and a rescan. It implements `MidnightWallet`. |
| `MidnightWallet` | types | The trait that a provider funds and signs through: `shielded_recipient`, `fund_transaction`, `sign_transaction`. Implemented for `&T` and `Arc<T>`. Both `midnight-wallet` and `midnight-provider` depend on `midnight-types`, and neither depends on the other. |
| `FilledTx` / `FundedTx` / `ProvenTx` / `TxEnvelope` | types | The stages of a transaction: filled by the provider, funded by the wallet, proven by the provider, signed by the wallet. Each from `FundedTx` on carries the reservation of what the wallet spends, and a drop of an unsent one hands it back. |
| `BackgroundSync` | wallet | The handle of the task that `Wallet::sync_in_background` starts. The task syncs the wallet after each pause. Dropping the handle stops the sync. `last_error()` names the error of the last pass. `wait_until(done)` waits until `done` holds for the balance. It checks at the call and after each pass. |
| `WalletBuilder` | wallet | The builder that `Wallet::from_seed(seed)` starts and that syncs a `Wallet` from an indexer. |
| `Contract<P>` | contract | Stateless, immutable handle. Holds address + provider; fetches fresh state per call. |
| `DeployBuilder<'_, P>` / `ConnectBuilder<P>` | contract | Typestate builders; `DeployBuilder::send` submits the deploy and returns a `PendingDeploy`. |
| `TransactionRequest` | types | What to build: `shielded`, `unshielded`, `shielded_swap`, `register_dust`, `register_dust_to` or `sponsor`, adjusted with `without_fee`, `with_ttl` and `with_coin_selection`. `send_transaction` and `fill` take it. Not `Clone`. |
| `PendingTx` / `TransactionReceipt` | provider | Watch handle over `submit_and_watch`; `get_receipt` waits for finality. It returns the `TransactionReceipt` only when the chain applied the transaction; a landed transaction that did not apply fails with `ProviderError::NotApplied`, and other failures carry a typed `SubmitError`. |
| `PendingCall<T>` | contract | A submitted circuit call and its decoded result. `get_receipt()` waits with no SDK deadline and settles the private-state snapshot by the verdict. A drop leaves the snapshot `Pending`. |
| `Constructor` | contract | A contract's constructor as an `ir::Circuit`, the declarations it calls and the state it starts from. The generated `Contract::deploy` sets it, and the deploy runs it before any proof. |
| `PendingDeploy<P>` | contract | Same as `PendingTx` for deploys, plus `into_contract()`. A wait fails with `TransactionFailed` when the deploy did not apply. `into_contract()` waits for the best block itself when no `get_receipt` ran, then reads the contract from the node at the block that applied the deploy. One deadline bounds the wait and the read. `get_receipt` settles the deploy's private-state snapshot as a call's wait does, and the best-block wait of `into_contract` leaves it `Pending`. |
| `Prover` | provider | Where the provider proves: `Prover::local()` (the default) or `Prover::remote(url)`, an HTTP proof server (`/check` + `/prove`). Set with `with_prover`. `Prover::custom` takes any `midnight_helpers::ProofProvider` behind the `test-util` feature. |

## Provider ↔ Wallet model

The wallet owns the keys of each role, synced zswap / dust / unshielded state, ledger parameters, the latest `BlockContext`, and a `PendingReservations` set. It exposes accessors. A provider reaches it only through `MidnightWallet`: the wallet funds a filled transaction and reserves its inputs, and it signs a proven one. The only I/O it drives is the replay phase of a sync, a resync or a shielded rescan, and the network and chain-pin queries to a node. Every funding resyncs the wallet first, and when asked, `Wallet::sync_in_background` syncs it in a background task. A sync derives the keys from the seed, and the seed then drops. It replays from the indexer at the `indexer_url` it gets. It asks its `SyncSource`, such as the provider, for the network and the chain pin. The wallet keeps that indexer URL for later replays, and the provider answers a resync's pin queries.

Each of those three splits into plan → run → commit, so the replay runs with the wallet free: the plan is snapshotted under a read lock, the replay touches nothing, and the commit takes a write lock. `Wallet` composes the three under a mutex of its own, which serializes its syncs and coin watches. Under one hold of that mutex, a coin watch registers and then rescans. Outside a provider, `wallet.sync(&provider)` syncs it.

`MidnightProvider<W>` holds the wallet `W` that it gets and never names an implementation. The local one is built on its own and attached:

```
let provider = MidnightProvider::new(node_url)

Wallet::from_seed(seed)                             // midnight-wallet; Seed::from_hex, Seed::from_mnemonic or str::parse
      .with_network(Network::Preprod)               // optional: the node's network without it
      .with_storage(dir)                            // optional persistence
      .sync(&provider, indexer_url).await           // one-shot sync → Wallet; the source is any SyncSource
    or .sync_with_progress(&provider, indexer_url).await // streaming progress

provider
  .with_wallet(&wallet)                             // a &Wallet, an Arc<Wallet>, a Wallet, or another
                                                    // MidnightWallet → MidnightProvider<W>
  .with_prover(Prover::remote(url))                 // optional, Prover::local() by default
  .wallet()                        → &W, the wallet it received
  .send_transaction(request).await → PendingTx: fill, fund, prove, sign and send
  .fill(request).await             → TxEnvelope: fill, fund, prove and sign; send nothing
  .prepare(envelope).await         → PreparedTx that holds the reservation (hashes known, not sent)
  .send_tx_envelope(envelope).await → PendingTx that carries the reservation of the envelope
  .health().await                  → node reachability

merge_transactions(&[..])          → one transaction from several proven ones (a free function)

wallet                                              // the wallet itself, or a handle on it
  .balance() / .spendable_shielded_coins()          // the state of the last sync
  .sync(&provider).await                            // one sync; a funding runs one first
  .sync_in_background(provider, pause) → BackgroundSync (syncs after each pause until dropped; an Arc<Wallet>)
      sync.last_error()            → the error of the last pass, None after a commit
      sync.wait_until(done).await  // wait until done(&WalletBalance) holds: at the call and after each pass
  .watch_for_coins(coins, &provider).await          // claim coins with no usable ciphertext: register, then rescan
```

Each build that the wallet funds runs one pipeline. The provider fills the transaction from node queries: its network, its ledger parameters, and the time of its best block, which the TTL counts from. The wallet's `fund_transaction` resyncs, then selects, assembles, balances the Dust fee with mock proofs and reserves, under one write lock. The funding puts a placeholder signature in each slot of the wallet's unshielded inputs and Dust registrations, so the fee counts the slot. The provider proves the result with its prover, with the wallet free. The wallet's `sign_transaction` then replaces each placeholder with a signature over the proven intent, and keeps the reservation across a restart. It signs only for unshielded inputs that the transaction reserved. The provider sends.

Each node read of the provider, its connect included, ends within 10 s (60 s for a contract state read), and a read with no answer returns `ProviderError::Rpc`.

A transfer request made `without_fee`, and a generated contract call's `.without_fee()`, give a fee-less `TxEnvelope`. That is the multi-party flow: the contributors fill fee-less and hand over their bytes (`TxEnvelope::into_bytes`), and one payer merges them with `merge_transactions` and sends the result with `TransactionRequest::sponsor`, which pays the fee. A `shielded_swap` half is always fee-less, because an unbalanced half cannot fund itself. `send_transaction` refuses a fee-less request before its fill, and `send_tx_envelope` and `prepare` refuse a fee-less envelope, because the node refuses them. See [`docs/wallet.md`](wallet.md). A contract call's `.without_fee()` records no private state, so it fails with `ContractError::UnrecordedPrivateState` when the call's witnesses change it. The refusal comes after the local run and before the fill reserves an input or makes a proof.

`with_network` takes a `Network` (via `impl Into<Network>`). Parse a name with `.parse::<Network>()?`, which refuses a name that is not safe in an address or in the storage path. See [`docs/wallet.md`](wallet.md#address-derivation) for the rule.

`WalletBuilder::sync` replays three indexer subscriptions (zswap ledger events, dust ledger events, unshielded transactions): the zswap and unshielded streams at the same time, then the Dust stream. It returns once all three have caught up. A resync replays all three at the same time. Each subscription keeps its socket alive with a client ping after idle and a hard idle timeout, and transient transport failures reconnect with bounded exponential backoff, resuming from the last applied cursor (a per-connection dedupe keeps re-delivered events from being double-applied). The latest-block query that each sync and resync makes retries on the same bound. A sync that gives up on the indexer returns `WalletError::Indexer`, whose `retryable` field tells whether a later sync can succeed. State is persisted under `{base}/{network}/{sha256(unshielded_address)}/` as `metadata.json` + `zswap-N.bin` + `dust_wallet-N.bin` + `pending.json`, with generation-based atomic writes (binary files first, atomic metadata rename, then old-generation cleanup). The sync persists only with `.with_storage(base)`, and `Wallet::default_storage_dir()` gives `~/.midnight/wallets`. `metadata.json` and `pending.json` also name the ledger generation whose types wrote them (`ledger_version`, ledger 8 when absent). The sync refuses a snapshot whose `metadata.json` names ledger 8 with `WalletError::Ledger8Snapshot`.

`PendingReservations` records the spends of builds that the chain has not yet confirmed. Every funding reserves its dust spends, unshielded UTXOs and shielded nullifiers before the proof, under the same hold of the wallet that selected them. Thus a second build in this process cannot pick the same input. A reservation reaches `pending.json` only when `sign_transaction` marks it proven, so a process that stops during the proof leaves the next process no reservation for those inputs. The set sits in a book behind a std mutex, apart from the wallet's state lock, and the `Reservation` that a funding makes holds the book weakly, so a drop of an unsent stage hands the inputs back with no async runtime.

A `transfer_*` build fills a transfer, which the wallet selects and assembles. A deploy with no shielded offer and a maintenance update fill an assembled transaction, whose fee the wallet pays in its intent. A sponsor fills a sealed transaction with `Cover::Fee`, and the wallet pays only its fee. A call, or a deploy with a shielded offer, proves first. It then fills a sealed transaction with `Cover::Shortfalls`, and the wallet covers its shielded deficits and its fee in a part of its own, in one reservation.

Reservations clear when event replay (sync or resync) observes the corresponding confirmed spends: a dust batch clears when any of its spend nullifiers appears in a `DustSpendProcessed` event, an unshielded reservation when its `(intent_hash, output_index)` key appears as a spent UTXO. TTL expiry (`evict_expired`, which every funded build runs through the wallet's `funding`) remains as a backstop for transactions that never confirm. Each reservation expires at the earliest TTL of the intents that its build made, which `TransactionRequest::with_ttl` makes shorter than the chain's `global_ttl`, and the wallet compares it with the time of the chain's latest block.

`Wallet::watch_for_coins` covers the shielded coin a wallet owns but cannot discover, because its output carries no ciphertext the wallet can read. It records the coin's commitment (`ZswapLocalState::watch_for`) and then replays `zswapLedgerEvents` from event zero, since a replay that meets an unclaimable output collapses that Merkle leaf and a resync resumes from its cursor. The wallet records and replays under one hold of its sync lock. The replay rebuilds `zswap_state` and its cursor only; dust, unshielded, parameters, and pending reservations are left alone. It re-registers every coin the wallet already holds first, because the ledger consumes a registration when it claims it and a coin recovered by an earlier registration has no ciphertext to be re-found by. A resync keeps each registration that its replay did not claim, because the replay starts from the state that holds them. A registration whose rebuilt `CoinInfo` matches no output rides along on every replay. See [`docs/wallet.md`](wallet.md#recovering-a-coin-the-wallet-cannot-discover).

## Ledger 9

The SDK runs on chains that run ledger 9. The wallet reads the encoding tag of the tip block's ledger parameters and of each ledger event. A tag that this build does not read fails the sync with `WalletError::UnknownLedger`, which names the tag, so a chain that runs ledger 8 fails the first sync. A full contract state read at a block before the fork (`at_block`) fails with `ContractError::StateFetch`, which names the tag `contract-state[v6]`.

A chain that forked from ledger 8 to ledger 9, such as mainnet, keeps its history from before the fork. The indexer serves those events in the ledger 8 encoding (`event[v9]`), in the same id sequence as the ledger 9 events (`event[v14]`). So the wallet keeps one private reader for the shielded events before the fork:

- A sync or a rescan from event zero reads them with ledger 8 types (`midnight_helpers::ledger_8`), so the wallet keeps the shielded coins that it received before the fork.
- At the first ledger 9 event, or at the end of a stream with no ledger 9 event, the reader converts the state with `midnight_helpers::fork::old_to_new_ser`. The sync then continues in ledger 9.
- The Dust replay skips the ledger 8 Dust events, because the fork emptied the chain's Dust state. The unshielded leg reads JSON in every generation.

A snapshot that a ledger 8 wallet wrote fails the sync with `WalletError::Ledger8Snapshot`, which names the directory to remove. See [`docs/wallet.md`](wallet.md#persistence).

A contract deployed before the fork keeps its `verifier-key[v6]` keys, and ledger 9 verifies a proof against them. The call's key check accepts `verifier-key[v6]` and `verifier-key[v7]`.

## Data flows

### Query state

One node RPC, `midnight_contractState`, serves the whole state. `MidnightProvider` exposes it as `get_contract_state`:

```
MidnightProvider.get_contract_state(address, at)      → hex string   // at = None: the best block
  → hex decode → tagged_deserialize → ContractState<InMemoryDB>
```

Generated bindings expose this as `contract.ledger().await?`, which calls `midnight_contractState` over node RPC and returns a sync `Ledger` struct with typed field accessors.

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
    ├─ .get_receipt().await      → (TransactionReceipt, PendingDeploy)   // TransactionFailed if not applied
    └─ .into_contract().await    → Contract<P>
```

Internally:

```
the state: initial_state as it is, or else the constructor's run (neither → Construction)
  ├─ Constructor::check_args                  // a missing argument → Construction
  ├─ provider.wallet().shielded_recipient().coin_public_key // ownPublicKey() returns the wallet's coin key
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
  ├─ provider.transaction_info().await        // hidden: node queries for the network,
  │                                           // get_ledger_parameters and the best block's time,
  │                                           // which the TTL counts from; the provider's prover
  ├─ build deploy intent
  └─ provider.fill(TransactionRequest::contract(part)).await   → DeployResult { address, transaction: TxEnvelope }
      ├─ no offer: ContractPart::Assembled
      │   ├─ fund_transaction: resync, then one write lock adds the funding view,
      │   │  balances the fee with mock proofs (speculative_spend loop) and
      │   │  reserves what it drew
      │   ├─ prove the balanced tx once, for real, with the wallet free
      │   └─ sign_transaction: the reservation reaches pending.json
      └─ an offer: ContractPart::Covered { pins: the offer's coins, .. }
          ├─ guaranteed offer: the payments
          ├─ the fill proves the deploy with no fee
          ├─ a sealed fill with Cover::Shortfalls: fund_transaction spends the coins,
          │  selects more for what they leave (InsufficientShielded when the wallet
          │  holds too little), adds the change and the Dust fee in a part of its own,
          │  and reserves it
          └─ the part proves on its own and merges in, and sign_transaction keeps its reservation
  ↓
provider.prepare(transaction).await           → PreparedTx (holds the reservation, knows the extrinsic hash)
  ↓
append_pending(address, extrinsic_hash, None, private_state)   // only for a private state that is not empty
  (fails → the PreparedTx drops, which releases the reservation; PendingSnapshotFailed)
  ↓
prepared.send().await                         → PendingTx (carries the reservation)
  (fails → SubmissionWait; the snapshot stays Pending)
  ↓
into_contract
  ├─ one deadline: deploy_timeout from the start of into_contract
  ├─ the receipt of get_receipt, or a best-block wait under the deadline
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
    └─ .get_receipt().await      → CallOutcome<T>        // no SDK deadline
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
provider.wallet().shielded_recipient().coin_public_key // the wallet's coin public key, which ownPublicKey() returns
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
provider.transaction_info()                   // hidden: node queries for the network,
                                              // get_ledger_parameters and the best block's time,
                                              // which the TTL counts from; the provider's prover
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
  → ContractPart::Covered { tx_info, pins }
  ↓
provider.fill(TransactionRequest::contract(part))   // [.without_fee()] for the call builder's without_fee
  → the fill proves the call with no fee, even when the call pays a fee
  ↓
a sealed fill with Cover::Shortfalls { pins, pay_fees }
  └─ fund_transaction resyncs, then under one write lock: for each negative
     shielded (token, segment) entry of the call's balance, the wallet spends
     the pinned coins first, then selects coins, adds the change at that
     segment, and with pay_fees the Dust fee in an intent of its own, and
     reserves what it spends. The provider proves the part on its own and
     merges it in, so the circuit proof is not redone. sign_transaction then
     keeps the reservation across a restart.
  ↓
TxEnvelope: the merged bytes, and the reservation of the part's coins and Dust
  ↓
provider.prepare(envelope) → PreparedTx                   // the extrinsic hash, before the submit
  ↓
decode the typed return value from ExecutionResult.result
  └─ an error: the PreparedTx drops, which hands the inputs back
  ↓
append the Pending private-state snapshot, if the witnesses changed the state
  └─ an error: the PreparedTx drops, then PendingSnapshotFailed
  ↓
prepared.send().await → PendingTx → PendingCall<T>        // .send() returns here
  └─ SubmitRpc: SubmissionWait, the snapshot stays Pending, and the inputs stay reserved
  ↓
PendingCall::get_receipt → PendingTx::get_receipt         // no SDK deadline; Contract::call_with bounds the PendingTx wait by DEFAULT_TX_FINALIZE_TIMEOUT
  ↓
settle the result of the wait:
  ├─ Ok: confirm the snapshot, if the call recorded one
  └─ NotApplied (PartialSuccess or Failure): mark_failed that snapshot, then TransactionFailed
  ↓
CallOutcome<T> → caller
```

`Contract<P>` is not mutated; the new state is discarded because the next call will fetch fresh state anyway.

### Transfer (shielded / unshielded / swap / register dust / sponsor)

A `TransactionRequest` names the transaction as data: a constructor, then the modifiers `without_fee`, `with_ttl` and `with_coin_selection`. The provider needs a wallet (`W: MidnightWallet`) to fill it. `send_transaction` fills and sends; `fill` stops at the signed `TxEnvelope`:

```
TransactionRequest::shielded(token_type, amount, recipient)      // a typed recipient; no work yet
                    unshielded(token_type, amount, recipient)
                    shielded_swap(give_token, give_amount, receive_token, receive_amount)   // fee-less
                    register_dust() / register_dust_to(dust_key)
                    sponsor(tx_bytes)                            // pay the fee of another party's tx
  [.without_fee()] [.with_ttl(ttl)] [.with_coin_selection(CoinSelection)]

  ↓ provider.send_transaction(request).await? (or provider.fill(request).await? to stop before the send)

build::fill(request, chain_context, prover)   // refuse without_fee on register_dust and sponsor,
                                     // and a TTL on sponsor; then node queries for the network,
                                     // the ledger parameters and the best block's time
  ↓
MidnightWallet::fund_transaction(chain, filled)   // the wallet's sync, then one hold of the wallet
  funding (the funding view, with the wallet's Dust time; evicts expired pending)
  TransferBuilder::prepare(request), or for a sponsor balance_external with Cover::Fee
    └─ select inputs from the funding view
    └─ a placeholder signature for each unshielded input and Dust registration
    └─ balance Dust fees (speculative_spend loop, mock proofs only)
    └─ prepare_no_validate
  reserve_pending(dust_nullifiers, spent_unshielded_inputs, shielded, reserved_at, expires_at)
  → FundedTx, which carries the Reservation
  ↓
(wallet released)    prove with the provider's prover      // the only real proving
  → ProvenTx
  ↓
MidnightWallet::sign_transaction(proven)          // a signature for each placeholder, then
                                                  // the reservation reaches pending.json
  → TxEnvelope                                    // marked fee-less for without_fee or a swap half
  ↓
(send_transaction only)  provider.send_tx_envelope(envelope).await → PendingTx
```

`send_transaction` returns `PendingTx`; the caller then waits with `get_receipt`. `fill` stops before the send and returns the `TxEnvelope`, which the caller can send with `send_tx_envelope`. Reservations clear during the next sync/resync, when event replay observes the confirmed spends, or get evicted on TTL expiry the next time a funded build runs `funding`.

## Transaction submission

One auto-reconnecting websocket carries everything the node serves: raw Substrate and `midnight_*` RPCs through subxt's `RpcClient`, and the calls that need runtime metadata (submission, and storage reads such as `MidnightProvider::get_block_timestamp`) through the `OnlineClient` built on the same transport. The connection is opened on first use and cached for the provider's lifetime.

`MidnightProvider::send_tx_envelope` wraps the bytes of a `TxEnvelope` as an unsigned `Midnight::send_mn_transaction` extrinsic, calls `submit_and_watch`, and hands back:

- `PendingTx` — owns the watch stream.
  - `extrinsic_hash() → ExtrinsicHash`; format it with `{:x}` for its hex digest
  - `transaction_hash() → TransactionHash`: the ledger's own identity for the tx
  - `get_receipt(self) → Result<TransactionReceipt, _>`: waits for finality; its verdict is final
  - it returns `Ok` only for `Success`, and `ProviderError::NotApplied` for any other verdict
- `TransactionReceipt { block_hash, extrinsic_hash, transaction_hash, verdict }`
- `NotApplied(TransactionReceipt)`: a transaction that landed but did not apply
- `Verdict`: `Success` (`TxApplied`), `PartialSuccess` (`TxPartialSuccess`: the guaranteed phase committed, a fallible segment did not), or `Failure`.

`get_receipt` consumes the handle, so read the hashes before the wait. Cancelling the future is safe but does not retract the extrinsic from the mempool. Failures with no verdict surface as `ProviderError::Submission(SubmitError)`; the variant tells the caller whether resubmitting is safe (`Invalid`: definitive rejection; `NotSubmitted`: never left the process) or risks a double spend (`Dropped` / `NodeError`: the tx may still land) or is a wait/decode issue that leaves the tx in flight (`WatchStream`: transport-only; `VerdictFetch`: landed but events undecodable; re-query the chain rather than resubmit). `SubmitRpc` does not say whether the node holds the tx, so a resubmit after it also risks a double spend.

`MidnightProvider::prepare` stops one step earlier: it builds the unsigned `send_mn_transaction` extrinsic locally from the node's metadata and returns a `PreparedTx` whose extrinsic hash is already known, so a caller can durably record state keyed by that hash before the transaction reaches the mempool. The metadata checks only the call's shape, so the node validates the transaction only at the send. `PreparedTx::send` then hands back the same `PendingTx`.

The `PreparedTx` holds the reservation that the envelope carries. A failed prepare hands the inputs back before it returns, whatever the error (the connect, the metadata read or the encoding), because nothing reached the node. A `PreparedTx` that drops before its send hands them back too. `PreparedTx::send` moves the reservation to the `PendingTx` before the call starts, so a `get_receipt` that reads `Invalid`, which can arrive long after the fill returned, hands the inputs back. A failed send, or a dropped `PendingTx`, keeps them reserved until their TTL. `send_tx_envelope` is `prepare`, then `PreparedTx::send`. A contract call and a deploy prepare with `prepare`, because each records its private-state snapshot between the prepare and the send.

`prepare` and `send_tx_envelope` refuse a `TxEnvelope` that pays no fee, from a request made `without_fee` or a swap half, with `ProviderError::Transaction`, and its inputs go back. `send_transaction` refuses such a request with the same error before its fill, so it reserves no input and makes no proof. `TxEnvelope::into_bytes` gives bytes that keep their inputs reserved, for another party. Those inputs stay reserved until a sync sees the chain spend them, or until their TTL passes.

## Block pinning

A contract handle pins its reads with `Contract::at(..).at_block(hash)`, and the hash is a node block hash. The state read that honours it is the node RPC `midnight_contractState`, for a whole `ContractState`. It takes only a block hash, so a caller holding a height resolves it to a hash first (`MidnightProvider::get_block_hashes_by_height`). A circuit call on a pinned handle also reads the block time (`Timestamp::Now`) at the pin, so its clock checks run at the time of that block.

## External dependencies

| Crate | Source | Purpose |
|---|---|---|
| `midnight-ledger-v9` (+ its companion crates, and the `midnight-transient-crypto` 2.2 it also uses) | crates.io | Ledger 9: transaction types, VM, proving, and the on-chain runtime the Compact side reads |
| `midnight-ledger` (ledger 8, + its `midnight-zswap`, `midnight-onchain-*` and its own `midnight-storage` 2.x) | crates.io, through the helpers | Ledger 8 types, which the wallet's reader of the shielded events before the fork uses |
| `midnight-serialize`, `midnight-base-crypto`, `midnight-storage` | crates.io, one instance each | The tagged codec, `HashOutput`, `AlignedValue`, `Sp` |
| `midnight-ledger-unsafe-helpers` (+ the `midnight-node-ledger-helpers` it builds on) | `midnightntwrk/midnight-node`, a release tag | A `ledger_9` module: the type surface, and the transaction and wallet builders. A `ledger_8` module for the wallet's reader |
| `subxt` | crates.io | Substrate RPC (including `midnight_contractState`), extrinsic submission, watch streams, reconnecting client |
| `tokio-tungstenite` | crates.io | Indexer WebSocket subscriptions |

The ledger crates come from crates.io at the versions that the helpers' midnight-node release names, so each one has a single instance, and a crate that depends on midnight-rs needs no `[patch.crates-io]` table. Bump the ledger pins and the helpers' tag together, as the root `Cargo.toml` says. `midnight-zkir` 3.x is the exception: it follows the compactc at `COMPACT_REV`.

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
| Lazy field queries | A read of one ledger field through the `midnight_queryContractState` RPC of MIP-0009, with no download of the whole state. No released node serves the RPC. [midnightntwrk/midnight-node#2348](https://github.com/midnightntwrk/midnight-node/pull/2348) adds it, with tagged keys and a tagged `AlignedValue` for each result |
| Production proving | `ProofProvider::prove` returns `Transaction`, not `Result`, so a backend signals failure by panicking. The SDK catches that unwind and reports `WalletError::Proving` rather than losing the task, but the trait has no error channel and this is not mainnet-ready |
| A remote signer | `sign_transaction` signs after the proof, and `ProvenTx::signature_requests` and `ProvenTx::add_signature` let a custom wallet sign each request elsewhere, such as in an HSM. A custom wallet still funds through a `Wallet`, which derives the unshielded secret key from its seed: no constructor takes the role keys alone |
