# Devnet example contracts

The Compact contracts that the examples and the devnet tests deploy to the local devnet. Each directory holds the contract source and a `compiled/` directory with the compiler output.

This directory holds only contract assets. It is not a Rust crate. It sits outside `examples/`, so the `examples/*` glob of the workspace does not see it as a crate.

| Contract | Used by |
| --- | --- |
| [`counter`](counter) | [`example-counter`](../../examples/counter), [`example-contract-maintenance`](../../examples/contract-maintenance), [`example-combine-and-sponsor`](../../examples/combine-and-sponsor), and the counter tests of `midnight-contract` and `midnight-typed-state` |
| [`secret-counter`](secret-counter) | [`example-private-state`](../../examples/private-state): a stateful `witness next_secret()` that keeps its value in per-contract private state |
| [`shielded-mint`](shielded-mint) | [`example-shielded-mint`](../../examples/shielded-mint), [`example-shielded-swap`](../../examples/shielded-swap), and the `mint_external_recipient` and `recover_unencrypted_mint` devnet tests of `midnight-contract` |
| [`unshielded-payout`](unshielded-payout) | the `unshielded_payout_to_user` devnet test of `midnight-contract`. Its `compiled/` directory is not committed: `make compile-contracts` writes it, and the test skips when it is absent. |

## Layout

Each `compiled/` directory keeps the layout that compactc writes:

```text
compiled/
├── compiler/analyzed-ir.sexp   the artifact that the contract! macro reads
├── keys/                       the prover and verifier keys of each circuit
└── zkir/                       the circuit IR that the prover reads
```

The SDK reads no other compiler output, so `make compile-contracts` drops the rest, for example the TypeScript `contract/` directory.

## Use a contract from a crate

Give the artifact path to `contract!`, and the `compiled/` directory to `with_zk_config`. The macro resolves its path against the `Cargo.toml` directory of the crate. `with_zk_config` resolves a relative path against the working directory of the process, so the examples build the path from `CARGO_MANIFEST_DIR`:

```rust
mod counter {
    midnight_core::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

let contract = counter::Contract::deploy(&provider)
    .with_initial_state(counter::LedgerInitialState::default())
    .with_zk_config(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../devnet/contracts/counter/compiled"
    ))
    .await?;
```

## Recompile

Only a fork of the Compact compiler writes `compiler/analyzed-ir.sexp`, when it runs with `--analyzed-ir`. The root [`Makefile`](../../Makefile) runs that compiler from its image, `ghcr.io/romarq/compactc:<COMPACT_REV>`, so a recompile needs Docker. `COMPACT_REV` in the `Makefile` names the commit of the fork. From the root of the repository:

```bash
make compile-contracts  # recompile each contract here into its compiled/ directory
```

To compile a contract outside this repository, follow [Compile a contract](../../README.md#compile-a-contract). To use your own build of the fork, set `COMPACTC=<path>`.
