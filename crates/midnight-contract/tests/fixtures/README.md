## Interpreter test fixtures

These fixtures back the unit and integration tests in `crates/midnight-contract/tests/`. Each contract lives in its own subdirectory:

```
<name>/
├── <name>.compact              # source (with any local includes alongside)
└── compiler/analyzed-ir.sexp   # regenerated artifact consumed by the SDK
```

The sources come from the compiler fork [`RomarQ/compact`](https://github.com/RomarQ/compact), at the commit that `COMPACT_REV` in the root `Makefile` names:

| Fixture    | Source origin |
|------------|---------------|
| `bboard`   | [`test-center/test-contracts/bboard.compact`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/test-center/test-contracts/bboard.compact) |
| `counter`  | [`examples/counter.compact`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/examples/counter.compact) |
| `election` | [`examples/election.compact`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/examples/election.compact) |
| `tiny`     | [`examples/tiny.compact`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/examples/tiny.compact) |

The sources are committed alongside the artifact so a fresh check-out can reproduce every artifact without reaching outside this directory.

### Regenerating

After a change of `COMPACT_REV`, re-emit the artifact from the in-place sources. The target runs the compiler image, so it needs Docker:

```bash
make regen-test-fixtures # recompiles every <name>/compiler/analyzed-ir.sexp
cargo test -p midnight-contract  # verify
```

The Makefile target lives at the repo root; it loops over `$(TEST_FIXTURES)` and invokes the compactc at `COMPACT_REV` for each `<name>/<name>.compact`. If you add a new fixture, drop its `.compact` source(s) into a new subdirectory and append the name to `TEST_FIXTURES` in the root `Makefile`.

### Updating a source

When the upstream `.compact` you're tracking changes, copy the new version into the fixture subdirectory (along with any local includes that contract requires) and run `make regen-test-fixtures`.
