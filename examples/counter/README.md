# Counter Example

Deploys a counter contract to a local dev node, then calls the `increment` circuit on-chain.

## Contract

```compact
import CompactStandardLibrary;

export ledger round: Counter;

export circuit increment(): [] {
  round.increment(1);
}
```

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
   Deployed at: af59d7d6...
   round = 0
2. Calling increment on-chain...
   round = 1

=== Done ===
```

Stop the devnet:

```bash
docker compose down
```

## Recompile the contract

If you modify `counter.compact`, recompile with the [extended Compact compiler](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions). ZK keys are required for on-chain deployment:

```bash
compactc counter.compact compiled
```
