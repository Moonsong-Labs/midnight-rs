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
- Byte-only comparison of serialized proof preimages: exact but unreadable on failure. Kept as one field of the report, not the whole gate.

## Comparison channels (per case)

The TS `CircuitResults`/`ProofData` and the Rust `ExecutionResult` expose the same information. The report is canonical JSON with these fields, diffed structurally so failures are readable:

- `input` and `output` aligned values (hex value segments plus alignment): the ZK statement binding; `output` covers `disclose()` communication outputs.
- `publicTranscript`: the raw op list including `popeq` read results, normalized to one JSON shape from `Op<AlignedValue>` (TS) and `Op<ResultModeGather>` plus `reads` (Rust).
- `privateTranscriptOutputs`: witness returns in call order.
- `result`: the circuit return value as an aligned value.
- `postState`: `ContractState.serialize()` hex (byte-exact; both sides use midnight tagged serialization) plus `StateValue` encoded JSON for readable diffs.
- `proofPreimage`: hex of `proofDataIntoSerializedPreimage(input, output, publicTranscript, privateTranscriptOutputs)` vs the Rust equivalent, a single exactness backstop over the whole statement.
- `initialState`: serialized initial `ContractState` produced by TS `Contract.initialState` vs Rust bindgen's `LedgerInitialState::into_ledger()`, which also gates deploy-state conformance (contract addresses derive from it).
- Zswap outputs (`createZswapOutput` coins) when a circuit mints.

Determinism: fixed contract address, fixed block time, scripted witness values shared by both sides, no communication commitment randomness (we compare its inputs instead).

## Layout

```
tests/conformance/
  Cargo.toml               workspace member; test-only crate
  src/                     report model + normalizers (Value/AlignedValue/Op/StateValue -> canonical JSON)
  tests/conformance.rs     runs interpreter per case, diffs against expected/
  cases/<fixture>/<case>.json     circuit, args, witness script
  fixtures/<name>/         <name>.compact + compiler/contract-info.json + contract/index.js (committed codegen)
  expected/<fixture>/<case>.json  golden reports emitted by the TS driver
  ts-driver/               node package: driver.mjs + vendored canonical runtime
```

## Corpus

Seed fixtures, chosen for op coverage:

- `counter`: minimal Counter ledger op.
- `tiny`: enum state cell, witness, assert, `persistentHash`, `pad`, `disclose`, Maybe.
- `bboard`: Maybe/Opaque, Counter, `Field as Bytes<32>` cast, `persistentHash`.
- `election`: MerkleTree insert/checkRoot/path witness, Set member/insert, commitments, nullifiers, the broadest ledger coverage.
- `ops` (new, purpose-built): one circuit per whack-a-mole builtin family so a divergence pinpoints the op: full-width field arithmetic including the mod-r reduction shape from the gateway bug, `transientHash`, `persistentHash`, `transientCommit`, `persistentCommit`, `degradeToTransient`, `upgradeFromTransient`, `hashToCurve`, `ecAdd`, `ecMul`, `ecMulGenerator`, casts, `pad`.

Fixtures are compiled with the pinned fork compactc (`make build-compactc`), and both `compiler/contract-info.json` and the generated `contract/index.js` are committed so CI needs neither Nix nor the compiler. The submodule pointer is bumped to the commit the artifacts were compiled with.

## Canonical runtime versioning

Generated code calls `checkRuntimeVersion('0.16.101')`, which requires runtime patch >= codegen patch; npm's released `0.16.0` refuses to load it. The driver therefore vendors the runtime built from the compiler submodule (`tools/compact-compiler/runtime`, version 0.16.101, the source of truth the codegen was built against). Building it needs Chez Scheme once, locally, per compiler bump; the built package is committed under `ts-driver/vendor/` so CI only runs `npm ci`. `@midnight-ntwrk/onchain-runtime-v3` comes from npm at the version closest to the Rust workspace pin (`=3.1.0`); the byte-exact channels (state serialize, proof preimage) will surface any serialization skew between the two as an explicit diff.

## Gate wiring

- `cargo test -p conformance` (part of `make test`): Rust interpreter vs committed goldens. No node required, so the default dev loop and existing CI jobs stay pure-Rust.
- New CI job `conformance`: setup node, `npm ci` in `ts-driver`, regenerate `expected/`, `git diff --exit-code tests/conformance/expected` (fails if goldens are stale, i.e. the TS runtime disagrees with what is committed), then `cargo test -p conformance`.
- `make conformance-regen`: run the TS driver locally to refresh goldens.
- `make regen-conformance-fixtures`: recompile corpus contracts with the pinned compactc (local, needs Nix).

Adding coverage for a new op is: extend `ops.compact` (or add a case JSON), recompile fixtures, regen goldens, commit all three.
