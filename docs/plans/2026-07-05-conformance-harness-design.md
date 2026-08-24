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
- `state` after each step: the serialized `ContractState` as hex, normalized to a state carrying only `data`, plus the `StateValue` as canonical JSON for readable diffs. The serialization drops the ledger version tag (`midnight:contract-state[vN]:`) because the two executors link different ledger releases; everything after the tag is compared byte for byte.
- `initialState`: the TS `Contract.initialState` output, both as canonical JSON (the Rust side decodes it to seed circuit runs) and as serialized bytes the Rust decoder must reproduce exactly, which pins the serialization and the maintenance-authority defaults across the two stacks.
- Zswap outputs (`createZswapOutput` coins) when a circuit mints (corpus support pending; the driver rejects cases that produce them).

Determinism: fixed contract address, scripted witness values shared by both sides, no communication commitment randomness (we compare its inputs instead). The block time is not shared: the driver runs at a fixed one and the Rust interpreter has no way to set it, so no case may read the kernel clock (see Follow-ups).

## Layout

```
tests/conformance/
  Cargo.toml               workspace member; test-only crate
  package.json             npm root (node_modules must sit above fixtures/ for codegen imports)
  src/                     report model + normalizers (Value/AlignedValue/Op/StateValue -> canonical JSON)
  tests/harness.rs         runs interpreter per case, diffs against expected/
  cases/<fixture>/<case>.json     circuit, args, witness script
  fixtures/<name>/         <name>.compact + compiler/analyzed-ir.sexp (committed); contract/ (generated, ignored)
  expected/<fixture>/<case>.json  golden reports emitted by the TS driver
  ts-driver/               driver.mjs + vendored canonical runtime tarball
```

## Corpus

Seed fixtures, chosen for op coverage:

- `counter`: minimal Counter ledger op.
- `tiny`: enum state cell, witness, assert, `persistentHash`, `pad`, `disclose`, Maybe.
- `bboard`: Maybe/Opaque, Counter, `Field as Bytes<32>` cast, `persistentHash`.
- `ops` (new, purpose-built): one circuit per whack-a-mole builtin family so a divergence pinpoints the op: full-width field arithmetic including the mod-r reduction shape from the gateway bug, `transientHash`, `persistentHash`, `transientCommit`, `persistentCommit`, `degradeToTransient`, `upgradeFromTransient`, `hashToCurve`, `ecAdd`, `ecMul`, `ecMulGenerator`, casts, `pad`.
- `containers` (purpose-built): Set, Map, List and Counter operations, for the Impact instructions their templates carry and nothing else emits (`rem`, `size`, `eq`, `type`, `concat`, `subi`, `lt`, `jmp`, `pop`).
- `trees` (purpose-built): MerkleTree and HistoricMerkleTree writes, the only source of `root`.
- `kernel` (purpose-built): the Kernel operations that reach past the contract's own state into the transaction effects (`ckpt`, `swap`, `neg`, `branch`, `add`).

Together the corpus reaches 22 of the 23 Impact instructions the interpreter implements. The exception is `noop`: the compiler reads one (`zkir-passes/print-zkir.ss`) but no ledger template emits one, so no Compact source can produce it.

`election` (Merkle-path witnesses, the broadest ledger coverage) is a planned follow-up: it still needs Merkle-path witness scripting.

Fixtures are compiled with the pinned fork compactc (`make regen-conformance-fixtures`, which needs `make build-compactc` first).

Of the two compiler outputs, only `compiler/analyzed-ir.sexp` is committed. `cargo test` reads it, so committing it keeps the whole test suite runnable without Nix. The TS codegen under `contract/` is read by the driver alone, and anyone running the driver has just built it, so it is ignored rather than committed.

## Canonical runtime versioning

compactc writes its own `--runtime-version` into every generated `index.js` as a `checkRuntimeVersion(...)` call, and the runtime throws when the minor differs. That version is not published to npm, so the driver vendors the runtime built from the compiler submodule (`tools/compact-compiler/runtime`) as a tarball under `ts-driver/vendor/`, and CI only runs `npm ci`.

`make vendor-compact-runtime` rebuilds it: it nix-builds the submodule's runtime package, drops the build scripts (they need the compiler toolchain, and `npm pack` would run them), packs the tarball, and repoints `package.json` and the lockfile at it. Run it after `make build-compactc` whenever the compiler moves, then `make regen-conformance-fixtures` and `make conformance-regen`.

The runtime brings its own `@midnightntwrk/onchain-runtime-v4` and re-exports the on-chain types the driver needs (`ContractState`, `ChargedState`, `dummyContractAddress`), so the driver takes them from the runtime rather than depending on a second copy. That build is a ledger release ahead of the Rust workspace pin, which is why the state channel drops the version tag; the payload after it still has to match byte for byte, and does.

## Gate wiring

- `cargo test -p conformance` (part of `make test`, so it runs in the ordinary CI `test` job): Rust interpreter vs committed goldens. No node and no compiler, so the default dev loop stays pure Rust.
- The `codegen-drift` workflow rebuilds the pinned compactc, recompiles the fixtures and re-derives the goldens, then fails on any difference from what is committed. That is what proves the goldens still follow the compiler, and it is why the codegen itself need not be committed. It runs nightly and on a compiler or driver change, because building the compiler takes 90+ minutes on a cold Nix cache.
- `make conformance-regen`: run the TS driver locally to refresh goldens. It refuses to run before `make regen-conformance-fixtures` has produced the codegen.
- `make regen-conformance-fixtures`: recompile corpus contracts with the pinned compactc (local, needs Nix). It refuses a `compactc` build older than the submodule pin, which otherwise fails with a bare `Usage: compactc` line.
- `make vendor-compact-runtime`: rebuild the driver's runtime from the submodule (local, needs Nix and Node).

Adding coverage for a new op is: extend a fixture's `.compact` (or add a case JSON), recompile fixtures, regen goldens, commit all three. Adding a whole fixture also means listing it in the Makefile's `CONFORMANCE_FIXTURES`.

## Findings from the first corpus run (2026-07-05)

The first run caught four real divergences, validating the whole premise:

1. **Implicit output encoding ignored the declared result type.** A `Field`-returning circuit whose value fit `u64` bound an 8-byte output where the canonical runtime binds a field-aligned one. Fixed: `CircuitDefs` carries the result type; the interpreter encodes the implicit communication output with it.
2. **Circuit input encoding ignored declared argument types.** `Uint<32>` arguments became 8-byte atoms in `ContractCallPrototype::input`. Fixed: both call builders route through the shared typed encoder `interpreter::encode_circuit_input`.
3. **`default<T>` lost its type.** `default<Bytes<32>>` written to a ledger cell produced a unit-valued cell instead of an empty `Bytes<32>` atom. Fixed: defaults materialize at their declared type.
4. **The fork compiler's portable IR typed enum ledger writes as `Field`** where its own TS codegen pushes a `Bytes<1>` enum cell. Fixed in the fork (`save-contract-info-passes.ss`): integer-literal ledger-op arguments now carry the operation's declared argument type, so enum writes emit `Uint` at the enum's 1-byte width. Fixtures recompiled and the affected `tiny`/`bboard` cases run in the main corpus.

## Findings from the corpus expansion (2026-08-24)

Reaching the rest of the instruction set caught four IR nodes the interpreter refused outright, each taking down every circuit that used it:

1. **`push` of a structured state value.** `List.pushFront` pushes a three-slot array and every `resetToDefault` pushes the container's empty shape. `push_value` handled only scalars. Fixed: it builds the array, map and blank Merkle tree subtrees.
2. **A computed `concat` count.** A `List` read sizes its `concat` as `(+ 2 (max-sizeof <element type>))`, and the count operand accepted only literals. Fixed: `max-sizeof` measures the type's own FAB alignment, the way the canonical runtime's `maxAlignedSize` does.
3. **`(null type)` inside an `aligned-concat`.** `List.head` builds its empty answer from the `is_some` flag joined to a default value of the element type. Fixed: the operand encoder materializes the default at its declared type.
4. **`(leaf-hash x)` as a push operand.** A Merkle tree stores the leaf digest, so every tree write pushes one. Fixed: the same digest the `leafHash` builtin computes, now shared as `compact_runtime::merkle_leaf_hash`.

## Follow-ups

- Block time: the interpreter runs every circuit at the epoch (`QueryContext::new` takes the default `CallContext`), with no way to set it, so `kernel.blockTimeLessThan` and `blockTimeGreaterThan` answer against the wrong clock. Until it is threaded through, no corpus case may read the kernel clock.
- `election` fixture: Merkle-path witness scripting.
- Zswap corpus (`mintShieldedToken`/`createZswapOutput`) with output comparison, plus `kernel.self()` (needs a fixed contract address shared by both drivers).
- Optional exactness backstop: compare `proofDataIntoSerializedPreimage` bytes against a Rust-built proof preimage.
