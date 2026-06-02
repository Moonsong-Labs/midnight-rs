// Node-side driver for the cross-SDK interop tests.
//
// Invoked from `crates/midnight-private-state/tests/interop.rs` with one of
// these modes; each takes an input file produced by the Rust side, drives
// midnight-js's `level-private-state-provider`, and writes a re-exported
// envelope back to disk so the Rust side can import it again.
//
//   round-trip-states <in> <out> <password> <contract-address>
//   round-trip-keys   <in> <out> <password>
//
// `<in>` is an `EncryptedExport` JSON produced by `FsPrivateStateProvider`.
// `<out>` is where we write the midnight-js re-export of the same data.
// Each invocation uses a fresh OS tmpdir for its LevelDB instance so runs
// are isolated.
//
// Run `pnpm install --frozen-lockfile` here before invoking, or use the
// `make test-interop` target which does that for you.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { levelPrivateStateProvider } from '@midnight-ntwrk/midnight-js-level-private-state-provider';

async function main() {
  const [, , mode, inPath, outPath, password, address] = process.argv;
  if (!mode || !inPath || !outPath || !password) {
    throw new Error(
      'usage: node interop.mjs <round-trip-states|round-trip-keys> <in> <out> <password> [address]',
    );
  }

  // `midnightDbName` controls the LevelDB directory path (default
  // `midnight-level-db` would collide between parallel test runs).
  // `privateStateStoreName` is a sublevel name inside that directory; we
  // leave it at the default. Each invocation gets a fresh tmpdir so parallel
  // tests don't trip the LevelDB file lock.
  const dbDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mjs-interop-'));
  const provider = await levelPrivateStateProvider({
    midnightDbName: dbDir,
    privateStoragePasswordProvider: async () => password,
    // accountId scopes storage in midnight-js. Unimportant for this test; a
    // fixed value keeps the key layout deterministic across runs.
    accountId: 'midnight-rs-interop',
  });

  switch (mode) {
    case 'round-trip-states': {
      if (!address) {
        throw new Error('round-trip-states requires <contract-address> as the last arg');
      }
      provider.setContractAddress(address);

      const incoming = JSON.parse(fs.readFileSync(inPath, 'utf8'));
      const importResult = await provider.importPrivateStates(incoming, { password });
      process.stderr.write(`midnight-js import: ${JSON.stringify(importResult)}\n`);

      const reexport = await provider.exportPrivateStates({ password });
      fs.writeFileSync(outPath, JSON.stringify(reexport));
      break;
    }

    case 'round-trip-keys': {
      const incoming = JSON.parse(fs.readFileSync(inPath, 'utf8'));
      const importResult = await provider.importSigningKeys(incoming, { password });
      process.stderr.write(`midnight-js import: ${JSON.stringify(importResult)}\n`);

      const reexport = await provider.exportSigningKeys({ password });
      fs.writeFileSync(outPath, JSON.stringify(reexport));
      break;
    }

    default:
      throw new Error(`unknown mode: ${mode}`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
