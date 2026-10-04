# Intermediate Representations in the Midnight / Compact stack

This directory documents the distinct IRs a Compact contract passes through, from source to on-chain execution, and how they depend on each other. One document per IR:

- [compact-pipeline.md](compact-pipeline.md): the Compact compiler's nanopass IR chain (source to lowered circuit).
- [zkir.md](zkir.md): ZKIR, the off-chain proving IR (compiled to prover/verifier keys).
- [impact-onchain-vm.md](impact-onchain-vm.md): the Impact VM, the on-chain transcript instruction set.
- [circuit-body-ir.md](circuit-body-ir.md): the analyzed IR in `compiler/analyzed-ir.sexp`, which an SDK interprets off-chain.

## The IRs at a glance

| IR | Where it lives | On/off chain | Purpose |
|---|---|---|---|
| Compact pipeline (`Lparser`..`Lflattened`) | Compact compiler (Scheme) | off-chain, compile time | Lower Compact source to a circuit and emit the downstream artifacts |
| ZKIR (`IrSource`) | `midnight-ledger/zkir-v3` | off-chain (verifier key on-chain) | Prove the circuit. Compiled to prover/verifier keys |
| Impact VM (`Op`) | `midnight-ledger/onchain-vm` | on-chain | Execute a call's public transcript to apply and validate state |
| Analyzed IR (`Lloweredemit`) | `compiler/analyzed-ir.sexp`, written behind `--analyzed-ir` (fork) | off-chain | Interpret a circuit off-chain to build the transcript and proof preimage |

The same artifact carries the typed schema that an SDK needs: the circuit and witness signatures, and the ledger field layout. [circuit-body-ir.md](circuit-body-ir.md) describes it.

## Dependency order

Compile time (Compact compiler):

```
Compact source
  │  parser + frontend + analysis passes
  ▼
Lloweredemit  (analyzed IR: fully typed, disclose-checked)
  ├──► save-analyzed-ir   ──► compiler/analyzed-ir.sexp   (with --analyzed-ir; the IR that midnight-rs interprets)
  ├──► typescript passes  ──► Ltypescript ──► generated TS Contract   (off-chain interaction layer)
  └──► circuit lowering   ──► Lnovectorref   (enums/map/fold/slices lowered; still expression-structured)
            │
            ▼  flatten
        Lcircuit ──► Lflattened ──► Lzkir ──► ZKIR (IrSource)
                                              │  keygen (off-chain)
                                              ├──► ProverKey    (off-chain)
                                              └──► VerifierKey  (stored on-chain)
```

The diagram shows only the outputs that this stack reads. [compact-pipeline.md](compact-pipeline.md) shows more of the compiler's outputs, such as `contract-manifest.json`.

Runtime (calling a circuit):

```
1. Off-chain interaction layer  (generated TS Contract, OR midnight-rs interpreting the analyzed IR)
     runs the circuit against live contract state and host witnesses
       ──► public transcript (an Impact `Op` program)  +  ProofPreimage (inputs, private/public transcripts)
2. Off-chain prover:   ProverKey + ZKIR + ProofPreimage  ──► Proof
3. On-chain:  the transaction carries the transcript + Proof
       ──► Impact VM executes the transcript `Op` program to apply and validate state
       ──► Proof verified against the on-chain VerifierKey
```

The load-bearing point: ZKIR does not build the transcript or call witnesses. It proves a preimage that the interaction layer already produced. The transcript-building and witness-invocation logic comes from the circuit body (the TS Contract, or the analyzed IR for non-JS SDKs). See [zkir.md](zkir.md) and [circuit-body-ir.md](circuit-body-ir.md).
