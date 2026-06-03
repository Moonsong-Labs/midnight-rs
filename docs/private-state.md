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

- **Private state** — the witness state blob, one per contract address. A Compact
  contract has exactly one private-state type (the `PS` object, with one field per
  private variable), shared by all its witnesses, so one blob per contract is the
  whole model.
- **Signing keys** — a general per-contract signing-key slot, one per contract
  address, distinct from the wallet's spending keys. This SDK's contract
  governance signs maintenance updates externally and does not use it (see
  [contract-maintenance-governance.md](./contract-maintenance-governance.md)); the
  slot is here for apps that manage their own per-contract keys.

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

Lives in the `midnight-private-state` crate, re-exported from `midnight-provider`.
Async (via `async_trait`, matching the existing `Provider` trait) so non-filesystem
backends are possible. Both private state and signing keys are keyed by contract
address — a Compact contract has exactly one `PS` struct per address, so one stored
blob per address covers the whole model.

```rust
#[async_trait]
pub trait PrivateStateProvider: Send + Sync {
    // Private state, keyed by contract address.
    async fn set(&self, address: &str, state: &[u8]) -> Result<(), PrivateStateError>;
    async fn get(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError>;
    async fn remove(&self, address: &str) -> Result<(), PrivateStateError>;
    async fn clear(&self) -> Result<(), PrivateStateError>;

    // Per-contract signing-key slot, keyed by contract address (general; this
    // SDK's governance signs externally and does not use it — see above).
    async fn set_signing_key(&self, address: &str, key: &[u8]) -> Result<(), PrivateStateError>;
    async fn get_signing_key(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError>;
    async fn remove_signing_key(&self, address: &str) -> Result<(), PrivateStateError>;
    async fn clear_signing_keys(&self) -> Result<(), PrivateStateError>;

    // Password-encrypted backup. Signing keys are exported separately from
    // private states. Both return the same `EncryptedExport` envelope; a
    // `format` tag prevents importing one as the other.
    async fn export_private_states(&self, opts: &ExportOptions) -> Result<EncryptedExport, PrivateStateError>;
    async fn import_private_states(&self, data: &EncryptedExport, opts: &ImportOptions) -> Result<ImportResult, PrivateStateError>;
    async fn export_signing_keys(&self, opts: &ExportOptions) -> Result<EncryptedExport, PrivateStateError>;
    async fn import_signing_keys(&self, data: &EncryptedExport, opts: &ImportOptions) -> Result<ImportResult, PrivateStateError>;
}
```

- **Values are opaque `Vec<u8>`.** Because witnesses are untyped (`Value`), the store
  and the `WitnessContext` both hold caller-serialized bytes. The caller owns the
  encoding of its private-state type, packing all private variables into the one blob.
- **Signing keys are opaque `Vec<u8>` too.** There is no contract `SigningKey` type
  surfaced in the Rust stack yet (governance passes `ContractMaintenanceAuthority::
  default()`); a typed key would be premature. Contract governance, when it lands, can
  tighten this.

### Filesystem default: `FsPrivateStateProvider`

Mirrors the wallet's storage discipline (`crates/midnight-wallet/src/storage.rs`):
per-key files, written to a `.tmp` sibling and `rename`d into place so a crash never
leaves a half-written file. Default root `~/.midnight/private-state/`.

```
<root>/
  states/
    <sha256(address)>.json   # { address, data: base64 }
  signing-keys/
    <sha256(address)>.json   # { address, data: base64 }
```

Each entry is a small self-describing JSON record rather than a raw blob: the filename
is a hash (safe, fixed-length, collision-resistant), and the record carries the
plaintext `address` so an export can recover the original keys when enumerating the
directory. The opaque private-state / signing-key bytes are base64 in `data`.

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
  `[{ "address", "data": "<base64>" }, …]` for both states and signing keys.
- **Guards (mirroring midnight-js):** the export password must be at least 16
  characters (enforced on export; import succeeds or fails purely on AES-GCM
  authentication); at most `MAX_EXPORT_ENTRIES = 10_000` entries per export.
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

1. **Load** — `store.get(address)` seeds a `WitnessContext` private-state buffer before
   the circuit runs.
2. **Execute** — each `call_witness` receives `&mut WitnessContext`; witnesses read and
   mutate the buffer in place. `WitnessContext::contract_address()` returns
   `Option<&str>` so the same `WitnessContext` shape supports pure-interpreter exercises
   with no deployed contract (the address is `None` in that case).
3. **Persist** — after the transaction lands and the indexer reports
   `TransactionResult::Success` for the fallible phase, the buffer is written back with
   `store.set(address, &buffer)` — but only if a witness actually changed it, so unchanged
   state isn't rewritten on every call. If a witness cleared the state to empty, it's
   removed instead.

The build-only path is also threaded: `build_unproven_call_tx` takes an
`Option<&mut WitnessContext>` so cold-signing / custodian flows that build a transaction
without submitting can still capture the post-call private-state buffer.

So the same `WitnessProvider` instance can be reused across calls; the durable state
lives in the store, not in the provider object.

## Limitations and future work

- **Persist-after-submit, no rollback.** The updated state is written once the tx lands
  in a block. If the *fallible* phase then fails (`PartialSuccess`/`Failure`), the
  on-chain state did not advance but the persisted private state did — the classic
  private-state/on-chain desync. midnight-js has the same hazard. Re-deriving from chain
  state after a failed call is left to the caller.
- **Concurrent calls to one contract must be serialized by the caller.** A call reads the
  private state, runs/submits/waits, then persists; the SDK does not lock around that
  window. Two in-flight calls to the same contract both start from the same baseline and
  the last to persist wins (a lost update). In practice the competing transactions also
  collide on-chain (the contract's own state advanced), so one is rejected, but callers
  that fan out calls to the same contract should serialize them.
- **Signing keys are unused.** Contract maintenance/governance is not implemented, so
  the signing-key half of the provider is forward-looking storage with no consumer yet.
- **Plaintext at rest.** Only export is encrypted.

See [`midnight-js-comparison.md`](./midnight-js-comparison.md) for the broader mapping
between the two SDKs.
