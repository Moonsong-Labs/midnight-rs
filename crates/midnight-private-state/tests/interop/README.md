# Cross-SDK interop fixtures

`fixtures/` holds real `EncryptedExport` JSON files produced by midnight-js's `@midnight-ntwrk/midnight-js-level-private-state-provider`. The integration tests in `../interop.rs` `include_str!` them and assert that our `FsPrivateStateProvider::import_*` path recovers the original bytes — proving wire-format compatibility against an actual midnight-js encoder rather than against the spec on paper.

The Rust tests run as part of the default `cargo test --workspace` (no Node, no `--ignored` gate) because the fixtures are committed.

## Regenerating the fixtures

Run this whenever the export format changes, or if you bump the pinned `@midnight-ntwrk/midnight-js-level-private-state-provider` version in `package.json`:

```bash
pnpm install --frozen-lockfile
node regenerate-fixtures.mjs
```

Each fixture file's encrypted bytes change on every regeneration (random salt + IV per call), which is expected; the tests assert on the decrypted content, not the wire bytes.

The inputs the fixtures encode — password, contract address, state bytes, signing-key bytes — are kept in lockstep between `regenerate-fixtures.mjs` and `../interop.rs`. Change them in both places.
