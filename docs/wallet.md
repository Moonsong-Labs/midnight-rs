# Wallet

The SDK's wallet covers Midnight's three asset legs:

- **Shielded coins**: Zswap-based privacy coins
- **Unshielded UTXOs**: public UTXO-model balances (NIGHT lives here)
- **Dust**: fee-token, generated continuously from NIGHT holdings

`Wallet` holds the keys and the synced state of one seed. Callers start one with `Wallet::from_seed` and sync it from a source such as the provider (see [Sync](#sync)). Then they attach it with `with_wallet`, and the provider builds transactions with it. The wallet can also sync itself in a background task (see [Background sync](#background-sync)).

`Wallet` locks its own state. The lock is a private field, so no caller holds a guard, and a `&Wallet` serves every call. `Wallet` is not `Clone`. To share it, put it in an `Arc`.

`MidnightProvider<W>` holds the wallet `W` that `with_wallet` gets, and calls it only through the `MidnightWallet` trait. `Wallet` implements the trait, and so do `&Wallet` and `Arc<Wallet>`. Attach a borrow to keep using the wallet beside the provider, or an `Arc<Wallet>` when a background sync shares it. `provider.wallet()` gives the attached wallet back.

```rust
let provider = provider.with_wallet(&wallet);
let balance = wallet.balance().await;
```

The trait has three methods:

- `shielded_recipient`: the keys that `ownPublicKey()` and the wallet's own shielded outputs use.
- `fund_transaction`: resync, then add the wallet's inputs, change and Dust fee to a transaction that the provider filled, and reserve what they spend. Each signature slot that it adds holds a placeholder.
- `sign_transaction`: add the signatures that the transaction still needs after its proof, then keep its reservation across a restart.

A transaction goes through four stages: the provider fills a `FilledTx` from node queries, the wallet funds it into a `FundedTx`, the provider proves it into a `ProvenTx`, and the wallet signs it into a `TxEnvelope`. The reservation travels with the stages, so the trait has no release method (see [Pending reservations](#pending-reservations)). `FilledTx` and `FundedTx` are opaque, so a custom wallet funds through a `Wallet` that it wraps.

The `sign_transaction` of a custom wallet can sign elsewhere, such as in an HSM: `ProvenTx::signature_requests` gives one `SignatureRequest` for each intent segment and signer, and `ProvenTx::add_signature` puts a 64-byte signature over its `data` in every placeholder of that signer. It then ends with the inner `sign_transaction`, which signs nothing more and keeps the reservation. A request can name an intent that another party built, such as in a sponsored transaction. A signature authorizes every spend and Dust registration of its signer in that intent. So a custom wallet signs only the requests of the intents that it funded.

The wallet serializes its own syncs and coin watches. So a handle can run them too, from any number of tasks or providers. A handle passes the provider as the chain view, for example `wallet.sync(&provider)`.

## Seeds

A wallet starts from its `Seed`. `Wallet::from_seed` returns a `WalletBuilder`, and the [sync](#sync) of the builder gives the `Wallet`:

```rust
use midnight_core::{Seed, Wallet};

// A hex seed of 16, 32 or 64 bytes.
let seed = Seed::from_hex("13e772040e60bf21946c1f15dbf8161cf4ff05266f62830437d5c1c7ec72480f")?;

// A BIP-39 mnemonic of 12, 15, 18, 21 or 24 words, with an empty passphrase.
let seed = Seed::from_mnemonic(
    "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon diesel",
)?;

// 25th-word passphrase, matching @scure/bip39's mnemonicToSeed(phrase, passphrase).
let seed = Seed::from_mnemonic_with_passphrase(phrase, "extra-word")?;

let builder = Wallet::from_seed(seed);
```

The hex form and the mnemonic form give the same on-chain identity. `Seed::from_mnemonic("abandon … diesel")` and `Seed::from_hex("a51c86de…")` of the matching bytes produce byte-identical keys. For an input that is not a seed, a constructor returns `SeedError::InvalidHex`, `SeedError::InvalidLength` or `SeedError::InvalidMnemonic`.

### The `Seed` type

`Seed` is the SDK's wallet-seed type. Hold one when you need the seed for more than a sync. It derives addresses before a sync, derives HD keys, and makes a new seed. `Wallet::from_seed` takes the `Seed` by value, so to keep the `Seed` after the sync, pass a clone. The sync derives the keys of each role and drops the seed, and the `Wallet` holds only the keys: the shielded and Dust spends prove with their secret keys, and the unshielded key signs.

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
let dust       = seed.dust_address(Network::Preprod);
// mn_addr_preprod1...
// mn_shield-addr_preprod1...
// mn_dust_preprod1...
```

Addresses are deterministic per `(seed, network)` and include the network suffix in the bech32 HRP. The `Seed` address methods take the network as `impl Into<Network>`, so a `Network` or a `&Network`. For a name, such as the value of an environment variable, parse it first:

```rust
let network: Network = env::var("MIDNIGHT_NETWORK")?.parse()?;
```

The parse gives the variant of a known name (`Network::Preprod`, `Network::Mainnet` and the others), and `Network::Other` for any other valid name, for example the name of a custom devnet. A valid name has 1 to 68 characters, each a lowercase ASCII letter, a digit, `-` or `_`. Such a name is safe in an address prefix and in the storage path. The parse refuses any other name with `InvalidNetworkName`.

`Network::Mainnet` produces bech32 HRPs without a `_<name>` suffix (`mn_addr1...`, `mn_shield-addr1...`, `mn_dust1...`), matching the upstream convention; all other variants get the suffix.

A synced `Wallet` gives its own addresses on the network it synced as, so these methods take no network:

```rust
let unshielded = wallet.unshielded_address();
let shielded   = wallet.shielded_address();
let dust       = wallet.dust_address();
```

## Sync

```rust
use midnight_core::{MidnightProvider, Network, Seed, Wallet};

let provider = MidnightProvider::new(node_url);
let wallet = Wallet::from_seed(Seed::from_mnemonic(phrase)?)
    .with_network(Network::Preprod)   // optional; the node's network without it
    .with_storage(storage_dir)        // optional; in-memory only without it
    .sync(&provider, indexer_url)
    .await?;
let provider = provider.with_wallet(&wallet);
```

A `WalletBuilder` holds no connection and does no work until a sync step runs. `.sync(&provider, indexer_url)` replays three subscriptions of the indexer at `indexer_url` in the current task: the zswap and unshielded streams at the same time, then the Dust stream. It returns the `Wallet` once all three have caught up:

| Subscription | What it fills |
|---|---|
| `zswapLedgerEvents` | Shielded coin state (Merkle tree + nullifiers) |
| `unshieldedTransactions` | UTXO set for the wallet's unshielded address |
| `dustLedgerEvents` | Dust UTXOs derived from NIGHT holdings |

Dust sync from genesis can take 30+ minutes on a mainnet-sized history. Progress is checkpointed to disk after each batch when `with_storage(...)` is set, so subsequent runs resume from the last cursor.

The SDK reads the data of a chain that runs ledger 9. On a chain that forked from ledger 8 to ledger 9, such as mainnet, the zswap stream starts with the ledger 8 events from before the fork. A sync or a rescan from event zero reads them, so the wallet keeps the shielded coins that it received before the fork. The Dust replay skips the ledger 8 Dust events, because the fork emptied the chain's Dust state (see [`dust-and-fees.md`](dust-and-fees.md#dust-after-the-fork-to-ledger-9)). A chain that runs ledger 8 fails the sync with `WalletError::UnknownLedger`, which names the tag of the data.

The wallet keeps `indexer_url` for its later resyncs. The provider is the sync's source: its node reports the network and pins the wallet to the chain. The source can be anything that implements `SyncSource`, the trait in `midnight-types` that adds the node's network to the two-query `ChainView`.

Before the replay, the sync asks the node which network it runs. Without `.with_network(..)`, the wallet takes that network. The sync then fails with `WalletError::NetworkUnknown` when the node cannot answer, reports no network, or reports an id that the parse of a `Network` refuses (see [Address derivation](#address-derivation)).

`.with_network(..)` names the network, as a `Network`. A node that runs another network then fails the sync with `WalletError::NetworkMismatch`, before the sync reads the indexer. A node that cannot answer, reports no network, or reports an id that the parse refuses does not fail a sync with a named network. Name the network when your program reads it from its configuration, with `.with_network(env::var("MIDNIGHT_NETWORK")?.parse::<Network>()?)`. The sync then refuses a node URL of another network.

The pin is the chain-reset guard, and every sync takes one. A snapshot's cursors are counts, so a snapshot from a replaced chain resumes cleanly and reports the dead chain's balance. To catch this, the sync checks a stored snapshot's pin against the chain before it replays. A replaced chain fails the sync with `WalletError::ChainMismatch`. The sync also gives the wallet a fresh pin, which every later resync checks again.

A node that cannot answer for the pin does not fail the sync. The sync continues without a fresh pin, and keeps a stored snapshot's pin. When the node is down, each query waits ten seconds at most.

Sync also survives transient network trouble within a run: each subscription keeps its socket alive with a client ping after idle and a hard idle timeout, so a silently dead connection is detected rather than hanging forever. A transport failure reconnects with bounded exponential backoff and resumes from the last applied cursor, with a per-connection dedupe so re-delivered events aren't applied twice. The latest-block query that each sync and resync makes retries on the same bound. A non-retryable error fails the sync at once, and a retryable one fails it after the last retry. An indexer that keeps the socket alive but sends no event in time also fails the sync, unless a resync reads the silence as the tip. When the sync gives up on the indexer, it returns `WalletError::Indexer`, whose `retryable` field tells whether a later sync can succeed.

For a long sync with UI updates, end the builder with `.sync_with_progress(&provider, indexer_url)` in place of `.sync(&provider, indexer_url)`:

```rust
use midnight_core::{Seed, SyncProgress};

let (mut rx, handle) = Wallet::from_seed(Seed::from_mnemonic(phrase)?)
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
let provider = provider.with_wallet(&wallet);
```

`.sync(&provider, indexer_url)` runs the sync in the current task. `.sync_with_progress(&provider, indexer_url)` does the network check and the pin work first, so it can refuse a sync as `.sync(&provider, indexer_url)` does. Then it spawns the sync in a background task, and gives you progress events and a `SyncHandle` that resolves to the synced `Wallet`.

The spawned sync lives exactly as long as both returned ends do: dropping the progress receiver mid-sync cancels the task (the handle resolves to `WalletError::SyncCancelled`), and dropping the `SyncHandle` aborts it. Either way the three indexer WebSocket subscriptions are torn down promptly instead of running on with no consumer. The `while rx.recv().await` loop above keeps the receiver alive naturally. For a sync without progress events, use `.sync(&provider, indexer_url)`.

To incrementally refresh an already-synced wallet without replaying from the cursor's start, call `wallet.sync(&provider).await`. Every funding resyncs first, and no read does: `balance()` and every other read of the wallet return the state of the last sync. To keep the reads fresh, run a [background sync](#background-sync). For one fresh read, call `wallet.sync(&provider)` before it. A sync takes the wallet's read/write lock only briefly, at its start (to snapshot replay inputs) and at its end (to commit). So reads like `balance()` keep completing while one is in flight. The wallet serializes concurrent syncs.

### Background sync

`Wallet::sync_in_background` syncs the wallet in a background task until you drop the handle that it returns. Each pass is one `Wallet::sync` against the chain view that you give. So the pass resumes the replay from the wallet's cursors. When the wallet holds a chain pin, the pass checks that pin. Give `sync_in_background` the provider that the wallet is attached to:

```rust
use std::sync::Arc;
use std::time::Duration;

let wallet = Arc::new(wallet);
let provider = Arc::new(provider.with_wallet(Arc::clone(&wallet)));
let sync = wallet.sync_in_background(provider.clone(), Duration::from_secs(5));
```

The first pass starts one pause after the call, because the wallet is synced when you make it. Each later pass starts one pause after the one before it ends, so passes never overlap. The wallet serializes each pass with its other sync methods. So a build waits for a pass in flight, and reads keep completing during one.

A failed pass does not stop the sync. The task logs the error, and the next pass runs after the pause. `sync.last_error()` returns the error of the last pass, and `None` after a pass commits. A pass that panics ends the task, and `last_error()` then returns `WalletError::SyncTaskJoin`. Read `last_error()` to show a wallet that does not sync, such as a wallet on a replaced chain (`WalletError::ChainMismatch`).

`sync.wait_until(done)` waits until `done` holds for the balance of the wallet. A finalized transaction reaches the wallet only through a resync, and one resync can end before it sees the transaction. So wait on the balance before a read or a build that needs the effect of the transaction. `done` runs at the call and after each pass, outside the lock of the wallet. This example waits until a transfer of `amount` of `token` leaves the wallet:

```rust
use std::time::Duration;

// `balance()` returns the state of the last pass. Sync first, so that
// `before` is the state that the build of the transfer starts from.
wallet.sync(&*provider).await?;
let before = wallet.balance().await.shielded_value(token);
provider
    .send_transaction(TransactionRequest::shielded(token, amount, recipient))
    .await?
    .get_receipt()
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

Use `Wallet::default_storage_dir()` for `~/.midnight/wallets/`. Writes are generation-based: new `zswap-N+1.bin` / `dust_wallet-N+1.bin` files are written first, then `metadata.json` is atomically renamed, then the old generation is cleaned up. A crash mid-write leaves the previous generation intact. The SDK syncs each file to disk before its rename and, on Unix, the directory after it, so a power loss mid-write also leaves it intact.

The sync refuses a snapshot that holds ledger 8 state. When the `ledger_version` field of `metadata.json` is `V8`, or the file has no such field, the sync fails with `WalletError::Ledger8Snapshot`, which names the directory. Delete the directory and sync again from genesis.

Persistence is not a one-shot at initial sync: every successful `Wallet::sync` re-saves the moved cursors, refreshed parameters, and pending set, and `sign_transaction` rewrites `pending.json` with the reservation of each transaction once its proof succeeds. `pending.json` is removed once no reservations remain.

## Balance

```rust
let balance = wallet.balance().await;

balance.shielded.coins;                    // Vec<ShieldedCoinBalance { token_type, value, spendable }>
balance.shielded.total_count;              // usize
balance.shielded_value(token);             // u128, summed over the coins of one ShieldedTokenType
balance.spendable_shielded_value(token);   // u128, the same sum over the spendable coins only
balance.unshielded;                        // Vec<UnshieldedUtxoInfo { token_type, value, spendable }>
balance.unshielded_value(NIGHT);           // u128, summed over the UTXOs of one UnshieldedTokenType
balance.spendable_unshielded_value(NIGHT); // u128, the same sum over the spendable UTXOs only
balance.dust.spendable_utxos;              // usize: the Dust UTXOs no pending build reserves
balance.dust.balance_speck;                // u128: all the Dust, reserved or not (1 DUST = 10^15 SPECK)
balance.dust.spendable_speck;              // u128: the Dust a new build can draw on now
```

A build reserves the inputs it spends: shielded coins, unshielded UTXOs and Dust UTXOs. A reservation ends when a release hands the inputs back, or when its TTL ends (see [Pending reservations](#pending-reservations)). A reservation of unshielded UTXOs or Dust UTXOs also ends when a sync or resync sees the chain confirm the transaction. A confirmed shielded coin leaves the balance when a sync or resync drops it from the wallet's Zswap state.

A reserved coin or UTXO reads `spendable: false`. `spendable_shielded_value` and `spendable_unshielded_value` leave it out, and `shielded_value` and `unshielded_value` count it. In the same way, `spendable_utxos` and `spendable_speck` leave the reserved Dust out, and `balance_speck` counts it. So a wallet whose only Dust UTXO a pending build reserves reads a `spendable_speck` of 0 and a positive `balance_speck`.

`token_type` is typed: `UnshieldedTokenType` for `balance.unshielded[i]`, `ShieldedTokenType` for `balance.shielded.coins[i]`. Use `.token_type_hex()` for display / log output (64-char hex, no `0x` prefix). For comparison against the chain's native unshielded token, use `token_type == midnight_core::wallet::NIGHT`. NIGHT is denominated in STAR (1 NIGHT = 10⁶ STAR); DUST in SPECK (1 DUST = 10¹⁵). The byte pattern `[0; 32]` in a `ShieldedTokenType` is **not** NIGHT. See [`tokens.md`](tokens.md) for the two-ledger model.

Every reading returns an owned value taken under a short read lock, so nothing a caller holds can block a resync running in another task.

## Dust registration

Before NIGHT holdings can generate spendable Dust, the wallet must publish a **dust registration** that binds its dust address to its unshielded address. One `TransactionRequest::register_dust` registers one tNIGHT UTXO and pays its own fee from that UTXO, so a wallet with no Dust can register. The rustdoc of [`register_dust`](../crates/midnight-types/src/request.rs) gives the full rule.

```rust
let pending = provider.send_transaction(TransactionRequest::register_dust()).await?;
pending.get_receipt().await?;
```

A UTXO whose creation time the indexer did not report counts as created one hour before the chain time of the fill. The transaction takes a few seconds to land; Dust starts generating once it's finalized.

tNIGHT that arrives after the first registration generates Dust with no further call. tNIGHT that the wallet already held does not: send `register_dust` again until `DustBalance::unregistered_night_utxos` is 0. Each registration registers one UTXO:

```rust
use std::time::{Duration, Instant};

// A balance read does not sync.
wallet.sync(&provider).await?;
let mut left = wallet.balance().await.dust.unregistered_night_utxos;
while left > 0 {
    provider
        .send_transaction(TransactionRequest::register_dust())
        .await?
        .get_receipt()
        .await?;
    // The indexer serves the registration a moment after the node finalizes it.
    let deadline = Instant::now() + Duration::from_secs(60);
    while wallet.balance().await.dust.unregistered_night_utxos == left {
        if Instant::now() >= deadline {
            return Err("the indexer did not serve the Dust registration".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        wallet.sync(&provider).await?;
    }
    left = wallet.balance().await.dust.unregistered_night_utxos;
}
```

To send the Dust to another wallet, send `TransactionRequest::register_dust_to(target)` in the loop (see [Registration to another Dust address](#registration-to-another-dust-address)).

See [`dust-and-fees.md`](dust-and-fees.md) for the full Dust model, generation rate, and how fees are balanced.

### Registration to another Dust address

A registration can send the Dust to the Dust address of another wallet. For example, a cold wallet that holds the tNIGHT registers it to the Dust address of a hot wallet. The hot wallet then gets the Dust. The cold wallet fills its transactions `without_fee`, and the hot wallet pays their fees with `TransactionRequest::sponsor` ([`examples/combine-and-sponsor`](../examples/combine-and-sponsor) shows that flow). Read the address with `parse_dust_address`, and give the key to `register_dust_to`. `network` is the wallet's network, `wallet.network()`:

```rust
use midnight_core::wallet::parse_dust_address;

let hot_address = hot_wallet.dust_address(); // mn_dust_undeployed1...
let target = parse_dust_address(&hot_address, &network)?;
provider
    .send_transaction(TransactionRequest::register_dust_to(target))
    .await?
    .get_receipt()
    .await?;
```

The registration sets the Dust address of the whole NIGHT address, not only of the UTXO it spends:

- tNIGHT that reaches the address after the registration generates Dust for the Dust address that the last registration set.
- A later `register_dust` points the address back to the wallet's own Dust address. So send `register_dust_to(target)` each time until `unregistered_night_utxos` is 0.
- tNIGHT that already generates Dust keeps its Dust address until the wallet spends it.

The cold wallet gets no Dust of its own. Its registrations still pay their own fees, and its other transactions need a sponsor.

## Transfers

Two flavors, same shape. A `TransactionRequest` names the transfer, and `send_transaction` fills, funds, proves, signs and sends it in one step:

```rust
use midnight_core::provider::TransactionRequest;
use midnight_core::wallet::{NIGHT, parse_unshielded_recipient};

let recipient = parse_unshielded_recipient(&recipient_address, &network)?;
let pending = provider
    .send_transaction(TransactionRequest::unshielded(NIGHT, amount_in_star, recipient))
    .await?;
pending.get_receipt().await?;
```

```rust
use midnight_core::wallet::parse_shielded_recipient;

let recipient = parse_shielded_recipient(&recipient_shielded_address, &network)?;
let pending = provider
    .send_transaction(TransactionRequest::shielded(token_type, amount, recipient))
    .await?;
```

`recipient` is typed: an `UnshieldedRecipient`, the user address that an `mn_addr_*` address carries, or a `ShieldedRecipient`, the two public keys that an `mn_shield-addr_*` address carries. `parse_unshielded_recipient` and `parse_shielded_recipient` read one from an address string and check its network against `network`, the wallet's network (`wallet.network()`). A parse refuses an address of another network with `WalletError::AddressNetworkMismatch`, and a string that is not such an address with `WalletError::InvalidAddress`. The parse runs before the transfer, so a bad address costs no resync. To pay a wallet of your own, use `wallet.unshielded_recipient()` and `wallet.shielded_recipient()`, or the same methods of its `Seed`. `TransactionRequest::shielded` accepts any `ShieldedTokenType`; only `register_dust` is intrinsically NIGHT-specific. See [`tokens.md`](tokens.md) for the asset/ledger model. `send_transaction`:

1. Fills the transaction context from node queries: the node's network and ledger parameters, and the time of its best block.
2. Hands it to the wallet's `fund_transaction`, which resyncs, then takes a write lock on the wallet, selects inputs from its local UTXO set, balances the Dust fee with a `speculative_spend` loop over mock proofs, and reserves the selected inputs so the next concurrent build can't re-pick the same coins.
3. Proves the transaction with the provider's prover, with the wallet free.
4. Hands it to the wallet's `sign_transaction`, which replaces the placeholder signature of each unshielded input and Dust registration that the funding added. The reservation then reaches `pending.json`.
5. Submits the signed bytes to the node and returns a `PendingTx`.

Each intent of the transaction carries a TTL. The chain does not apply a transaction after the TTL of one of its intents. By default the TTL is the chain's `global_ttl` after the time of the node's best block that the fill read. That is the longest TTL that the chain accepts. `global_ttl` is a chain parameter (14 days on the networks that midnight-node ships). To make a transaction expire sooner, call `.with_ttl(duration)` on the request:

```rust
use std::time::Duration;

let request = TransactionRequest::unshielded(NIGHT, amount_in_star, recipient)
    .with_ttl(Duration::from_secs(300));
let pending = provider.send_transaction(request).await?;
```

The TTL counts in whole seconds. A TTL under one second or longer than `global_ttl` fails the fill with `WalletError::TtlOutOfRange`, before it spends or proves anything. A fee-less shielded transfer (`without_fee`) and a shielded swap half are Zswap offers with no intent, so they cannot carry a TTL, and their fill fails with `WalletError::TtlWithoutIntent`. A sponsor keeps the TTL of the transaction that it pays for, so its fill refuses a TTL with `WalletError::Transfer`.

The wallet draws the coins and UTXOs of a transfer largest first, which spends the fewest inputs. To spend small coins first, for example to consolidate them, call `.with_coin_selection(CoinSelection::SmallestFirst)` on the request. Its rustdoc gives the cost of that order and the requests that read it:

```rust
use midnight_core::provider::CoinSelection;

let request = TransactionRequest::shielded(token_type, amount, recipient)
    .with_coin_selection(CoinSelection::SmallestFirst);
```

To fill the transaction without a send, call `fill` in place of `send_transaction`. It returns the signed `TxEnvelope`, which carries the reservation of its inputs:

```rust
let envelope = provider
    .fill(TransactionRequest::shielded(token_type, amount, recipient))
    .await?;
let pending = provider.send_tx_envelope(envelope).await?;
```

`fill` reserves the spent inputs just like `send_transaction`. `send_tx_envelope` hands them back when the node never gets the transaction or rejects it, and a drop of the envelope hands them back at once. `TxEnvelope::into_bytes` gives the bytes for another party and keeps the inputs reserved until a sync sees the chain spend them or their TTL ends (see [Pending reservations](#pending-reservations)).

A request made `without_fee`, and a swap half, give a transaction that pays no fee. `send_tx_envelope` and `prepare` refuse it with `ProviderError::Transaction`, because the node refuses it. `send_transaction` refuses such a request with the same error before its fill, so it reserves no input and makes no proof. Hand its bytes to the party that pays its fee with `TransactionRequest::sponsor`.

## Submission and waiting

`send_tx_envelope` (and so `send_transaction`) connects to the node, submits the bytes as an unsigned `Midnight::send_mn_transaction` extrinsic via `submit_and_watch`, and hands back a `PendingTx`:

```rust
let pending = provider.send_tx_envelope(envelope).await?;
println!("ext: {:x}", pending.extrinsic_hash());

let receipt = pending.get_receipt().await?;
```

`get_receipt` waits for the transaction to reach a finalized block and returns its `TransactionReceipt`. It consumes the handle, so read the hashes before the wait. Cancelling the future is safe but does not retract the extrinsic from the mempool.

`get_receipt` returns `Ok` only when the chain applied the transaction. When the transaction landed but did not apply (`PartialSuccess` or `Failure`), it fails with `ProviderError::NotApplied`, whose `NotApplied` holds the `TransactionReceipt` with the verdict.

When a wait fails for any other reason, the error is `ProviderError::Submission` carrying a typed `SubmitError`. Match its variants to decide what to do next: `Invalid` is a definitive rejection (safe to rebuild and resubmit with fresh inputs), `Dropped` / `NodeError` are not (the tx may still be re-included; resubmitting the same inputs risks a double spend), `WatchStream` means only the watch subscription broke (the tx stays in the pool and may still land), and `VerdictFetch` means the tx landed but its events couldn't be decoded (it's on chain, so don't resubmit, re-query for the verdict). The send itself fails with `SubmitRpc`, and after it the node can still hold the tx.

When the fate is unknown, a transaction can still land until its TTL passes. After that, a rebuild from the same inputs is safe from a double spend, and the wallet frees the inputs at its next build. Fill with `.with_ttl(duration)` (see [Transfers](#transfers)) to make that wait shorter than `global_ttl`.

## Recovering a coin the wallet cannot discover

A shielded coin reaches its owner through the discovery ciphertext on its output. That channel fails when the party that built the transaction attached no ciphertext, or sealed it to a key the owner does not hold. The coin still exists, the owner's coin public key still owns it, and it stays spendable, as long as the owner can rebuild its `CoinInfo` (nonce, token type, value) from somewhere else. A contract that evolves one public nonce per mint is such a case: the owner reads the nonce from chain state.

Hand the rebuilt coin to `watch_for_coins` and the wallet claims it from the chain's own output, decrypting nothing:

```rust
use midnight_core::provider::{CoinInfo, HashOutput, Nonce, ShieldedTokenType};

let coin = CoinInfo {
    nonce: Nonce(HashOutput(nonce)),                  // rebuilt from chain state
    type_: ShieldedTokenType(HashOutput(token_type)),
    value,
};
wallet.watch_for_coins(vec![coin], &provider).await?;

// A claimed coin is spendable like any other.
let coins = wallet.spendable_shielded_coins().await;
```

Registration records the coin's commitment; a replay that meets the matching output claims the coin from that record. The order matters, and it is why `watch_for_coins` replays for you. A replay that meets an output it cannot claim collapses that Merkle leaf and moves on, and a resync resumes from its cursor rather than revisiting the event, so registering after the fact would change nothing on its own. `watch_for_coins` therefore replays the whole `zswapLedgerEvents` stream from its first event, with the registration in place. Dust state, unshielded state, and their cursors are untouched.

Every coin the wallet already holds is re-registered before that replay, so a rescan never costs you a coin. This matters because the ledger consumes a registration the moment a replay claims it: a coin recovered by an earlier `watch_for_coins` has no ciphertext to be found by, and a replay that started without it would collapse its leaf again.

That full replay costs more than a resync, so register every coin you know about in one `watch_for_coins` call, and treat this as a recovery step rather than a routine one. Registrations are persisted with the rest of the wallet state, so one made for a coin that has not landed yet survives a restart, and every later resync carries it rather than dropping it.

To check the outcome, look for the coin in `wallet.spendable_shielded_coins()`. If the coin does not appear there, it is not on chain yet, or its rebuilt `CoinInfo` is wrong. A wrong registration never matches an output, and every later replay carries it.

The registration is recorded and persisted before the replay starts. If the replay fails, the registration is still there. To retry, call `watch_for_coins` again with the same coins: the second registration changes nothing, and the replay runs again.

The replay runs against the indexer that the wallet synced from, and reads keep completing while it runs. The wallet runs the registration and the replay under one hold, so no resync runs between them.

## Pending reservations

In-flight spends that have been built but not yet confirmed on-chain are tracked in `PendingReservations`. They serve two purposes:

- **Prevent double-spending across builds.** Input selection skips reserved coins.
- **Drop on confirmation or TTL.** Event replay (initial sync and every resync) collects the dust nullifiers and unshielded UTXO keys it observes spent and clears the matching reservations at its commit point; `evict_expired` (which every funded build runs through the wallet's `funding`) drops entries whose TTL passed, as a backstop for transactions that never confirm.

Each reservation expires at the earliest TTL of the intents that its build made (see [Transfers](#transfers)). The wallet drops the reservation once the time of the chain's latest block passes that TTL, because the transaction can no longer land.

A reservation reaches `pending.json`, next to the wallet state, once its transaction is proven. A process that stops during a proof (Ctrl-C, a kill or a panic) leaves no reservation for that transaction in `pending.json`, so the next process can spend its inputs. A proven transaction keeps its reservation across a restart, also when its bytes never reach a node, because the wallet cannot know whether they left the process. `sign_transaction` writes it, because the provider calls it after the proof. A custom wallet that wraps `Wallet` ends its `sign_transaction` with the inner one, or the reservation never reaches disk.

The reservation travels with the transaction, from the `FundedTx` to the `ProvenTx`, the `TxEnvelope`, the `PreparedTx` and the `PendingTx`. A release drops it, for a transaction that can never land. It matches each entry by its input and by the stamp of its reservation, so a late release cannot drop an entry that a later build made over the same input. These paths release:

- A drop of a `FundedTx`, `ProvenTx`, `TxEnvelope` or `PreparedTx`, so a build whose proof fails, or that stops before its submit, hands its inputs back at once, with no async runtime. A contract call and a deploy that stop before their submit release this way, for example on a failed `append_pending`.
- `MidnightProvider::send_tx_envelope` and `prepare`, when the prepare fails (the connect included), or when they refuse a transaction that pays no fee.
- The `PendingTx`, when `get_receipt` reads `Invalid`. A send moves the reservation to the `PendingTx` before the call starts.

A release changes the wallet's memory at once. `pending.json` follows at the next write of the wallet: the next `sign_transaction`, or a sync whose commit moves a cursor or the ledger parameters, clears a confirmed reservation, or follows a failed save. Until then the file keeps a proven entry that a release dropped, and a restart before that write holds the entry until a sync sees the chain spend its inputs or its TTL ends.

A failed submit call (`SubmitRpc`) keeps the inputs reserved, because the node can still hold the transaction, and so does a dropped `PendingTx`. The bytes of `TxEnvelope::into_bytes` keep their inputs reserved too, so their reservation ends only when a sync sees the chain spend them, or when their TTL ends. Other than that, you don't normally interact with this directly: the funding reserves, and the sync loop clears.

## Lifecycle summary

```
MidnightProvider::new(node_url)
  ↓
Wallet::from_seed(seed)
  [.with_network(network)][.with_storage(dir)]
  ↓
.sync(&provider, indexer_url).await  or  .sync_with_progress(&provider, indexer_url).await → (progress, handle)
  │ take the node's network when no network is named
  │ check the node's network and the stored pin, and take the pin this sync carries
  │ subscribe zswap + unshielded + dust  (parallel)
  │ persist (metadata + binary state + pending)
  ↓
provider.with_wallet(&wallet)         or an Arc<Wallet>; provider.wallet() gives it back
  ↓
  wallet.balance()                       read-only
  wallet.sync(&provider)                 incremental refresh + re-persist
  wallet.sync_in_background(provider, pause)   sync in a task until the handle drops (an Arc<Wallet>)
      sync.wait_until(done)              wait until done holds for the balance (at the call and after each pass)
  wallet.watch_for_coins(coins, &provider)   claim coins with no usable ciphertext: register, then replay
  provider.send_transaction(request).await        fills, funds, proves, signs + sends → PendingTx
      TransactionRequest::unshielded(...)         an unshielded transfer
      TransactionRequest::shielded(...)           a shielded transfer
      TransactionRequest::register_dust()         registers one tNIGHT UTXO, self-funded
      TransactionRequest::register_dust_to(key)   the Dust goes to another wallet's Dust address
       │  provider.fill(request).await → TxEnvelope (no send)
       │
       └─ PendingTx
            └ .get_receipt().await → TransactionReceipt
```

## Examples

- [`examples/wallet-sync`](../examples/wallet-sync): end-to-end sync, balance display, optional dust registration, optional unshielded self-transfer. Runs against the local devnet by default, or against another network that runs ledger 9.
