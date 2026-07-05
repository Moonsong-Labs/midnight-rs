# Quarantined conformance cases

Cases here reproduce a **known divergence between the fork compiler's portable IR and its own TS codegen**, found by the harness on 2026-07-05. They are excluded from `cases/` so the gate stays green while the compiler fix is pending; move them back once the fork is fixed and the fixtures are recompiled.

## Enum ledger writes are typed `Field` in the portable IR

For `state = STATE.x` the portable IR emits `push {lit type=Field value="<ordinal>"}`, while the TS codegen at the same site pushes through `CompactTypeEnum` (a `Bytes<1>` cell). The Rust interpreter faithfully encodes what the IR declares, so every enum ledger write diverges from the canonical runtime: different cell alignment, different serialized state, different transcript hashes.

The fix belongs in the fork's IR lowering (`compiler/save-contract-info-passes.ss` in `tools/compact-compiler`): enum-typed literals must carry the enum type (or `Uint<0..255>` with a 1-byte width) instead of `Field`.

Affected cases:

- `tiny/set-get-clear.json`: `set` and `clear` write `state: STATE`.
- `bboard/post-take-down.json`: `post` and `take_down` write `state: STATE`.
