# Testing contracts

This page explains what the local run of a contract call checks, and gives the rules for devnet tests. Each rule cites the code line or the commit that sets it.

## Local runs

Every contract call first reads the contract state and the block time from the node, at one block ([`contract.rs:1170`](../crates/midnight-contract/src/contract.rs#L1170), [`contract.rs:1383`](../crates/midnight-contract/src/contract.rs#L1383)). Then it runs its circuit in the interpreter on that state ([`call.rs:148`](../crates/midnight-contract/src/call.rs#L148)). This page calls that interpreter step the local run. So a local run needs a node that holds the deployed contract. The wallet resync ([`call.rs:195`](../crates/midnight-contract/src/call.rs#L195)), the Dust check and the proof come after the local run. The `.await`, `.send()`, `.build()` and `.without_dust()` of a call builder all start this way ([`contract.rs:1186`](../crates/midnight-contract/src/contract.rs#L1186), [`contract.rs:1404`](../crates/midnight-contract/src/contract.rs#L1404)).

So a failed `assert` costs no proof and no fee, and the SDK submits nothing. The call returns `ContractError::Interpreter` ([`error.rs:40`](../crates/midnight-contract/src/error.rs#L40)). It holds `InterpreterError::AssertionFailed` with the message of the `assert` ([`lib.rs:575`](../crates/compact/interpreter/src/lib.rs#L575)).

### Limits of a local run

A call reads the contract state and the block time at one block: the best block of the node, or the block of the `at_block` pin ([`state.rs:93-110`](../crates/midnight-contract/src/state.rs#L93-L110)). The local run uses the time of that block, in whole seconds, as its block time ([`call.rs:157`](../crates/midnight-contract/src/call.rs#L157)). It reads the contract balance from that state ([`lib.rs:202-207`](../crates/compact/interpreter/src/lib.rs#L202-L207)). The SDK then partitions the transcripts of the call at the same time and balance ([`per_ledger/call.rs:84-93`](../crates/midnight-contract/src/per_ledger/call.rs#L84-L93)). So a clock or balance check in a local run gives the result that the chain gives at that block.

The chain replays the ledger operations of the circuit at the time and the balance of the block that includes the transaction. It checks each read against the value that the local run recorded ([`call.rs:259`](../crates/midnight-contract/src/call.rs#L259)). That block comes after the block that the local run read. So a check that passes locally can fail on chain:

- **A deadline.** `blockTimeLt(deadline)` passes locally and fails on chain when the transaction lands in a block at or after the deadline. Do not call close to a deadline.
- **A balance.** A balance check reads the balance at the block of the local run. A transaction that changes the balance before the call lands can make the check fail on chain.
- **A pinned handle.** A handle with an `at_block` pin runs at the time and the balance of the pinned block, which can be far behind the chain.

A check that fails locally stops a call that the chain can apply. The SDK submits nothing:

- **A start time.** `blockTimeGt(start)` fails locally while the time of the best block is at or before `start`, even when the transaction would land in a later block. Call again when the time of the best block is after `start`.
- **A deposit.** `unshieldedBalanceGt(color, amount)` fails locally until the deposit that raises the balance is in the best block, even when the deposit would land before the call.

## Devnet rules

The devnet tests share one funded wallet, the dev seed. A wallet reads the chain through the indexer, which serves only finalized blocks, two or three behind the best block of the node ([`Makefile:180-185`](../Makefile#L180-L185)). Until the block of a spend is final, the other wallets on the dev seed cannot see that spend. A wallet that cannot see a spend draws the same Dust again. The node then rejects the transaction with custom error 196, `DustDoubleSpend` ([`node_error.rs:83`](../crates/midnight-provider/src/node_error.rs#L83)). The rules below stop a test from drawing Dust that another test spent.

### One submitting test per binary

A submitting test sends a transaction to the node. Put at most one submitting test on the dev seed in each test binary ([`f191c6bb`](https://github.com/Moonsong-Labs/midnight-rs/commit/f191c6bb)).

Two submitting tests in one binary fail whether they run in parallel or one at a time ([`f191c6bb`](https://github.com/Moonsong-Labs/midnight-rs/commit/f191c6bb)):

- In parallel, which is the default, the node refuses one of them with `Extrinsic marked as invalid`.
- With `--test-threads=1`, the new wallet of the second test can sync before the indexer shows the spend of the first test. It draws the same Dust, and the node rejects it with error 196.

A provider in a `static` does not help. Each `#[tokio::test]` owns its runtime, and the first runtime drops the node connection when it shuts down ([`e2e_contracts.rs:963-969`](../crates/midnight-contract/tests/e2e_contracts.rs#L963-L969)).

To test several transactions, send them from one test on one provider. A wallet skips the inputs that its own earlier builds reserved (see [Pending reservations](wallet.md#pending-reservations)). `balancing_a_bare_contract_call_is_accepted_on_chain` deploys a contract and then submits a call on one provider in this way, and ends on finality ([`balance_bare_call.rs:46-90`](../crates/midnight-contract/tests/balance_bare_call.rs#L46-L90)).

### `--test-threads=1` beside a submitting test

A binary that holds its submitting test beside other devnet tests runs all of them one at a time. `make test-e2e` passes `--test-threads=1` to the wallet integration tests and to `e2e_contracts` ([`Makefile:260`](../Makefile#L260), [`Makefile:274`](../Makefile#L274)). A binary whose devnet tests only read or build runs them in parallel. The flag does not make a second submitting test safe.

### End each submitting test on finality

End each submitting test after its last transaction is final ([`0e07bec9`](https://github.com/Moonsong-Labs/midnight-rs/commit/0e07bec9), on main as [`23a75b61`](https://github.com/Moonsong-Labs/midnight-rs/commit/23a75b61), [`balance_bare_call.rs:87-90`](../crates/midnight-contract/tests/balance_bare_call.rs#L87-L90)). `make test-e2e` runs its binaries one after another and runs no `make dev-settle` between them ([`Makefile:258-274`](../Makefile#L258-L274)). Each binary is a new process on the dev seed, and its wallet must see the spends of the binary before it.

Call `wait_finalized()` on the pending handle, or end on the `.await` of a call, which waits for finality (see [What `.await` waits for](#what-await-waits-for)). `wait_best()` returns before finality. To end on a deploy, call `.send()`, then `wait_finalized()`, then `into_contract()`.

Finality does not make a spend visible at once. A wallet sees the zswap events of a transaction about 1.1 s after `wait_finalized` returns ([`provider.rs:36-38`](../crates/midnight-provider/src/provider.rs#L36-L38)). To read the wallet of the test after a transaction, wait with `wait_observed` ([`provider.rs:436`](../crates/midnight-provider/src/provider.rs#L436)). For an output on a leg that the transaction spends nothing from, wait with `resync_until` ([`provider.rs:492`](../crates/midnight-provider/src/provider.rs#L492)).

### `make dev-settle` between two processes on one seed

Before a process spends from a seed that another process spent from, run `make dev-settle` ([`Makefile:180-202`](../Makefile#L180-L202)). It reads the number of the best block from the node. Then it polls the indexer until the latest block of the indexer reaches that number. CI runs each example as `make dev-settle run-<example>` ([`ci.yml:166-188`](../.github/workflows/ci.yml#L166-L188)), and `make examples` runs it before each example ([`Makefile:296-303`](../Makefile#L296-L303)).

## What `.await` waits for

- The `.await` of a call builder proves and submits the call, then waits for finality ([`contract.rs:1261-1266`](../crates/midnight-contract/src/contract.rs#L1261-L1266)). A 60 s bound ([`contract.rs:193`](../crates/midnight-contract/src/contract.rs#L193)) ends the wait with `ContractError::FinalizeTimeout`. A call that the chain did not apply fails with `ContractError::TransactionFailed` ([`contract.rs:146`](../crates/midnight-contract/src/contract.rs#L146)).
- The `.send()` of a call builder returns a `PendingCall` after the submit. Its `wait_finalized()` waits for finality with no deadline ([`contract.rs:243`](../crates/midnight-contract/src/contract.rs#L243)).
- The `.await` of a deploy is `send()` and then `into_contract()` ([`contract.rs:591-592`](../crates/midnight-contract/src/contract.rs#L591-L592)). `into_contract` waits for the best block, and fails with `ContractError::TransactionFailed` when the chain did not apply the deploy ([`contract.rs:724-725`](../crates/midnight-contract/src/contract.rs#L724-L725)). Then it polls the indexer until the indexer shows the contract ([`deploy.rs:101`](../crates/midnight-contract/src/deploy.rs#L101)). One deadline bounds the two waits: 60 s by default ([`contract.rs:404`](../crates/midnight-contract/src/contract.rs#L404)), set with `with_deploy_timeout` ([`contract.rs:462`](../crates/midnight-contract/src/contract.rs#L462)). When it passes, the deploy fails with `ContractError::DeployTimeout`.
- The `.await` of a deploy never calls `wait_finalized`. `.send()` returns the `PendingDeploy`, and its `wait_finalized()` waits for finality with no deadline ([`contract.rs:459-461`](../crates/midnight-contract/src/contract.rs#L459-L461)). That wait fails with `ContractError::TransactionFailed` when the chain did not apply the deploy ([`contract.rs:672-674`](../crates/midnight-contract/src/contract.rs#L672-L674)). A later `into_contract()` then polls the indexer ([`contract.rs:721`](../crates/midnight-contract/src/contract.rs#L721)).
- The `.await` of a transfer, a Dust registration or a maintenance update returns a `PendingTx` after the submit ([`transfer.rs:107`](../crates/midnight-provider/src/transfer.rs#L107), [`transfer.rs:381`](../crates/midnight-provider/src/transfer.rs#L381), [`maintenance.rs:509`](../crates/midnight-contract/src/maintenance.rs#L509)). Its `wait_best()` and `wait_finalized()` return `Ok` only when the chain applied the transaction, and fail with `ProviderError::NotApplied` otherwise (see [Observing inclusion explicitly](../README.md#observing-inclusion-explicitly)).

## Gate pattern

Skip a devnet test only when `MIDNIGHT_NODE_URL` or `MIDNIGHT_INDEXER_URL` is absent, so `make test` passes on a machine with no devnet. `make test-e2e` sets `MIDNIGHT_E2E` ([`Makefile:254`](../Makefile#L254)). In `wallet_effect.rs`, a missing URL also panics when `MIDNIGHT_E2E` is set. This is its guard ([`wallet_effect.rs:22-31`](../crates/midnight-provider/tests/wallet_effect.rs#L22-L31)):

```rust
let (Ok(node_url), Ok(indexer_url)) = (
    std::env::var("MIDNIGHT_NODE_URL"),
    std::env::var("MIDNIGHT_INDEXER_URL"),
) else {
    if std::env::var_os("MIDNIGHT_E2E").is_some() {
        panic!("MIDNIGHT_NODE_URL or MIDNIGHT_INDEXER_URL is missing under make test-e2e");
    }
    eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
    return;
};
```

Make the test create the other state that it needs, such as a funded wallet or an unused seed. Make any other missing precondition panic when `MIDNIGHT_E2E` is set ([test-audit skill](../.agents/skills/test-audit/SKILL.md#devnet-tests)). A skipped test still reports `ok`. So in this repository, a devnet test proves something only when a line of `make test-e2e` or `make test-e2e-node-restart` runs it ([`Makefile:258-280`](../Makefile#L258-L280)).

## Devnet limits

- **The clock.** The node takes the block time from `pallet_timestamp`, which follows the wall clock, in whole seconds ([midnight-node `pallets/midnight/src/lib.rs:86-101`](https://github.com/midnightntwrk/midnight-node/blob/toolkit-2.1.0-rc.3/pallets/midnight/src/lib.rs#L86-L101)). The node serves no RPC that sets it ([`node_error.rs:10-12`](../crates/midnight-provider/src/node_error.rs#L10-L12)). So a test of a deadline waits for the real time to pass.
- **The dev seed.** Every devnet test gets its funds from one wallet, the dev seed, directly or through a transfer from it ([`Makefile:33`](../Makefile#L33)). Genesis gives it tNIGHT, shielded tokens and Dust ([`dust_registration_submit.rs:18`](../crates/midnight-provider/tests/dust_registration_submit.rs#L18), [`wallet_effect.rs:17`](../crates/midnight-provider/tests/wallet_effect.rs#L17)). A new seed holds nothing. To fund one, send tNIGHT to it from the dev seed ([`dust_registration_submit.rs:72-80`](../crates/midnight-provider/tests/dust_registration_submit.rs#L72-L80)). A wallet sees a finalized transfer only after a short delay. So wait until the wallet of the new seed sees the tNIGHT, as [`dust_registration_submit.rs:88-96`](../crates/midnight-provider/tests/dust_registration_submit.rs#L88-L96) does. `resync_until(timeout, |b| b.dust.unregistered_night_utxos > 0)` does this wait ([`provider.rs:492`](../crates/midnight-provider/src/provider.rs#L492)). Then call `register_all_night`, which registers that tNIGHT for Dust and waits until the wallet can spend Dust ([`provider.rs:805`](../crates/midnight-provider/src/provider.rs#L805)).
- **Finalized reads.** The indexer serves only finalized blocks ([`Makefile:180-182`](../Makefile#L180-L182)). A wallet sync and the deploy wait read the indexer, so they trail the best block of the node.
- **One chain until `make dev-down`.** The devnet keeps its chain between test runs. A registration or a deployed contract from an earlier run stays on it. A test that needs a seed with no history makes one, as `unused_seed` does ([`dust_registration_submit.rs:24-31`](../crates/midnight-provider/tests/dust_registration_submit.rs#L24-L31)). `make dev-down` removes the containers, and the chain goes with them ([`Makefile:204-205`](../Makefile#L204-L205)). The compose files declare no volume, so the next `make dev-up` starts from genesis.

## Stack size

A debug build of a devnet test can run out of stack. See [Stack size in a debug build](../README.md#stack-size-in-a-debug-build).
