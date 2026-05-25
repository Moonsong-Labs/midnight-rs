# Per-contract private state

Some Compact contracts use **witnesses** that are themselves stateful: a counter the
caller keeps secret, an unspent-note set, a running commitment opening. The witness
value the circuit consumes on call _N+1_ depends on what happened on call _N_. That
"between calls" data is the contract's **private state** — it never touches the chain,
but it has to survive across calls and across process restarts.

This document explains the private-state model on Midnight, how the TypeScript
reference SDK ([midnight-js](https://github.com/midnightntwrk/midnight-js)) exposes it,
and the `PrivateStateProvider` this SDK ships.

## The model

A Midnight contract call runs the circuit locally to produce a transcript and a ZK
proof. Witnesses are the circuit's private inputs. In the full Compact model a witness
is not a pure function of its arguments — it reads the current private state and may
return an updated private state alongside the value the circuit uses:

```
witness(currentPrivateState, ...args) -> (newPrivateState, value)
```

The chain only ever sees the proof and the disclosed outputs. The `newPrivateState`
is kept off-chain by whoever built the transaction. Lose it and you can no longer
produce correct witnesses for the next call.

Two things therefore need persistent, contract-scoped, off-chain storage:

- **Private state** — the witness state blob, one per `(contract address, private
  state id)`. A single contract can hold several distinct private states under
  different ids.
- **Signing keys** — the key that authorizes contract _maintenance_ (verifier-key
  rotation, authority replacement), one per contract address. Distinct from the
  wallet's spending keys.

## How midnight-js does it

midnight-js splits storage behind a `PrivateStateProvider` interface
(`packages/types/src/private-state-provider.ts`) and ships a LevelDB-backed
implementation (`level-private-state-provider`). The interface stores private state
keyed by a string `privateStateId` scoped to a contract address, stores signing keys
keyed by address, and supports password-encrypted export/import (PBKDF2-HMAC-SHA256 at
600,000 iterations + AES-256-GCM) for backup and device migration.

Crucially, midnight-js also **threads** the private state through witness execution:
its contract-call layer (`midnight-js-contracts`) reads the stored private state before
building the call, hands it to every witness as `ctx.privateState`, collects the
updated state the witnesses return, and writes it back after the transaction is built.
The provider is just the store; the threading lives in the call path.

## How this SDK does it

This SDK's witnesses are **stateless** today. The `WitnessProvider` trait
(`crates/midnight-contract/src/interpreter.rs`) is:

```rust
pub trait WitnessProvider: Send + Sync {
    fn call_witness(
        &self,
        ctx: &mut WitnessContext<'_>,
        name: &str,
        args: &[Value],
    ) -> Result<Value, InterpreterError>;
}

pub struct WitnessContext<'a> { /* contract_address: &str, private_state: &mut Vec<u8> */ }
```

`ctx` carries the contract's current private state (opaque bytes) and its address.
A witness reads `ctx.private_state()` to compute its value and calls
`ctx.set_private_state(..)` / `ctx.private_state_mut()` to record changes — mirroring
midnight-js's `(ctx, ...args) => [newPrivateState, value]`.

This SDK ships **both halves**:

1. **Storage** — a `PrivateStateProvider` trait plus a filesystem default
   (`FsPrivateStateProvider`), with password-encrypted export/import.
2. **Threading** — when a `PrivateStateProvider` is attached to the
   `MidnightProvider`, a circuit call (`Contract::circuits(..).<circuit>()`)
   automatically loads the contract's private state before execution, threads it
   through every witness via `WitnessContext`, and persists the updated state after
   the transaction lands. Stateful-witness contracts "just work" across calls without
   the caller managing storage.

### Trait surface

Lives in the new `midnight-private-state` crate, re-exported from `midnight-provider`.
Async (via `async_trait`, matching the existing `Provider` trait) so non-filesystem
backends are possible. Keys are explicit parameters rather than midnight-js's stateful
`setContractAddress` handle — no hidden mutable state.

```rust
#[async_trait]
pub trait PrivateStateProvider: Send + Sync {
    // Private state, keyed by (contract address, private state id).
    async fn set(&self, address: &str, id: &PrivateStateId, state: &[u8]) -> Result<(), PrivateStateError>;
    async fn get(&self, address: &str, id: &PrivateStateId) -> Result<Option<Vec<u8>>, PrivateStateError>;
    async fn remove(&self, address: &str, id: &PrivateStateId) -> Result<(), PrivateStateError>;
    async fn clear(&self) -> Result<(), PrivateStateError>;

    // Contract maintenance signing keys, keyed by contract address.
    async fn set_signing_key(&self, address: &str, key: &[u8]) -> Result<(), PrivateStateError>;
    async fn get_signing_key(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError>;
    async fn remove_signing_key(&self, address: &str) -> Result<(), PrivateStateError>;
    async fn clear_signing_keys(&self) -> Result<(), PrivateStateError>;

    // Password-encrypted backup. Signing keys are exported separately from
    // private states (matching midnight-js, which never bundles keys with state).
    // Both return the same `EncryptedExport` envelope; a `format` tag prevents
    // importing one as the other.
    async fn export_private_states(&self, opts: &ExportOptions) -> Result<EncryptedExport, PrivateStateError>;
    async fn import_private_states(&self, data: &EncryptedExport, opts: &ImportOptions) -> Result<ImportResult, PrivateStateError>;
    async fn export_signing_keys(&self, opts: &ExportOptions) -> Result<EncryptedExport, PrivateStateError>;
    async fn import_signing_keys(&self, data: &EncryptedExport, opts: &ImportOptions) -> Result<ImportResult, PrivateStateError>;
}
```

- **Values are opaque `Vec<u8>`.** Because witnesses are untyped (`Value`), the store
  and the `WitnessContext` both hold caller-serialized bytes. The caller owns the
  encoding of its private-state type.
- **Signing keys are opaque `Vec<u8>` too.** There is no contract `SigningKey` type
  surfaced in the Rust stack yet (governance passes `ContractMaintenanceAuthority::
  default()`); a typed key would be premature. Contract governance, when it lands, can
  tighten this.
- **`PrivateStateId`** is a thin newtype over `String` with `From<&str>` / `From<String>`,
  on-brand with the repo's typed-`Network` direction.

### Filesystem default: `FsPrivateStateProvider`

Mirrors the wallet's storage discipline (`crates/midnight-wallet/src/storage.rs`):
per-key files, written to a `.tmp` sibling and `rename`d into place so a crash never
leaves a half-written file. Default root `~/.midnight/private-state/`.

```
<root>/
  states/
    <sha256(address || 0x1f || id)>.json   # { address, id, data: base64 }
  signing-keys/
    <sha256(address)>.json                 # { address, data: base64 }
```

Each entry is a small self-describing JSON record rather than a raw blob: the filename
is a hash (safe, fixed-length, collision-resistant), and the record carries the
plaintext `address`/`id` so an export can recover the original keys when enumerating
the directory. The opaque private-state / signing-key bytes are base64 in `data`.

State is stored **plaintext at rest**, consistent with how the wallet persists its own
state today. Encryption is applied on export, which is the secure-transport surface.
At-rest encryption is a possible later extension.

### Encrypted export/import

Our own envelope — no cross-SDK interoperability with midnight-js exports (that would
require mirroring their exact KDF parameters and payload schema). Format:

```jsonc
{
  "format": "midnight-rs-private-state-export-v1",  // or "...-signing-key-export-v1"
  "salt": "<hex, 32 bytes>",
  "ciphertext": "<base64(nonce[12] || aes_256_gcm_ciphertext)>"
}
```

- **KDF:** Argon2id over the password + 32-byte random salt → 32-byte key.
- **Cipher:** AES-256-GCM, random 96-bit nonce prepended to the ciphertext.
- **Plaintext payload:** a JSON array of the same self-describing records used on disk —
  `[{ "address", "id", "data": "<base64>" }, …]` for states, `[{ "address", "data" }, …]`
  for signing keys.
- **Guards (mirroring midnight-js):** password must be at least 16 characters;
  at most `MAX_EXPORT_ENTRIES = 10_000` entries per export.
- **Import conflict strategy:** `Skip` | `Overwrite` | `Error` (default `Error`),
  returning counts of imported / skipped / overwritten.

A wrong password fails AES-GCM authentication and surfaces as
`PrivateStateError::Decrypt` rather than silently producing garbage.

### Provider integration

`MidnightProvider` gains an optional `Arc<dyn PrivateStateProvider>`, set with
`.with_private_state(provider)` and read with `.private_state()`, mirroring the existing
optional `with_proof_provider`. It is optional because contracts with stateless
witnesses never need it.

### Threading through a circuit call

When a `PrivateStateProvider` is attached, `Contract::call_with` (used by the generated
`Contract::circuits(..).<circuit>()` methods) does the work around execution:

1. **Load** — `store.get(address, "default")` seeds a `WitnessContext` private-state
   buffer before the circuit runs.
2. **Execute** — each `call_witness` receives `&mut WitnessContext`; witnesses read and
   mutate the buffer in place.
3. **Persist** — after the transaction is submitted and lands in a block, a non-empty
   buffer is written back with `store.set(address, "default", &buffer)`.

So the same `WitnessProvider` instance can be reused across calls; the durable state
lives in the store, not in the provider object.

## Limitations and future work

- **One threaded state per contract.** Threading uses a fixed private-state id
  (`"default"`) keyed by contract address. Contracts needing several private states use
  the `PrivateStateProvider` store directly with distinct ids.
- **Persist-after-submit, no rollback.** The updated state is written once the tx lands
  in a block. If the *fallible* phase then fails (`PartialSuccess`/`Failure`), the
  on-chain state did not advance but the persisted private state did — the classic
  private-state/on-chain desync. midnight-js has the same hazard. Re-deriving from chain
  state after a failed call is left to the caller.
- **Signing keys are unused.** Contract maintenance/governance is not implemented, so
  the signing-key half of the provider is forward-looking storage with no consumer yet.
- **Plaintext at rest.** Only export is encrypted.

See [`midnight-js-comparison.md`](./midnight-js-comparison.md) for the broader mapping
between the two SDKs.
