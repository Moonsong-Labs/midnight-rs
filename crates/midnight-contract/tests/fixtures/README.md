## Interpreter test fixtures

These fixtures back the unit and integration tests in `crates/midnight-contract/tests/`. Each contract lives in its own subdirectory:

```
<name>/
├── <name>.compact              # source (with any local includes alongside)
└── compiler/contract-info.json # regenerated artifact consumed by the SDK
```

| Fixture    | Source origin                                                                        |
|------------|--------------------------------------------------------------------------------------|
| `bboard`   | `tools/compact-compiler/test-center/test-contracts/bboard.compact`                   |
| `counter`  | `tools/compact-compiler/examples/counter.compact`                                    |
| `election` | `tools/compact-compiler/examples/election.compact`                                   |
| `tiny`     | `tools/compact-compiler/examples/tiny.compact`                                       |

The sources are committed alongside the JSON so a fresh check-out can reproduce every artifact without reaching outside this directory.

### Regenerating

After bumping the pinned compactc (the `tools/compact-compiler` submodule), re-emit the JSON from the in-place sources:

```bash
make build-compactc      # only if compactc isn't built yet (needs Nix)
make regen-test-fixtures # recompiles all four <name>/compiler/contract-info.json
cargo test -p midnight-contract  # verify
```

The Makefile target lives at the repo root; it loops over `$(TEST_FIXTURES)` and invokes the pinned compactc for each `<name>/<name>.compact`. If you add a new fixture, drop its `.compact` source(s) into a new subdirectory and append the name to `TEST_FIXTURES` in the root `Makefile`.

### Updating a source

When the upstream `.compact` you're tracking changes, copy the new version into the fixture subdirectory (along with any local includes that contract requires) and run `make regen-test-fixtures`.
