# Private State Example

Deploys a contract with a **stateful witness** and calls it twice, showing the SDK's per-contract private-state loop end to end: with a `PrivateStateProvider` attached, a circuit call loads the contract's private state, hands it to the witness, and persists whatever the witness wrote, so the next call sees the update.

## Contract

```compact
import CompactStandardLibrary;

export ledger total: Counter;
witness next_secret(): Uint<16>;            // value comes from private state

export circuit contribute(): Uint<16> {
  const s = next_secret();
  total.increment(disclose(s));             // fold the secret into public total
  return disclose(s);
}
```

The witness `next_secret` is the private input. The `contract!` macro generates a typed `Witnesses` trait, so the implementation is plain Rust — one typed method per witness over a typed `PrivateState`:

```rust
impl secret_counter::Witnesses for SecretWitness {
    type PrivateState = u64;                       // your type; the chain doesn't define it
    fn next_secret(&self, ps: &mut u64) -> Result<u16, secret_counter::WitnessError> {   // typed args + return, typed &mut state
        *ps += 1;
        Ok(*ps as u16)
    }
}
```

Each `next_secret()` returns the next value and advances the running counter. So calling `contribute()` twice returns 1 then 2 — the second call only knows to return 2 because the first call's `1` was persisted and reloaded. The chain sees only the disclosed contributions in `total`; the running counter stays off-chain.

The SDK loads the private state before the call and persists it after; the witness just reads and writes the typed `ps`. No string dispatch, no `Value`, no byte (de)coding. `PrivateState` is serialized with `serde` (a primitive like `u64` needs nothing; a struct derives `Serialize`, `Deserialize` and `Default`). The SDK starts from `Default` when the store holds no state for the contract.

## Run

Start the devnet (node, indexer and proof server) from the repository root:

```bash
make dev-up   # from the repo root
```

`make dev-up` waits for the chain to produce a block past genesis. Starting the compose file directly is not enough: a transaction built while only genesis exists carries an expired TTL and the node rejects it.

Run the example:

```bash
make run-private-state   # from the repo root
```

The first call uses `.send()`. It returns a `PendingCall`, which shows the transaction hash before the chain's verdict. Its `wait_finalized` then confirms the private-state snapshot.

Output:

```
=== Midnight Private State Example ===

0. Syncing wallet and attaching the private-state store...
   synced.

1. Deploying secret-counter...
   address: ...
   on-chain total = 0

2. Calling contribute()...
   submitted transaction ...
   finalized; the private-state snapshot is Confirmed
   witness disclosed 1; on-chain total = 1
3. Calling contribute()...
   witness disclosed 2; on-chain total = 3

The calls disclosed [1, 2] because the SDK loaded and persisted the
counter between calls. The private state survives across calls, and the
chain sees only the disclosed contributions (total = [1, 3]).
The state is on disk under .../midnight-private-state-example, so it also survives restarts.

=== Done ===
```

The private counter advances 0 → 1 → 2 across calls (loaded and persisted each time) while the chain only sees the disclosed contributions in `total` (1, then 3). The counter is written to disk, so it survives restarts.

The store also supports password-encrypted (Argon2id + AES-256-GCM) export/import for backup and device migration — see [`docs/private-state.md`](../../docs/private-state.md).

Stop the devnet (from the repo root):

```bash
make dev-down
```

## Recompile the contract

The contract source lives in [`devnet/contracts/secret-counter`](../../devnet/contracts/secret-counter). `make run-private-state` first compiles each devnet contract whose output is missing or out of date. So a change to `secret_counter.compact` or to `COMPACT_REV` in the root [`Makefile`](../../Makefile) takes effect on the next run. The `Makefile` runs the Compact compiler fork that `COMPACT_REV` names from its image, so that step needs Docker ([Compile a contract](../../README.md#compile-a-contract) gives the details). To recompile every contract, run this from the root of the repository:

```bash
make compile-contracts  # recompile every contract in devnet/contracts
```

The compiled artifacts keep the layout that [`devnet/contracts/README.md`](../../devnet/contracts/README.md#layout) describes. A deploy on chain needs the keys in `keys/`.
