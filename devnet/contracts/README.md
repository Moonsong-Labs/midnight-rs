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

Only a fork of the Compact compiler writes `compiler/analyzed-ir.sexp`, when it runs with `--analyzed-ir`. The [`tools/compact-compiler`](../../tools/compact-compiler) submodule pins that fork. Run the commands below from the root of the repository.

The first way runs the compiler image of the pin, which needs Docker. [`tools/compactc-docker`](../../tools/compactc-docker) runs compactc in the image:

```bash
make compile-contracts COMPACTC=tools/compactc-docker  # recompile each contract here in the image
```

The image is `ghcr.io/romarq/compactc:<pin>`, where `<pin>` is the commit of the submodule. At each push to the [`midnight-rs`](https://github.com/RomarQ/compact/tree/midnight-rs) branch of `RomarQ/compact`, a workflow publishes the image of the new branch head. The image exists only after that workflow run ends. A pin on any other commit of the fork, such as a commit on another branch, has no image. For that pin, use the Nix build. To run a different image, set `COMPACTC_IMAGE`.

The second way builds the compiler with Nix:

```bash
make build-compactc     # init the submodule and build the compiler with Nix
make compile-contracts  # recompile each contract here into its compiled/ directory
```

`make build-compactc` force-checks out the submodule pin, so it discards local edits in `tools/compact-compiler`. To use a different compactc build, set `COMPACTC=<path>`. It must be a build of the same fork: the `Makefile` refuses a compactc that does not take `--analyzed-ir`.
