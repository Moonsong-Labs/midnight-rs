# Counter Example

Deploys a counter contract to a local dev node, calls circuits on-chain, reconnects
from a fresh handle, and exercises a circuit with a typed argument and a typed
return value.

## Contract

```compact
import CompactStandardLibrary;

export ledger round: Counter;

export circuit increment(): Uint<64> {
  round.increment(1);
  return disclose(1);
}

export circuit increment_by(amount: Uint<16>): Uint<16> {
  round.increment(disclose(amount));
  return disclose(amount);
}
```

The `disclose(...)` calls emit communication commitments that become the
circuit's typed return value — surfaced back to the caller by the generated
bindings.

## Run

Start the devnet (node + indexer) from the repository root, then wait until both
are serving:

```bash
docker compose up -d   # run from the repo root (docker-compose.yml lives there)
# node RPC
while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
# indexer (any HTTP response = port is up)
while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
```

Run the example:

```bash
cargo run -p example-counter
```

Output:

```
=== Midnight Counter Example ===

0. Syncing wallet state from indexer...
   synced.

1. Deploying counter contract...
   ext hash:  ...
   best:      ...
   finalized: ...
   address:   0200...
   round = 0
2. Calling increment on-chain...
   returned = 1
   round = 1
3. Calling increment_by(5) on-chain...
   returned = 5
   round = 6
4. Reconnecting via Contract::at and calling increment...
   returned = 1
   round = 7

=== Done ===
```

Step 1 uses the high-level builder's `.send().await?` method which returns a
`PendingDeploy`. From there `wait_best()` and `wait_finalized()` drive subxt's
watch stream so you can act on inclusion as soon as it lands in a block, and
again once the chain finalizes it. `into_contract()` then waits for the indexer
and yields the typed `Contract`. On the local dev chain best and finalized are
usually the same hash because finalization is near-instant.

For the simple case where you don't need to observe both states, `.await?` the
builder directly. That's still supported and yields the `Contract` after a
single internal `wait_best`.

Stop the devnet (from the repo root):

```bash
docker compose down
```

## Recompile the contract

The contract source and compiled artifacts live in the shared
[`examples/contracts/counter`](../contracts/counter) (reused by the
`contract-maintenance` example too). If you modify `counter.compact`, recompile
with the [extended Compact compiler](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions).
ZK keys are required for on-chain deployment:

```bash
cd ../contracts/counter && compactc counter.compact compiled
```
