# Intermediate Representations in the Midnight / Compact stack

This directory documents the distinct IRs a Compact contract passes through, from source to on-chain execution, and how they depend on each other. One document per IR:

- [compact-pipeline.md](compact-pipeline.md) — the Compact compiler's nanopass IR chain (source to lowered circuit).
- [zkir.md](zkir.md) — ZKIR, the off-chain proving IR (compiled to prover/verifier keys).
- [impact-onchain-vm.md](impact-onchain-vm.md) — the Impact VM, the on-chain transcript instruction set.
- [circuit-body-ir.md](circuit-body-ir.md) — the off-chain Compact circuit body IR an SDK interprets (the `ir` in `contract-info.json`).

## The IRs at a glance

| IR | Where it lives | On/off chain | Purpose |
|---|---|---|---|
| Compact pipeline (`Lparser`..`Lflattened`) | Compact compiler (Scheme) | off-chain, compile time | Lower Compact source to a circuit and emit the downstream artifacts |
| ZKIR (`IrSource`) | `midnight-ledger/zkir-v3` | off-chain (verifier key on-chain) | Prove the circuit. Compiled to prover/verifier keys |
| Impact VM (`Op`) | `midnight-ledger/onchain-vm` | on-chain | Execute a call's public transcript to apply and validate state |
| Circuit body IR (`ir`) | `contract-info.json` (fork) | off-chain | Interpret a circuit off-chain to build the transcript and proof preimage |

The typed schema in `contract-info.json` (circuit/witness signatures and ledger field layout) is not an IR but is needed alongside the circuit body IR. It is described in [circuit-body-ir.md](circuit-body-ir.md).

## Dependency order

Compile time (Compact compiler):

```
Compact source
  │  parser + frontend + analysis passes
  ▼
Lnodisclose  (analyzed IR: fully typed, disclose-checked)
  ├──► save-contract-info ──► contract-info.json   (typed schema; the fork also emits the circuit body IR)
  ├──► typescript passes  ──► Ltypescript ──► generated TS Contract   (off-chain interaction layer)
  └──► circuit lowering    ──► Lnovectorref   (enums/map/fold/slices lowered; still expression-structured)
            │  (the fork serializes THIS as the circuit body `ir`)
            ▼  flatten
        Lcircuit ──► Lflattened ──► Lzkir ──► ZKIR (IrSource)
                                              │  keygen (off-chain)
                                              ├──► ProverKey    (off-chain)
                                              └──► VerifierKey  (stored on-chain)
```

Runtime (calling a circuit):

```
1. Off-chain interaction layer  (generated TS Contract, OR midnight-rs interpreting the circuit body IR)
     runs the circuit against live contract state and host witnesses
       ──► public transcript (an Impact `Op` program)  +  ProofPreimage (inputs, private/public transcripts)
2. Off-chain prover:   ProverKey + ZKIR + ProofPreimage  ──► Proof
3. On-chain:  the transaction carries the transcript + Proof
       ──► Impact VM executes the transcript `Op` program to apply and validate state
       ──► Proof verified against the on-chain VerifierKey
```

The load-bearing point: ZKIR does not build the transcript or call witnesses. It proves a preimage that the interaction layer already produced. The transcript-building and witness-invocation logic comes from the circuit body (the TS Contract today, or the circuit body IR for non-JS SDKs). See [zkir.md](zkir.md) and [circuit-body-ir.md](circuit-body-ir.md).
