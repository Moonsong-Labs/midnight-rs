# Contract Maintenance Example

Deploys a contract governed by a maintenance authority, then performs two
maintenance updates against a local dev node:

1. **rotate a verifier key** — `remove_verifier_key` + `insert_verifier_key` in
   one signed, atomic update, and
2. **replace the authority** — hand control to a fresh committee.

A contract's maintenance authority is a k-of-n committee of verifying keys
allowed to change its verifier keys or replace itself. This SDK holds no signing
key: you set the committee (public keys) at deploy and sign each update
externally, so a real k-of-n committee works without the SDK ever seeing a
member's secret. Here the committee is **2-of-3**, with each key modeled as
living on a separate machine — every update is signed off-process and only the
signatures are collected for submission. Each update is signed by a different
pair of members to show that any two of the three satisfy the quorum.

It reuses the shared counter contract ([`examples/contracts/counter`](../contracts/counter)),
so the deployed contract has the `increment` / `increment_by` circuits to rotate.

## Run

Start the local devnet (the repo-root `docker-compose.yml`):

```bash
# from the repository root
docker compose up -d
# wait for both services
while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
```

Run the example:

```bash
cargo run -p example-contract-maintenance
```

Output:

```
=== Midnight Contract Maintenance Example (2-of-3) ===

0. Syncing wallet state from indexer...
   synced.

1. Deploying a governable contract (2-of-3 committee)...
   address: 0200...
   authority: 3 member(s), threshold 2, counter 0

2. Rotating the `increment` verifier key (members 0 and 2 sign)...
   rotated.
   authority: 3 member(s), threshold 2, counter 1

3. Replacing the maintenance authority (members 0 and 1 sign)...
   replaced. Future updates must be signed by the new committee.
   authority: 3 member(s), threshold 2, counter 2

=== Done ===
```

Each maintenance update advances the authority's `counter` by one. After step 3
the on-chain committee is the new key, so further updates must be signed by it.

Stop the devnet (from the repo root):

```bash
docker compose down
```

## How a multi-party committee works

For a k-of-n committee, deploy with `with_maintenance_authority(committee, k)`
where `committee` is the members' verifying keys. To run an update, one party
calls `.prepare()`, distributes `prepared.data_to_sign()` to the members, and
collects their signatures via `prepared.add_signature(committee_index, sig)`
until at least `k` distinct members have signed, then `.await`s to submit. See
`docs/contract-maintenance-governance.md`.
