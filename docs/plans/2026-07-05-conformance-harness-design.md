# Conformance harness: Rust IR interpreter vs @midnight-ntwrk/compact-runtime

Issue: https://github.com/RomarQ/midnight-rs/issues/98

## Problem

`midnight-contract`'s interpreter executes each circuit's portable IR in Rust. It is a from-scratch reimplementation of what the compiler's TS codegen plus `@midnight-ntwrk/compact-runtime` do in midnight-js, so any op can silently diverge from canonical semantics and we only find out when a circuit exercises it at runtime (#97 field arithmetic, #101 `degradeToTransient`). Divergences on the soundness path must become a systematic CI gate instead of one-off runtime failures.

## What actually needs cross-checking

The Rust interpreter already delegates the ledger VM (`Idx`/`Ins`/`Push`/`Member`/...) to the released `midnight-onchain-runtime` crate and the crypto builtins to `midnight-base-crypto`/`midnight-transient-crypto`, which are the same Rust crates the TS runtime wraps as WASM (`@midnight-ntwrk/onchain-runtime-v3`). So pinning builtins against each other tests nothing. The real divergence surface is the interpreter's own mapping from IR to those primitives: FAB encoding widths, builtin selection (the #101 bug class), value conversions and casts, op sequencing, disclose/output ordering, private-transcript ordering. That is only exercised by running whole circuits.

## Approach

Run the same compiled contract, initial state, circuit arguments, and scripted witness values through both executors and diff a canonical report:

1. Rust: `interpreter::execute_with_owned` (the path `call.rs` uses).
2. TS: the compiler's generated `contract/index.js` executed against the canonical `@midnight-ntwrk/compact-runtime` (the exact midnight-js semantics).

Approaches considered and rejected:

- Builtin-level golden vectors only (the issue's "lighter interim step"): both sides bind to the same underlying crates, so this misses the IR-mapping bugs that actually happened.
- Byte-only comparison of serialized proof preimages: exact but unreadable on failure. Kept as a follow-up backstop, not the gate.

## Comparison channels (per case)

The TS `CircuitResults`/`ProofData` and the Rust `ExecutionResult` expose the same information. The report is canonical JSON with these fields, diffed structurally so failures are readable:

- `input` and `output` aligned values (hex value segments plus alignment): the ZK statement binding. `output` is the disclosed result / communication outputs, so a separate decoded `result` field is unnecessary (and would reintroduce cross-language value-shape ambiguity).
- `publicTranscript`: the raw op list including `popeq` read results, normalized to one JSON shape from `Op<AlignedValue>` (TS) and `Op<ResultModeGather>` plus `reads` (Rust).
- `privateTranscriptOutputs`: witness returns in call order.
- `state` after each step: `ContractState.serialize()` hex (byte-exact; both sides use midnight tagged serialization, normalized to a state carrying only `data`) plus the `StateValue` as canonical JSON for readable diffs.
- `initialState`: the TS `Contract.initialState` output, both as canonical JSON (the Rust side decodes it to seed circuit runs) and as serialized bytes the Rust decoder must reproduce exactly, which pins tagged-serialization and maintenance-authority defaults across the two stacks.
- Zswap outputs (`createZswapOutput` coins) when a circuit mints (corpus support pending; the driver rejects cases that produce them).

Determinism: fixed contract address, fixed block time, scripted witness values shared by both sides, no communication commitment randomness (we compare its inputs instead).

## Layout

```
tests/conformance/
  Cargo.toml               workspace member; test-only crate
  package.json             npm root (node_modules must sit above fixtures/ for codegen imports)
  src/                     report model + normalizers (Value/AlignedValue/Op/StateValue -> canonical JSON)
  tests/harness.rs         runs interpreter per case, diffs against expected/
  cases/<fixture>/<case>.json     circuit, args, witness script
  cases-quarantine/        cases blocked on known upstream divergences (see its README)
  fixtures/<name>/         <name>.compact + compiler/contract-info.json + contract/index.js (committed codegen)
  expected/<fixture>/<case>.json  golden reports emitted by the TS driver
  ts-driver/               driver.mjs + vendored canonical runtime tarball
```

## Corpus

Seed fixtures, chosen for op coverage:

- `counter`: minimal Counter ledger op.
- `tiny`: enum state cell, witness, assert, `persistentHash`, `pad`, `disclose`, Maybe.
- `bboard`: Maybe/Opaque, Counter, `Field as Bytes<32>` cast, `persistentHash`.
- `ops` (new, purpose-built): one circuit per whack-a-mole builtin family so a divergence pinpoints the op: full-width field arithmetic including the mod-r reduction shape from the gateway bug, `transientHash`, `persistentHash`, `transientCommit`, `persistentCommit`, `degradeToTransient`, `upgradeFromTransient`, `hashToCurve`, `ecAdd`, `ecMul`, `ecMulGenerator`, casts, `pad`.

`election` (MerkleTree insert/checkRoot/path witnesses, the broadest ledger coverage) is a planned follow-up: it needs bounded-Merkle-tree decode support in the state JSON layer and Merkle-path witness scripting.

Fixtures are compiled with the pinned fork compactc (`make build-compactc`), and both `compiler/contract-info.json` and the generated `contract/index.js` are committed so CI needs neither Nix nor the compiler (`make regen-conformance-fixtures` refreshes them).

## Canonical runtime versioning

Generated code calls `checkRuntimeVersion('0.16.101')`, which requires runtime patch >= codegen patch; npm's released `0.16.0` refuses to load it. The driver therefore vendors the runtime built from the compiler submodule (`tools/compact-compiler/runtime`, version 0.16.101, the source of truth the codegen was built against). Building it needs Chez Scheme once, locally, per compiler bump; the built package is committed under `ts-driver/vendor/` so CI only runs `npm ci`. `@midnight-ntwrk/onchain-runtime-v3` comes from npm at `3.1.0-rc.1`, the closest published build to the Rust workspace pin (`=3.1.0`); the byte-exact state channel surfaces any serialization skew between the two as an explicit diff (none so far: the goldens' serialized states match the Rust bytes exactly).

## Gate wiring

- `cargo test -p conformance` (part of `make test`): Rust interpreter vs committed goldens. No node required, so the default dev loop and existing CI jobs stay pure-Rust.
- New CI job `conformance`: setup node, `make conformance-regen` (npm ci plus driver), `git diff --exit-code tests/conformance/expected` (fails if goldens are stale, i.e. the TS runtime disagrees with what is committed), then `make conformance`.
- `make conformance-regen`: run the TS driver locally to refresh goldens.
- `make regen-conformance-fixtures`: recompile corpus contracts with the pinned compactc (local, needs Nix).

Adding coverage for a new op is: extend `ops.compact` (or add a case JSON), recompile fixtures, regen goldens, commit all three.

## Findings from the first corpus run (2026-07-05)

The first run caught four real divergences, validating the whole premise:

1. **Implicit output encoding ignored the declared result type.** A `Field`-returning circuit whose value fit `u64` bound an 8-byte output where the canonical runtime binds a field-aligned one. Fixed: `CircuitDefs` carries the result type; the interpreter encodes the implicit communication output with it.
2. **Circuit input encoding ignored declared argument types.** `Uint<32>` arguments became 8-byte atoms in `ContractCallPrototype::input`. Fixed: both call builders route through the shared typed encoder `interpreter::encode_circuit_input`.
3. **`default<T>` lost its type.** `default<Bytes<32>>` written to a ledger cell produced a unit-valued cell instead of an empty `Bytes<32>` atom. Fixed: defaults materialize at their declared type.
4. **The fork compiler's portable IR types enum ledger writes as `Field`** where its own TS codegen pushes a `Bytes<1>` enum cell. This is a compiler-side divergence (`save-contract-info-passes.ss` lowering); the affected `tiny` and `bboard` cases are quarantined under `cases-quarantine/` until the fork is fixed.

## Follow-ups

- Fix the fork compiler's enum-literal lowering, recompile fixtures, and un-quarantine the `tiny`/`bboard` cases.
- `election` fixture: bounded-Merkle-tree state decode plus Merkle-path witness scripting.
- Zswap corpus (`mintShieldedToken`/`createZswapOutput`) with output comparison, plus `kernel.self()` (needs a fixed contract address shared by both drivers).
- Optional exactness backstop: compare `proofDataIntoSerializedPreimage` bytes against a Rust-built proof preimage.
