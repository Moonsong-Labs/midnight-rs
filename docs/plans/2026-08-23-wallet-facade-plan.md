# Plan: put the wallet behind a facade

**Date**: 2026-08-23

Implements #196. The provider holds a concrete `Wallet` behind `Arc<RwLock<_>>` and hands the lock guard to callers, so nothing else can play the wallet's part. This plan measures the coupling, takes what transfers from the reference design in [midnight-wallet](https://github.com/midnightntwrk/midnight-wallet), and lays out the change in steps that each leave the tree green.

## What the coupling actually is

Counting references overstates it. `midnight-provider` names `Wallet` 88 times across its sources and calls about 22 of its methods, but most of those are readings, and readings are easy to put behind a trait. The coupling that matters is in three places.

### The guard accessors publish the lock

```rust
pub async fn wallet(&self) -> Result<RwLockReadGuard<'_, Wallet>, ProviderError>
pub async fn wallet_mut(&self) -> Result<RwLockWriteGuard<'_, Wallet>, ProviderError>
```

These commit every caller to one concrete type and to `tokio::sync::RwLock` as the way it is shared. Neither can change without breaking the signature.

They are used in five places outside the provider crate, and every use is narrow. `examples/wallet-sync` and `dust_registration_offer` take the guard to iterate `unshielded_utxos()`. `midnight-contract`'s `deploy`, `call` and `maintenance` take `wallet_mut()` to call `reserve_pending(..)`. Nothing outside the crate holds a guard across anything else. So the public surface to replace is two readings and one mutation, not a wallet.

### `TransferBuilder` borrows the concrete wallet

```rust
// crates/midnight-wallet/src/transfer.rs:63
pub fn new(state: &'a Wallet, context: Arc<LedgerContext<DefaultDB>>, proof_provider: Arc<dyn ProofProvider<DefaultDB>>) -> Self
```

Inside, a build reads four things from it: `seed()` (4 sites), `network()` (2), `parameters()` (1), `dust_wallet()` (1). Everything else a build needs is already in the `LedgerContext` it is handed. That is the whole of what a build wants from a wallet.

### `TransferGuard` holds the write lock across select and reserve

This is the one lock hold that has to survive, and the reason it exists is correct. Selection reads the pending set, reservation writes it, and #177 showed that letting another consumer select in between draws the same input twice. #183 narrowed the hold to exactly that span, with proving outside it. The facade must keep that property, so it cannot simply hide the lock and expose async methods that each take it briefly.

## What the reference does, and what transfers

The facade package composes three wallets behind one API. Its structure is three layers, and the layering is the useful part.

**Capabilities** are pure functions over an immutable `CoreWallet`: `coinsAndBalances`, `keys`, `serialization`. No I/O, no lock, no `this`.

**Services** do I/O: transaction history, submission, proving, validation. They are injected.

**The API** composes the two over a state cell. A mutation is `(secretKey, state, request) -> [result, newState]`, run under the cell's lock by the runtime, so no caller ever holds a lock and no second wallet type exists to own one. `balanceTransactions` returns a new transaction and block data; it does not mutate in place.

What transfers to midnight-rs:

- **The seam is the API, not the aggregate.** The reference's `WalletFacade` aggregates shielded, unshielded and dust. Our `Wallet` already does. What we lack is a trait the provider programs against, so the facade here is a trait plus one implementation, not a new aggregate.
- **Pure transitions make the lock hold short and internal.** `open_transfer_guard` already snapshots state and `reserve` already takes the `PreparedTransfer` the snapshot produced. That is a transition in all but shape. Naming it as one function on the trait, `select_and_reserve(request) -> Prepared`, keeps the hold inside the implementation, which is what #175 could not do and #177 asked for.
- **Services injected, not reached for.** The provider already injects the proof provider. The indexer and node are the other two, and a facade should take them the same way rather than the wallet crate calling out.

What does not transfer:

- **Effect and RxJS.** The reference's state is an `Observable`, and every mutation is an `Effect`. We have `async fn` and `tokio`, and a trait of `async fn`s is the idiomatic shape. The reference's *discipline* transfers; its machinery does not.
- **Per-type wallets behind the facade.** The reference has three wallets because it ships three packages. Splitting our `Wallet` into three to match would be a rewrite with no consumer asking for it, and it is not what #196 asks for.

## The design

One trait in `midnight-wallet`, named for the role rather than the struct, with `Wallet` as its first implementation.

```rust
pub trait WalletFacade: Send + Sync {
    // Identity, cheap and lock-free to read.
    fn network(&self) -> Network;
    fn unshielded_address(&self) -> String;
    fn shielded_public_keys(&self) -> ShieldedPublicKeys;

    // Readings. Each returns an owned value, never a guard.
    async fn balance(&self) -> WalletBalance;
    async fn unshielded_utxos(&self) -> Vec<TrackedUtxo>;
    async fn spendable_shielded_coins(&self) -> Vec<CoinInfo>;
    async fn dust_synced(&self) -> bool;

    // Transitions. Selection and reservation are one call, so the hold that
    // keeps them consistent lives inside the implementation.
    async fn prepare_transfer(&self, request: TransferRequest) -> Result<ReservedBuild, WalletError>;
    async fn reserve(&self, spend: SpentInputs, reserved_at: Timestamp) -> Result<(), WalletError>;
    async fn release(&self, spend: &SpentInputs) -> Result<(), WalletError>;

    // Sync. The plan/run/commit split stays, because the replay must run
    // without the lock and the commit must take it.
    async fn resync(&self, indexer: &IndexerClient) -> Result<(), WalletError>;
    async fn chain_pin(&self) -> Option<ChainPin>;
    async fn set_chain_pin(&self, pin: ChainPin);
}
```

Three things about this shape are deliberate.

**Readings return owned values.** `balance()` returns a `WalletBalance`, not a guard over a wallet that has a balance. That is what removes `RwLockReadGuard` from the public API. The cost is a clone per reading, which these types already pay at most call sites today.

**`prepare_transfer` is the transition.** It replaces `open_transfer_guard` plus `TransferGuard::reserve`. The request carries what the three transfer builders take today (token, amount, recipient, coin selection, whether to pay fees), and the result is the `ReservedBuild` the provider already proves from. The write lock is taken and released inside the implementation, and proving stays outside it, as #183 established.

**`reserve` stays public for the contract paths.** `deploy`, `call` and `maintenance` build against a `LedgerContext` and reserve afterwards. #177 recorded that this split is a known weakness on those paths, and fixing it means giving them a `prepare_*` transition of their own. That is a second piece of work. Until it lands, they need `reserve`, and a trait method is a better home for it than `wallet_mut().reserve_pending(..)`.

### What `TransferBuilder` takes

`TransferBuilder::new(&Wallet, ..)` becomes `TransferBuilder::new(&dyn BuildInputs, ..)` where `BuildInputs` is the four readings a build uses: `seed`, `network`, `parameters`, `dust_wallet`. `Wallet` implements it. This is the smallest change that removes the concrete type from the build path, and it is independent of everything else here, so it goes first.

### What the provider holds

```rust
wallet: Option<Arc<dyn WalletFacade>>,
```

The `RwLock` moves inside `Wallet`'s implementation of the trait, as a private field. The provider stops knowing how the wallet is shared, which is the property #175 needed and could not have.

`with_wallet` takes `impl WalletFacade + 'static`. `sync_wallet` keeps building a `Wallet` and wraps it.

### What goes away

`wallet()` and `wallet_mut()` are removed, not deprecated. The five external callers each move to a trait method: `unshielded_utxos()` for the two readers, `reserve(..)` for the three contract paths. Keeping the guard accessors alongside the trait would leave the old coupling available, and every new caller would reach for the shorter name.

## Order of work

Each step compiles, passes the workspace suite, and passes `make test-e2e` on its own. None depends on a later one.

1. **`BuildInputs` for `TransferBuilder`.** Four-method trait, `Wallet` implements it, builder takes `&dyn BuildInputs`. Touches `transfer.rs` only. Removes the concrete type from the build path.
2. **Owned readings on the provider.** Add `unshielded_utxos()` returning `Vec<TrackedUtxo>` beside the existing `balance()`. Move the two external readers onto it. Touches the provider and two callers.
3. **`reserve` and `release` on the provider.** Typed methods taking `SpentInputs`, replacing the three `wallet_mut().reserve_pending(..)` sites in `midnight-contract`. After this step nothing outside the provider crate calls a guard accessor.
4. **Remove `wallet()` and `wallet_mut()`.** The compiler lists what is left inside the provider crate; each site becomes a private helper or a trait call.
5. **`prepare_transfer` as one transition.** Fold `open_transfer_guard` and `TransferGuard::reserve` into one method. `TransferGuard` becomes private to the implementation. The existing `the_wallet_is_readable_while_a_build_proves` test from #183 guards that proving stays outside the hold.
6. **Lift the trait.** Extract `WalletFacade` from the provider's private helpers, implement it on `Wallet` with the `RwLock` inside, and change the field to `Arc<dyn WalletFacade>`. This is the step that lands #196, and by now it is mostly moving code that already has the right shape.
7. **Drop `midnight-contract`'s direct dependency on `midnight-wallet`** if step 3 left it with none. Check with `cargo tree`; the contract crate reaches the wallet through the provider today.

Steps 1 to 4 are each small and independently mergeable. Steps 5 and 6 are the design change. Step 7 is a cleanup that may turn out to be a one-line `Cargo.toml` edit.

## What to settle before step 5

**One trait or two.** Readings and transitions have different callers: a UI reads, a build transitions. Splitting them (`WalletReadings`, `WalletTransitions`) lets a read-only consumer take the smaller one. Against that, every implementation will implement both, and two traits means two `Arc<dyn>` or a supertrait. Start with one; split only when a consumer appears that wants half.

**Whether the contract paths get their own transition now.** Step 3 keeps them on `reserve` after a `LedgerContext` build, which is the split #177 called out. Folding them into a `prepare_deploy` / `prepare_call` transition is the right end state, and it touches `midnight-contract`'s build pipeline, which #182 has just reshaped. Do it as its own change after step 6, against the trait, rather than inside this plan.

**Whether an external wallet is a real target.** #178 recorded that a trait seam does not enable a browser extension or an HSM while `build_context` needs the concrete wallet. #182 split the context since. After step 1, a build needs `seed`, `network`, `parameters` and `dust_wallet`, and `seed` is the one an external signer will not hand over. So the trait as designed enables a *different local* wallet, not a remote signer. That is what #196 asks for. A remote signer needs `prepare_transfer` to return something unsigned and a separate signing step, which is a further change and should be named as one rather than implied.

## What this does not change

- The sync pipeline. `sync_inner`, `resync_plan` and `commit_resync` keep their shape; the trait's `resync` wraps them.
- The reservation model. `PendingReservations` and its TTL eviction are unchanged; only the door to them moves.
- The chain pin. It stays on `Wallet`, exposed through the trait.
- Anything in `midnight-contract`'s build pipeline beyond the three `reserve` sites.
