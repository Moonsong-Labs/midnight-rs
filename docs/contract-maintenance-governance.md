# Contract maintenance and governance

A deployed contract is not frozen. Its on-chain entry points (the circuits it accepts calls for) and the party allowed to change them can both be updated after the fact. Midnight calls this **contract maintenance**, and the controlling party is the contract's **maintenance authority**.

You reach for this when you:

- **rotate a ZK setup** — recompile a circuit and swap its verifier key,
- **add a circuit post-deploy** — insert a verifier key for an entry point that didn't exist at deploy time,
- **retire a circuit** — remove a verifier key so calls to that entry point stop verifying,
- **hand over control** — replace the authority that is allowed to do any of the above.

This document describes the on-chain mechanics (independent of any SDK) and how this SDK exposes them — including why a contract is ungovernable unless you opt in at deploy.

## The on-chain model

Every contract's `ContractState` carries two things that matter here (types live in `midnight-onchain-state`, re-exported through `midnight-bindgen`):

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

So the authority is a **k-of-n committee**. The signatures come from `base_crypto::signatures::SigningKey` (Schnorr over secp256k1); each committee member is the `VerifyingKey` of one such key. The ledger supports full k-of-n.

## What you can change: `SingleUpdate`

An update is a list of `SingleUpdate`s (from `midnight-ledger`'s `structure` module), applied **in order** within one signed batch:

```rust
pub enum SingleUpdate {
    ReplaceAuthority(ContractMaintenanceAuthority),
    VerifierKeyRemove(EntryPointBuf, ContractOperationVersion),
    VerifierKeyInsert(EntryPointBuf, ContractOperationVersionedVerifierKey),
}
```

- **`VerifierKeyInsert`** adds a verifier key for an entry point. It **does not replace** an existing one — the doc comment on the variant is explicit: *"This operation does not replace existing keys, which must first be explicitly removed."* So "rotate a key" is `VerifierKeyRemove` then `VerifierKeyInsert`, which can be batched into one update.
- **`VerifierKeyRemove`** drops the key at an entry point + version.
- **`ReplaceAuthority`** swaps in a new `ContractMaintenanceAuthority`.

`EntryPointBuf` is the circuit name (the same key under which `ContractState.operations` stores the verifier key). The current version tag is `ContractOperationVersion::V3`, and the verifier key is wrapped as `ContractOperationVersionedVerifierKey::V3(VerifierKey)` where `VerifierKey` is `transient_crypto::proofs::VerifierKey`.

## How an update is authorized: `MaintenanceUpdate`

The batch is wrapped in a `MaintenanceUpdate` and attached to an `Intent` (alongside any other actions) via `Intent::add_maintenance_update`:

```rust
pub struct MaintenanceUpdate<D> {
    pub address: ContractAddress,
    pub updates: Array<SingleUpdate, D>,
    pub counter: u32,
    pub signatures: Array<SignaturesValue, D>, // SignaturesValue(committee_index: u32, Signature)
}
```

Construction is `MaintenanceUpdate::new(address, updates, counter)` followed by `.add_signature(idx, sig)` (which keeps the signatures sorted). Each committee member signs the **same** payload, produced by `data_to_sign()`:

```text
b"midnight:contract-update:" || serialize(address) || serialize(updates) || serialize(counter)
```

i.e. `SigningKey::sign(rng, &update.data_to_sign())`, attached at the member's committee index.

### Validation rules (the ones that bite)

From `MaintenanceUpdate::well_formed` in `midnight-ledger`'s `verify` module (verified against ledger 8.1.0):

1. **Signatures sorted, strictly increasing by index.** `sigs[i].index < sigs[i+1].index` — no duplicate signers, no out-of-order entries. Otherwise `NotNormalized`.
2. **Replace-authority must bump the counter.** Any `ReplaceAuthority(new_auth)` in the batch must have `new_auth.counter == update.counter + 1`. Otherwise `NotNormalized`.
3. **Every signer must be in the committee.** `committee[idx]` must exist, else `KeyNotInCommittee`; and `vk.verify(data_to_sign, sig)` must hold, else `InvalidCommitteeSignature`.
4. **Threshold must be met.** `signatures.len() >= authority.threshold`, else `ThresholdMissed`.

The `counter` is the replay guard: it is part of the signed payload, and the authority's counter advances by one each time an update applies. Reusing a stale counter invalidates the signatures (they were computed over a different `data_to_sign`).

## Guaranteed vs. fallible: a governance tx can "succeed" and still not apply

Maintenance updates run in the transaction's **fallible** phase (see [dust-and-fees.md](./dust-and-fees.md) and [intents-and-zswap-mechanics.md](./intents-and-zswap-mechanics.md) for the two-phase model). The consequences are the same as for contract calls:

- A malformed update (bad signatures, missed threshold, wrong counter) is rejected up front — the transaction is **not included**.
- A well-formed update that fails to apply — inserting a key that is **already present** (`VerifierKeyAlreadyPresent`) or removing one that is **not found** (`VerifierKeyNotFound`) — lands in a block as a **partial success**. Fees are paid and no maintenance change takes effect.

So "the tx made it into a block" is not "the key rotated." You have to check the chain-side `TransactionResult` afterwards (this SDK exposes `MidnightProvider::wait_transaction_result` for exactly that).

## Using it in this SDK

### The default deploy is ungovernable — on purpose

The generated `InitialState::build` hardcodes the default authority:

```rust
ContractState::new(
    StateValue::Array(/* ... */),
    StorageHashMap::new(),
    ContractMaintenanceAuthority::default(), // committee: [], threshold: 1, counter: 0
)
```

`ContractMaintenanceAuthority::default()` is an **empty committee with threshold 1**. By the validation rules above, no `MaintenanceUpdate` can ever satisfy it: you can never collect one valid signature from a zero-member committee, so `ThresholdMissed` (or `KeyNotInCommittee`) is unavoidable. A contract deployed this way is permanently frozen.

Governance is **opt-in and key-custodial-free**: the SDK never assigns an authority you didn't ask for, and it **stores no signing key**. You set the committee (public keys) at deploy; every maintenance op is signed externally, by whoever holds the committee keys, and you submit the transaction with the collected signatures. That makes real k-of-n committees work without the SDK ever touching a member's secret key.

### Opt in at deploy

`with_maintenance_authority` takes the committee (verifying keys) and a threshold:

```rust
// Single-owner contract (1-of-1): you keep `authority`, the SDK only sees its public half.
let authority = SigningKey::sample(rand::thread_rng());
let contract = Contract::deploy(provider)
    .with_initial_state(state)
    .with_zk_keys("compiled/counter")
    .with_maintenance_authority(vec![authority.verifying_key()], 1)
    .await?;

// k-of-n: collect the members' public keys, pick a threshold.
.with_maintenance_authority(vec![vk_a, vk_b, vk_c], 2)
```

This sets `committee`, `threshold`, `counter = 0` in the deployed state. Nothing is stored and no private-state provider is required — the committee members keep their own keys.

The committee is validated before it goes on-chain: it must be non-empty, its members must be **distinct**, and `1 <= threshold <= committee.len()`, else deploy errors with `ContractError::Maintenance`. This rejects footguns the ledger itself would accept: `threshold > committee.len()` (or an empty committee), which is permanently un-maintainable; `threshold == 0`, which the ledger treats as "zero signatures required" — i.e. anyone could govern the contract; and a duplicate key (e.g. `[vk, vk]` with `threshold 2`), which a single signer could satisfy by signing at two indices, collapsing k-of-n to 1-of-1. The same validation applies to the new committee in `replace_authority`.

### Run maintenance ops

Maintenance lives behind a sub-builder. Because signing is external, an op is a three-step flow: **prepare** (build the unsigned update), **sign** (each member signs the same bytes), **submit**. You can chain several operations on one builder — they go into **one signed update, applied in order, atomically**:

```rust
let contract = Contract::at(provider, address).build();

// Single op (1-of-1 convenience): prepare, sign locally, submit.
let vk_bytes = std::fs::read("compiled/counter/keys/increment.verifier")?;
contract.maintenance()
    .insert_verifier_key("new_circuit", vk_bytes)
    .prepare().await?      // fetch counter + build unsigned update
    .sign(0, &authority)   // sign data_to_sign() with the committee key at index 0
    .await?;               // build + submit -> PendingTx

// Batch: rotate a verifier key atomically (insert never replaces, so remove
// first — both land in one transaction).
let new_vk = std::fs::read("compiled/counter/keys/increment.verifier")?;
contract.maintenance()
    .remove_verifier_key("increment")
    .insert_verifier_key("increment", new_vk)
    .prepare().await?
    .sign(0, &authority)
    .await?;

// Hand control to a new committee.
contract.maintenance()
    .replace_authority(vec![new_vk_a, new_vk_b], 2)
    .prepare().await?
    .sign(0, &authority)
    .await?;
```

The batch precondition check simulates each step in order, so `remove("x")` followed by `insert("x", ..)` is valid even though a lone `insert("x", ..)` on an existing circuit is not.

For a **k-of-n** committee, distribute the bytes to sign and collect signatures out of band:

```rust
let prepared = contract.maintenance().remove_verifier_key("increment").prepare().await?;
let payload = prepared.data_to_sign();           // send to each member
// each member: let sig = their_key.sign(&mut rng, &payload);
let pending = prepared
    .add_signature(0, sig_from_member_0)
    .add_signature(2, sig_from_member_2)         // any >= threshold members, by committee index
    .await?;                                     // build + submit
```

`prepare()` fetches the current authority `counter` (the update must carry it) and runs the precondition check (`insert` fails if the circuit already exists; `remove` fails if it doesn't). The returned [`PreparedMaintenance`] exposes `data_to_sign()` (the exact bytes a member signs: the update `counter` plus the rest of the payload — the contract address and the ordered update list — behind a `b"midnight:contract-update:"` domain prefix), `add_signature(index, sig)` (attach an externally-produced signature at the signer's committee position), and `sign(index, &key)` (the local convenience). `.await` builds + submits (returning a `PendingTx`); `.build().await` returns the proven bytes without submitting. The signed update rides the same dust-balancing path as a deploy.

Before building, the attached signatures are checked against the committee captured at `prepare()`: indices must be **distinct** and in range, each signature must **verify** over `data_to_sign`, and the count must meet the threshold. A duplicate index, an out-of-range index, a wrong-key signature, or too few all fail here with a specific `ContractError::Maintenance`, rather than after paying to build and submit a transaction the chain would reject. At most one `replace_authority` is allowed per update (a second would silently overwrite the first on apply).

**Confirming success.** Like any contract action, a maintenance update runs in the fallible phase: a *well-formed* update (valid signatures, threshold met, correct counter) is included even if its effect can't apply — e.g. inserting a key for a circuit that was concurrently defined lands as a *partial success* (fees paid, no change). So `.await` returning a `PendingTx` that reaches a block does not by itself prove the rotation applied; confirm with [`MidnightProvider::wait_transaction_result`](../crates/midnight-provider/src/provider.rs) if you need certainty.

Replacing the authority does **not** touch any local state — the SDK has none. The new committee's members keep their new keys; pass their verifying keys to `replace_authority`.

### Read the current committee

`Contract::maintenance_authority()` returns the on-chain `ContractMaintenanceAuthority` (committee, threshold, counter). A member uses it to find the index they sign at:

```rust
let authority = contract.maintenance_authority().await?;
let my_index = authority
    .committee
    .iter()
    .position(|vk| *vk == my_key.verifying_key())
    .expect("not on the committee") as u32;
// ... prepared.add_signature(my_index, my_key.sign(&mut rng, &payload)) ...
```

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

Attach an update with `Intent::add_maintenance_update(update)`, then balance/prove/submit the transaction exactly like a deploy (a maintenance update carries no ZK proof of its own, so it flows through the same path as `deploy_funded`).
