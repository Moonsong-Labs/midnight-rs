# Counter Example

Deploys a counter contract, reads the initial state, and increments it 3 times.

## Contract

```compact
import CompactStandardLibrary;

export ledger round: Counter;

export circuit increment(): [] {
  round.increment(1);
}
```

## Run locally (no node needed)

```bash
MIDNIGHT_LEDGER_TEST_STATIC_DIR=/tmp cargo run -p example-counter
```

Output:

```
=== Midnight Counter Example ===

1. Building initial state...
   round = 0

2. Deploying contract...
   Address: 82c8a97...
   In ledger: true

3. Reading initial state...
   round = 0

4. Incrementing...
   round = 1
   round = 2
   round = 3

5. Final: round = 3 ✓

=== Done ===
```

## Run with a devnet

Start the node and indexer:

```bash
docker compose up -d
```

Wait for the node to be healthy:

```bash
until curl -sf http://localhost:9944/health; do sleep 2; done
```

Then modify `src/main.rs` to use `deploy_with_provider` and `submit` instead of `deploy_local`.

Stop when done:

```bash
docker compose down
```

## Recompile the contract

If you modify `counter.compact`, recompile with the [extended Compact compiler](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions):

```bash
compactc --skip-zk counter.compact compiled
```

The output `compiled/compiler/contract-info.json` contains both typed ledger metadata and circuit IR.
