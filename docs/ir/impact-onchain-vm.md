# Impact VM (on-chain transcript VM)

**Where:** `midnight-ledger`, `onchain-vm` crate. The instruction set is the `Op` enum (`onchain-vm/src/ops.rs`), executed by `onchain-runtime`.

**On/off chain:** on-chain. This is the genuinely on-chain instruction set.

**Purpose:** execute a contract call's public transcript, a stack-machine `Op` program, to read and update the contract's on-chain state and validate the state transition. The same transcript is also built and run off-chain by the interaction layer when producing a transaction.

## Op set

A stack machine over `StateValue` / `AlignedValue`. The ops: `Noop`, `Lt`, `Eq`, `Type`, `Size`, `New`, `And`, `Or`, `Neg`, `Log`, `Root`, `Pop`, `Popeq`, `Addi`, `Subi`, `Push`, `Branch`, `Jmp`, `Add`, `Sub`, `Concat`, `Member`, `Rem`, `Dup`, `Swap`, `Idx`, `Ins`, `Ckpt`.

Notable ones:
- `Idx` navigates into the current `StateValue` (Array, Map, Cell, BoundedMerkleTree) by an `AlignedValue` key.
- `Ins` / `Rem` insert and remove. `Member` tests membership. `Root` reads a Merkle root. `Popeq` pops and asserts equality. `Ckpt` checkpoints.

## State model

The VM walks the contract's self-describing `StateValue` tree (each node carries its own variant tag) using `AlignedValue` keys. It needs no Compact-level type schema to navigate, the keys are raw field-aligned values. The typed schema needed to construct those keys and decode results lives in `contract-info.json` (see [circuit-body-ir.md](circuit-body-ir.md)).

## Relationship to ZKIR

The transcript's public values are exactly the `public_transcript_inputs` / `public_transcript_outputs` that ZKIR binds to in its proof. The chain runs the transcript through the Impact VM to apply and validate state, and separately verifies the ZK proof against the on-chain verifier key. The two are tied together by those public values. See [zkir.md](zkir.md).

## Depends on / produces

- **Depends on:** a transcript (`Op` program) built off-chain by the interaction layer.
- **Produces:** the applied and validated contract state transition on-chain.
