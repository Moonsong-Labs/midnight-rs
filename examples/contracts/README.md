# Shared example contracts

Compiled Compact contracts (source + `compiled/` artifacts) for the examples,
kept out of the individual example crates (and reused where more than one
example needs the same contract).

This directory is **not** a Rust crate — it's excluded from the workspace
(`exclude = ["examples/contracts"]`) and holds only contract assets.

| Contract | Used by |
| --- | --- |
| [`counter`](counter) | [`example-counter`](../counter), [`example-contract-maintenance`](../contract-maintenance) |
| [`secret-counter`](secret-counter) | [`example-private-state`](../private-state) — a stateful `witness next_secret()` backed by per-contract private state |

Examples reference these via a relative path, e.g.
`contract!("../contracts/counter/compiled/contract-info.json")` and
`concat!(env!("CARGO_MANIFEST_DIR"), "/../contracts/counter/compiled")`.

To recompile a contract (requires the [extended Compact compiler](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions)):

```bash
cd counter && compactc counter.compact compiled
```
