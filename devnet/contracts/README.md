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

These need the extended Compact compiler (the [`contract-info-extensions`](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions) fork — the stock `compactc` doesn't emit the `ir` field the bindgen macro reads). It's vendored as a git submodule at [`tools/compact-compiler`](../../tools/compact-compiler) and builds with Nix. From the repo root:

```bash
make build-compactc     # init the submodule + nix build (the compiler + zkir)
make compile-contracts  # recompile counter + secret-counter into compiled/
```

`make compile-contracts` arranges the output into the layout the macro expects (top-level `contract-info.json` + `keys/` + `zkir/`). Override `COMPACTC=<path>` to use your own build instead of the submodule.
