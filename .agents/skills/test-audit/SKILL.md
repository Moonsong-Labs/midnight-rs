---
name: test-audit
description: Use when a task adds, changes, reviews, or deletes tests in midnight-rs. Examples are a request to add unit tests or raise coverage, a regression test for a bug fix, and a review of the tests in a diff. It also covers a sweep for low-value tests, devnet tests that skip, and `#[ignore]` tests that no target runs. It is not for a rename or a signature update that keeps what each test asserts.
---

# Test Audit

This skill has one value bar and three modes. Choose the mode by the result that the task asks for, not by the size of the scope. A mechanical edit that keeps what each test asserts, such as a rename or a signature update, needs no mode.

- **Authoring mode** gates every new or changed test, when you write it or review a diff that adds it. Use [Where tests run](#where-tests-run), the [authoring gate](#authoring-gate), and the [junk patterns](#junk-patterns). Then use [Validation](#validation).
- **Audit mode** returns a short list of candidates with evidence, for any number of crates. Optimize for confidence, not for the deletion count.
- **Campaign mode** removes every low-value test from one crate or crate group, in one PR. Read [CAMPAIGN.md](CAMPAIGN.md) before you start one.

## Candidates and outcomes

A **candidate** is an existing test that restates the source, repeats stronger proof, or couples to the implementation. A test that runs nowhere, or keeps a test-only seam alive, is also a candidate. An audit follows these steps:

1. Run the sweeps in [DISCOVERY.md](DISCOVERY.md).
2. Apply the [value bar](#value-bar), the junk patterns, and the [retention bar](#retention-bar) to each lead.
3. Record the [candidate evidence](#candidate-evidence) and one outcome for each candidate.
4. Without edits, report the candidates, the false positives you kept, and the commands you ran.
5. With edits, continue with [Edit shape](#edit-shape) and Validation.

Each candidate gets one **outcome**:

- **retain**: the test guards a contract. Name the contract and the bug it catches.
- **fix**: the contract stays, but the test needs a repair. Repair the assertion, or repair the route of a test that runs nowhere.
- **consolidate**: another test takes the assertion. Name that test.
- **delete**: no test needs to take the assertion. Name the keeper that remains, or say why no contract exists.

The **keeper** of a contract is the one test that proves it at its strongest boundary. Every mode protects the keepers.

## Where tests run

A test is proof only where something runs it. Know the route before you judge a test.

| Kind | Where it lives | What runs it |
| --- | --- | --- |
| Unit | `mod tests` in the owner's `src/` file | `make test` (`cargo test --workspace`) in the Linux and macOS CI jobs |
| Doc test | a code block in `///` or `//!` docs | `make test`. A `no_run` block only compiles, and an `ignore` block never runs. |
| Integration | one binary per file in `<crate>/tests/` | `make test` |
| Shared bindings | `tests/integration` (the test modules in `src/lib.rs`), which compiles `contract!` over shared fixtures | `make test` |
| Devnet | a test in `<crate>/tests/` or `tests/*/src/` that needs the node or the indexer, and returns early when `MIDNIGHT_NODE_URL` or `MIDNIGHT_INDEXER_URL` is absent | only a `make test-e2e` or `make test-e2e-node-restart` line that selects it, in the CI E2E job. The line selects the binary with `-p <crate>`, or by a binary name that no other crate uses. |
| Ignored | `#[ignore]` on any test | only a target that runs its binary with `--ignored` |
| Conformance | `tests/conformance` | `make test`, against goldens from the canonical TS runtime (`make conformance-regen`, in the codegen-drift workflow) |
| Macro UI | `crates/compact/bindgen-macro/tests/ui` (trybuild) | `make test` |
| Example | `examples/*` | only an example with a `make dev-settle run-<example>` step in the CI E2E job. Other examples run nowhere. |

A devnet test passes in `make test` because it skips. It proves something only when a route names it and CI's devnet meets every precondition of the test. Otherwise it passes everywhere and runs nowhere (#190, #193). A skipped test still reports `ok`, so a green CI run does not show it. The E2E job's log shows the skips of the binaries that `make test-e2e` and `make test-e2e-node-restart` run. [DISCOVERY.md](DISCOVERY.md) shows how to read it, and how to see the other skips.

## Authoring gate

Before you add a test, answer four questions. If an answer is missing, do not add the test yet.

1. What observable behavior, invariant, or independent contract does it protect?
2. What credible regression makes it fail? Name the wrong implementation. If the answer is not obvious, prove it with a mutation. Save `git diff` to a file. Make the wrong implementation in the owner. Run the test, and make sure that it fails. Undo the mutation. Make sure that `git diff | cmp - <saved file>` succeeds.
3. Why does the current coverage not catch that failure already? The keeper of the contract usually does. A test at another layer needs its own risk, such as a node or indexer failure that the keeper cannot reach. Search the unit, integration, devnet, and conformance tests before you answer. Prefer a new case in an existing table-driven test over a near-duplicate test. Consolidate duplicated setup in the same change.
4. Does it need a test-only seam, which no production caller needs? If yes, move the test to the real boundary instead. A test-only seam is one of these:
   - a visibility that is wider only for a test
   - a `#[cfg(test)]` item outside `mod tests`
   - a `#[doc(hidden)] pub` export
   - a cargo feature that only dev-dependencies enable
   - a trait, generic, or parameter that only a test fills

   One exception applies. A shared fake transport behind a test-only feature can stay when two conditions are true. First, it drives real client code through failures that the real service cannot produce on demand. Second, no default feature enables it. The `test-util` mock indexer of `midnight-indexer-client` is an example.

Then compare the test with every [junk pattern](#junk-patterns). A match fails the gate unless the [retention bar](#retention-bar) names the contract that the test guards independently. A test that breaks under a refactor that keeps the behavior asserts the implementation, not the behavior. Rewrite it at the owning boundary before you land it.

Put the test where its contract lives:

- Private logic goes in the owner's `mod tests`. If the rule is buried in a long function, extract the decision into a small function and test that.
- A public API contract goes in `<crate>/tests/`.
- Behavior that needs a node or indexer goes in a devnet test. Follow the [devnet test rules](#devnet-tests).
- Interpreter behavior with a TS-runtime counterpart can go in the conformance corpus, where an independent implementation supplies the expected values.

Follow the sibling implementations. Some thin wrappers over the node or the indexer have no unit tests and rely on devnet tests. A new sibling method does the same. A fake node or indexer that exists for one method tests the fake.

For generated code, compile the expansion and use the generated item, in `tests/integration` or in a trybuild pass case. Assert on the generated text only for a property that the compiler cannot see.

### Devnet tests

These rules apply to every new devnet test. In an audit, an existing devnet test that breaks one of them gets the outcome **fix**.

- The test skips only when `MIDNIGHT_NODE_URL` or `MIDNIGHT_INDEXER_URL` is absent.
- Any other missing precondition panics when `MIDNIGHT_E2E` is set. `make test-e2e` sets it, so a precondition that CI does not meet fails the job instead of passing it.
- The test creates the wallet state and the fixtures that it needs. A skip on wallet state, or on a fixture that CI does not build, lets the test pass in CI with no proof.
- The test gets a line in `make test-e2e` in the same change, with `-p <crate>`. A test in `<crate>/tests/` uses `--test <file>`. A test in a test module of `tests/*/src/` uses `--lib`.

Put the guard just before the skip:

```rust
if std::env::var_os("MIDNIGHT_E2E").is_some() {
    panic!("<precondition> is missing under make test-e2e");
}
eprintln!("skipping: <precondition> is missing");
return;
```

Two tests in one binary can race when they submit from the same seed, even with `--test-threads=1`. A wallet can sync before the indexer shows the spend of the other test. It then selects the same Dust UTXO (#193).

### Regression tests

A regression test must fail on the pre-fix code for the intended reason, and pass after the fix at the owner. A regression test that never failed proves the fake, not the fix. One regression at the owner boundary covers the bug. Do not replay the same scenario at every layer it crosses.

To show the failure, run the test against the pre-fix production code:

1. Save `git diff`, so that you can prove the restore later.
2. Write the diff of the fix into a patch. For a committed fix, use `git diff <fix>^ <fix>`. For a fix in the working tree, use `git diff`.
3. Delete the test hunks from the patch. If the test calls an item that the fix added, also delete the hunk that adds the item.
4. Reverse the patch.
5. Run the test. It must fail, and the failure message must name the bug. A compile error does not count.
6. Apply the patch again. Make sure that `git diff` matches the saved diff.

```sh
tmp=$(mktemp -d)
git diff > "$tmp/before.diff"
git diff <fix>^ <fix> -- <files> > "$tmp/fix.patch"
# Delete the test hunks from "$tmp/fix.patch" here.
git apply -R "$tmp/fix.patch"
cargo test -p <crate> --lib -- --exact <test-path>   # for an integration test: --test <file> -- --exact <name>
git apply "$tmp/fix.patch"
git diff | cmp - "$tmp/before.diff"
```

## Junk patterns

This is the shared checklist for all modes. The authoring gate rejects a new test that matches one. An audit hunts for existing tests that match one.

- A test with no assertion on its contract. It only calls and unwraps, or it asserts nothing. An `unwrap` proves only that the call did not fail.
- A weak assertion that passes when most cases fail, such as `assert!(ok > 0)` over a list of circuits (#193). Other examples are `assert!(!v.is_empty())`, `assert!(r.is_err())` where the contract is one error variant, and `#[should_panic]` with no `expected`. `a.contains(x) || a.contains(y)` on generated text is weak for the same reason.
- A self-comparison or identity copy: `assert_eq!(x.clone(), x)`, a round trip through a derived `Serialize` and `Deserialize`, or a getter that returns what the constructor set.
- An expected value that the function under test, or the same helper, produced.
- A copied list: an enum's variants, a constant table, a fixture inventory, or an export list, restated in the test.
- An exact source or string grep. Examples are `include_str!` of a source file, and an assertion on `Debug` output or on error text that no caller reads.
- A test of a private predicate or call shape that a test at the real boundary covers already.
- Two tests of the same contract, or near-duplicates that reach the same branch with different data. A crate that replays the tests of a helper that another crate owns is the same mistake.
- A test that exists only to keep a test-only seam alive.
- Dead production code whose only callers are tests, often behind `#[allow(dead_code)]`. Count call sites, not name mentions (#191).
- A fake that implements the asserted behavior, or one fake that stands in for different APIs. A fake provider that returns the value the test then asserts proves the fake.
- A fixture that supplies what the owner should produce: a hand-built transaction, IR tree, or wallet state that no production path builds that way. A storage assertion against a store that the path under test never writes is the same mistake.
- A test that restates a declared flag or constant instead of exercising the behavior that the value promises.
- A test that runs nowhere. Examples are a devnet test that no route names, or whose precondition CI's devnet never meets. An `#[ignore]` test that no target runs with `--ignored` is another. A check behind a file guard (`if let Ok(..) = read_to_string(..)`) whose path does not resolve in CI is the same mistake. So is an `ignore` doc-test block. Prefer `no_run` for an example that needs a devnet, because `no_run` still compiles it.
- A skip on anything other than an absent devnet, even when CI meets the precondition today. The precondition can change with no code change, and the test then passes with no proof.
- A negative test that passes for an unrelated reason. The error can come from a different check than the one the test names. It can also come from a guard that the production path never reaches, or from a node that does not serve the method.
- A name or fixture that promises more than the input exercises. Judge a test by its assertions, not by its name.

## Value bar

A test earns its maintenance cost when it protects behavior, a credible regression, or an independent contract. In an audit, an existing test that must change for a refactor that keeps the behavior is suspect, but not automatically deletable. The authoring gate still rejects new ones.

Before you judge a candidate, read all of these:

- the complete test, its production owner, and the owner's entry point, callers, and callees
- the sibling implementations and every overlapping test (unit, integration, devnet, conformance)
- the route that runs it: the `Makefile` targets, `.github/workflows/`, and for a devnet test, the E2E job's log on `main`
- its history: `git log -S '<name>'`, `git log -L '/fn <name>/,+20:<path>'`, and the PR that added it. An early commit can have no PR, so read its message.
- the fixtures it reads, the target that regenerates each one, and the job that checks it for drift
- the dependency source when the test claims dependency behavior, such as midnight-ledger or subxt. `cargo metadata --format-version 1` gives each package's `manifest_path`.
- for node or indexer behavior, the images in `devnet/docker-compose.yml`. Read the upstream source at those tags, or read the captured output in the E2E log. The node crates in `Cargo.toml` come from a node fork at a pinned rev. They can declare an RPC method that the devnet image does not serve.

## Discovery

Keep discovery read-only, and report the evidence before you edit. Prefer a few candidates with high confidence over a long speculative list. [DISCOVERY.md](DISCOVERY.md) has the sweep commands.

For a broad scope, split the work into parallel **lanes** along production owners, one agent per lane:

- the Compact toolchain under `crates/compact/`, with `tests/conformance` and `tests/integration`
- chain access and transactions
- the wallet
- the core types and crypto
- the test routes: the `Makefile`, `.github/workflows/`, and the examples
- one sweep for patterns across the tree

Before you start, give each workspace member in scope to exactly one lane.

## Retention bar

Keep a test when it independently enforces one of these contracts:

- a public API
- a wire or serialization format
- compatibility with midnight-js or the ledger
- conformance with the canonical Compact runtime
- a cryptographic derivation (keys, addresses, commitments, nullifiers)
- a storage format (the wallet cache, private state)
- a security or privacy property
- a macro expansion
- the node RPC or the indexer schema
- a default
- an architecture boundary
- an export list, when nothing else compiles against those exports (an example, a doc test, or a dependent crate)

Also keep these:

- call ordering, when the order is observable behavior
- a regression test with a credible failure mode
- a source inspection, when it is the cheapest independent guard. It fails when the contract changes (the user-facing key, byte, or path), and it survives a rename.
- a retained test that fails on the baseline. Treat it as a possible product bug. Reproduce it. Then repair the owner instead of deleting the test.

A slow test, or a test that inspects source, can still be the only proof of its contract. `mint_external_recipient` is one of the slowest devnet tests. It stays because no other test proves its property. A test that resembles the implementation can still be the independent contract. Prove otherwise before you remove it.

A test that runs nowhere does not get the outcome **delete** for that reason alone. If its contract has no other proof, make it run (#190, #193). Use one of these repairs:

- Add its route.
- Build the fixture it needs in CI.
- Commit the compiled fixture, as `devnet/contracts/counter/compiled` is, when CI cannot build it.
- Let the test create its own state.

Delete it only when another test proves the contract, or when no contract exists.

## Candidate evidence

Record every field before you edit. If a field is missing, the candidate is not ready for its outcome. For a **fix**, write the repair in place of the deletion that it unlocks. Mark each claim that you inferred and did not run. In a read-only audit, a mutation you could not make becomes a validation step of the edit.

- the exact test name and location
- the failure it can actually detect
- the non-test callers of the production code or test-only seam it covers
- the keeper that remains, or why no proof is necessary
- the relevant history, and why the test or seam exists
- the production or test-support code that the deletion unlocks
- the risk, and the focused command that validates the change

## Edit shape

Choose one coherent batch at one owner boundary. Delete test-only exports, `#[cfg(test)]` accessors, features that only tests enable, and dead production paths. Do not keep aliases for them. Move retained regressions to their owners. Consolidate repeated assertions into one table-driven test.

A keeper can itself be a candidate in the same batch. Before you delete a test, make sure that its keeper stays. When a deletion depends on the removal of a public item, keep the test and the item, and record the removal as a follow-up. Test cleanup does not change the public API.

Prefer a net-negative line count in production code. Do not add replacement tests that restate the same implementation. Do not turn uncertain candidates into cleanup to raise the deletion count.

## Validation

Do not edit source or tests while a cargo build or test runs in the same checkout. Its result then covers an unknown mix of old and new code.

1. Run the owner and sibling tests. Name a unit test by its full path from the crate root, such as `tests::<name>` for a test module in `lib.rs`: `cargo test -p <crate> --lib -- --exact <test-path>`. For an integration test, run `cargo test -p <crate> --test <file> <name>`. Make sure that the `running N tests` line shows a count above zero, because a filter that matches nothing also passes. With no devnet, a devnet test skips and still reports `ok`, so use step 2 for it. For interpreter changes, also run `make conformance`.
2. For a devnet test, use a fresh devnet. CI always starts one, and an older chain can change which branch a test takes.
   1. Run `docker ps`. If a container holds a port that `devnet/docker-compose.yml` maps, stop and report it, even when it runs the images of this repository. Every worktree shares the fixed container names in that file.
   2. Start the devnet with `make dev-up`.
   3. Print the test's own command with `make -n test-e2e | grep -- '--test <file> '`. Two crates can have a binary with the same name, so pick the line with your crate. For a test that only `make test-e2e-node-restart` runs, run that target instead.
   4. Run the command. A `skipping:` line in the output means that the test proved nothing.
   5. Stop the devnet with `make dev-down`, because you started it.
3. For a removed source grep or list assertion, run the command that owns the real contract.
4. Run `cargo fmt --all`, then `git diff --check`.
5. Run the local gates in CI order: `make fmt-check clippy doc check test`. If the change touches a devnet test or its route, also run the steps of the CI E2E job in their order. These are `make test-e2e`, each `make dev-settle run-<example>` step, and `make test-e2e-node-restart`.
6. Read `git diff --numstat`. Report production and tooling lines separately from test and test-support lines.
7. Give the final diff to an independent, read-only reviewer agent. Tell it to list each contract that lost its only proof, and each new assertion that cannot fail. Forbid it to push, open a PR, or comment on a remote resource.

## Landing and continuation

Commit, push, open a PR, or land only when you have authorization. Land one coherent PR at a time. After it lands, update from `main`. Then run a new read-only discovery for the next batch with high confidence.

## Handoff

Report these items:

- the low-value categories you removed, and why those tests existed
- the simplifications in production owners
- the false positives you kept, and why they stay valuable
- the focused and full proof that you actually ran
- the production and test line counts, separately
- the PR and merge state
- the named follow-ups
