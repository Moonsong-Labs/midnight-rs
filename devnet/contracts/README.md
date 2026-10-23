# Devnet example contracts

The Compact contracts that the examples and the devnet tests deploy to the local devnet. Each directory holds the contract source. `make compile-contracts` writes the compiler output to a `compiled/` directory beside the source, and git ignores it.

This directory holds only contract assets. It is not a Rust crate. It sits outside `examples/`, so the `examples/*` glob of the workspace does not see it as a crate.

| Contract | Used by |
| --- | --- |
| [`call-context`](call-context) | the `call_context` devnet test of `midnight-contract`, and the `Simulator` tests of `tests/integration`: circuits that check the block time and the contract balance |
| [`counter`](counter) | [`example-counter`](../../examples/counter), [`example-contract-maintenance`](../../examples/contract-maintenance), [`example-combine-and-sponsor`](../../examples/combine-and-sponsor), and the counter tests of `midnight-contract` and `midnight-typed-state` |
| [`counter-zkir-v3`](counter-zkir-v3) | the `zkir_v3_local_proving` devnet test of `midnight-contract`: the `counter` source, compiled for ZKIR v3, which only a ledger 9 chain verifies |
| [`events`](events) | the `emit_event` devnet test of `midnight-contract`: a circuit that emits a `Misc` event between two ledger operations |
| [`receive-shielded`](receive-shielded) | the `receive_shielded_coin` devnet test of `midnight-contract`: a `deposit` circuit that receives a shielded coin from the caller |
| [`secret-counter`](secret-counter) | [`example-private-state`](../../examples/private-state), and the `Simulator` tests of `tests/integration`: a stateful `witness next_secret()` that keeps its value in per-contract private state |
| [`shielded-mint`](shielded-mint) | [`example-shielded-mint`](../../examples/shielded-mint), [`example-shielded-swap`](../../examples/shielded-swap), and the `mint_external_recipient`, `recover_unencrypted_mint` and `receive_shielded_coin` devnet tests of `midnight-contract` |
| [`unshielded-payout`](unshielded-payout) | the `unshielded_payout_to_user` devnet test of `midnight-contract`: a contract that mints an unshielded token to itself and pays it to a user |

## Layout

Each `compiled/` directory keeps the layout that compactc writes:

```text
compiled/
├── compiler/analyzed-ir.sexp   the artifact that the contract! macro reads
├── keys/                       the prover and verifier keys of each circuit
├── zkir/                       the circuit IR that the prover reads
└── source.cksum                the checksum of the source and COMPACT_REV that make contracts compares
```

The SDK reads no other compiler output, so `make compile-contracts` drops the rest, for example the TypeScript `contract/` directory. It writes `source.cksum` last, so the file exists only beside a whole output.

## Use a contract from a crate

Give the artifact path to `contract!`. The macro resolves it against the `Cargo.toml` directory of the crate. It also emits `ZK_CONFIG_DIR`, the absolute path of `compiled/` on the build machine, so a binary that runs on another machine passes its own directory. Pass the const to `with_zk_config`:

```rust
mod counter {
    midnight_core::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

let contract = counter::Contract::deploy(&provider)
    .with_initial_state(counter::LedgerInitialState::default())
    .with_zk_config(counter::ZK_CONFIG_DIR)
    .await?;
```

## Compile

Only a fork of the Compact compiler writes `compiler/analyzed-ir.sexp`, when it runs with `--analyzed-ir`. The root [`Makefile`](../../Makefile) runs that compiler from its image, `ghcr.io/romarq/compactc:<COMPACT_REV>`, so a compile needs Docker. `COMPACT_REV` in the `Makefile` names the commit of the fork.

The `make` targets that build or run a crate that uses these contracts, such as `make check`, `make clippy` and `make test`, first run `make contracts`. It compiles each contract whose output is missing, or whose `compiled/source.cksum` does not match its source and `COMPACT_REV`. So an edit, a pull or a branch switch that changes a contract takes effect on the next run. A plain `cargo` command compiles no contract, so run `make contracts` before it. CI compiles every contract from source in one job and gives the output to the other jobs.

To recompile every contract, run this from the root of the repository:

```bash
make compile-contracts  # recompile each contract here into its compiled/ directory
```

To add a contract, add its directory here and its name to `CONTRACTS` in the `Makefile`.

To compile a contract outside this repository, follow [Compile a contract](../../README.md#compile-a-contract). To use your own build of the fork, set `COMPACTC=<path>`.
