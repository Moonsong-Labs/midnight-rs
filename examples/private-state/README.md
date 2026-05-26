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
    fn next_secret(&self, ps: &mut u64) -> u16 {   // typed args + return, typed &mut state
        *ps += 1;
        *ps as u16
    }
}
```

Each `next_secret()` returns the next value and advances the running counter. So calling `contribute()` twice returns 1 then 2 — the second call only knows to return 2 because the first call's `1` was persisted and reloaded. The chain sees only the disclosed contributions in `total`; the running counter stays off-chain.

The SDK loads the private state before the call and persists it after; the witness just reads and writes the typed `ps`. No string dispatch, no `Value`, no byte (de)coding. `PrivateState` is serialized with `serde` (a primitive like `u64` needs nothing; a struct derives `Serialize`/`Deserialize`).

## Run

Start the devnet (node + indexer) from the repository root, then wait until both are serving:

```bash
docker compose -f devnet/docker-compose.yml up -d   # from the repo root
while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
```

Run the example:

```bash
cargo run -p example-private-state
```

Output:

```
=== Midnight Private State Example ===

0. Syncing wallet and attaching the private-state store...
   synced.

1. Deploying secret-counter...
   address: ...
   on-chain total = 0, private counter = 0

2. Calling contribute()...
   witness disclosed 1; on-chain total = 1, persisted private counter = 1
3. Calling contribute()...
   witness disclosed 2; on-chain total = 3, persisted private counter = 2
```

The private counter advances 0 → 1 → 2 across calls (loaded and persisted each time) while the chain only sees the disclosed contributions in `total` (1, then 3). The counter is written to disk, so it survives restarts.

The store also supports password-encrypted (Argon2id + AES-256-GCM) export/import for backup and device migration — see [`docs/private-state.md`](../../docs/private-state.md).

Stop the devnet (from the repo root):

```bash
docker compose -f devnet/docker-compose.yml down
```

## Recompile the contract

The contract source and compiled artifacts live in [`devnet/contracts/secret-counter`](../../devnet/contracts/secret-counter). If you modify `secret_counter.compact`, recompile with the [extended Compact compiler](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions) (ZK keys are required for on-chain deployment):

```bash
cd ../../devnet/contracts/secret-counter && compactc secret_counter.compact compiled
```
