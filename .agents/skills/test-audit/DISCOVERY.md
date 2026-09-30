# Discovery commands

These read-only commands give leads for audit and campaign mode. A hit is not a verdict. Read the test and its owner before you judge it. `git grep` exits with status 1 when it finds nothing.

Run the sweeps that fit your scope. The route commands apply to the whole tree. The devnet commands give nothing for a crate that has no devnet tests.

## Devnet tests that did not run in CI

`make test-e2e` and `make test-e2e-node-restart` pass `--show-output`. The log of CI's E2E job therefore prints the captured output of each passing test under `---- <name> stdout ----`. A skip line there marks a test that proved nothing.

```sh
# Take the latest run on main whose E2E job succeeded. The run itself can fail on another job.
gh run list --repo Moonsong-Labs/midnight-rs --workflow ci.yml --branch main --limit 5
gh run view <run-id> --repo Moonsong-Labs/midnight-rs --json headSha,jobs --jq '.headSha, (.jobs[] | select(.name | startswith("E2E")) | "\(.databaseId) \(.conclusion)")'
log=$(mktemp)
gh run view --job <job-id> --repo Moonsong-Labs/midnight-rs --log > "$log"

# Skips, with the test name on the line above each one
grep -B1 'skipping:' "$log"

# The captured output of each test. Read each block to its end, and look for errors under passing tests,
# such as an RPC method that the node does not serve.
grep -n -- '---- .* stdout ----' "$log"
```

The log describes the code at the run's `headSha`. Compare it with the commit your branch starts from (`git merge-base HEAD <remote>/main`). If they differ, say so in the report.

The log shows only the output of the two E2E targets and the example steps. `make test` hides the output of a passing test. A skip in the unit jobs is therefore not visible in CI. To see one, run the test locally without its precondition: `cargo test -p <crate> --lib -- --show-output` for a unit test, or `cargo test -p <crate> --test <file> -- --show-output` for an integration test. For a devnet test with no route, the missing route is the finding.

## Routes

```sh
# Devnet test files. Each one needs a make test-e2e or test-e2e-node-restart line that selects it.
git grep -l -E 'MIDNIGHT_(NODE|INDEXER)_URL|MIDNIGHT_E2E|ws://(127\.0\.0\.1|localhost):9944' -- '*/tests/*.rs' 'tests/*/src/*.rs'
make -n test-e2e test-e2e-node-restart | grep 'cargo test'

# Every skip site. A skip on anything other than an absent node or indexer URL is a candidate,
# even when CI meets its precondition today. Check that a MIDNIGHT_E2E panic comes before it.
git grep -n 'skipping:' -- '*.rs'

# Ignored tests, and the targets that run them with --ignored
git grep -n -A2 -E '^[[:space:]]*#\[ignore' -- '*.rs'
grep -n -- '--ignored' Makefile .github/workflows/*.yml

# Examples, and the examples that CI runs
ls examples
grep -o -E 'run-[a-z-]+' .github/workflows/ci.yml

# Doc tests that never run
git grep -n -E '^[[:space:]]*(///|//!)[[:space:]]*```[a-z,_]*ignore' -- '*.rs'
```

A `make` line without `-p` finds the test binary by name in every crate of the workspace. Two crates can have a binary with the same name, so match on the crate too.

## Assertions

```sh
# Weak assertions, on one line or wrapped by rustfmt. Many hits are production code, so read each one in context.
git grep -n -E 'assert!\(.*((is_ok|is_err|is_some)\(\)|!.*is_empty\(\)|> 0)[,)]' -- '*.rs'
git grep -n -E '^[[:space:]]+[^/[:space:]].*((is_ok|is_err|is_some)\(\)|> 0),$' -- '*.rs'
git grep -n -E '#\[should_panic\]' -- '*.rs'

# Checks behind a file guard. If the path does not resolve in CI, the assertions never run.
git grep -n -E 'if let Ok\(.*read_to_string|\.exists\(\)' -- '*.rs'
```

## Test-only seams

```sh
# cfg items that name test, outside a test module (this includes a test-util feature)
git grep -n -A1 -E '#\[cfg\((test|.*[^a-z_]test[^a-z_])' -- 'crates/*/src/*' 'tests/*/src/*' | grep -v -E 'mod [a-z_]*tests|#\[cfg|^--$'

# Features that only dev-dependencies enable
git grep -n -E 'features = \[[^]]*"test' -- '*Cargo.toml'

# dead_code shields and hidden exports. A hit inside quote! is generated code, not a seam.
git grep -n -E '#\[(allow|expect)\(dead_code|#\[doc\(hidden\)\]' -- 'crates/*/src/*'

# Call sites of one item. Discard the hits in mod tests, in tests/, and in doc comments.
git grep -n -w '<name>' -- '*.rs'
```

## Fixtures

For each fixture that a test reads, find the target that regenerates it and the workflow that checks it for drift:

```sh
grep -n -E '^(regen-[a-z-]+|compile-contracts|conformance-regen):' Makefile
grep -n 'make ' .github/workflows/codegen-drift.yml
```

A fixture that no target regenerates is hand-built. It can drift from the compiler, and no job reports the drift.
