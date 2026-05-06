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

Start the devnet:

```bash
docker compose up -d
while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
```

Run the example:

```bash
cargo run -p example-counter
```

Output:

```
=== Midnight Counter Example ===

1. Deploying counter contract...
   ext hash:  0x...
   best:      0x...
   finalized: 0x...
   address:   0200...
   round = 0
2. Calling increment on-chain...
   returned = 1
   round = 1
3. Calling increment_by(5) on-chain...
   returned = 5
   round = 6

=== Done ===
```

Step 1 uses the high-level builder's `.send().await?` method which returns a
`PendingDeploy`. From there `wait_best()` and `wait_finalized()` drive subxt's
watch stream so you can act on inclusion as soon as it lands in a block, and
again once the chain finalizes it. `into_contract()` then waits for the indexer
and yields the typed `Contract`. On the local dev chain best and finalized are
usually the same hash because finalization is near-instant.

For the simple case where you don't need to observe both states, `.await?` the
builder directly — that's still supported and yields the `Contract` after a
single internal `wait_best`.

Stop the devnet:

```bash
docker compose down
```

## Recompile the contract

If you modify `counter.compact`, recompile with the [extended Compact compiler](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions). ZK keys are required for on-chain deployment:

```bash
compactc counter.compact compiled
```
