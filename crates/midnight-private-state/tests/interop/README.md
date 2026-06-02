# Cross-SDK interop test harness

A small Node script that drives `@midnight-ntwrk/midnight-js-level-private-state-provider` for the integration tests in `../interop.rs`. The tests are `#[ignore]`d by default because they need Node + pnpm + an `npm install`; the canonical way to run them is:

```bash
make test-interop                    # from the repo root
```

which installs the JS deps via `pnpm install --frozen-lockfile` and then runs `cargo test -p midnight-private-state --test interop -- --ignored`.

The round-trip a single test exercises:

```
Rust FsPrivateStateProvider
    └─ export_private_states ─→ EncryptedExport JSON ─→ disk
                                                            │
                                                            ▼
                                          midnight-js levelPrivateStateProvider
                                                            │
                                                            ├─ importPrivateStates
                                                            └─ exportPrivateStates ─→ disk
                                                                                          │
                                                                                          ▼
                                                                  Rust FsPrivateStateProvider
                                                                                          │
                                                                                          └─ import_private_states + .get(addr)
                                                                                                  assertEq original bytes
```

If you change the export wire format, regenerate the lockfile with `pnpm install` and re-run.
