# Testing contracts

This page explains what the local run of a contract call checks, and gives the rules for devnet tests. Each rule cites the code line or the commit that sets it.

## Local runs

Every contract call first reads the contract state from the node ([`contract.rs:1188`](../crates/midnight-contract/src/contract.rs#L1188)). Then it runs its circuit in the interpreter on that state ([`call.rs:175`](../crates/midnight-contract/src/call.rs#L175)). This page calls that interpreter step the local run. So a local run needs a node that holds the deployed contract. The wallet resync ([`call.rs:187`](../crates/midnight-contract/src/call.rs#L187)), the Dust check and the proof come after the local run. The `.await`, `.send()`, `.build()` and `.without_dust()` of a call builder all start this way ([`contract.rs:1131`](../crates/midnight-contract/src/contract.rs#L1131), [`contract.rs:1389`](../crates/midnight-contract/src/contract.rs#L1389)).

So a failed `assert` costs no proof and no fee, and the SDK submits nothing. The call returns `ContractError::Interpreter` ([`error.rs:43`](../crates/midnight-contract/src/error.rs#L43)). It holds `InterpreterError::AssertionFailed` with the message of the `assert` ([`lib.rs:586`](../crates/compact/interpreter/src/lib.rs#L586)).

### Limits of a local run

The interpreter builds the context of each ledger operation with `QueryContext::new` ([`lib.rs:1743`](../crates/compact/interpreter/src/lib.rs#L1743)), which fills a default `CallContext`. So every local run uses these values, whether the SDK then submits the call or only builds it:

- The block time is 0, the Unix epoch. `blockTimeGt(t)` is false for every `t`, and `blockTimeLt(t)` is true for every `t` above 0.
- The contract balance is empty. `unshieldedBalance(color)` returns 0, and `unshieldedBalanceGt(color, amount)` is false for every amount.

The chain replays the ledger operations of the circuit with the real block time and the real balance. It checks each read against the value that the local run recorded ([`call.rs:245`](../crates/midnight-contract/src/call.rs#L245)). So a clock or balance check that passes locally can fail on chain. A check that fails locally stops a call that the chain can apply.

## Devnet rules

The devnet tests share one funded wallet, the dev seed. A wallet reads the chain through the indexer, which serves only finalized blocks, two or three behind the best block of the node ([`Makefile:211-216`](../Makefile#L211-L216)). Until the block of a spend is final, the other wallets on the dev seed cannot see that spend. A wallet that cannot see a spend draws the same Dust again. The node then rejects the transaction with custom error 196, `DustDoubleSpend` ([`node_error.rs:77`](../crates/midnight-provider/src/node_error.rs#L77)). The rules below stop a test from drawing Dust that another test spent.

### One submitting test per binary

A submitting test sends a transaction to the node. Put at most one submitting test on the dev seed in each test binary ([`f191c6bb`](https://github.com/Moonsong-Labs/midnight-rs/commit/f191c6bb)).

Two submitting tests in one binary fail whether they run in parallel or one at a time ([`f191c6bb`](https://github.com/Moonsong-Labs/midnight-rs/commit/f191c6bb)):

- In parallel, which is the default, the node refuses one of them with `Extrinsic marked as invalid`.
- With `--test-threads=1`, the new wallet of the second test can sync before the indexer shows the spend of the first test. It draws the same Dust, and the node rejects it with error 196.

A provider in a `static` does not help. Each `#[tokio::test]` owns its runtime, and the first runtime drops the node connection when it shuts down ([`e2e_contracts.rs:655-661`](../crates/midnight-contract/tests/e2e_contracts.rs#L655-L661)).

To test several transactions, send them from one test on one provider. A wallet skips the inputs that its own earlier builds reserved (see [Pending reservations](wallet.md#pending-reservations)). `balancing_a_bare_contract_call_is_accepted_on_chain` deploys a contract and then submits a call on one provider in this way, and ends on finality ([`balance_bare_call.rs:31-71`](../crates/midnight-contract/tests/balance_bare_call.rs#L31-L71)).

### `--test-threads=1` beside a submitting test

A binary that holds its submitting test beside other devnet tests runs all of them one at a time. `make test-e2e` passes `--test-threads=1` to the wallet integration tests and to `e2e_contracts` ([`Makefile:291`](../Makefile#L291), [`Makefile:308`](../Makefile#L308)). A binary whose devnet tests only read or build runs them in parallel. The flag does not make a second submitting test safe.

### End each submitting test on finality

End each submitting test after its last transaction is final ([`0e07bec9`](https://github.com/Moonsong-Labs/midnight-rs/commit/0e07bec9), on main as [`23a75b61`](https://github.com/Moonsong-Labs/midnight-rs/commit/23a75b61), [`balance_bare_call.rs:68-71`](../crates/midnight-contract/tests/balance_bare_call.rs#L68-L71)). `make test-e2e` runs its binaries one after another and runs no `make dev-settle` between them ([`Makefile:289-308`](../Makefile#L289-L308)). Each binary is a new process on the dev seed, and its wallet must see the spends of the binary before it.

Call `wait_finalized()` on the pending handle, or end on the `.await` of a call, which waits for finality (see [What `.await` waits for](#what-await-waits-for)). `wait_best()` returns before finality. To end on a deploy, call `.send()`, then `wait_finalized()`, then `into_contract()`.

Finality does not make a spend visible at once. A wallet sees the zswap events of a transaction about 1.1 s after `wait_finalized` returns ([`provider.rs:32-34`](../crates/midnight-provider/src/provider.rs#L32-L34)). To read the wallet of the test after a transaction, wait with `wait_observed` ([`provider.rs:436`](../crates/midnight-provider/src/provider.rs#L436)). For an output on a leg that the transaction spends nothing from, wait with `resync_until` ([`provider.rs:492`](../crates/midnight-provider/src/provider.rs#L492)).

### `make dev-settle` between two processes on one seed

Before a process spends from a seed that another process spent from, run `make dev-settle` ([`Makefile:211-233`](../Makefile#L211-L233)). It reads the number of the best block from the node. Then it polls the indexer until the latest block of the indexer reaches that number. CI runs each example as `make dev-settle run-<example>` ([`ci.yml:166-188`](../.github/workflows/ci.yml#L166-L188)), and `make examples` runs it before each example ([`Makefile:330-335`](../Makefile#L330-L335)).

## What `.await` waits for

- The `.await` of a call builder proves and submits the call, then waits for finality ([`contract.rs:1274`](../crates/midnight-contract/src/contract.rs#L1274)). A 60 s bound ([`contract.rs:210`](../crates/midnight-contract/src/contract.rs#L210)) ends the wait with `ContractError::FinalizeTimeout`. A call that the chain did not apply fails with `ContractError::TransactionFailed` ([`contract.rs:168`](../crates/midnight-contract/src/contract.rs#L168)).
- The `.send()` of a call builder returns a `PendingCall` after the submit. Its `wait_finalized()` waits for finality with no deadline ([`contract.rs:258`](../crates/midnight-contract/src/contract.rs#L258)).
- The `.await` of a deploy is `send()` and then `into_contract()` ([`contract.rs:594-595`](../crates/midnight-contract/src/contract.rs#L594-L595)). `into_contract` waits for the best block, and fails with `ContractError::TransactionFailed` when the chain did not apply the deploy ([`contract.rs:729-730`](../crates/midnight-contract/src/contract.rs#L729-L730)). Then it reads the contract from the node at the block that applied the deploy ([`deploy.rs:101`](../crates/midnight-contract/src/deploy.rs#L101)). One deadline bounds the wait and the read: 60 s by default ([`contract.rs:416`](../crates/midnight-contract/src/contract.rs#L416)), set with `with_deploy_timeout` ([`contract.rs:473`](../crates/midnight-contract/src/contract.rs#L473)). When it passes, the deploy fails with `ContractError::DeployTimeout`.
- The `.await` of a deploy never calls `wait_finalized`. `.send()` returns the `PendingDeploy`, and its `wait_finalized()` waits for finality with no deadline ([`contract.rs:470-472`](../crates/midnight-contract/src/contract.rs#L470-L472)). That wait fails with `ContractError::TransactionFailed` when the chain did not apply the deploy ([`contract.rs:674-676`](../crates/midnight-contract/src/contract.rs#L674-L676)). A later `into_contract()` then reads the contract from the node ([`contract.rs:742`](../crates/midnight-contract/src/contract.rs#L742)).
- The `.await` of a transfer, a Dust registration or a maintenance update returns a `PendingTx` after the submit ([`transfer.rs:106`](../crates/midnight-provider/src/transfer.rs#L106), [`transfer.rs:353`](../crates/midnight-provider/src/transfer.rs#L353), [`maintenance.rs:497`](../crates/midnight-contract/src/maintenance.rs#L497)). Its `wait_best()` and `wait_finalized()` return `Ok` only when the chain applied the transaction, and fail with `ProviderError::NotApplied` otherwise (see [Observing inclusion explicitly](../README.md#observing-inclusion-explicitly)).

## Gate pattern

Skip a devnet test only when `MIDNIGHT_NODE_URL` or `MIDNIGHT_INDEXER_URL` is absent, so `make test` passes on a machine with no devnet. `make test-e2e` sets `MIDNIGHT_E2E` ([`Makefile:285`](../Makefile#L285)). The test binaries of `midnight-provider` and `midnight-contract` share this guard through `tests/common/mod.rs`. `common::devnet_urls` returns the two URLs, or skips the test when one is missing. When `MIDNIGHT_E2E` is set, it panics in place of the skip ([`common/mod.rs:11-36`](../crates/midnight-provider/tests/common/mod.rs#L11-L36)). A devnet test starts with it, as `wallet_effect.rs` does ([`wallet_effect.rs:21-23`](../crates/midnight-provider/tests/wallet_effect.rs#L21-L23)):

```rust
let Some((node_url, indexer_url)) = common::devnet_urls() else {
    return;
};
```

Make the test create the other state that it needs, such as a funded wallet or an unused seed. Make any other missing precondition panic when `MIDNIGHT_E2E` is set ([test-audit skill](../.agents/skills/test-audit/SKILL.md#devnet-tests)). A skipped test still reports `ok`. So in this repository, a devnet test proves something only when a line of `make test-e2e` or `make test-e2e-node-restart` runs it ([`Makefile:289-314`](../Makefile#L289-L314)).

## Devnet limits

- **The clock.** The node takes the block time from `pallet_timestamp`, which follows the wall clock, in whole seconds ([midnight-node `pallets/midnight/src/lib.rs:86-101`](https://github.com/midnightntwrk/midnight-node/blob/toolkit-2.1.0-rc.3/pallets/midnight/src/lib.rs#L86-L101)). The node serves no RPC that sets it ([`node_error.rs:10-12`](../crates/midnight-provider/src/node_error.rs#L10-L12)). So a test of a deadline waits for the real time to pass.
- **The dev seed.** Every devnet test gets its funds from one wallet, the dev seed, directly or through a transfer from it ([`Makefile:58`](../Makefile#L58)). Genesis gives it tNIGHT, shielded tokens and Dust ([`common/mod.rs:6-9`](../crates/midnight-provider/tests/common/mod.rs#L6-L9)). A new seed holds nothing. To fund one, send tNIGHT to it from the dev seed ([`dust_registration_submit.rs:59-67`](../crates/midnight-provider/tests/dust_registration_submit.rs#L59-L67)). A wallet sees a finalized transfer only after a short delay. So wait until the wallet of the new seed sees the tNIGHT, as [`dust_registration_submit.rs:71-79`](../crates/midnight-provider/tests/dust_registration_submit.rs#L71-L79) does. `resync_until(timeout, |b| b.dust.unregistered_night_utxos > 0)` does this wait ([`provider.rs:492`](../crates/midnight-provider/src/provider.rs#L492)). Then call `register_all_night`, which registers that tNIGHT for Dust and waits until the wallet can spend Dust ([`provider.rs:803`](../crates/midnight-provider/src/provider.rs#L803)).
- **Finalized reads.** The indexer serves only finalized blocks ([`Makefile:211-213`](../Makefile#L211-L213)). A wallet sync reads the indexer, so it trails the best block of the node.
- **One chain until `make dev-down`.** The devnet keeps its chain between test runs. A registration or a deployed contract from an earlier run stays on it. A test that needs a seed with no history makes one, as `unused_seed` does ([`dust_registration_submit.rs:22-30`](../crates/midnight-provider/tests/dust_registration_submit.rs#L22-L30)). `make dev-down` removes the containers, and the chain goes with them ([`Makefile:235-236`](../Makefile#L235-L236)). The compose files declare no volume, so the next `make dev-up` starts from genesis.
