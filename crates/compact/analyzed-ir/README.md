# compact-analyzed-ir

Reader for `compiler/analyzed-ir.sexp`: the Compact compiler's analyzed IR printed in its own vocabulary, with each ledger operation expanded to its Impact VM instructions.

A `compactc` that supports `--analyzed-ir` writes the artifact:

```
compactc --skip-zk --analyzed-ir contract.compact out/
```

`tools/ir-hook.ss` in this repository prints the same artifact for a `compactc` that supports `--ir-hook` instead. The file imports nothing: the compiler hands it the program and the unparser.

```
compactc --skip-zk --ir-hook tools/ir-hook.ss contract.compact out/
```

The format's authority is the compiler itself: `compiler/langs.ss` defines the forms and their field order, and `compiler/midnight-ledger.ss` documents the instruction notation. This crate adds no schema on top; it parses that surface into a typed Rust model.

## What you get

`parse_str(&str) -> Result<AnalyzedIr, Error>`: versions, the export table (exported circuits and exported ledger fields), contract types, and program elements: circuits with `exported`/`pure`/`proof` flags and bodies, natives with their runtime entry, witnesses, the ledger layout, and the constructor. Expression bodies cover the whole analyzed language; each `public-ledger` and `emit` node carries its expanded `Instruction` list.

Failure mode is closed: an unrecognized expression, type or operand form is an error naming the form. The instruction set is the one deliberately open surface, mirroring the format: instructions parse generically (`op` plus named operands) and a consumer refuses unknown ops at execution time, because an omitted operation is a different program.

`maxval` reaches 2^248-1, so bounds are `BigUint`.

## Scope

Reading only. Execution (an interpreter over `Expr` and `Instruction`), FAB encoding, and proving belong to the SDK layers above, midnight-rs today.

## WASM

The crate is `wasm32-unknown-unknown` clean (no I/O in the API, `num-bigint` as the only dependency) so a bindings layer (wasm-bindgen or hand-written C-ABI exports) can ship it as a wasm blob for SDKs in other languages; that layer is deliberately not chosen here.

## Fixtures

`tests/fixtures/*.sexp` are real compiler output (compactc 0.33.122) for the midnight-rs conformance corpus and probes: counter, bboard, events (`emit`), ser (`serialize`), ccc (cross-contract call), loops (map/fold), slices, mint-probe (kernel and coin operations, nonzero `dup` arities), zerocash. Regenerate with the command above.
