# Compact native primitives and how the interpreter handles them

Reference for everyone touching `crates/midnight-contract/src/interpreter.rs`. It answers two questions: what is the complete set of Compact "native" primitives a circuit can invoke, and which ones our portable-IR interpreter implements today. The point is to have a single checklist so we can implement them all without missing one.

## Authoritative source

The complete, canonical list lives in the Compact compiler, not in `midnight-ledger`. It is the `declare-native-entry` table in `tools/compact-compiler/compiler/midnight-natives.ss`. There are exactly 18 entries, in two kinds:

- `declare-native-entry circuit NAME "__compactRuntime.SYMBOL" (args...) RetType`: a pure function (hashing, EC math).
- `declare-native-entry witness NAME "__compactRuntime.SYMBOL" (args...) RetType`: an effectful primitive that reads or mutates the circuit's Zswap/coin context.

`midnight-ledger`, `onchain-runtime`, and `onchain-vm` have no enum of these names. They operate one level below Compact, at the VM op (`Op`) and effects layer; by the time anything reaches the ledger the compiler has already lowered these calls into VM op sequences plus, for the witness natives, a `call-witness` marker carrying the bare string name. That is why the interpreter dispatches on the string `name` (see `Expr::CallWitness` handling and `try_builtin`), and why there is no upstream enum to import.

## The four execution surfaces

A name that shows up in a circuit's lowered IR reaches one of four places. Only the first two come from the native table; the other two are listed so the boundaries are clear.

1. **Native circuits (pure builtins).** The 15 `declare-native-entry circuit` primitives. In our interpreter these are `try_builtin(name, args)` arms.
2. **Native witnesses.** The 3 `declare-native-entry witness` primitives (`ownPublicKey`, `createZswapInput`, `createZswapOutput`). These are effectful: they read the caller's key or capture a coin to add to the transaction. In our interpreter they are modelled by the `WitnessNative` enum and matched exhaustively in the `Expr::CallWitness` arm of `eval_expr`, because routing them to the witness provider or `try_builtin` would error.
3. **Kernel ledger methods.** `kernel.self()`, `kernel.mintShielded(...)`, `kernel.claimZswapCoinSpend(...)`, `kernel.claimZswapCoinReceive(...)`, and friends are methods on the runtime-managed `Kernel` ledger type. They are not natives; the compiler lowers them to `ledger-query` op sequences, which the interpreter runs through `exec_ledger_query` (the same path as any other ledger read/write). `kernel.self()` is the one context read that needs the real contract address injected.
4. **High-level standard-library circuits.** `mintShieldedToken`, `sendShielded`, `receiveShielded`, `sendImmediateShielded`, `sendUnshielded`, `receiveUnshielded`, etc. (in `tools/compact-compiler/compiler/standard-library.compact`) are ordinary Compact circuits. They are compiled inline and decompose into the primitives above. They need no interpreter support of their own; they work as soon as the natives and kernel methods they call do. For example `mintShieldedToken` lowers to `tokenType` (persistentCommit) plus `kernel.self()`, `kernel.mintShielded`, `createZswapOutput`, `coinCommitment` (persistentHash), `kernel.claimZswapCoinSpend`, and the mint-to-self `kernel.claimZswapCoinReceive` branch.

## How the reference TypeScript runtime handles these

The reference path never dispatches on native names and has no native table of its own. For each native, the compiler emits TypeScript that calls the JS symbol named in the second field of the `declare-native-entry` (`__compactRuntime.SYMBOL`). `__compactRuntime` is the runtime package (the source is `tools/compact-compiler/runtime/src/`), which implements every primitive: the pure ones in `built-ins.ts`, the Zswap witnesses in `zswap.ts`. A consumer loads the compiler-generated contract JS and runs it; the runtime supplies the implementations by direct function call.

So that path gets "all of them, never missing one" for free: the compiler is the single source that both defines the natives and wires each call straight to its runtime implementation. The runtime's `createZswapOutput`, for instance, does exactly what our interpreter's special-case does (capture `(coin, recipient)`, insert the coin commitment, append to the local Zswap outputs); `ownPublicKey` just returns the caller's coin public key from the circuit context.

Our Rust SDK is different in kind: it does not run compiler-generated JS, it interprets the portable IR. There is no `__compactRuntime` to call, so each native has to be re-implemented in Rust. The native table is therefore our checklist, and the gap table below is the work item list.

## The 18 natives and our interpreter status

Status is against `crates/midnight-contract/src/interpreter.rs`. For pure circuits, "missing" means there is no `try_builtin` arm, so a call fails the builtin lookup. For witness natives, the closed set is modelled by the `WitnessNative` enum and matched exhaustively in `Expr::CallWitness`; the unimplemented variants fail with `unimplemented Compact witness native: NAME`, and adding a new variant forces the match to handle it (so a witness native can never be silently dropped).

### Native circuits (pure, `__compactRuntime.*`)

| Native | Returns | Status |
| --- | --- | --- |
All implemented pure natives delegate to the ledger's own primitives (`base-crypto`/`transient-crypto`), they are not reimplemented, so their values match what the prover computes by construction.

| Native | Returns | Status |
| --- | --- | --- |
| `transientHash` | Field | implemented (`try_builtin`, via `transient_hash`) |
| `transientCommit` | Field | implemented (via `transient_commit`) |
| `persistentHash` | Bytes 32 | implemented (via `PersistentHashWriter`) |
| `persistentCommit` | Bytes 32 | implemented (via `persistent_commit`) |
| `degradeToTransient` | Field | implemented |
| `upgradeFromTransient` | Bytes 32 | implemented (via `upgrade_from_transient`) |
| `keccak256` | Bytes 32 | missing (no ledger primitive to bind to; needs an external keccak) |
| `jubjubPointX` | Field | implemented |
| `jubjubPointY` | Field | implemented |
| `ecAdd` | JubjubPoint | implemented |
| `ecMul` | JubjubPoint | implemented |
| `ecMulGenerator` | JubjubPoint | implemented (arm matches both `ecMulGenerator` and `__builtin_ec_mul_generator`) |
| `hashToCurve` | JubjubPoint | implemented (via `hash_to_curve`) |
| `constructJubjubPoint` | JubjubPoint | implemented (via `EmbeddedGroupAffine::new`) |
| `jubjubScalarFromNative` | Field | missing (runtime symbol `reduceModJubjubOrder`; no direct ledger primitive identified) |

### Native witnesses (effectful, the `WitnessNative` enum in `Expr::CallWitness`)

| Native | Returns | Status | Notes |
| --- | --- | --- | --- |
| `ownPublicKey` | ZswapCoinPublicKey | recognized, not implemented | returns the caller's coin public key; needed by any circuit that reads its own key |
| `createZswapInput` | Void | implemented | captured into `ExecutionResult.zswap_inputs`; the call/deploy path builds a contract-owned `Input`, or a `Transient` when it pairs with a same-call self-output (as `receiveShielded` + `sendImmediateShielded` do). See `WitnessNative::CreateZswapInput` |
| `createZswapOutput` | Void | implemented | captured into `ExecutionResult.zswap_outputs`; see `WitnessNative::CreateZswapOutput` |

Today 15 of 18 are implemented. The 2 missing pure circuits (`keccak256`, `jubjubScalarFromNative`) have no ledger primitive to bind to, so they stay unimplemented until one is identified. The 1 missing witness native (`ownPublicKey`) is recognized by `WitnessNative` and fails with an explicit `unimplemented Compact witness native` error rather than silently; it unlocks `ownPublicKey`-using circuits and needs the same kind of context wiring `createZswapOutput`/`createZswapInput` got.

### Interpreter intrinsics outside the native table

`try_builtin` also implements a few names that are not `declare-native-entry` primitives (for example `leafHash`, `pad`) and the compiler keyword `disclose` is special-cased separately. These come from the compiler's intrinsics and Merkle helpers rather than the native table, so they are intentionally not in the list above. When auditing coverage, audit against the native table plus these known extras, not the table alone.

## Keeping this in sync

`midnight-natives.ss` is the source of truth and changes only when the Compact compiler is bumped. After a compiler bump, diff that file: any new `declare-native-entry` is a new primitive a contract can emit, and therefore a new row here and a potential interpreter gap.

This is guarded by the test `every_compact_native_is_handled_or_known_unimplemented` (in `crates/midnight-contract/src/interpreter.rs`). It holds a transcribed list of the native names and asserts each is either implemented (`try_builtin` arm or `WitnessNative`) or in an explicit `KNOWN_UNIMPLEMENTED` allowlist, so a native can never be silently dropped. When the `tools/compact-compiler` submodule is checked out (developer machines; CI does not init it), the test also re-parses the `declare-native-entry` names from `midnight-natives.ss` and asserts they match the transcribed list, so a compiler bump that adds or removes a native fails the test until this doc and the test list are updated.
