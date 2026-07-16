# Off-chain Compact circuit body IR

**Where:** emitted into `contract-info.json` (the `ir` field, per circuit) by the Compact compiler fork `RomarQ/compact` (branch `feat/contract-info-extensions`). The consumer-side schema is `crates/compact/compact-codegen/src/ir.rs` (`CircuitIrBody`, made of `Stmt` / `Expr` / `LedgerOp`).

**On/off chain:** off-chain.

**Purpose:** let an SDK in any language execute a Compact circuit off-chain, without the generated TypeScript and without forking the compiler per language, to produce the public transcript and the `ProofPreimage`.

## What it is

A serialization of the compiler's lowered `Lnovectorref` circuit body (see [compact-pipeline.md](compact-pipeline.md)). Because it is taken after lowering, enums are integers, map/fold are unrolled, and slices and safe-casts are gone. Unlike ZKIR it is expression-structured (statements and expressions, with Compact types preserved) rather than flattened to field-level ops, which is what makes it directly interpretable.

- `Stmt`: seq, let, expr-stmt, if, if-else.
- `Expr`: var, lit, arithmetic/comparison/boolean ops, field and index access, if-expr, assert, ledger-query, call-witness, call-pure, let-expr, new, cast, default, tuple, spread, byte/field/vector conversions, contract-call.
- `LedgerOp`: the Impact ops embedded inside a `ledger-query` (idx, ins, member, popeq, root, and so on), produced by expanding each ledger ADT operation's VM template.

## How it is used

midnight-rs's interpreter (`crates/midnight-contract/src/interpreter.rs`) executes the body against the current contract state, calling host witnesses, and returns an `ExecutionResult` (reads, gather ops, communication outputs, result). From that it builds the public transcript (an Impact `Op` program, see [impact-onchain-vm.md](impact-onchain-vm.md)) and the `ProofPreimage`, which then feeds ZKIR and the prover key to make the proof (see [zkir.md](zkir.md)).

It pairs with the typed schema also carried in `contract-info.json` (circuit and witness signatures, and the ledger field layout: which field is a `Map<K,V>`, its key/value types, which is a `Counter`, and so on). The interpreter needs that schema to encode keys and arguments and to decode results, because neither ZKIR nor the runtime `StateValue` tree carries Compact-level types.

The reusable runtime primitives the interpreter builds on live in a separate crate, `midnight-compact-runtime` (the Rust counterpart of Minokawa's `compact/runtime` TypeScript package): the runtime `Value` domain and its FAB encoding, the witness callback types, the `ExecutionResult`, the builtin circuits (hashes, commitments, EC ops), the value conversions, and the type-aware encoder (`compact_types`). `interpreter.rs` keeps the IR tree-walk (`eval_expr` / `exec_stmt` / `ExecContext`) and the ledger-query VM driver, and calls into that crate. Keeping the primitives standalone means any circuit-body front-end can reuse them, not just this interpreter. See #117.

## Status

Fork-only today, not in upstream Compact. A problem statement (`mps-xxxx-standard-contract-representation`, in the `midnight-improvement-proposals` repo, draft PR midnightntwrk/midnight-improvement-proposals#188) proposes standardizing a language-agnostic representation along these lines.

There is an open design tension worth recording: the Midnight team would prefer to make ZKIR the off-chain interpretable representation rather than maintain a second one. The decisive difference today is that ZKIR is flattened and untyped and consumes a pre-built preimage, whereas this IR is typed and structured and is what produces the preimage. See [zkir.md](zkir.md).

## Depends on / produces

- **Depends on:** the Compact pipeline (`Lnovectorref`), plus the typed schema in `contract-info.json`.
- **Produces:** the public transcript (an Impact `Op` program) and the `ProofPreimage` for the prover.
