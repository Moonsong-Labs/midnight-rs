# Wallet

The SDK's wallet covers Midnight's three asset legs:

- **Shielded coins** — Zswap-based privacy coins
- **Unshielded UTXOs** — public UTXO-model balances (NIGHT lives here)
- **Dust** — fee-token, generated continuously from NIGHT holdings

`Wallet` itself is a pure state machine. All network I/O — sync, resync, transfer building, transaction context construction — is driven by `MidnightProvider`, which owns the wallet behind `Arc<RwLock<Wallet>>`. Most callers never construct a `Wallet` directly; they call `MidnightProvider::sync_wallet` and then operate through the provider.

## Address derivation

If all you need is an address (e.g. to print a faucet target), no sync is required:

```rust
use midnight_wallet::{Network, WalletSeed, address};

let seed = WalletSeed::try_from_hex_str(
    "13e772040e60bf21946c1f15dbf8161cf4ff05266f62830437d5c1c7ec72480f",
)?;

let unshielded = address::derive_unshielded(&seed, Network::Preprod);
let shielded   = address::derive_shielded(&seed, Network::Preprod);
// mn_addr_preprod1...
// mn_shield-addr_preprod1...
```

Addresses are deterministic per `(seed, network)` and include the network suffix in the bech32 HRP. The `network` argument accepts both [`Network`] enum variants (`Network::Preprod`, `Network::Mainnet`, etc.) and free-form strings via `impl Into<Network>`, so `"preprod"` and `env::var("MIDNIGHT_NETWORK")` also work. Unknown names round-trip through `Network::Other(String)` — useful for custom devnets.

`Network::Mainnet` produces bech32 HRPs without a `_<name>` suffix (`mn_addr1...`, `mn_shield-addr1...`), matching the upstream convention; all other variants get the suffix.

## Sync

```rust
use midnight_provider::{MidnightProvider, Network};

let provider = MidnightProvider::new(node_url, indexer_url)?
    .sync_wallet(seed, Network::Preprod)
    .with_storage(storage_dir)        // optional; in-memory only without it
    .await?;
```

`sync_wallet` returns a [`SyncWalletBuilder`] that defers the work. The builder runs three concurrent indexer subscriptions and returns once all three have caught up:

| Subscription | What it fills |
|---|---|
| `zswapLedgerEvents` | Shielded coin state (Merkle tree + nullifiers) |
| `unshieldedTransactions` | UTXO set for the wallet's unshielded address |
| `dustLedgerEvents` | Dust UTXOs derived from NIGHT holdings |

Dust sync from genesis can take 30+ minutes on a mainnet-sized history. Progress is checkpointed to disk after each batch when `with_storage(...)` is set, so subsequent runs resume from the last cursor.

For long syncs where you want UI updates, switch the builder's terminal step from `.await` to `.stream()`:

```rust
use midnight_provider::SyncProgress;

let (mut rx, handle) = MidnightProvider::new(node_url, indexer_url)?
    .sync_wallet(seed, Network::Preprod)
    .with_storage(storage_dir)
    .stream();

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

let provider = handle.await?;
```

`.await` runs the sync in the current task; `.stream()` spawns it in a background task and gives you progress events plus a [`SyncHandle`] that resolves to the synced provider.

To incrementally refresh an already-synced wallet without replaying from the cursor's start, call `provider.resync_wallet().await`. Most provider methods (`balance` excepted) call this internally before doing anything that depends on a fresh chain view.

### Persistence

When `storage_dir` is `Some`, sync writes to:

```
{storage_dir}/{network}/{sha256(seed)[..16]}/
  ├── metadata.json     event cursors, last block, last tx id, generation pointers
  ├── zswap-N.bin       tagged-serialized ZswapLocalState
  ├── dust_wallet-N.bin tagged-serialized DustWallet
  └── pending.json      in-flight spend reservations (see below)
```

Use `Wallet::default_storage_dir()` for `~/.midnight/wallets/`. Writes are generation-based: new `zswap-N+1.bin` / `dust_wallet-N+1.bin` files are written first, then `metadata.json` is atomically renamed, then the old generation is cleaned up. A crash mid-write leaves the previous generation intact.

## Balance

```rust
let balance = provider.balance().await?;

balance.shielded.coins;          // Vec<ShieldedCoinBalance { token_type, value }>
balance.shielded.total_count;    // usize
balance.unshielded;              // Vec<UnshieldedUtxoInfo { token_type, value }>
balance.dust.spendable_utxos;    // usize
balance.dust.balance_speck;      // u128  (1 DUST = 10^15 SPECK)
```

`token_type` is typed: `UnshieldedTokenType` for `balance.unshielded[i]`, `ShieldedTokenType` for `balance.shielded.coins[i]`. Use `.token_type_hex()` for display / log output (64-char hex, no `0x` prefix). For comparison against the chain's native unshielded token, use `token_type == midnight_provider::NIGHT`. NIGHT is denominated in STAR (1 NIGHT = 10⁶ STAR); DUST in SPECK (1 DUST = 10¹⁵). The byte pattern `[0; 32]` in a `ShieldedTokenType` is **not** NIGHT — see [`tokens.md`](tokens.md) for the two-ledger model.

For lower-level access (parameters, raw state):

```rust
let wallet = provider.wallet().await?;
wallet.parameters().dust.night_dust_ratio;
wallet.zswap_event_id();
wallet.last_block_height();
// guard released when `wallet` goes out of scope — keep it short
```

`provider.wallet().await` acquires a read lock; `provider.wallet_mut().await` acquires a write lock. Hold either only as long as needed — background sync and other readers are blocked while a guard is alive. For the rare case where you need the raw `Arc<RwLock<Wallet>>` handle (e.g. to spawn a task that owns the wallet and acquires its own locks), use `provider.wallet_handle()`.

## Dust registration

Before NIGHT holdings can generate spendable Dust, the wallet must publish a one-time **dust registration** that binds its dust address to its unshielded address. This is a transaction (paid in… Dust, from the genesis allocation, or a faucet handout):

```rust
let pending = provider.register_dust(None).await?;     // None = use genesis ctime
let (in_block, _) = pending.wait_best().await?;
```

Pass `Some(utxo_ctime)` to register against a specific funding UTXO; pass `None` to use what the wallet finds. The transaction takes a few seconds to land; Dust starts generating once it's finalized.

See [`dust-and-fees.md`](dust-and-fees.md) for the full Dust model, generation rate, and how fees are balanced.

## Transfers

Two flavors, same shape — each method returns a builder; `.await?` builds and submits in one step:

```rust
use midnight_wallet::NIGHT;

let pending = provider
    .transfer_unshielded(NIGHT, amount_in_star, &recipient_address)
    .await?;
let (in_block, _) = pending.wait_best().await?;
```

```rust
let pending = provider
    .transfer_shielded(token_type, amount, &recipient_shielded_address)
    .await?;
```

`recipient` is the bech32 address string (`mn_addr_*` for unshielded, `mn_shield-addr_*` for shielded). `transfer_shielded` accepts any `ShieldedTokenType`; only `register_dust` is intrinsically NIGHT-specific. See [`tokens.md`](tokens.md) for the asset/ledger model. Each builder:

1. Takes a write lock on the wallet, resyncs, builds a `LedgerContext`.
2. Selects inputs from the wallet's local UTXO set.
3. Balances Dust fees via a `speculative_spend` loop (mock proofs first, real proofs once balanced).
4. Reserves the selected inputs in `pending.json` so the next concurrent build can't re-pick the same coins.
5. Submits the proven bytes to the node and returns a `PendingTx`.

To inspect the proven transaction without submitting (e.g. to route submission elsewhere, or to size-check), call `.build()` on the builder instead of awaiting it:

```rust
let result = provider
    .transfer_shielded(token_type, amount, &recipient)
    .build()
    .await?;
// result: TransferResult { tx_bytes, dust_batches, spent_unshielded_inputs }
let pending = provider.submit(&result.tx_bytes).await?;
```

`.build()` reserves the spent inputs just like the awaitable path; until the submitted transaction is observed on-chain (or its TTL expires), the inputs stay reserved.

## Submission and waiting

`MidnightProvider::submit(tx_bytes)` connects to the node, submits the bytes as an unsigned `Midnight::send_mn_transaction` extrinsic via `submit_and_watch`, and hands back a `PendingTx`:

```rust
let pending = provider.submit(&tx_bytes).await?;
println!("ext: {}", pending.extrinsic_hash_hex());

let (best,      pending) = pending.wait_best().await?;
let (finalized, _pending) = pending.wait_finalized().await?;
```

`wait_best` / `wait_finalized` consume `self` and return it back so callers re-bind through each step without `let mut`. Cancelling either future is safe but does not retract the extrinsic from the mempool.

## Pending reservations

In-flight spends that have been built but not yet confirmed on-chain are tracked in `PendingReservations`, persisted as `pending.json` next to the wallet state. They serve two purposes:

- **Prevent double-spending across builds.** Input selection skips reserved coins.
- **Drop on confirmation or TTL.** `apply_dust_event` / `apply_unshielded_event` clear matching reservations as events arrive; `evict_expired` (called from `build_context_inner`) drops entries whose TTL window elapsed. Transaction TTL defaults to one hour.

You don't normally interact with this directly — `transfer_*` and `register_dust` reserve and the sync loop clears.

## Lifecycle summary

```
new(node_url, indexer_url)
  └─ sync_wallet(seed, network, storage_dir)
       │ subscribe zswap + unshielded + dust  (parallel)
       │ persist (metadata + binary state + pending)
       ↓
  provider.balance()                     read-only
  provider.wallet() / .wallet_mut()      lower-level read/write access
  provider.resync_wallet()               incremental refresh
  provider.register_dust(None).await         one-time, before Dust generates
  provider.transfer_unshielded(...).await    builds + submits → PendingTx
  provider.transfer_shielded(...).await      builds + submits → PendingTx
       │     .build().await → TransferResult (escape hatch, no submit)
       │
       └─ PendingTx
            │ .wait_best().await
            └ .wait_finalized().await
```

## Examples

- [`examples/wallet-sync`](../examples/wallet-sync) — end-to-end sync, balance display, optional dust registration, optional unshielded self-transfer. Runs against preprod (with faucet funding) or a local devnet.
