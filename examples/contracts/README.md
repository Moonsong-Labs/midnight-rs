# Shared example contracts

Compiled Compact contracts (source + `compiled/` artifacts) reused by more than
one example, so they aren't duplicated per example.

This directory is **not** a Rust crate — it's excluded from the workspace
(`exclude = ["examples/contracts"]`) and holds only contract assets.

| Contract | Used by |
| --- | --- |
| [`counter`](counter) | [`example-counter`](../counter), [`example-contract-maintenance`](../contract-maintenance) |

Examples reference these via a relative path, e.g.
`contract!("../contracts/counter/compiled/contract-info.json")` and
`concat!(env!("CARGO_MANIFEST_DIR"), "/../contracts/counter/compiled")`.

To recompile a contract (requires the [extended Compact compiler](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions)):

```bash
cd counter && compactc counter.compact compiled
```
