# Per-contract private state

Some Compact contracts use **witnesses** that are themselves stateful: a counter the caller keeps secret, an unspent-note set, a running commitment opening. The witness value the circuit consumes on call _N+1_ depends on what happened on call _N_. That "between calls" data is the contract's **private state**: it never touches the chain, but it has to survive across calls and across process restarts.

This document explains the private-state model on Midnight and how it's stored in this SDK as a per-transaction journal. `Contract::call_with` reconciles automatically against the chain's verdict: on a finalized `Success` it confirms the snapshot, and on `PartialSuccess` / `Failure` it cascade-drops the snapshot via `mark_failed` so the next call's witness baseline is the last-known-good state. Manual `mark_failed` / `rollback_from` (which cascade through `depends_on`) remain available for a caller who learns out of band that a `Confirmed` snapshot should be reverted.

## The model

A Midnight contract call runs the circuit locally to produce a transcript and a ZK proof. Witnesses are the circuit's private inputs. In the full Compact model a witness is not a pure function of its arguments: it reads the current private state and may mutate it as a side effect:

```
witness(currentPrivateState, ...args) -> (newPrivateState, value)
```

The chain only ever sees the proof and the disclosed outputs. The `newPrivateState` is kept off-chain by whoever built the transaction. Lose it and you can no longer produce correct witnesses for the next call.

A Compact contract has exactly one `PS` (private state) type per address: a struct whose fields are the contract's private variables. All witnesses on a given contract operate on that one struct, so storing one blob per contract address is the whole model. Fields within the blob aren't separately addressed. Multiple `PS` slots at the same address are not a Compact concept.

## The journal

Rather than a single mutable slot per address, this SDK stores private state as an append-only **journal** of [`Snapshot`](../crates/midnight-private-state/src/lib.rs)s, one per submitted transaction whose witnesses actually modified the private state (a call whose post-buffer matches the baseline is a no-op and no snapshot is recorded). Each snapshot is keyed by the producing tx's `extrinsic_hash` and, once the chain finalizes that tx, by the block it landed in (`block_hash` + `block_height`). Snapshots have a small lifecycle:

| Status      | Meaning                                                                                                                                                                                                |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `Pending`   | Tx submitted; finality not yet established. The snapshot is the SDK's best guess at the post-call state. Subsequent calls may chain off it (using its bytes as the next witness baseline).             |
| `Confirmed` | Tx finalized on chain AND the Midnight pallet reported `TxApplied` for the extrinsic (full success: every fallible segment applied). `Contract::call_with` only promotes Pending to Confirmed in this case. `PartialSuccess` / `Failure` are handled automatically: the snapshot is cascade-dropped via `mark_failed` and the call returns `ContractError::TransactionFailed`. |

A snapshot whose tx is later discovered to have been **reorged out** or to have **failed in the fallible phase** is dropped via [`PrivateStateProvider::mark_failed`]. Drops cascade: every snapshot that transitively `depends_on` the failed one is dropped too, so the journal head always represents a chain-consistent state.

### Trait surface

Lives in the `midnight-private-state` crate, re-exported from `midnight-provider`. Async (via `async_trait`, matching the existing `Provider` trait) so non-filesystem backends are possible.

```rust
#[async_trait]
pub trait PrivateStateProvider: Send + Sync {
    /// Append a new pending snapshot. `depends_on` should be the current head's
    /// extrinsic_hash (or `None` for the first snapshot at this address).
    async fn append_pending(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
        depends_on: Option<[u8; 32]>,
        state: &[u8],
    ) -> Result<(), PrivateStateError>;

    /// Promote a pending snapshot to confirmed. `block_height` is optional
    /// so callers that only know the block hash can encode "unknown"
    /// explicitly instead of passing a sentinel that is indistinguishable
    /// from a genuine genesis-block confirmation.
    async fn confirm(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
        block_height: Option<u64>,
        block_hash: [u8; 32],
    ) -> Result<(), PrivateStateError>;

    /// Drop a snapshot and every snapshot transitively depending on it.
    async fn mark_failed(&self, address: &str, extrinsic_hash: [u8; 32]) -> Result<(), PrivateStateError>;
    async fn rollback_from(&self, address: &str, extrinsic_hash: [u8; 32]) -> Result<(), PrivateStateError>;

    /// Read the journal head, the next call's witness baseline.
    async fn head(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError>;
    async fn head_extrinsic(&self, address: &str) -> Result<Option<[u8; 32]>, PrivateStateError>;

    /// Inspect / drop the journal.
    async fn snapshots(&self, address: &str) -> Result<Vec<Snapshot>, PrivateStateError>;
    async fn forget(&self, address: &str) -> Result<(), PrivateStateError>;
    async fn forget_all(&self) -> Result<(), PrivateStateError>;

    // ... signing-key + encrypted export/import methods (unchanged in spirit) ...
}
```

The `data` field on a snapshot is opaque `Vec<u8>`. The caller owns the encoding of its PS type and packs every private variable into the one blob.

### Filesystem default: `FsPrivateStateProvider`

One directory per contract address, one file per snapshot:

```
<root>/
  states/
    <sha256(address)>/
      address.txt                                    # plaintext address marker (export reads this)
      <020-padded-unix-nanos>-<extrinsic_hash>.json  # one per snapshot
        { status, extrinsicHash, blockHeight?, blockHash?, dependsOn?, data: base64 }
  signing-keys/
    <sha256(address)>.json
      { address, data: base64 }
```

Snapshot filenames begin with a 020-padded nanosecond timestamp purely for human inspection (a directory listing reads in append-time order). The journal head is derived from the `dependsOn` graph: the head is the snapshot no other snapshot depends on. Filename order is not load-bearing, since an export/import round-trip rewrites timestamps. Snapshots carry the producing tx's `extrinsic_hash` plus a `dependsOn` link to the previous snapshot at that address; `mark_failed` / `rollback_from` walk that graph to cascade-drop dependents. Writes go to a `.tmp` sibling and are `rename`d into place, so a crash never leaves a half-written file.

State is stored **plaintext at rest**. Encryption is applied on export, which is the secure-transport surface.

### Provider integration

`MidnightProvider` carries an optional `Arc<dyn PrivateStateProvider>`, set with `.with_private_state(provider)` and read with `.private_state()`. It is optional because contracts with stateless witnesses never need it.

### Threading through a circuit call

When a `PrivateStateProvider` is attached, `Contract::call_with` (used by the generated `Contract::circuits(..).<circuit>()` methods) does the work around execution:

1. **Load.** `store.head_with_extrinsic(address)` returns both the witness baseline and the previous snapshot's `extrinsic_hash` (captured for the new snapshot's `depends_on`) in a single read, so a concurrent `append_pending` can't produce a torn read where the two come from different journal versions.
2. **Execute.** Each `call_witness` receives `&mut WitnessContext`; witnesses read and mutate the buffer in place.
3. **Prepare.** `provider.prepare(tx_bytes)` builds and validates the proven tx against the node *without submitting it*, exposing the `extrinsic_hash` the tx will have once it lands.
4. **Append pending.** *Before* the tx reaches the mempool, the post-call buffer is written as a `Pending` snapshot keyed by that `extrinsic_hash`. Recording first closes the window where a crash between submit and append would leave a tx on the wire with no journal entry, so the next call would build on a stale baseline. `append_pending` also rejects a write whose `depends_on` is no longer the journal's leaf (`JournalConflict`), which together with a per-address lock stops two concurrent calls on the same contract from branching the journal. The tx is then submitted; if submit fails the speculative entry is rolled back, since the tx never went out.
5. **Wait.** `wait_finalized` blocks until the chain has finalized the tx, bounded by a 60s timeout. Past finality the block can't be reorged out under honest-majority assumptions. The returned `TxInBlock` carries a `verdict` derived from the `Midnight` pallet's outcome events (`TxApplied` / `TxPartialSuccess`) or `System::ExtrinsicFailed`. On timeout or wait error the pending snapshot is **left on disk**: cancelling the wait does not retract the tx from the mempool (per `PendingTx` docs), so the tx may still land. A failed wait surfaces as `ContractError::SubmissionWait`, which carries the `extrinsic_hash` and the underlying provider error as typed fields, so the caller can match the `SubmitError` kind (`Invalid` means definitively rejected, safe to `mark_failed`; `Dropped` / `NodeError` / `WatchStream` mean the tx's fate is unknown; `VerdictFetch` means the tx is in a block but its outcome events couldn't be read, so only the verdict is unknown); a timeout surfaces as `ContractError::FinalizeTimeout`, which carries the `extrinsic_hash` and the elapsed deadline as typed fields. Either way the caller queries the chain and invokes `confirm` (it landed and applied) or `mark_failed` (it didn't).
6. **Branch on verdict.** `Success`: promote the snapshot to `Confirmed` with the block hash (and `None` block_height; the model doesn't depend on the height for correctness). `PartialSuccess` or `Failure`: cascade-drop the Pending snapshot via `mark_failed` and return `ContractError::TransactionFailed { extrinsic_hash, block_hash, status }` so the caller knows the chain didn't advance. `status` is the typed `midnight_provider::Verdict`, not a string, so callers match on the variant.

The build-only path (`build_unproven_call_tx`) takes an `Option<&mut WitnessContext>` so cold-signing / custodian flows that build a transaction without submitting can still capture the post-call private-state buffer.

### Fallible-phase verdict

The SDK reads the `Midnight` pallet's outcome events for the extrinsic and exposes them as `TxInBlock::verdict`:

- `Verdict::Success` ↔ `Midnight::TxApplied`: every fallible segment applied; the chain state advanced fully.
- `Verdict::PartialSuccess` ↔ `Midnight::TxPartialSuccess`: guaranteed phase committed, at least one fallible segment failed.
- `Verdict::Failure` ↔ no `Midnight` outcome event and `System::ExtrinsicFailed`: the dispatch errored entirely.

`Contract::call_with` builds a single-fallible-segment tx (one contract call per submission), so `PartialSuccess` unambiguously means the contract action did not apply. The Pending snapshot is dropped via `mark_failed` and the call returns `ContractError::TransactionFailed { extrinsic_hash, block_hash, status }`, carrying the typed `Verdict`. Callers that learn out of band that a tx in `Confirmed` state should be reverted can still invoke `mark_failed` manually.

## Recovery and rollback

The journal model naturally supports two recovery flavors:

- **User-initiated rollback.** `PrivateStateProvider::rollback_from(address, extrinsic_hash)` drops a snapshot and every snapshot that transitively depends on it. Use when an application discovers a tx didn't actually apply, or when reverting to a known-good point for debugging.
- **Chain-driven recovery (lazy).** A future call into the contract should verify the most recent confirmed snapshots are still in the canonical chain: query the node for each snapshot's `block_hash`; if the node doesn't know it, the block was reorged out and that snapshot (plus its dependents) gets `mark_failed`ed. This automatic check is not yet wired into `call_with`; the trait surface supports it.

## Termination hazard

If the process is interrupted between `submit` and the snapshot write (a small window, since append happens immediately after submit returns), the tx may land on chain without a local snapshot. The next call will start from the previous head, building a transaction whose witness inputs reflect pre-tx state. The chain will likely reject it, and the user can recover by re-syncing state.

If the process is interrupted between the pending append and `wait_finalized`, the pending snapshot is on disk. The next run treats it as the current head; once the chain catches up, manual `confirm` (or, when wired, automatic confirmation via finality detection) promotes it. If the tx actually failed, `mark_failed` drops it. midnight-js has the same hazard and documents it as a known limitation; this SDK does the same.

## Limitations and future work

- **Block height in confirm**: `confirm` takes `block_height: Option<u64>`, and `Contract::call_with` passes `None` because subxt's `TxInBlock` only exposes the hash. A follow-up that fetches the height via a one-shot block query will fill this in; the model doesn't depend on it for correctness.
- **Automatic re-org reconciliation on call entry**, described above. The trait and storage support it; the contract path doesn't invoke it yet.
- **Concurrent calls to one contract must be serialized by the caller.** Two in-flight calls with the same `depends_on` and conflicting state mutations will produce non-deterministic snapshot ordering on the local journal. Real pipelining (multiple in-flight txs against the same address) is supported by the storage layer's pending/depends_on model; user-facing API support is a separate change.
- **Plaintext at rest.** Only export is encrypted.

See [`midnight-js-comparison.md`](./midnight-js-comparison.md) for the broader mapping between the two SDKs.
