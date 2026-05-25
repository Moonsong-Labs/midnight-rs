# Contract maintenance and governance

A Compact contract is not frozen once deployed. Its on-chain entry points (the circuits
it accepts calls for) and the party allowed to change them can both be updated after the
fact. Midnight calls this **contract maintenance**, and the controlling party is the
contract's **maintenance authority**.

You reach for this when you:

- **rotate a ZK setup** — recompile a circuit and swap its verifier key,
- **add a circuit post-deploy** — insert a verifier key for an entry point that didn't
  exist at deploy time,
- **retire a circuit** — remove a verifier key so calls to that entry point stop
  verifying,
- **hand over control** — replace the authority that is allowed to do any of the above.

This document describes the on-chain mechanics (independent of any SDK), how the
TypeScript reference SDK ([midnight-js](https://github.com/midnightntwrk/midnight-js))
exposes them, and one gotcha that currently makes contracts deployed by **this** SDK
ungovernable. For where governance sits in the broader provider model, see
[midnight-js-comparison.md](./midnight-js-comparison.md); for the signing-key store it
relies on, see [private-state.md](./private-state.md).

## The on-chain model

Every contract's `ContractState` carries two things that matter here (types live in
`midnight-onchain-state`, re-exported through `midnight-bindgen`):

```rust
// the entry points, each holding a verifier key
operations: HashMap<EntryPointBuf, ContractOperation>,

// who may change them, and the replay/threshold guard
maintenance_authority: ContractMaintenanceAuthority,
```

```rust
pub struct ContractMaintenanceAuthority {
    pub committee: Vec<VerifyingKey>, // the n public keys allowed to authorize updates
    pub threshold: u32,               // k — how many of them must sign
    pub counter: u32,                 // monotonic replay guard
}
```

So the authority is a **k-of-n committee**. The signatures come from
`base_crypto::signatures::SigningKey` (Schnorr over secp256k1); each committee member is
the `VerifyingKey` of one such key. midnight-js only ever uses the **1-of-1** case
(`committee = [vk]`, `threshold = 1`), but the ledger supports full k-of-n.

## What you can change: `SingleUpdate`

An update is a list of `SingleUpdate`s (from `midnight-ledger`'s `structure` module),
applied **in order** within one signed batch:

```rust
pub enum SingleUpdate {
    ReplaceAuthority(ContractMaintenanceAuthority),
    VerifierKeyRemove(EntryPointBuf, ContractOperationVersion),
    VerifierKeyInsert(EntryPointBuf, ContractOperationVersionedVerifierKey),
}
```

- **`VerifierKeyInsert`** adds a verifier key for an entry point. It **does not replace**
  an existing one — the doc comment on the variant is explicit: *"This operation does not
  replace existing keys, which must first be explicitly removed."* So "rotate a key" is
  `VerifierKeyRemove` then `VerifierKeyInsert`, which can be batched into one update.
- **`VerifierKeyRemove`** drops the key at an entry point + version.
- **`ReplaceAuthority`** swaps in a new `ContractMaintenanceAuthority`.

`EntryPointBuf` is the circuit name (the same key under which `ContractState.operations`
stores the verifier key). The current version tag is `ContractOperationVersion::V3`, and
the verifier key is wrapped as `ContractOperationVersionedVerifierKey::V3(VerifierKey)`
where `VerifierKey` is `transient_crypto::proofs::VerifierKey`.

## How an update is authorized: `MaintenanceUpdate`

The batch is wrapped in a `MaintenanceUpdate` and attached to an `Intent` (alongside any
other actions) via `Intent::add_maintenance_update`:

```rust
pub struct MaintenanceUpdate<D> {
    pub address: ContractAddress,
    pub updates: Array<SingleUpdate, D>,
    pub counter: u32,
    pub signatures: Array<SignaturesValue, D>, // SignaturesValue(committee_index: u32, Signature)
}
```

Construction is `MaintenanceUpdate::new(address, updates, counter)` followed by
`.add_signature(idx, sig)` (which keeps the signatures sorted). Each committee member
signs the **same** payload, produced by `data_to_sign()`:

```text
b"midnight:contract-update:" || serialize(address) || serialize(updates) || serialize(counter)
```

i.e. `SigningKey::sign(rng, &update.data_to_sign())`, attached at the member's committee
index.

### Validation rules (the ones that bite)

From `MaintenanceUpdate::well_formed` in `midnight-ledger`'s `verify` module
(verified against ledger 8.1.0):

1. **Signatures sorted, strictly increasing by index.** `sigs[i].index < sigs[i+1].index`
   — no duplicate signers, no out-of-order entries. Otherwise `NotNormalized`.
2. **Replace-authority must bump the counter.** Any `ReplaceAuthority(new_auth)` in the
   batch must have `new_auth.counter == update.counter + 1`. Otherwise `NotNormalized`.
3. **Every signer must be in the committee.** `committee[idx]` must exist, else
   `KeyNotInCommittee`; and `vk.verify(data_to_sign, sig)` must hold, else
   `InvalidCommitteeSignature`.
4. **Threshold must be met.** `signatures.len() >= authority.threshold`, else
   `ThresholdMissed`.

The `counter` is the replay guard: it is part of the signed payload, and the authority's
counter advances by one each time an update applies. Reusing a stale counter invalidates
the signatures (they were computed over a different `data_to_sign`).

## Guaranteed vs. fallible: a governance tx can "succeed" and still not apply

Maintenance updates run in the transaction's **fallible** phase (see
[dust-and-fees.md](./dust-and-fees.md) and
[intents-and-zswap-mechanics.md](./intents-and-zswap-mechanics.md) for the two-phase
model). The consequences are the same as for contract calls:

- A malformed update (bad signatures, missed threshold, wrong counter) is rejected up
  front — the transaction is **not included**.
- A well-formed update that fails to apply — inserting a key that is **already present**
  (`VerifierKeyAlreadyPresent`) or removing one that is **not found**
  (`VerifierKeyNotFound`) — lands in a block as a **partial success**. Fees are paid and
  no maintenance change takes effect.

So "the tx made it into a block" is not "the key rotated." You have to check the
chain-side `TransactionResult` afterwards (this SDK exposes
`MidnightProvider::wait_transaction_result` for exactly that).

## How midnight-js exposes it

All in `packages/contracts/src/governance/`. The shape is uniform across the three
operations: assert the contract state, **load the signing key** from the private-state
provider, build the `MaintenanceUpdate`, sign, submit, and throw unless the final status
is `SucceedEntirely`.

| Operation | Function | Notes |
| --- | --- | --- |
| Insert VK | `submitInsertVerifierKeyTx(providers, compiled, addr, circuitId, newVk)` | Asserts the circuit is **not** already defined. `newVk` is **caller-supplied** bytes. |
| Remove VK | `submitRemoveVerifierKeyTx(providers, compiled, addr, circuitId)` | Asserts the circuit **is** defined. No VK input. |
| Replace authority | `submitReplaceAuthorityTx(providers, compiled, addr)(newAuthority)` | Curried. On success, **overwrites** the stored key. |

There are also ergonomic wrappers — a per-circuit `CircuitMaintenanceTxInterface`
(`insertVerifierKey` / `removeVerifierKey`) and a contract-level
`ContractMaintenanceTxInterface` (`replaceAuthority`).

### The signing key is established at deploy

This is the part that makes governance work end-to-end. `deployContract` takes an
**optional** `signingKey`; if you don't pass one it samples a fresh key:

```ts
signingKey: deployContractOptions.signingKey ?? sampleSigningKey()
```

That key is fed into the contract constructor (as a `KEYS_SIGNING` config entry), which
is what builds the initial `ContractState` with `committee = [verifyingKey]`,
`threshold = 1`, `counter = 0`. After a successful deploy the key is persisted under the
new contract's address:

```ts
await providers.privateStateProvider.setSigningKey(contractAddress, signingKey);
```

Every later governance op loads exactly that key with `getSigningKey(contractAddress)`,
so the deploy and its maintenance ops share one authority. `replaceAuthority` additionally
overwrites the stored key with the new one on success — and midnight-js's own comment
flags the crash-recovery gap: if the process dies between submit and `setSigningKey`, the
on-chain authority and the locally-stored key diverge.

The "pass your own key" path exists so two contracts can share a maintenance authority.

The actual `ContractMaintenanceAuthority` construction (deriving the verifying key,
setting the threshold) lives in the closed-source `@midnight-ntwrk/compact-js` package,
not in midnight-js itself.

## Gotcha: contracts deployed by this SDK are currently ungovernable

This SDK's codegen does **not** have compact-js's constructor logic. The generated
`InitialState::build` hardcodes the default authority:

```rust
// crates/compact/compact-codegen/src/expand/ledger.rs
ContractState::new(
    StateValue::Array(/* ... */),
    StorageHashMap::new(),
    ContractMaintenanceAuthority::default(), // committee: [], threshold: 1, counter: 0
)
```

`ContractMaintenanceAuthority::default()` is an **empty committee with threshold 1**. By
the validation rules above, no `MaintenanceUpdate` can ever satisfy it: you can never
collect one valid signature from a zero-member committee, so `ThresholdMissed` (or
`KeyNotInCommittee`) is unavoidable. The contract is permanently frozen.

To support governance, the deploy path has to assign a real authority — generate (or
accept) a `SigningKey`, set `committee = [signing_key.verifying_key()]`, `threshold = 1`,
`counter = 0` in the deployed state, and persist the key via the private-state provider —
mirroring what compact-js does for midnight-js. The signing-key store this needs already
exists: `PrivateStateProvider::{set,get,remove}_signing_key` (see
[private-state.md](./private-state.md)). `SigningKey` round-trips to 32 bytes via
`to_bytes` / `from_bytes`, which is what those methods take.

## Type reference

| Concept | Type | Crate |
| --- | --- | --- |
| Authority (committee/threshold/counter) | `ContractMaintenanceAuthority` | `midnight-onchain-state` (re-exported by `midnight-bindgen`) |
| One update step | `SingleUpdate` | `midnight-ledger` (`structure`) |
| Signed batch | `MaintenanceUpdate<D>` | `midnight-ledger` (`structure`) |
| Signature + committee index | `SignaturesValue(u32, Signature)` | `midnight-ledger` (`structure`) |
| Version tag | `ContractOperationVersion::V3` | `midnight-ledger` (`structure`) |
| Versioned verifier key | `ContractOperationVersionedVerifierKey::V3(VerifierKey)` | `midnight-ledger` (`structure`) |
| Entry-point / circuit name | `EntryPointBuf` | `midnight-onchain-state` |
| Signing / verifying key, signature | `SigningKey` / `VerifyingKey` / `Signature` | `midnight-base-crypto` (`signatures`) |
| Verifier key | `VerifierKey` | `midnight-transient-crypto` (`proofs`) |

Attach an update with `Intent::add_maintenance_update(update)`, then balance/prove/submit
the transaction exactly like a deploy (a maintenance update carries no ZK proof of its
own, so it flows through the same path as `deploy_funded`).
