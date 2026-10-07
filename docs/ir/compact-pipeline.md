# Compact compiler IR pipeline

**Where:** the Compact compiler (Chez Scheme, nanopass framework), `compiler/langs.ss` (language definitions) and `compiler/passes.ss` (pass ordering).

**On/off chain:** off-chain, compile time only. Internal to the compiler (the Minokawa project). Not a stable public artifact.

**Purpose:** progressively lower Compact source through a chain of typed intermediate languages to a flattened circuit. Then emit the downstream artifacts: ZKIR, the typed `contract-info.json`, the TypeScript `Contract`, `contract-manifest.json`, and, with the fork's `--analyzed-ir`, `analyzed-ir.sexp`.

## Pass ordering

```
source ─parser─► Lparser/Lsrc ─frontend─► ... ─analysis─► Lloweredemit (analyzed IR)
   ├─ save-contract-info-passes  (on the analyzed IR) ─► compiler/contract-info.json
   ├─ save-analyzed-ir-passes    (on the analyzed IR, with --analyzed-ir) ─► compiler/analyzed-ir.sexp
   ├─ typescript-passes          (on the analyzed IR) ─► Ltypescript ─► contract/index.{js,d.ts}
   └─ circuit-passes             (on the analyzed IR) ─► Lnovectorref ─► Lcircuit ─► Lflattened
                                                          ├─ zkir-passes     (on Lflattened) ─► Lzkir ─► zkir/*.zkir
                                                          └─ manifest-passes (on Lflattened) ─► compiler/contract-manifest.json
```

The fork `RomarQ/compact`, which the `Makefile` runs from the image `ghcr.io/romarq/compactc`, adds `save-analyzed-ir-passes` and the `--analyzed-ir` flag that runs it. The pass prints the analyzed IR as one S-expression, with each ledger operation and each `emit` expanded to its Impact VM instructions. See [circuit-body-ir.md](circuit-body-ir.md).

## Milestone languages

The chain has many languages, and most are single-pass refinements. The ones that matter:

| Language | What it establishes |
|---|---|
| `Lparser` / `Lsrc` | Parsed source. |
| `Ltypes` | First fully type-checked language. The Compact type system is explicit from here on. |
| `Lnodisclose` | Disclose checks are done. Still fully typed and structured (map/fold, slices, enums, structs all present). |
| `Lloweredemit` | The analyzed IR, the last language of the analysis passes. `serialize` and `deserialize` are expanded, and each `emit` carries its VM code. This is the branch point: `contract-info.json`, `analyzed-ir.sexp`, the TypeScript backend and the circuit passes all start here. |
| `Lnovectorref` | Lowered: enums resolved to integers, loops unrolled, helper circuits inlined, safe-casts removed, slices removed. Still expression-structured (statements and expressions, typed). |
| `Lcircuit` / `Lflattened` | Datatypes flattened to the field level. Final IR before ZKIR. |
| `Lzkir` | Prints to ZKIR. See [zkir.md](zkir.md). |

## Depends on / produces

- **Depends on:** Compact source.
- **Produces:** ZKIR (via `Lzkir`), the typed `contract-info.json`, the generated TypeScript `Contract`, `contract-manifest.json` (the size and SHA-256 hash of each output file), and, with the fork's `--analyzed-ir`, `analyzed-ir.sexp` (the analyzed IR).
