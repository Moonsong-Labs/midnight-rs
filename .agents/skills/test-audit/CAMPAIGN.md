# Test-pruning campaign

Campaign mode removes every low-value test from one crate or crate group, in one PR. Examples of a scope are `midnight-wallet`, or the Compact toolchain under `crates/compact/`. The value bar, the retention bar, the candidate evidence, and the validation in [SKILL.md](SKILL.md) apply to every lane. This file adds the order of work. Each step ends on its completion criterion. Do not start the next step early.

Keep the baseline, the ledgers, and the lane plans in files outside the working tree, so that no commit takes them by accident. Each lane agent returns its file to the agent that coordinates the campaign. The PR description carries a summary.

## 1. Baseline

Pin a `main` commit. At that commit, record the line counts of the tests and the test support in scope. Also record the pass or fail state of every test binary. For devnet tests, record whether each one ran or printed `skipping:` ([DISCOVERY.md](DISCOVERY.md) shows how). Keep the baseline failures and the skips in their own list. Each one can be a product bug or a test that runs nowhere, not only a stale test.

Done when every test binary in scope has a recorded baseline result.

## 2. Lanes and inventory

Split the surface into **lanes** along production owner boundaries (modules and their public entry points), not along file names. Include the devnet tests, the examples, the conformance fixtures, and the `tests/integration` cases of the scope.

Done when every test file, devnet test, example, conformance fixture, and `tests/integration` case in scope belongs to exactly one lane.

## 3. Read-only ledger per lane

Give each lane to its own read-only agent. The agent reads every assigned test in full, including its case tables. It also reads the production owners and their entry points, callers, history, and route (`Makefile`, `.github/workflows/`). Each test function goes into a written **ledger** with one mark. A table-driven test is one entry, unless its cases need different marks. Then mark each case. The marks are the [outcomes](SKILL.md#candidates-and-outcomes) of SKILL.md:

- `R`: retain. A retained test that only moves to a better file stays `R`, with the move noted.
- `F`: fix. Say whether the assertion or the route needs the repair.
- `C`: consolidate. Name the keeper that takes the assertion first. It can be a case in a sibling table, a stronger test at the boundary, or a test in another crate.
- `D`: delete.

Apply the [junk patterns](SKILL.md#junk-patterns) to every entry.

Done when every test in the lane has a mark and an evidence line.

## 4. Lane plan

The ledger is input, not the edit list. A second read-only pass starts from the ledger and writes the **lane plan**. The pass looks for a redundant **layer**. An example is a set of unit tests that replay through a fake provider what a devnet test proves against the real node. Name the keeper for each contract. Prefer the real boundary (the devnet, the conformance goldens, a real encoding) over a fake collaborator. Correct any ledger errors that this pass finds.

Done when each lane plan names its retired files and its keeper per contract. The plan also names the assertions to carry into keepers and the test-only seams it unlocks.

## 5. Cutover

Edit lane by lane. Let one agent make all changes to shared test support, such as `tests/common` modules and the `tests/integration` crate. With each lane, remove the test-only seams it unlocks: `#[cfg(test)]` accessors, `#[doc(hidden)]` exports, features that only tests enable, injection parameters, and indirection layers. Add to `make test-e2e` each devnet binary that moved, is new, or has an `F` mark for its route. Remove each retired binary. Add to [SKILL.md](SKILL.md) the durable test-ownership rules that this campaign found.

Done when every lane plan is applied and the keepers of each lane pass.

## 6. Preservation review

Before you claim completion, have independent reviewers compare the deleted coverage with the keepers, one reviewer per lane. They look for contracts that lost their only proof. They also look for new assertions that cannot fail, such as an error case that the production code never reaches.

For each restored contract, make one deliberate **mutation** of the production owner. Confirm that the keeper fails. Save `git diff` to a file before the mutation. After you undo the mutation, make sure that `git diff | cmp - <saved file>` succeeds.

Done when every reported gap is restored or rejected with source evidence, and every restored contract has a caught mutation.

## 7. Product defects

A baseline failure that survives into a keeper is a bug report. Fix it at its owner in a separate commit. Prove the fix through the real flow, such as a devnet test or an example. Add a **control** run that reverts the fix and shows the old behavior. Record unrelated product problems as follow-ups, and do not fix them in the campaign.

Done when each repaired defect has a failing control run and a passing run with the fix, on the same harness.

## 8. Reconcile and hand off

A campaign runs across many `main` commits. Merge `main` into a long campaign branch instead of rebasing it. When `main` changed a file that the campaign deleted, keep the deletion. Port the new contract into the keeper. Make sure that each new regression test from `main` stays in a keeper file that still runs. Run the whole suite in scope again on the merged head, including the devnet tests.

List every retired file and every retired binary in the PR description, because review tooling can truncate the file list of a large diff.

Hand off with the items in [Handoff](SKILL.md#handoff), plus these items:

- the baseline and final line counts for tests and test support, with production counted separately
- the lanes, the retired layers, and the keepers
- the preservation gaps found, and their mutations
- the product defects, with proof from the control run and from the run with the fix
