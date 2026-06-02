// Regenerate the cross-SDK interop fixtures under `fixtures/`. Not a test;
// just a one-shot generator you run manually if you change the export format
// or the constants below.
//
//   pnpm install --frozen-lockfile
//   node regenerate-fixtures.mjs
//
// Anything the Rust side asserts on (address, payload bytes, password) must
// be kept in sync with the constants in `../interop.rs`. The encrypted bytes
// in each fixture differ on every regeneration (random salt + IV per call) —
// that's fine; the test asserts on the *decrypted* content, not the wire
// bytes.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { levelPrivateStateProvider } from '@midnight-ntwrk/midnight-js-level-private-state-provider';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FIXTURES_DIR = path.join(HERE, 'fixtures');

// Kept in lockstep with `tests/interop.rs`.
const PASSWORD = 'correct-horse-battery-staple-x7Q';
const CONTRACT_ADDR =
  '0200aabbccddeeff00112233445566778899aabbccddeeff00112233445566';
// Simulates a contract with two witness-backed maps. midnight-js stores the
// payload as a Uint8Array; on the Rust side it's opaque Vec<u8>. Same literal
// content as `STATE_BYTES` in `../interop.rs` — keep in sync.
const STATE_BYTES = new TextEncoder().encode(
  '{"votes":{"alice":3,"bob":5},"deposits":{"acc-1":100,"acc-2":250}}',
);
const SIGNING_KEY_BYTES = new Array(32).fill(0x42);

async function freshProvider() {
  const dbDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mjs-fixtures-'));
  return levelPrivateStateProvider({
    midnightDbName: dbDir,
    privateStoragePasswordProvider: async () => PASSWORD,
    accountId: 'midnight-rs-interop',
  });
}

async function regenerateStatesFixture() {
  const p = await freshProvider();
  p.setContractAddress(CONTRACT_ADDR);
  await p.set(CONTRACT_ADDR, new Uint8Array(STATE_BYTES));
  const exp = await p.exportPrivateStates({ password: PASSWORD });
  const out = path.join(FIXTURES_DIR, 'midnight-js-private-state-export.json');
  fs.writeFileSync(out, JSON.stringify(exp, null, 2));
  return out;
}

async function regenerateKeysFixture() {
  const p = await freshProvider();
  // midnight-js's `SigningKey` type is a hex string (validated against
  // /^[0-9a-fA-F]+$/ on import), not a Buffer/Uint8Array. Encode here so
  // the exported payload's `keys[addr]` value is shaped the way midnight-js
  // would normally produce it — which is what our Rust side hex-decodes on
  // import.
  const hex = Buffer.from(SIGNING_KEY_BYTES).toString('hex');
  await p.setSigningKey(CONTRACT_ADDR, hex);
  const exp = await p.exportSigningKeys({ password: PASSWORD });
  const out = path.join(FIXTURES_DIR, 'midnight-js-signing-key-export.json');
  fs.writeFileSync(out, JSON.stringify(exp, null, 2));
  return out;
}

async function main() {
  fs.mkdirSync(FIXTURES_DIR, { recursive: true });
  const a = await regenerateStatesFixture();
  const b = await regenerateKeysFixture();
  console.log('wrote:');
  console.log(' ', path.relative(process.cwd(), a));
  console.log(' ', path.relative(process.cwd(), b));
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
