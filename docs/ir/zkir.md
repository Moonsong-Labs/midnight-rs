# ZKIR

**Where:** `midnight-ledger`, `zkir-v3` crate. The type is `IrSource` (serialization tag `ir-source[v3]`). The circuit synthesizer/evaluator is `zkir-v3/src/ir_vm.rs`.

**On/off chain:** off-chain. Only the derived verifier key is stored on-chain.

**Purpose:** the proving IR. Its own doc comment calls it "a low-level IR allowing the prover to populate circuit witnesses." It encodes a circuit as a flat instruction list and is compiled to Plonk prover and verifier keys.

## Shape

- **Value types (`IrType`):** exactly two, `Native` (an `Fr` field element) and `JubjubPoint`. Every Compact-level type (Uint, Bytes, Boolean, Vector, Tuple, Struct, Enum, and the ledger ADTs) has been flattened away before ZKIR.
- **Instructions:** `Encode`, `Decode`, `Assert`, `CondSelect`, `ConstrainBits`, `ConstrainEq`, `ConstrainToBoolean`, `Copy`, `Impact`, `EcMul`, `EcMulGenerator`, `HashToCurve`, `DivModPowerOfTwo`, `ReconstituteField`, `Output`, `TransientHash`, `PersistentHash`, `TestEq`, `Add`, `Mul`, `Neg`, `Not`, `LessThan`, `PublicInput`, `PrivateInput`.

## How it is used

ZKIR is run via `IrSource::prove(preimage)` and `check(preimage)`. The `ProofPreimage` (`transient-crypto/src/proofs.rs`) already contains the finished computation:

- `inputs: Vec<Fr>` — direct circuit inputs.
- `private_transcript: Vec<Fr>` — the witness values, consumed by `PrivateInput`.
- `public_transcript_inputs` / `public_transcript_outputs: Vec<Fr>` — the ledger statement calls and their results, consumed by `PublicInput`.

So ZKIR **consumes** a pre-built preimage. It does not build the transcript, invoke witnesses, or read contract state. Those happen in the off-chain interaction layer (see [circuit-body-ir.md](circuit-body-ir.md)) before ZKIR runs. `ir_vm.rs` is the off-chain circuit synthesis (`*_incircuit` builds the Plonk constraints, `*_offcircuit` evaluates witness values), used for key generation and proving.

## On-chain footprint

A contract stores, per entry point, a `ContractOperation { v2: Option<VerifierKey> }` (`onchain-state/src/state.rs`). On-chain transaction validation verifies the proof against that verifier key. ZKIR instructions are not stored or executed on-chain. There are plans to put ZKIR on-chain so it can be interpreted off-chain (for contract upgrades and cross-contract calls), but that is not the case today.

## Depends on / produces

- **Depends on:** the Compact pipeline (`Lzkir`). See [compact-pipeline.md](compact-pipeline.md).
- **Produces:** a Plonk `ProverKey` (off-chain) and `VerifierKey` (on-chain). Consumed by the prover together with a `ProofPreimage` built by the interaction layer.
