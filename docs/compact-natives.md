# Compact native primitives and how the interpreter handles them

Reference for everyone touching the Compact interpreter (`crates/compact/interpreter`) and its builtins (`crates/compact/runtime`). It answers two questions: what is the complete set of Compact "native" primitives a circuit can invoke, and which ones our portable-IR interpreter implements today. The point is to have a single checklist so we can implement them all without missing one.

## Authoritative source

The complete, canonical list lives in the Compact compiler, not in `midnight-ledger`. It is two `declare-native-entry` tables of the compiler fork, at the commit that `COMPACT_REV` in the `Makefile` names:

- [`compiler/midnight-natives.ss`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/compiler/midnight-natives.ss) declares the natives of every compile.
- [`compiler/zkir-v3-natives.ss`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/compiler/zkir-v3-natives.ss) declares the natives that the compiler adds with `--feature-zkir-v3`, and the native types of the secp256k1, secp256r1 and Curve25519 curves.

Each entry has one of two kinds:

- `declare-native-entry circuit NAME "__compactRuntime.SYMBOL" (args...) RetType`: a pure function (hashing, EC math).
- `declare-native-entry witness NAME "__compactRuntime.SYMBOL" (args...) RetType`: an effectful primitive that reads or mutates the circuit's Zswap/coin context.

`NAME` is the source name, and it is not unique: the ZKIR v3 table declares `ecAdd`, `ecMul` and `ecMulGenerator` again for each foreign curve, and `neg` and `inv` for each foreign field. `SYMBOL` is unique. The analyzed IR keeps each entry as a native declaration with its symbol (`entry`), and the interpreter dispatches a call on that symbol (`Program::runtime_name`).

`midnight-ledger`, `onchain-runtime`, and `onchain-vm` have no enum of these names. They operate one level below Compact, at the VM op (`Op`) and effects layer; by the time anything reaches the ledger the compiler has already lowered these calls into VM op sequences plus, for the witness natives, a `call-witness` marker carrying the bare string name. That is why the interpreter dispatches on a string (see `eval_witness_call` and `try_builtin`), and why there is no upstream enum to import.

## The four execution surfaces

A name that shows up in a circuit's lowered IR reaches one of four places. Only the first two come from the native table; the other two are listed so the boundaries are clear.

1. **Native circuits (pure builtins).** The `declare-native-entry circuit` primitives. In our interpreter these are `try_builtin(symbol, args)` arms.
2. **Native witnesses.** The `declare-native-entry witness` primitives (`ownPublicKey`, `createZswapInput`, `createZswapOutput`). These are effectful: they read the caller's key or capture a coin to add to the transaction. In our interpreter they are modelled by the `WitnessNative` enum and matched exhaustively in `eval_witness_call`, because routing them to the witness provider or `try_builtin` would error.
3. **Kernel ledger methods.** `kernel.self()`, `kernel.mintShielded(...)`, `kernel.claimZswapCoinSpend(...)`, `kernel.claimZswapCoinReceive(...)`, and friends are methods on the runtime-managed `Kernel` ledger type. They are not natives; the compiler lowers them to `ledger-query` op sequences, which the interpreter runs through `exec_ledger_query` (the same path as any other ledger read/write). `kernel.self()` is the one context read that needs the real contract address injected.
4. **High-level standard-library circuits.** `mintShieldedToken`, `sendShielded`, `receiveShielded`, `sendImmediateShielded`, `sendUnshielded`, `receiveUnshielded`, etc. (in [`compiler/standard-library.compact`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/compiler/standard-library.compact) of the fork) are ordinary Compact circuits. They are compiled inline and decompose into the primitives above. They need no interpreter support of their own; they work as soon as the natives and kernel methods they call do. For example `mintShieldedToken` lowers to `tokenType` (persistentCommit) plus `kernel.self()`, `kernel.mintShielded`, `createZswapOutput`, `coinCommitment` (persistentHash), `kernel.claimZswapCoinSpend`, and the mint-to-self `kernel.claimZswapCoinReceive` branch.

## How the reference TypeScript runtime handles these

The reference path never dispatches on native names and has no native table of its own. For each native, the compiler emits TypeScript that calls the JS symbol named in the second field of the `declare-native-entry` (`__compactRuntime.SYMBOL`). `__compactRuntime` is the runtime package (the source is [`runtime/src/`](https://github.com/RomarQ/compact/tree/fa2181fbc6dac2135defdb4f55ce10d8332185d5/runtime/src) of the fork), which implements every primitive: the pure ones in `built-ins.ts`, the Zswap witnesses in `zswap.ts`. A consumer loads the compiler-generated contract JS and runs it; the runtime supplies the implementations by direct function call.

So that path gets "all of them, never missing one" for free: the compiler is the single source that both defines the natives and wires each call straight to its runtime implementation. The runtime's `createZswapOutput`, for instance, does exactly what our interpreter's special-case does (capture `(coin, recipient)`, insert the coin commitment, append to the local Zswap outputs); `ownPublicKey` just returns the caller's coin public key from the circuit context.

Our Rust SDK is different in kind: it does not run compiler-generated JS, it interprets the portable IR. There is no `__compactRuntime` to call, so each native has to be re-implemented in Rust. The native table is therefore our checklist, and the gap table below is the work item list.

## The natives and our interpreter status

Status is against `crates/compact/interpreter` and `crates/compact/runtime`. For pure circuits, "missing" means there is no `try_builtin` arm, so a call fails the builtin lookup. For witness natives, the `WitnessNative` enum models the closed set, and `eval_witness_call` matches it exhaustively. A new variant forces the match to handle it, so the interpreter cannot drop a witness native silently.

### Native circuits (pure, `__compactRuntime.*`)

The source name and the symbol of each native in this table are the same. Every implemented pure native except `keccak256` calls the ledger's own primitives (`base-crypto`/`transient-crypto`). So its value matches what the prover computes. `keccak256` digests the same bytes as `persistentHash` with Keccak-256 from the `sha3` crate, as ZKIR v3 does.

| Native | Returns | Status |
| --- | --- | --- |
| `transientHash` | Field | implemented (`try_builtin`, via `transient_hash`) |
| `transientCommit` | Field | implemented (via `transient_commit`) |
| `persistentHash` | Bytes 32 | implemented (via `persistent_hash`) |
| `persistentCommit` | Bytes 32 | implemented (via `persistent_commit`) |
| `degradeToTransient` | Field | implemented |
| `upgradeFromTransient` | Bytes 32 | implemented (via `upgrade_from_transient`) |
| `keccak256` | Bytes 32 | implemented (via `sha3::Keccak256` over the `persistentHash` bytes) |
| `jubjubPointX` | Field | implemented |
| `jubjubPointY` | Field | implemented |
| `ecAdd` | JubjubPoint | implemented |
| `ecNeg` | JubjubPoint | implemented (via `Neg` on `EmbeddedGroupAffine`) |
| `ecMul` | JubjubPoint | implemented |
| `ecMulGenerator` | JubjubPoint | implemented (arm matches both `ecMulGenerator` and `__builtin_ec_mul_generator`) |
| `hashToCurve` | JubjubPoint | implemented (via `hash_to_curve`) |
| `constructJubjubPoint` | JubjubPoint | implemented (via `EmbeddedGroupAffine::new`) |

### Native witnesses (effectful, the `WitnessNative` enum in `eval_witness_call`)

| Native | Returns | Status | Notes |
| --- | --- | --- | --- |
| `ownPublicKey` | ZswapCoinPublicKey | implemented | returns `Env.coin_public_key` and appends it to the private transcript, as a witness result. The call path takes the key from the attached wallet. With no key, it fails with `InterpreterError::Witness`. See `WitnessNative::OwnPublicKey` |
| `createZswapInput` | Void | implemented | captured into `ExecutionResult.zswap_inputs`; the call/deploy path builds a contract-owned `Input`, or a `Transient` when it pairs with a same-call self-output (as `receiveShielded` + `sendImmediateShielded` do). See `WitnessNative::CreateZswapInput` |
| `createZswapOutput` | Void | implemented | captured into `ExecutionResult.zswap_outputs`; see `WitnessNative::CreateZswapOutput` |

The interpreter implements every native of `midnight-natives.ss`.

### ZKIR v3 natives (`zkir-v3-natives.ss`)

The compiler declares these natives only with `--feature-zkir-v3`. A contract compiled for ZKIR v3 gets `verifier-key[v7]` keys, which only ledger 9 accepts.

| Native | Symbols | Returns | Status |
| --- | --- | --- | --- |
| `sha512` | `sha512` | Bytes 64 | implemented (via `sha2::Sha512` over the `persistentHash` bytes, as ZKIR v3 does) |
| `neg`, `inv` | `{secp256k1,secp256r1,curve25519}{Base,Scalar}{Neg,Inv}` | the same field | missing |
| `secp256k1PointX`, `secp256k1PointY` | the same | Secp256k1Base | missing |
| `secp256r1PointX`, `secp256r1PointY` | the same | Secp256r1Base | missing |
| `curve25519PointX`, `curve25519PointY` | the same | Curve25519Base | missing |
| `ecAdd`, `ecMul`, `ecMulGenerator` | `{secp256k1,secp256r1,curve25519}{Add,Mul,MulGenerator}` | the curve's point | missing |

The missing natives all need foreign field and point values, which the interpreter does not have. The analyzed-IR parser knows no secp256r1 or Curve25519 type, and the interpreter and the codegen refuse the secp256k1 types. The TypeScript runtime also exports `+`, `-` and `*` for each foreign field (`secp256k1BaseAdd` and the others). They are not in the native tables, but they need the same values. ZKIR v3 computes these values with the `k256`, `p256` and `curve25519` modules of `midnight-curves`, which this workspace already pins.

### Interpreter intrinsics outside the native table

`try_builtin` also implements a few names that are not `declare-native-entry` primitives (for example `leafHash`, `pad`) and the compiler keyword `disclose` is special-cased separately. These come from the compiler's intrinsics and Merkle helpers rather than the native table, so they are intentionally not in the list above. When auditing coverage, audit against the native table plus these known extras, not the table alone.

## Keeping this in sync

`midnight-natives.ss` and `zkir-v3-natives.ss` are the source of truth and change only when the Compact compiler is bumped. After a compiler bump, diff both files: any new `declare-native-entry` is a new primitive a contract can emit, and therefore a new row here and a potential interpreter gap.

This is guarded by the test `every_compact_native_is_handled_or_known_unimplemented` (in `crates/compact/interpreter/src/lib.rs`). It reads the runtime symbols from `crates/compact/interpreter/src/compact-natives.txt`, which `make compact-natives` writes from both tables at `COMPACT_REV`. It asserts that each symbol is either implemented (`try_builtin` arm or `WitnessNative`) or in an explicit `KNOWN_UNIMPLEMENTED` allowlist, so a native can never be silently dropped. The codegen-drift workflow runs `make compact-natives` and fails when the file changes. So a compiler bump that adds or removes a native fails there until the file is regenerated. The unit test then names each new native that has no implementation. Update this doc in the same change.
