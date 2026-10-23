## Interpreter test fixtures

These fixtures back the unit and integration tests in `crates/midnight-contract/tests/`. Each contract lives in its own subdirectory:

```
<name>/
├── <name>.compact              # source
└── compiler/analyzed-ir.sexp   # artifact that `make fixtures` writes, not committed
```

The sources come from the compiler fork [`RomarQ/compact`](https://github.com/RomarQ/compact), at the commit that `COMPACT_REV` in the root `Makefile` names:

| Fixture    | Source origin |
|------------|---------------|
| `bboard`   | [`test-center/test-contracts/bboard.compact`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/test-center/test-contracts/bboard.compact) |
| `counter`  | [`examples/counter.compact`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/examples/counter.compact) |
| `election` | [`examples/election.compact`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/examples/election.compact) |
| `tiny`     | [`examples/tiny.compact`](https://github.com/RomarQ/compact/blob/fa2181fbc6dac2135defdb4f55ce10d8332185d5/examples/tiny.compact) |

Only the sources are committed.

### Compiling

`make fixtures` compiles each fixture whose artifact is missing, or whose source or `COMPACT_REV` changed since its last compile. The make targets that build or test the workspace run it first. It runs the compiler image, so it needs Docker. In CI, one job compiles the fixtures and the other jobs download its output.

```bash
make fixtures                   # compiles every stale <name>/compiler/analyzed-ir.sexp
make contracts                  # the tests also read the devnet contracts
cargo test -p midnight-contract # verify
```

To add a fixture, put its `.compact` source in a new subdirectory and append the name to `TEST_FIXTURES` in the root `Makefile`.

### Updating a source

When the upstream `.compact` you're tracking changes, copy the new version into the fixture subdirectory. The next `make fixtures` compiles it again.
