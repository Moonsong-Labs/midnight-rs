# Full shielded mint from Rust (recipient-discoverable, no `watchFor`)

Status: Scoped. Gap analysis corrected against the lowered IR (2026/06/26). Effort revised L/XL → M.
Date: 2026/06/26
Scope: `midnight-contract` (interpreter + call/deploy builders), with a small `midnight-wallet`/`midnight-helpers` touch. No compiler-fork change required (see Phase 0).

## Goal

Let a Rust caller invoke a circuit that mints a shielded coin to an **external** recipient such that the recipient's wallet discovers the coin through normal sync, with **no** `watchFor`. Concretely: support calling a contract like `gateway.mint(..., recipient_cpk, ...)` (which internally calls `mintShieldedToken`) and have the resulting on-chain Zswap output carry a ciphertext encrypted to the recipient's encryption public key.

This is the Rust equivalent of midnight-js's `additionalCoinEncPublicKeyMappings` call option (a `coinPk → encPk` map that attaches the discovery ciphertext to circuit-created outputs).

## Background: why two keys, and why `watchFor` today

A Midnight shielded recipient address is a pair `(coin_public_key, encryption_public_key)`:

- **coin_public_key (cpk)**: ownership. Hashed into the coin commitment that lands in the Merkle tree (`coinCommitment(coin, cpk)`). Only the holder of the matching coin secret key can later nullify (spend) it. This is the only key a Compact circuit sees: `createZswapOutput(coin, recipient_cpk)` / `mintShieldedToken(..., recipient_cpk)` take no encryption key.
- **encryption_public_key (epk)**: discovery. `Output::new(rng, coin, segment, cpk, Some(epk))` encrypts the coin note into the output's ciphertext, which the recipient's wallet trial-decrypts during sync to find the coin. Without the ciphertext the recipient must `watchFor(cpk, coin)`, i.e. already know the coin's nonce/type/value out of band.

So a circuit-created output to an external recipient never carries a ciphertext (the circuit has no epk), and the recipient needs `watchFor`. The fix is to attach the ciphertext at transaction-build time using a caller-supplied `cpk → epk` map. The contract circuit stays unchanged.

## Phase 0: compiler emission (DONE, verified)

Question: does the forked `compactc` (`RomarQ/compact` `feat/contract-info-extensions`, `0.31.104`) lower `mintShieldedToken` and emit the kernel native into the per-circuit `ir`, or does it drop it?

Verified by compiling a minimal `mintShieldedToken` contract with the pinned `tools/compact-compiler/result/bin/compactc` and inspecting `contract-info.json`:

- `circuits[0].ir.body.stmts[2].expr.body.body.bindings[0].value.name == "createZswapOutput"`: the kernel native **is** emitted into the lowered IR body (as a `call-witness`).
- The surrounding IR also carries the coin construction: `tokenType` → `persistentCommit`, the coin commitment preimage → `persistentHash<CoinPreimage>`, plus `degradeToTransient` / `transientHash` for the nonce. All hash/EC natives the interpreter already supports are present.

**Conclusion: no compiler-fork work is needed.** The IR is sufficient; the gap is entirely in the Rust interpreter and the call/deploy assembly.

## The gap (corrected after reading the lowered IR, 2026/06/26)

Compiling a minimal `mintShieldedToken` probe (`tools/compact-compiler/result/bin/compactc`, pinned) and inspecting the lowered IR plus the runtime sources changes the picture from the original framing. The mint/spend effects are **already emitted** by the existing interpreter; the real missing piece is much smaller.

What `mintShieldedToken` actually lowers to (verified against the probe IR and `tools/compact-compiler/compiler/standard-library.compact`):

```
const coin = ShieldedCoinInfo { nonce, color: tokenType(domain_sep, kernel.self()), value };
kernel.mintShielded(domain_sep, value);   // ledger-query: idx[4] member add ins  -> Effects.shielded_mints[domain_sep] += value
createZswapOutput(coin, recipient);        // call-witness: NO effect; marks "attach a Zswap output here"
const cm = coinCommitment(coin, recipient);// persistentHash<CoinPreimage> (already supported)
kernel.claimZswapCoinSpend(cm);            // ledger-query: idx[2] push push ins   -> Effects.claimed_shielded_spends.insert(cm)
if (!recipient.is_left && recipient.right == kernel.self()) {
  kernel.claimZswapCoinReceive(cm);        // ledger-query: idx[1] ... ins         -> only when minting to self (contract)
}
```

The probe IR confirms exactly this: 5 `ledger-query` nodes (`idx[0]` self, `idx[4]` mint, `idx[2]` spend, `idx[0]` self, `idx[1]` receive) and 1 `call-witness` (`createZswapOutput`).

Consequences:

1. **The mint and spend effects are ledger-query ops, not a new native.** `kernel.mintShielded` and `kernel.claimZswapCoinSpend` lower to the `program_fragments` op sequences (`idx` into Effects field N, then `ins`). The interpreter already runs every `ledger-query` op via `exec_ledger_query` (`interpreter.rs:1887`), pushing them into `gather_ops`. `partition_transcripts` re-runs the derived `verify_ops` through its own `QueryContext` (which owns the Effects register: `ContractStateExt::query` builds `QueryContext { effects: Default::default(), .. }`, onchain-runtime 3.1.0 `contract_state_ext.rs:38`), so the transcript carries `shielded_mints` + `claimed_shielded_spends` with **no interpreter change**. The `verify_ops` filter (`call.rs:293`) keeps them (`idx` path non-empty, `Ins { n: 2 }`).

2. **For an external user recipient the effect is `shielded_mints` + `claimed_shielded_spends`, NOT `claimed_shielded_receives`.** `claimed_shielded_receives` is only written on the mint-to-self (contract) branch. The original plan conflated the two. The ledger's `well_formed` matches *contract-addressed* outputs (`o.contract_address.is_some()`) against `claimed_shielded_receives` (`midnight-ledger 8.1.0 verify.rs`); a user-recipient output has `contract_address == None` and is governed by **balance** (`shielded_mints` adds `+value` for `custom_shielded_token_type(domain_sep)`, the output removes it) plus the `claimed_shielded_spends` membership that `partition_transcripts` uses to assign the output's segment.

3. **The only interpreter gap is `createZswapOutput`.** It is a `call-witness` with no handler today, so dispatch falls through witness → builtin → helper and **errors** (`"no witness provider, builtin, or helper for: createZswapOutput"`, `interpreter.rs:912`). It must be handled as a unit-returning no-op that **also captures its `(coin, recipient)` args**, which is precisely the data the offer output needs. No faithful, commitment-matching coin reconstruction is required: the commitment the ledger checks (`cm` in `claimed_shielded_spends`, and the output's `coin_com`) is computed by the IR's own `persistentHash<CoinPreimage>` and re-derived identically by `Output::new` from the same coin fields.

4. **The call path still emits an empty Zswap offer.** `call.rs:493` sets `OfferInfo { inputs: [], outputs: [], transients: [] }`. The real feature work is building `Output::new(rng, &coin, segment, &cpk, Some(epk))` from the captured coin/recipient and placing it there.

The reusable primitive already exists and is used by the wallet transfer path: `Output::new(rng, &coin, segment, &cpk, Some(epk))` (`midnight-zswap` 8.1.0; re-exported via `midnight-helpers`). No ledger version bump.

## Plan

### Phase 1: capture the circuit-created coin in the interpreter (corrected: not the hard part)

The mint/spend effects already reach the transcript (see "The gap" above). Phase 1 is now just: stop erroring on `createZswapOutput`, and surface the coin it would create so the call path can attach the offer output.

- In `interpreter.rs`, add a `createZswapOutput` arm to `CallWitness` dispatch (next to the existing `disclose` special-case). Evaluate its two args (`coin: ShieldedCoinInfo`, `recipient: Either<ZswapCoinPublicKey, ContractAddress>`), record `(coin, recipient)` on a new `ExecutionResult` field (e.g. `zswap_outputs: Vec<CircuitCoinOutput>`), and return `Value::Void`. Do **not** route it to the witness provider/builtin/helper.
- Decode the evaluated `coin` arg into the fields `Output::new` needs: `nonce: Bytes<32>`, `color/type_: ShieldedTokenType(Bytes<32>)`, `value: Uint`. The `coin` arrives as the `Value` of the IR `new ShieldedCoinInfo { nonce, color, value }` node, so it is a struct-encoded `AlignedValue`; slice it with the shipped `ShieldedCoinInfo` layout (the interpreter already does field-slicing of struct `AlignedValue`s). The `recipient` gives `is_left` + the 32-byte `left`/`right`.
- No independent commitment computation: `Output::new` re-derives `coin_com` from these fields exactly as the IR's `coinCommitment`/`claimZswapCoinSpend` did, so they match by construction.
- Interpreter detail to handle: the `Either` / `ZswapCoinPublicKey` argument structs are declared **inline in the circuit `arguments`**, not in the shipped `structs` table. The interpreter needs their layout to destructure `recipient.is_left` / `recipient.left.bytes`. Confirm whether codegen surfaces inline arg structs as `StructDef`s; if not, synthesize them (or pass via `arg_types` + a structs supplement) so field access resolves.

Files: `crates/midnight-contract/src/interpreter.rs` (`createZswapOutput` arm + `ExecutionResult.zswap_outputs`).

Effort: **S/M**. No coin/kernel effect model to build; the effects are already emitted by the existing `ledger-query` path. The work is capturing two args and decoding the coin struct.

Residual integration risks to validate empirically (devnet), previously hidden under the "commitment mismatch" headline:
- `kernel.self()`: **resolved by inspection.** `CompactStandardLibrary` declares `export ledger kernel: Kernel;` first, so the contract's own address lives in the `kernel` ledger at **state index 0**, and `kernel.self()` lowers to `dup idx[0] popeq` (cached: `idxc`/`popeqc`). It reads the contract state, **not** the `QueryContext.address`; the interpreter's existing `ContractStateExt::query` (address defaulted) is therefore fine; the address comes from the real deployed state. Caveat for tests: the cached ops need the content-addressed cache that only a real deserialized state carries; a hand-built `StateValue` yields `CacheMiss`, so full-circuit execution is a devnet-E2E concern, not a unit test (see `crates/midnight-contract/tests/circuit_call.rs::interpreter_runs_mint_shielded_token_circuit`, `#[ignore]`).
- Segment (guaranteed vs fallible) assignment for the output, matched via `claimed_shielded_spends` membership of `out.coin_com` in `partition_transcripts`.
- `well_formed` / balance: `shielded_mints(+value)` vs the user output (`-value`), and token-type equality (`Output` coin color must equal `custom_shielded_token_type(domain_sep)`).

### Phase 2: the encryption-key mapping (the feature proper)

Once Phase 1 produces a coin commitment, attach the discovery ciphertext.

- Public API: add a call builder method `with_coin_encryption_keys(map: impl IntoIterator<Item = (CoinPublicKey, EncryptionPublicKey)>)`. There is no call builder today (`call`/`call_with` are direct async methods, already `#[allow(clippy::too_many_arguments)]`), so introduce a `CallBuilder` rather than growing the positional list. Mirror the existing `DeployBuilder::with_shielded_offer` shape (`contract.rs:209`).
- Thread the map into `call::call_funded_with`. For each circuit-created coin whose recipient `cpk` is in the map, build `Output::new(rng, &coin, segment, &cpk, Some(epk))` and place it in the guaranteed (or fallible) offer instead of the empty offer at `call.rs:493`. Segment selection must match where the call's claimed receive landed (mirror upstream `add_calls` matching by `coin_com`).
- Deploy-path equivalent: constructors can mint, and `deploy_funded` already threads a custom `shielded_offer` (`deploy.rs:113`). Add `DeployBuilder::with_coin_encryption_keys` and apply the same output rewrite.

Files: `crates/midnight-contract/src/contract.rs` (`CallBuilder` + `with_coin_encryption_keys`; `DeployBuilder::with_coin_encryption_keys`), `crates/midnight-contract/src/call.rs` (accept map, build offer), `crates/midnight-contract/src/deploy.rs` (deploy mints).

Effort: **M**. Primitive exists, deploy path already accepts a custom offer; mostly builder + offer assembly + segment matching.

### Implementation status (2026/06/26)

- **Phase 1: done.** `interpreter.rs` now captures `createZswapOutput(coin, recipient)` on `ExecutionResult.zswap_outputs: Vec<CircuitZswapOutput>` (a `{ coin: Value, recipient: Value }`) and returns unit, instead of erroring. Unit test: `crates/midnight-contract/tests/circuit_call.rs::interpreter_captures_create_zswap_output` (green). The full lowered-circuit execution test (`interpreter_runs_mint_shielded_token_circuit`) is `#[ignore]` because it needs a real cached deployed state (see open question 4); it is the devnet-E2E surface.
- Probe fixture committed at `tests/fixtures/mint-probe-contract-info.json` (minimal `mintShieldedToken` contract) for the above.
- **Phase 2: code done (call path), unit-verified; deploy path + E2E pending.**
  - `call.rs`: `decode_shielded_output` (`CircuitZswapOutput` → `CoinInfo` + recipient), a `BuildOutput` impl `MintedCoinOutput` that emits the exact circuit coin (with optional discovery ciphertext), and `build_shielded_offer_outputs` that attaches the `epk` when the recipient's `cpk` is in the map. The empty guaranteed offer at `call.rs:493` is replaced by these outputs. Unit test `decode_shielded_output_extracts_coin_and_user_recipient` ✅.
  - `contract.rs`: `Contract::with_coin_encryption_keys(impl IntoIterator<Item=(CoinPublicKey, EncryptionPublicKey)>)` builder + a `coin_encryption_keys` field, threaded through `call_with` → `call_funded_with` (new `coin_encryption_keys` param).
  - Builds clean, clippy 0, fmt clean, full contract unit suite green.
  - **Deploy path (`DeployBuilder::with_coin_encryption_keys`): not done.** Not needed for the call-mint E2E (deploy a mint contract, then call `mint`).
- **arg-types gap: CLOSED (2026/06/26).** `call_funded_with` previously called the interpreter with `arg_types: &[]`, so a **struct argument** (the `Either` recipient) had no declared type and the interpreter couldn't destructure `recipient.is_left` / `.left.bytes`. Fixed:
  - New pure module `compact-codegen/src/arg_types.rs`: `type_node_to_type_ref` (ABI `type-name` schema → IR `TypeRef`), `collect_inline_defs`/`collect_argument_defs` (harvest inline `Either`/`ZswapCoinPublicKey`/`ContractAddress` struct+enum defs from a circuit's `arguments`), and `circuit_arg_types`. 5 unit tests.
  - `arg_types: &[(&str, TypeRef)]` threaded through `Contract::call_with` → `call_funded_with` → `execute_with_owned` (was `&[]` at `call.rs`).
  - Codegen (`expand/ledger.rs`, `expand/circuit_calls.rs`): generated `call_*` methods now embed per-circuit arg-types JSON and pass `&__arg_types`; `__STRUCTS_JSON`/`__ENUMS_JSON` now include harvested inline arg structs/enums. Generated bindings for all fixture contracts still compile; workspace clippy clean.
  - Tests: `harvested_defs_cover_inline_either_recipient` (real fixture → harvested registry covers `Either`/`ZswapCoinPublicKey`/`ContractAddress`); `mint_probe_ir_and_structs` refactored to use `collect_argument_defs` instead of the hand-built `either_struct_defs()`. Full contract suite green (12 pass, 2 ignored), 68 lib + 5 codegen-arg_types unit tests pass.
- **Phase 3: pending (devnet is up).** A local devnet is running (node 0.22.5 :9944, indexer 4.3.2 :8088, proof-server 8.1.0 :6300). The mint contract is compiled **with** proving keys (probe source: `mint(domain_sep, value, nonce, recipient)` calling `mintShieldedToken`). Remaining E2E steps, in order:
  1. ~~Close the arg-types gap~~. **Done** (above).
  2. Deploy a mint contract. Open: the contract's `ledger` is empty but `kernel.self()` reads state index 0 (the runtime-managed `kernel`); confirm whether deploy must seed the kernel state or the node injects it.
  3. Call `mint` with a second wallet's `(cpk, epk)` via `with_coin_encryption_keys`; resolve any balance reconciliation (the `shielded_mints(+value)` effect vs the offer output `-value`, token-type `custom_shielded_token_type(domain_sep)`).
  4. Sync the second wallet and assert it discovers the coin (right type/value) with no `watchFor`.
  Recipient `(cpk, epk)` derivation for the test: `midnight_wallet::address::derive_shielded(&seed, network)` then `midnight_wallet::transfer::parse_shielded_recipient(addr)` → `ShieldedWallet { coin_public_key, enc_public_key }`.

### Portable-IR fixes for the mint circuit (2026/06/26): interpreter now runs it end to end

Wiring the interpreter to run the mint circuit against an empty deployed state surfaced four portable-IR gaps, all now fixed; the full circuit executes in-memory and the captured coin's color correctly depends on the contract address.

- **kernel.self() resolution: done.** Threaded the real `contract_address` into `execute_with_owned` → `ExecContext` and special-cased the `kernel.self()` read (result-type `ContractAddress`, `dup; idx[0]; popeq` shape) to return the supplied address. Covered by `interpreter_resolves_kernel_self_to_supplied_address`.
- **`persistentCommit` builtin: done + proven.** Added the missing `persistentCommit(value, opening)` builtin (the token-color derivation `tokenType(domain_sep, self())`). Proven byte-for-byte against the ledger's own `ContractAddress::custom_shielded_token_type` in `persistent_commit_matches_custom_shielded_token_type`. The minted coin's color is therefore correct.
- **stack-keyed `idx`: done.** The portable IR encodes a stack-keyed `idx` (e.g. keyed by a coin commitment) as a value literal `"stack"` typed `Uint<255>`; mapped back to `Key::Stack`.
- **Boolean from a struct slice: done.** A Boolean field sliced out of a struct (`recipient.is_left`) arrives as a 1-byte `AlignedValue`; `encode_typed` now re-encodes it as Boolean so it can flow into another struct field (`CoinPreimage.dataType`).
- **`dup` arity: RESOLVED via compiler patch (user's choice).** Root cause: `contract-info.json` serialized **every** `dup` as `{"op":"dup"}` with no `n`, but the compiled VM ops for `mintShieldedToken`/`claimZswapCoinSpend` use `dup{n:1}`/`dup{n:2}` to reach the `shielded_mints`/`claimed_shielded_spends` map deeper in the stack. With the forced `n:0` the effect ops mis-navigated (`member` saw the key, not the map). Fix has two halves:
  - **Compiler** (`tools/compact-compiler`, branch `fix/portable-ir-dup-arity`): `save-contract-info-passes.ss` now emits `(cons "n" ...)` for `dup`: the arity is already in the vminstr; the pass just dropped it (`swap`/`ins`/`noop` already kept theirs). **REBUILT AND VERIFIED BY EXECUTION** (Chez from brew; libdirs = `compiler` + `third_party/compiler` + nanopass `f3100ce` + rough-draft `6a5e64a` + `srcMaps`). Recompiled `mint.compact` with `--skip-zk`: the regenerated `contract-info.json` emits `dup` arities `[2,1,1,2,2,2]`, identical to the authoritative JS. **Not committed/pushed** (branch change is uncommitted).
  - **Interpreter** (our repo): `LedgerOp::Dup { #[serde(default)] n }` (old artifacts still parse as `n=0`, covered by `dup_op_parses_arity_and_defaults_to_zero`); emits `Op::Dup { n }`.
  - **Verified end to end:** `interpreter_runs_mint_shielded_token_circuit` runs the full circuit using `tests/fixtures/mint-probe-contract-info-dupn.json`, which is now the **genuine patched-compiler output** (byte-identical mint IR to the earlier JS-derived hand-correction). The captured coin's color depends on the contract address.
  - **Real E2E: written, blocked by a devnet dust-timing issue unrelated to mint.** Test: `crates/midnight-contract/tests/mint_external_recipient.rs` (gated on `MINT_KEYED_DIR` + node/indexer). Keyed dir pairs the regenerated `contract-info.json` (dup arities) with the existing proving keys (`dup.n` is ledger-effect IR only, not the ZK circuit, so keys are unaffected). Running it: the deploy tx builds and **proves** fine, then the chain rejects it with **custom error 171 = `MalformedError::OutOfDustValidityWindow`**. The canonical `example-counter` deploy **fails identically** against this devnet, so it is a pre-existing SDK/devnet dust-timing issue, not a mint problem. Root cause: the dust spend's `ctime` is set to wall-clock now, but the validating best block's `tblock` lags ~5s (blocks are ~6s apart), and the check is `ctime > tblock || ctime + grace < tblock` with zero future tolerance. Needs either a devnet refresh or an SDK fix clamping the dust `ctime` to the chain's latest `tblock`, tracked separately from this feature.
  - **Net:** the mint feature (compiler + interpreter + call-path + offer assembly) is complete and verified at every layer the local environment allows; only the live submit is gated on the unrelated dust-timing issue.

### Phase 3: end-to-end verification

- Compile + deploy a `mintShieldedToken` contract (and `gateway.compact`) on the local devnet (`devnet/`).
- Call `mint` from Rust with a *second* wallet's `(cpk, epk)` via `with_coin_encryption_keys`.
- Assert the second wallet's normal sync (no `watchFor`) shows the minted coin with the right type/value.
- Add a regression test under `crates/midnight-contract/tests/` using a native-mint fixture.

## Effort & risks (corrected)

- Total: **M**, dominated by Phase 2 (offer assembly + builder API). Phase 1 dropped to **S/M** once the IR showed the mint/spend effects are already-emitted `ledger-query` ops and `createZswapOutput` is just an unhandled `call-witness`.
- Top *remaining* risk: balance/segment correctness on a real proof (token-type equality and guaranteed-vs-fallible placement of the mint output). `kernel.self()` is no longer a risk (it reads the `kernel` ledger at state[0], confirmed by inspection).
- Lower risk than originally feared: commitment matching. `Output::new` re-derives `coin_com` from the same coin the IR built, and the spend-claim `cm` is the IR's own `persistentHash<CoinPreimage>`. No independent reconstruction.
- Not a risk: ledger API (the `Some(epk)` ciphertext constructor exists in the pinned 8.1.0) and compiler emission (Phase 0 confirmed; the corrected gap analysis re-confirmed via the probe IR).
- Secondary: the helpers path uses `intent.add_call` (singular), not the upstream `add_calls` that auto-matches `zswap_outputs`; we replicate the segment/`coin_com` matching locally in `call_funded_with` (lower risk than refactoring to `add_calls`).

## Open questions (resolved / remaining)

1. ~~How `createZswapOutput` is tagged in the IR~~. **Resolved**: it is a `call-witness` (`Expr::CallWitness { name: "createZswapOutput", args: [coin, recipient] }`). Special-case it in `CallWitness` dispatch like `disclose`: capture args, return unit, never reach the provider.
2. ~~Whether the mint/spend/receive kernel effects need explicit emission~~. **Resolved**: no. `kernel.mintShielded` / `kernel.claimZswapCoinSpend` / `kernel.claimZswapCoinReceive` are `ledger-query` op sequences already executed and already carried into the transcript by `partition_transcripts`. For an external user only `shielded_mints` + `claimed_shielded_spends` are written; `claimed_shielded_receives` is the contract-self branch only.
3. Segment (guaranteed vs fallible) assignment for the mint output, matched via `claimed_shielded_spends` membership of `out.coin_com`: **remaining**, validate on devnet.
4. **`kernel.self()`: PREVIOUS RESOLUTION WAS WRONG (corrected 2026/06/26).** It does **not** read user state[0]. Verified against the compiled JS + the onchain-runtime: `kernel.self()` lowers to `dup{n:2} idx[0] popeq`. The VM stack is `[context, effects, state]` (`QueryContext::to_vm_stack`), and `Op::Dup{n}` pushes `stack[len-n-1]`, so `dup{n:2}` duplicates the **context**, whose `idx[0]` is `context.address` (`impl From<&QueryContext> for VmValue` puts the address cell at array index 0). The deployed contract `data` is an **empty array** (`initialState` builds `StateValue.newArray()` with no kernel cell), so there is nothing at state[0]. Two concrete problems for our portable-IR interpreter:
   - The portable IR drops the `dup` n: every `dup` in `contract-info.json` is `{"op":"dup"}` (no `n`), and the interpreter hardcodes `Op::Dup{n:0}`, so it duplicates the (empty) state and `idx[0]` fails with `CacheMiss` (exactly the symptom seen in the ignored test).
   - `ContractStateExt::query` builds its `QueryContext` with `address: Default::default()` (zero), so even with the right `n` the interpreter would compute the mint coin's color from a **zero** address. The coin color is `tokenType(domain_sep, contractAddress)`, so a wrong address means a wrong token type and an offer Output that won't match the on-chain `shielded_mints` effect.
   - **Fix (in progress):** thread the real `contract_address` into the interpreter and special-case the `kernel.self()` ledger-query (`result_type == Struct ContractAddress` with the `dup; idx[0]; popeq` shape) to return the real address directly, bypassing the VM. This is the only context-read in the mint circuit; the mint/spend effect ops run against the default effects fine.
5. ~~Whether codegen surfaces the inline `Either`/`ZswapCoinPublicKey` arg structs as `StructDef`s the interpreter can use to destructure `recipient`~~. **Resolved**: the compiler inlines them in the arg type and does **not** add them to the top-level `structs` table for a mint-only contract. We now harvest them via `compact_codegen::arg_types::collect_argument_defs`, both at codegen time (merged into `__STRUCTS_JSON`/`__ENUMS_JSON`) and available to direct callers. The interpreter receives the full registry plus per-arg `arg_types`.
