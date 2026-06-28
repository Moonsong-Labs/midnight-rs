# Compact compiler IR pipeline

**Where:** the Compact compiler (Chez Scheme, nanopass framework), `compiler/langs.ss` (language definitions) and `compiler/passes.ss` (pass ordering).

**On/off chain:** off-chain, compile time only. Internal to the compiler (the Minokawa project). Not a stable public artifact.

**Purpose:** progressively lower Compact source through roughly 27 typed intermediate languages to a flattened circuit, and emit the downstream artifacts: ZKIR, the typed `contract-info.json`, and the TypeScript `Contract`.

## Pass ordering

```
source ─parser─► Lparser/Lsrc ─frontend─► ... ─analysis─► Lnodisclose (analyzed IR)
   ├─ save-contract-info-passes  (on the analyzed IR) ─► contract-info.json
   ├─ typescript-passes          (on the analyzed IR) ─► Ltypescript ─► contract/index.{js,d.ts}
   └─ circuit-passes             (on the analyzed IR) ─► Lnovectorref ─► Lcircuit ─► Lflattened
                                                          └─ zkir-passes ─► Lzkir ─► zkir/*.zkir
```

The fork `RomarQ/compact` (branch `feat/contract-info-extensions`) splits `circuit-passes` into `circuit-passes-lower` (to `Lnovectorref`) and `circuit-passes-flatten` (to `Lflattened`), so `save-contract-info` can also serialize the lowered `Lnovectorref` circuit body. See [circuit-body-ir.md](circuit-body-ir.md).

## Milestone languages

The chain has many languages, and most are single-pass refinements. The ones that matter:

| Language | What it establishes |
|---|---|
| `Lparser` / `Lsrc` | Parsed source. |
| `Ltypes` | First fully type-checked language. The Compact type system is explicit from here on. |
| `Lnodisclose` | The analyzed IR. Disclose checks are done. Still fully typed and structured (map/fold, slices, enums, structs all present). This is the branch point: `contract-info.json` and the TypeScript backend are both emitted from here. |
| `Lnovectorref` | Lowered: enums resolved to integers, loops unrolled, helper circuits inlined, safe-casts removed, slices removed. Still expression-structured (statements and expressions, typed). The fork serializes the circuit body `ir` from this level. |
| `Lcircuit` / `Lflattened` | Datatypes flattened to the field level. Final IR before ZKIR. |
| `Lzkir` | Prints to ZKIR. See [zkir.md](zkir.md). |

## Depends on / produces

- **Depends on:** Compact source.
- **Produces:** ZKIR (via `Lzkir`), the typed `contract-info.json`, the generated TypeScript `Contract`, and (in the fork) the circuit body IR.
