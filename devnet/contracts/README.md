# Devnet example contracts

Compiled Compact contracts (source + `compiled/` artifacts) that the examples deploy to the local devnet, kept here alongside [`../docker-compose.yml`](../docker-compose.yml) rather than in the individual example crates.

This directory holds only contract assets — it is **not** a Rust crate, and it sits outside `examples/` so the workspace's `examples/*` glob never treats it as one.

| Contract | Used by |
| --- | --- |
| [`counter`](counter) | [`example-counter`](../../examples/counter), [`example-contract-maintenance`](../../examples/contract-maintenance) |
| [`secret-counter`](secret-counter) | [`example-private-state`](../../examples/private-state) — a stateful `witness next_secret()` backed by per-contract private state |

Examples reference these by relative path from their crate, e.g.
`contract!("../../devnet/contracts/counter/compiled/contract-info.json")` and
`concat!(env!("CARGO_MANIFEST_DIR"), "/../../devnet/contracts/counter/compiled")`.

To recompile a contract (requires the [extended Compact compiler](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions)):

```bash
cd counter && compactc counter.compact compiled
```
