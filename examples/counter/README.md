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

Start the devnet (node + indexer) from the repository root:

```bash
make dev-up   # from the repo root
```

`make dev-up` waits for the chain to produce a block past genesis. Starting the compose file directly is not enough: a transaction built while only genesis exists carries an expired TTL and the node rejects it.

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

Step 1 uses the high-level builder's `.send().await?` method which returns a `PendingDeploy`. From there `wait_best()` and `wait_finalized()` drive subxt's watch stream so you can act on inclusion as soon as it lands in a block, and again once the chain finalizes it. Each wait fails with `ContractError::TransactionFailed` when the chain did not apply the deploy. `into_contract()` then waits for the indexer and yields the typed `Contract`. On the local dev chain best and finalized are usually the same hash because finalization is near-instant.

For the simple case where you don't need to observe both states, `.await?` the builder directly. It submits the deploy, waits for the best block, fails there when the chain did not apply the deploy, and waits for the indexer. `into_contract()` does the same when no wait ran before it. One deadline, set with `with_deploy_timeout` (60 s by default), bounds the wait for the block and the indexer poll together.

Stop the devnet (from the repo root):

```bash
docker compose -f devnet/docker-compose.yml down
```

## Recompile the contract

The contract source and its compiled artifacts live in [`devnet/contracts/counter`](../../devnet/contracts/counter). Other examples use the same contract, as [`devnet/contracts/README.md`](../../devnet/contracts/README.md) shows. If you change `counter.compact`, recompile it with the Compact compiler fork that `COMPACT_REV` in the root [`Makefile`](../../Makefile) names. The `Makefile` runs that compiler from its image, so the command needs Docker ([Compile a contract](../../README.md#compile-a-contract) gives the details). From the root of the repository:

```bash
make compile-contracts  # recompile every contract in devnet/contracts
```

The compiled artifacts keep the layout that [`devnet/contracts/README.md`](../../devnet/contracts/README.md#layout) describes. A deploy on chain needs the keys in `keys/`.
