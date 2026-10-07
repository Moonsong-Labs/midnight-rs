# Analyzed IR

**Where:** `compiler/analyzed-ir.sexp`, which the Compact compiler fork `RomarQ/compact` writes when it runs with `--analyzed-ir`. The `Makefile` runs the fork from its image, `ghcr.io/romarq/compactc`. [crates/compact/analyzed-ir/README.md](../../crates/compact/analyzed-ir/README.md) documents the format. The compiler's `compiler/langs.ss` defines it. The `compact-analyzed-ir` crate parses the artifact into a typed model.

**On/off chain:** off-chain.

**Purpose:** let an SDK in any language execute a Compact circuit off-chain, without the generated TypeScript and without forking the compiler per language, to produce the public transcript and the `ProofPreimage`.

## What it is

The compiler's analyzed IR (`Lloweredemit`, see [compact-pipeline.md](compact-pipeline.md)), printed as one S-expression in the compiler's own vocabulary. The TypeScript backend starts from the same IR. Each ledger operation and each `emit` carries its expanded Impact VM instructions. Unlike ZKIR, it keeps the Compact types and the expression structure rather than flattening them to field-level ops. Map/fold, slices, enums and structs are all present. That is what makes it directly interpretable.

The artifact holds the whole program, not only the circuit bodies. It has the export table, the contract types, each circuit with its signature and body, the natives, the witnesses, the ledger layout, and the constructor. The analyzed-ir README lists what each part carries.

## How it is used

`compact-codegen` reads the artifact through its `artifact` module (`crates/compact/codegen/src/artifact.rs`). The `compact_bindgen::contract!` macro calls it at compile time. Codegen generates the typed Rust bindings and embeds each circuit, witness and native in them as typed `ir` values. The generated bindings do not read the artifact at run time. A caller can also load the artifact at run time with `compact_codegen::artifact::load_str`.

In both cases, the interpreter (`crates/compact/interpreter/src/lib.rs`) executes `ir::Circuit` values. It runs a circuit body against the current contract state, calls host witnesses, and returns an `ExecutionResult` (reads, gather ops, communication outputs, result). From that result, `midnight-contract` builds the public transcript (an Impact `Op` program, see [impact-onchain-vm.md](impact-onchain-vm.md)) and the `ProofPreimage`. ZKIR and the prover key then make the proof (see [zkir.md](zkir.md)).

The artifact also carries the typed schema, and the bindings embed it. The schema holds the circuit and witness signatures and the ledger field layout. The layout tells which field is a `Map<K,V>`, its key and value types, which field is a `Counter`, and so on. The SDK uses the schema to encode keys and arguments and to decode results. Neither ZKIR nor the runtime `StateValue` tree carries Compact-level types.

The interpreter builds on the runtime primitives in `compact-runtime` (`crates/compact/runtime`). That crate is the Rust counterpart of Minokawa's `compact/runtime` TypeScript package. It holds the runtime `Value` domain and its FAB encoding, the witness callback types, and the `ExecutionResult`. It also holds the builtin circuits (hashes, commitments, EC ops), the value conversions, and the type-aware encoder. `compact-interpreter` keeps the tree-walk over the analyzed IR (`eval_expr`, `ExecContext`) and the ledger-query driver, and calls into that crate. Any front-end for the analyzed IR can reuse the primitives, not only this interpreter.

## Status

The `--analyzed-ir` flag exists only in the fork, not in upstream Compact. The analyzed-ir README also gives a route through `tools/ir-hook.ss`, for a `compactc` that supports `--ir-hook`. The problem statement MPS-0022 ([midnightntwrk/midnight-improvement-proposals#188](https://github.com/midnightntwrk/midnight-improvement-proposals/pull/188)) asks for a standard, language-agnostic representation of a compiled Compact contract.

There is an open design tension worth recording: the Midnight team would prefer to make ZKIR the off-chain interpretable representation rather than maintain a second one. The decisive difference today is that ZKIR is flattened and untyped and consumes a pre-built preimage, whereas the analyzed IR is typed and structured and is what produces the preimage. See [zkir.md](zkir.md).

## Depends on / produces

- **Depends on:** the Compact pipeline (`Lloweredemit`) and the fork's `--analyzed-ir` flag.
- **Produces:** the typed Rust bindings that codegen generates and, through the interpreter, the public transcript (an Impact `Op` program) and the `ProofPreimage` for the prover.
