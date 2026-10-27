# Wallet

The SDK's wallet covers Midnight's three asset legs:

- **Shielded coins**: Zswap-based privacy coins
- **Unshielded UTXOs**: public UTXO-model balances (NIGHT lives here)
- **Dust**: fee-token, generated continuously from NIGHT holdings

`Wallet` holds the synced state of one seed. Callers start one with `Wallet::from_seed` or `Wallet::from_mnemonic` and sync it from a source such as the provider (see [Sync](#sync)). Then they attach it with `with_wallet` and operate through the provider, which starts the resyncs, builds the transfers and makes the transaction contexts. The wallet can also resync itself in a background task (see [Background sync](#background-sync)).

The provider holds the wallet as `Arc<dyn WalletFacade>`, not as a `Wallet`. `WalletFacade` is the API a consumer programs against: every reading returns an owned value and every mutation is one call, so no caller holds a lock and the implementation chooses how its state is shared. `LocalWallet` is that API over a `Wallet` this process owns, and it keeps the lock as a private field. To attach a wallet you already synced, pass it as it is: `provider.with_wallet(wallet)` puts it behind a `LocalWallet`. To keep a handle on the attached wallet, attach an `Arc<LocalWallet>`. Another implementation goes in as itself when it implements `WalletFacade` and the `WalletBuilds` trait of each ledger generation (`midnight_core::provider::ledger_8::WalletBuilds` and `midnight_core::provider::ledger_9::WalletBuilds`). Such an implementation must serialize its own `resync`, `rescan_shielded`, `watch_for_coins` and `forget_coins` against each other, because the provider holds no lock across them. Its `rescan_shielded` and `watch_for_coins` take the chain view, as `resync` does. When its state is in an earlier ledger generation than the chain, a rescan first crosses the fork with a resync. `watch_for_coins` registers the coins and then rescans.

```rust
use std::sync::Arc;
use midnight_core::WalletFacade;
use midnight_core::wallet::LocalWallet;

let wallet = Arc::new(LocalWallet::new(wallet));
let provider = provider.with_wallet(Arc::clone(&wallet));
let balance = wallet.balance().await;
```

The wallet serializes its own resyncs, rescans, coin watches and forgets. So the handle can run them too, from any number of tasks or providers. A handle passes the provider as the chain view, for example `wallet.rescan_shielded(&provider)`.

## Seeds

A wallet starts from its seed. Each constructor below returns a `WalletBuilder`, and the [sync](#sync) of the builder gives the `Wallet`:

```rust
use midnight_core::{Wallet, WalletBuilder};

// A hex seed of 16, 32 or 64 bytes.
let builder = Wallet::from_seed("13e772040e60bf21946c1f15dbf8161cf4ff05266f62830437d5c1c7ec72480f")?;

// A BIP-39 mnemonic of 12, 15, 18, 21 or 24 words, with an empty passphrase.
let builder = Wallet::from_mnemonic(
    "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon diesel",
)?;

// 25th-word passphrase, matching @scure/bip39's mnemonicToSeed(phrase, passphrase).
let builder = Wallet::from_mnemonic_with_passphrase(phrase, "extra-word")?;

// A `Seed` that you already hold, by value or by reference.
let builder = WalletBuilder::new(&seed);
```

The hex form and the mnemonic form give the same on-chain identity. `Wallet::from_mnemonic("abandon … diesel")` and `Wallet::from_seed("a51c86de…")` of the matching bytes produce byte-identical keys. For an input that is not a seed, a constructor returns `SeedError::InvalidHex`, `SeedError::InvalidLength` or `SeedError::InvalidMnemonic`.

### The `Seed` type

`Seed` is the SDK's wallet-seed type. Hold one when you need the seed for more than a sync. It derives addresses before a sync, derives HD keys, and makes a new seed. To keep the `Seed` after the sync, pass `&seed` to `WalletBuilder::new`.

```rust
use midnight_core::Seed;
use midnight_core::wallet::{Role, mnemonic};

let seed = Seed::from_mnemonic(phrase)?;
let seed = Seed::from_mnemonic_with_passphrase(phrase, "extra-word")?;

// Raw entropy (16, 32, or 64 bytes).
let seed = Seed::from_hex("13e7…480f")?;
let seed = Seed::from_bytes(&entropy)?;

// Fresh random 32-byte seed.
let seed = Seed::generate();

// FromStr tries hex, then mnemonic, so either form parses.
let seed: Seed = env::var("MIDNIGHT_WALLET_SEED")?.parse()?;
```

`Seed` implements the HD-wallet layout from Midnight's [Wallet Engine Specification, *HD wallet structure*](https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure): the path `m / 44' / 2400' / <account>' / <role> / <index>`, the role table with five indices, and BIP-39 mnemonic input. The SDK's `Role` enum names the five roles:

| Index | `Role` variant | Spec name |
|---|---|---|
| 0 | `Role::UnshieldedExternal` | Unshielded External chain |
| 1 | `Role::UnshieldedInternal` | Unshielded Internal chain |
| 2 | `Role::Dust` | Dust |
| 3 | `Role::Zswap` | Shielded |
| 4 | `Role::Ecdsa` | Metadata |

`Seed` zeroizes its bytes on drop, and `Debug` / `Display` redact them.

### Mnemonic utilities

```rust
let words = mnemonic::generate(mnemonic::Strength::Words24);   // 24-word phrase
let phrase = mnemonic::join(&words);
assert!(mnemonic::validate(&phrase));
```

### Explicit HD derivation

The default sync, address-derivation, and transfer paths already use the standard per-asset accounts (account 0, the role's default leaf). For an explicit `(account, role, index)` key:

```rust
let key: [u8; 32] = seed
    .account(0)
    .role(Role::Zswap)
    .derive_at(0)?;
```

Returns the 32-byte private-key material the chain's signing / encryption schemes consume. Not an address; address derivation lives below.

## Address derivation

If all you need is an address (e.g. to print a faucet target), no sync is required:

```rust
use midnight_core::{Network, Seed};

let seed = Seed::from_hex(
    "13e772040e60bf21946c1f15dbf8161cf4ff05266f62830437d5c1c7ec72480f",
)?;

let unshielded = seed.unshielded_address(Network::Preprod);
let shielded   = seed.shielded_address(Network::Preprod);
// mn_addr_preprod1...
// mn_shield-addr_preprod1...
```

Addresses are deterministic per `(seed, network)` and include the network suffix in the bech32 HRP. The `Seed` address methods take the network as `impl Into<Network>`, so a `Network` or a `&Network`. For a name, such as the value of an environment variable, parse it first:

```rust
let network: Network = env::var("MIDNIGHT_NETWORK")?.parse()?;
```

The parse gives the variant of a known name (`Network::Preprod`, `Network::Mainnet` and the others), and `Network::Other` for any other valid name, for example the name of a custom devnet. A valid name has 1 to 68 characters, each a lowercase ASCII letter, a digit, `-` or `_`. Such a name is safe in an address prefix and in the storage path. The parse refuses any other name with `InvalidNetworkName`.

`Network::Mainnet` produces bech32 HRPs without a `_<name>` suffix (`mn_addr1...`, `mn_shield-addr1...`), matching the upstream convention; all other variants get the suffix.

A synced `Wallet` gives its own addresses on the network it synced as, so these methods take no network:

```rust
let unshielded = wallet.unshielded_address();
let shielded   = wallet.shielded_address();
```

## Sync

```rust
use midnight_core::{MidnightProvider, Network, Wallet};

let provider = MidnightProvider::new(node_url);
let wallet = Wallet::from_mnemonic(phrase)?
    .with_network(Network::Preprod)   // optional; the node's network without it
    .with_storage(storage_dir)        // optional; in-memory only without it
    .sync(&provider, indexer_url)
    .await?;
let provider = provider.with_wallet(wallet);
```

A `WalletBuilder` holds no connection and does no work until a sync step runs. `.sync(&provider, indexer_url)` replays three subscriptions of the indexer at `indexer_url` in the current task: the zswap and unshielded streams at the same time, then the Dust stream. It returns the `Wallet` once all three have caught up:

| Subscription | What it fills |
|---|---|
| `zswapLedgerEvents` | Shielded coin state (Merkle tree + nullifiers) |
| `unshieldedTransactions` | UTXO set for the wallet's unshielded address |
| `dustLedgerEvents` | Dust UTXOs derived from NIGHT holdings |

Dust sync from genesis can take 30+ minutes on a mainnet-sized history. Progress is checkpointed to disk after each batch when `with_storage(...)` is set, so subsequent runs resume from the last cursor.

The wallet keeps `indexer_url` for its later resyncs. The provider is the sync's source: its node reports the network and pins the wallet to the chain. The source can be anything that implements `SyncSource`, the trait in `midnight-types` that adds the node's network to the two-query `ChainView`.

Before the replay, the sync asks the node which network it runs. Without `.with_network(..)`, the wallet takes that network. The sync then fails with `WalletError::NetworkUnknown` when the node cannot answer, reports no network, or reports an id that the parse of a `Network` refuses (see [Address derivation](#address-derivation)).

`.with_network(..)` names the network, as a `Network`. A node that runs another network then fails the sync with `WalletError::NetworkMismatch`, before the sync reads the indexer. A node that cannot answer, reports no network, or reports an id that the parse refuses does not fail a sync with a named network. Name the network when your program reads it from its configuration, with `.with_network(env::var("MIDNIGHT_NETWORK")?.parse::<Network>()?)`. The sync then refuses a node URL of another network.

The pin is the chain-reset guard, and every sync takes one by default. A snapshot's cursors are counts, so a snapshot from a replaced chain resumes cleanly and reports the dead chain's balance. To catch this, the sync checks a stored snapshot's pin against the chain before it replays. A replaced chain fails the sync with `WalletError::ChainMismatch`. The sync also gives the wallet a fresh pin, which every later resync checks again.

A node that cannot answer for the pin does not fail the sync. The sync continues without a fresh pin, and keeps a stored snapshot's pin. When the node is down, each query waits ten seconds at most.

`.unpinned()` is the opt-out: it skips the network check and the pin. With a named network, the sync then asks the node nothing. With no network named, the sync still asks the node for its network, and nothing else. The sync still keeps a stored snapshot's pin, and every later resync checks that pin.

Sync also survives transient network trouble within a run: each subscription keeps its socket alive with a client ping after idle and a hard idle timeout, so a silently dead connection is detected rather than hanging forever. A transport failure reconnects with bounded exponential backoff and resumes from the last applied cursor, with a per-connection dedupe so re-delivered events aren't applied twice. The latest-block query that each sync and resync makes retries on the same bound. A non-retryable error fails the sync at once, and a retryable one fails it after the last retry. An indexer that keeps the socket alive but sends no event in time also fails the sync, unless a resync reads the silence as the tip. When the sync gives up on the indexer, it returns `WalletError::Indexer`, whose `retryable` field tells whether a later sync can succeed.

For a long sync with UI updates, end the builder with `.sync_with_progress(&provider, indexer_url)` in place of `.sync(&provider, indexer_url)`:

```rust
use midnight_core::SyncProgress;

let (mut rx, handle) = Wallet::from_mnemonic(phrase)?
    .with_storage(storage_dir)
    .sync_with_progress(&provider, indexer_url)
    .await?;

while let Some(progress) = rx.recv().await {
    match progress {
        SyncProgress::ZswapEvents { current, max }     => { /* render */ }
        SyncProgress::DustEvents  { current, max }     => { /* render */ }
        SyncProgress::ZswapComplete { events }         => { /* ... */ }
        SyncProgress::DustComplete  { events }         => { /* ... */ }
        SyncProgress::UnshieldedCaughtUp { utxos }     => { /* ... */ }
        SyncProgress::Resuming { zswap_event_id, dust_event_id } => { /* ... */ }
    }
}

let wallet = handle.await?;
let provider = provider.with_wallet(wallet);
```

`.sync(&provider, indexer_url)` runs the sync in the current task. `.sync_with_progress(&provider, indexer_url)` does the network check and the pin work first, so it can refuse a sync as `.sync(&provider, indexer_url)` does. Then it spawns the sync in a background task, and gives you progress events and a `SyncHandle` that resolves to the synced `Wallet`.

The spawned sync lives exactly as long as both returned ends do: dropping the progress receiver mid-sync cancels the task (the handle resolves to `WalletError::SyncCancelled`), and dropping the `SyncHandle` aborts it. Either way the three indexer WebSocket subscriptions are torn down promptly instead of running on with no consumer. The `while rx.recv().await` loop above keeps the receiver alive naturally. For a sync without progress events, use `.sync(&provider, indexer_url)`.

To incrementally refresh an already-synced wallet without replaying from the cursor's start, call `provider.resync_wallet().await`. Every build resyncs first, and no read does: `balance()` and every other read of the wallet return the state of the last sync or resync. To keep the reads fresh, run a [background sync](#background-sync). For one fresh read, call `resync_wallet()` before it. A resync takes the wallet's read/write lock only briefly, at its start (to snapshot replay inputs) and at its end (to commit). So reads like `balance()` keep completing while one is in flight. The wallet serializes concurrent resyncs.

### Background sync

`LocalWallet::sync_in_background` resyncs the wallet in a background task until you drop the handle that it returns. Each pass is one resync against the chain view that you give. So the pass resumes the replay from the wallet's cursors. When the wallet holds a chain pin, the pass checks that pin. Give `sync_in_background` the provider that the wallet is attached to:

```rust
use std::sync::Arc;
use std::time::Duration;
use midnight_core::wallet::LocalWallet;

let wallet = Arc::new(LocalWallet::new(wallet));
let provider = Arc::new(provider.with_wallet(Arc::clone(&wallet)));
let sync = wallet.sync_in_background(provider.clone(), Duration::from_secs(5));
```

The first pass starts one pause after the call, because the wallet is synced when you make it. Each later pass starts one pause after the one before it ends, so passes never overlap. The wallet serializes each pass with its other sync methods. So a build waits for a pass in flight, and reads keep completing during one.

A failed pass does not stop the sync. The task logs the error, and the next pass runs after the pause. `sync.last_error()` returns the error of the last pass, and `None` after a pass commits. A pass that panics ends the task, and `last_error()` then returns `WalletError::SyncTaskJoin`. Read `last_error()` to show a wallet that does not sync, such as a wallet on a replaced chain (`WalletError::ChainMismatch`).

`sync.wait_until(done)` waits until `done` holds for the balance of the wallet. A finalized transaction reaches the wallet only through a resync, and one resync can end before it sees the transaction. So wait on the balance before a read or a build that needs the effect of the transaction. `done` runs at the call and after each pass, outside the lock of the wallet. This example waits until a transfer of `amount` of `token` leaves the wallet:

```rust
use std::time::Duration;

// `balance()` returns the state of the last pass. Resync first, so that
// `before` is the state that the build of the transfer starts from.
provider.resync_wallet().await?;
let before = provider.balance().await?.shielded_value(token);
provider
    .transfer_shielded(token, amount, &recipient)
    .await?
    .wait_finalized()
    .await?;
// The transfer succeeded, so `before` covered `amount`.
let after = before - amount;
tokio::time::timeout(
    Duration::from_secs(60),
    sync.wait_until(|b| b.shielded_value(token) == after),
)
.await??;
```

After a deploy, a contract call or another fee-paying transaction, wait until no build reserves Dust: `|b| b.dust.spendable_speck == b.dust.balance_speck`. A resync that sees the Dust spend ends the reservation of the build.

The wait has no deadline, so wrap it in `tokio::time::timeout`. A failed pass does not end the wait, because a later pass can commit. After a timeout, `sync.last_error()` tells whether the passes fail. When the background task has ended, the wait fails with `WalletError::SyncTaskJoin`.

Dropping the handle stops the sync at the task's next await, and closes the subscriptions of a pass in flight. The wallet then keeps its state from before that pass, or the whole commit of the pass. Bind the handle to a named variable. `let _ = wallet.sync_in_background(..)` drops it at once, and `#[must_use]` does not warn about `let _ =`. The task holds the provider and the wallet until the handle drops. Start one background sync for each wallet.

### Across a hard fork

The wallet's state is in the ledger generation its chain runs, and `Wallet::ledger_version()` names it. A sync reads the generation from the chain. When the chain moves from ledger 8 to ledger 9, the wallet crosses with it in three cases:

- A sync from genesis replays the ledger 8 history, then continues on ledger 9.
- A wallet stored before the fork resumes from its snapshot and crosses on the way.
- An attached wallet crosses at its next resync, and every build resyncs first.

The shielded coins carry across. The Dust state starts empty, because the fork emptied the chain's Dust state and every Dust registration. So after the fork the wallet registers its NIGHT again before it can pay a fee. See [`ledger-generations.md`](ledger-generations.md) for the procedure and the errors a crossing can return.

### Indexer trust model

The indexer is the wallet's sole data source: shielded state, dust state, the unshielded UTXO set, and the ledger parameters used for fee and TTL math are all rebuilt from indexer subscriptions and blocks. Nothing is cross-checked against a node. A hostile or compromised indexer can therefore fabricate UTXOs that do not exist on chain (the node rejects transactions built from them) or withhold real ones (funds look missing until you sync against an honest indexer). Point the provider at an indexer you trust as much as your node.

What sync does enforce is the shape of the data: event ids must not go backwards within a subscription connection (`WalletError::EventOrder`), an event with a malformed field rejects the whole event before any part of it is applied (`WalletError::MalformedUtxo`, decode errors), and decoded ledger parameters are sanity-checked before any fee math consumes them (`WalletError::CorruptParameters`). Sync keeps the owner address of a UTXO as it comes, because the subscription asks for the UTXOs of one address. These checks catch corruption and protocol violations, not dishonesty. Actively cross-checking indexer answers against the node (for example `midnight_queryUnshielded`) is explicitly out of scope today; revisit if a threat model requires operating against an untrusted indexer.

### Persistence

With `.with_storage(dir)`, the sync writes to:

```
{dir}/{network}/{sha256(unshielded_address)}/
  ├── metadata.json     event cursors, last block, last tx id, generation pointers,
  │                     and the ledger generation that wrote the state files
  ├── zswap-N.bin       tagged-serialized ZswapLocalState
  ├── dust_wallet-N.bin tagged-serialized DustWallet, which holds the Dust secret key
  └── pending.json      in-flight spend reservations (see below)
```

`{network}` is the network that `.with_network(..)` names or, without `.with_network(..)`, the node's network.

The directory holds the Dust secret key, so protect it as key material. On Unix, the SDK creates the wallet's directory with mode 0700 and its files with mode 0600.

Use `Wallet::default_storage_dir()` for `~/.midnight/wallets/`. Writes are generation-based: new `zswap-N+1.bin` / `dust_wallet-N+1.bin` files are written first, then `metadata.json` is atomically renamed, then the old generation is cleaned up. A crash mid-write leaves the previous generation intact.

A snapshot that names no ledger generation reads as ledger 8, the only generation an older build wrote. A snapshot of a later generation than the chain runs belongs to a replaced chain: the sync returns `WalletError::LedgerRegression`, which names the directory to remove.

Persistence is not a one-shot at initial sync: every successful `resync_wallet` re-saves the moved cursors, refreshed parameters, and pending set, and each transfer build rewrites `pending.json` with its new reservation. `pending.json` is removed once no reservations remain.

## Balance

```rust
let balance = provider.balance().await?;

balance.shielded.coins;          // Vec<ShieldedCoinBalance { token_type, value }>
balance.shielded.total_count;    // usize
balance.shielded_value(token);   // u128, summed over the coins of one ShieldedTokenType
balance.unshielded;              // Vec<UnshieldedUtxoInfo { token_type, value }>
balance.unshielded_value(NIGHT); // u128, summed over the UTXOs of one UnshieldedTokenType
balance.dust.spendable_utxos;    // usize: the Dust UTXOs no pending build reserves
balance.dust.balance_speck;      // u128: all the Dust, reserved or not (1 DUST = 10^15 SPECK)
balance.dust.spendable_speck;    // u128: the Dust a new build can draw on now
```

A build reserves the Dust UTXOs it spends. The reservation ends when a sync or resync sees the chain confirm the transaction. It also ends when a release hands the UTXOs back, or when its TTL ends (see [Pending reservations](#pending-reservations)). `spendable_utxos` and `spendable_speck` leave the reserved Dust out, and `balance_speck` counts it. So a wallet whose only Dust UTXO a pending build reserves reads a `spendable_speck` of 0 and a positive `balance_speck`.

`token_type` is typed: `UnshieldedTokenType` for `balance.unshielded[i]`, `ShieldedTokenType` for `balance.shielded.coins[i]`. Use `.token_type_hex()` for display / log output (64-char hex, no `0x` prefix). For comparison against the chain's native unshielded token, use `token_type == midnight_core::wallet::NIGHT`. NIGHT is denominated in STAR (1 NIGHT = 10⁶ STAR); DUST in SPECK (1 DUST = 10¹⁵). The byte pattern `[0; 32]` in a `ShieldedTokenType` is **not** NIGHT. See [`tokens.md`](tokens.md) for the two-ledger model.

For the rest of what the wallet holds:

```rust
provider.parameters().await?.dust.night_dust_ratio;
provider.unshielded_utxos().await?;

let cursors = provider.sync_cursors().await?;
cursors.zswap_event_id;
cursors.last_block_height;
```

Every reading returns an owned value taken under a short read lock, so nothing a caller holds can block a resync running in another task. `sync_cursors()` returns the four counters together because they advance together: reading them one at a time can report a mixture of two syncs.

## Dust registration

Before NIGHT holdings can generate spendable Dust, the wallet must publish a **dust registration** that binds its dust address to its unshielded address. One `register_dust` call registers one tNIGHT UTXO and pays its own fee from that UTXO, so a wallet with no Dust can register. The rustdoc of [`register_dust`](../crates/midnight-provider/src/provider.rs) gives the full rule.

```rust
let pending = provider.register_dust(None).await?;     // None = now - 1 hour for a UTXO with no ctime
pending.wait_best().await?;
```

`utxo_ctime` is a fallback creation time in seconds since the epoch. The build uses it only for a UTXO whose creation time the indexer did not report, and `None` makes that fallback `now - 1 hour`. The transaction takes a few seconds to land; Dust starts generating once it's finalized.

tNIGHT that arrives after the first registration generates Dust with no further call. tNIGHT that the wallet already held does not: call `register_dust` again until `DustBalance::unregistered_night_utxos` is 0. [`ledger-generations.md`](ledger-generations.md#register-dust-again) shows that loop.

See [`dust-and-fees.md`](dust-and-fees.md) for the full Dust model, generation rate, and how fees are balanced.

## Transfers

Two flavors, same shape. Each method returns a builder, and `.await?` builds and submits in one step:

```rust
use midnight_core::wallet::NIGHT;

let pending = provider
    .transfer_unshielded(NIGHT, amount_in_star, &recipient_address)
    .await?;
pending.wait_best().await?;
```

```rust
let pending = provider
    .transfer_shielded(token_type, amount, &recipient_shielded_address)
    .await?;
```

`recipient` is the bech32 address string (`mn_addr_*` for unshielded, `mn_shield-addr_*` for shielded). `transfer_shielded` accepts any `ShieldedTokenType`; only `register_dust` is intrinsically NIGHT-specific. See [`tokens.md`](tokens.md) for the asset/ledger model. Each builder:

1. Resyncs, then takes a write lock on the wallet and builds the context of the wallet's ledger generation.
2. Selects inputs from the wallet's local UTXO set.
3. Balances Dust fees via a `speculative_spend` loop (mock proofs first, real proofs once balanced).
4. Reserves the selected inputs in `pending.json` so the next concurrent build can't re-pick the same coins.
5. Submits the proven bytes to the node and returns a `PendingTx`.

To inspect the proven transaction before you submit it (e.g. to size-check, or to show the user the fee before they confirm), call `.build()` on the builder instead of awaiting it:

```rust
let result = provider
    .transfer_shielded(token_type, amount, &recipient)
    .build()
    .await?;
// result: TransferResult { tx_bytes, ledger_version, spent, fee_speck }
println!("fee: {} SPECK ({:.6} DUST)", result.fee_speck, result.fee_speck as f64 / SPECKS_PER_DUST as f64);
let pending = provider.submit_reserved(result).await?;
```

`fee_speck` is the deterministic Dust fee the chain will charge, computed via `Transaction::fees(&ledger.parameters, false)` against the parameters the build saw. The `false` (no `enforce_time_to_dismiss`) matches the node's own estimation RPC, so the quote agrees with what the node reports and the indexer later reports as `paidFees` for an accepted, included transaction. The node applies one more check at submit, which the quote skips. The rustdoc of [`TransferResult::fee_speck`](../crates/midnight-types/src/transfer.rs) describes it. `.build()` reserves the spent inputs just like the awaitable path; until the submitted transaction is observed on-chain (or its TTL expires), the inputs stay reserved. `submit_reserved` hands them back when the node never gets the transaction or rejects it (see [Pending reservations](#pending-reservations)).

## Submission and waiting

`MidnightProvider::submit(tx_bytes)` connects to the node, submits the bytes as an unsigned `Midnight::send_mn_transaction` extrinsic via `submit_and_watch`, and hands back a `PendingTx`:

```rust
let pending = provider.submit(&tx_bytes).await?;
println!("ext: {}", pending.extrinsic_hash_hex());

let (best,      pending) = pending.wait_best().await?;
let (finalized, _pending) = pending.wait_finalized().await?;
```

`wait_best` / `wait_finalized` consume `self` and return it back so callers re-bind through each step without `let mut`. Cancelling either future is safe but does not retract the extrinsic from the mempool.

Both waits return `Ok` only when the chain applied the transaction. When the transaction landed but did not apply (`PartialSuccess` or `Failure`), the wait fails with `ProviderError::NotApplied`, whose `NotApplied` holds the `TxInBlock` with the verdict. The verdict of `wait_best` is provisional: a reorg can drop the block, and the transaction can land again with another verdict, which `wait_best` does not follow. `wait_finalized` gives the final verdict. An `Err` consumes the handle, so when the final verdict matters, call `wait_finalized` in place of `wait_best`.

When a wait fails for any other reason, the error is `ProviderError::Submission` carrying a typed `SubmitError`. Match its variants to decide what to do next: `Invalid` is a definitive rejection (safe to rebuild and resubmit with fresh inputs), `Dropped` / `NodeError` are not (the tx may still be re-included; resubmitting the same inputs risks a double spend), `WatchStream` means only the watch subscription broke (the tx stays in the pool and may still land), and `VerdictFetch` means the tx landed but its events couldn't be decoded (it's on chain, so don't resubmit, re-query for the verdict). The pre-watch variants come from the submit: after `NotSubmitted` the tx never left the process, and after `SubmitRpc` the node can still hold it.

## Recovering a coin the wallet cannot discover

A shielded coin reaches its owner through the discovery ciphertext on its output. That channel fails when the party that built the transaction attached no ciphertext, or sealed it to a key the owner does not hold. The coin still exists, the owner's coin public key still owns it, and it stays spendable, as long as the owner can rebuild its `CoinInfo` (nonce, token type, value) from somewhere else. A contract that evolves one public nonce per mint is such a case: the owner reads the nonce from chain state.

Hand the rebuilt coin to `watch_for_coin` and the wallet claims it from the chain's own output, decrypting nothing:

```rust
use midnight_core::provider::{CoinInfo, HashOutput, Nonce, ShieldedTokenType};

provider
    .watch_for_coin(CoinInfo {
        nonce: Nonce(HashOutput(nonce)),                  // rebuilt from chain state
        type_: ShieldedTokenType(HashOutput(token_type)),
        value,
    })
    .await?;

// A claimed coin is spendable like any other.
let coins = provider.spendable_shielded_coins().await?;
```

Registration records the coin's commitment; a replay that meets the matching output claims the coin from that record. The order matters, and it is why `watch_for_coin` replays for you. A replay that meets an output it cannot claim collapses that Merkle leaf and moves on, and a resync resumes from its cursor rather than revisiting the event, so registering after the fact would change nothing on its own. `watch_for_coin` therefore replays the whole `zswapLedgerEvents` stream from its first event, with the registration in place. Dust state, unshielded state, and their cursors are untouched.

Every coin the wallet already holds is re-registered before that replay, so a rescan never costs you a coin. This matters because the ledger consumes a registration the moment a replay claims it: a coin recovered by an earlier `watch_for_coin` has no ciphertext to be found by, and a replay that started without it would collapse its leaf again.

That full replay costs more than a resync, so register every coin you know about in one `watch_for_coins([...])` call, and treat this as a recovery step rather than a routine one. Registrations are persisted with the rest of the wallet state, so one made for a coin that has not landed yet survives a restart, and every later resync carries it rather than dropping it.

To check the outcome, look for the coin in `provider.watched_coins()`, or in `wallet.watched_coins()` on a handle. It lists the registrations that no replay claimed, ordered by nonce, then token type, then value. A coin that left that list was claimed. `provider.spendable_shielded_coins()` tells only whether a coin is spendable now. It also leaves out a claimed coin that a build reserved or that the wallet spent.

A coin still in the list after the replay is not on chain yet, or its rebuilt `CoinInfo` is wrong. It can also be a coin that the replay claimed and then saw spent, for example by another wallet with the same seed. Every later replay carries a wrong or spent registration. Drop it with `provider.forget_coin(coin)`. Forgetting drops the registration only, and a coin that the wallet already claimed stays spendable.

The registration is recorded and persisted before the replay starts. If the replay fails, the registration is still there, so retry with `provider.rescan_shielded()` rather than registering again.

`provider.rescan_shielded()` runs the replay alone. A handle (`Arc<LocalWallet>`) has the same two methods: `watch_for_coins(coins, &provider)` and `rescan_shielded(&provider)`. The replay runs against the indexer that the wallet synced from, and reads keep completing while it runs. The wallet runs the registration and the replay under one hold, so no resync runs between them. When the wallet's state is in an earlier ledger generation than the chain, a rescan first crosses the fork with a resync.

## Pending reservations

In-flight spends that have been built but not yet confirmed on-chain are tracked in `PendingReservations`, persisted as `pending.json` next to the wallet state. They serve two purposes:

- **Prevent double-spending across builds.** Input selection skips reserved coins.
- **Drop on confirmation or TTL.** Event replay (initial sync and every resync) collects the dust nullifiers and unshielded UTXO keys it observes spent and clears the matching reservations at its commit point; `evict_expired` (which every funded build runs through the wallet's `add_funding`) drops entries whose TTL window elapsed, as a backstop for transactions that never confirm. Transaction TTL defaults to one hour.

The wallet also drops a reservation of another ledger generation when it loads `pending.json`, because a transaction built for one generation cannot land on a chain that runs another.

A release also drops a reservation, for a transaction that can never land. It matches each entry by its input and by the stamp of its reservation, so a late release cannot drop an entry that a later build made over the same input. These paths release:

- `MidnightProvider::submit_reserved`, when its prepare fails (the connect included), and its `PendingTx` when a wait reads `Invalid`. The transfers, `register_dust`, a deploy and a maintenance update submit this way. Submit the result of a `.build()` or of `balance_transaction` with it too.
- `MidnightProvider::prepare_reserved`, when the prepare fails, and its `PreparedTx` when it drops before its submit or when you call `PreparedTx::release`. Its submit moves the reservation to the `PendingTx`, which releases when a wait reads `Invalid`. A contract call prepares this way, so a call that stops before its submit releases, for example on a failed `append_pending`.
- A build whose proof fails, and a deploy whose shielded offer fails before its fee is paid.
- `MidnightProvider::release(&spent)`, for a `TransferResult` that you build and never submit.

A failed submit call (`SubmitRpc`) keeps the inputs reserved, because the node can still hold the transaction. The bytes of a contract call's `.build()` or `.without_dust()` do not name their inputs, so their reservation ends only when a sync sees the chain spend them, or when their TTL ends. Other than that, you don't normally interact with this directly: the builds reserve, and the sync loop clears.

## Lifecycle summary

```
MidnightProvider::new(node_url)
  ↓
Wallet::from_seed(hex)? | Wallet::from_mnemonic(phrase)? | WalletBuilder::new(seed)
  [.with_network(network)][.with_storage(dir)][.unpinned()]
  ↓
.sync(&provider, indexer_url).await  or  .sync_with_progress(&provider, indexer_url).await → (progress, handle)
  │ take the node's network when no network is named
  │ check the node's network and the stored pin, and take the pin this sync carries (not when unpinned)
  │ subscribe zswap + unshielded + dust  (parallel)
  │ persist (metadata + binary state + pending)
  ↓
provider.with_wallet(wallet)
  ↓
  provider.balance()                     read-only
  provider.parameters() / .sync_cursors() / .unshielded_utxos()
  provider.resync_wallet()               incremental refresh + re-persist
  wallet.sync_in_background(provider, pause)   resync in a task until the handle drops (an Arc<LocalWallet>)
      sync.wait_until(done)              wait until done holds for the balance (at the call and after each pass)
  provider.watch_for_coin(coin)          claim a coin with no usable ciphertext: register, then replay
  provider.rescan_shielded()             replay the shielded stream (a resync first, to cross a fork)
  provider.forget_coin(coin)             drop a registration that matched nothing
  provider.watched_coins()               the registrations that no replay claimed
  provider.register_dust(None).await         registers one tNIGHT UTXO, self-funded
  provider.transfer_unshielded(...).await    builds + submits → PendingTx
  provider.transfer_shielded(...).await      builds + submits → PendingTx
       │     .build().await → TransferResult (escape hatch, no submit)
       │
       └─ PendingTx
            │ .wait_best().await
            └ .wait_finalized().await
```

## Examples

- [`examples/wallet-sync`](../examples/wallet-sync): end-to-end sync, balance display, optional dust registration, optional unshielded self-transfer. Runs against preprod (with faucet funding) or a local devnet.
