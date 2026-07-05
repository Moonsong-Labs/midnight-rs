// Conformance golden generator: runs each case in ../cases through the
// compiled contract's generated JS codegen on the canonical
// @midnight-ntwrk/compact-runtime, and writes the canonical report to
// ../expected. The Rust side (tests/harness.rs) produces the same report from
// the IR interpreter and diffs the two.
//
// Canonical JSON forms mirror tests/conformance/src/{state_json,report}.rs:
// hex strings for byte content, `{tag, ...}` objects rebuilt key-order-stable,
// map entries sorted by the JSON text of the key.

import { readdirSync, readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import * as rt from '@midnight-ntwrk/compact-runtime';
import * as ocrt from '@midnight-ntwrk/onchain-runtime-v3';

const here = dirname(fileURLToPath(import.meta.url));
const casesDir = resolve(here, '../cases');
const fixturesDir = resolve(here, '../fixtures');
const expectedDir = resolve(here, '../expected');

// Deterministic execution environment. The block time only matters for
// circuits reading the kernel clock; the coin public key seeds the (empty)
// Zswap local state.
const BLOCK_TIME = 1_700_000_000;
const COIN_PUBLIC_KEY = '0'.repeat(64);

const hex = (u8) => Buffer.from(u8).toString('hex');

// --- canonical JSON builders (mirror state_json.rs) -----------------------

const atomToJson = (atom) =>
  atom.tag === 'bytes' ? { tag: 'bytes', length: atom.length } : { tag: atom.tag };

const segmentToJson = (segment) =>
  segment.tag === 'atom'
    ? { tag: 'atom', value: atomToJson(segment.value) }
    : { tag: 'option', value: segment.value.map((alignment) => alignment.map(segmentToJson)) };

const alignedValueToJson = (av) => ({
  value: av.value.map(hex),
  alignment: av.alignment.map(segmentToJson),
});

const stateValueToJson = (encoded) => {
  switch (encoded.tag) {
    case 'null':
      return { tag: 'null' };
    case 'cell':
      return { tag: 'cell', content: alignedValueToJson(encoded.content) };
    case 'map': {
      const entries = [...encoded.content.entries()].map(([key, value]) => {
        const keyJson = alignedValueToJson(key);
        return [JSON.stringify(keyJson), [keyJson, stateValueToJson(value)]];
      });
      entries.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
      return { tag: 'map', content: entries.map(([, entry]) => entry) };
    }
    case 'array':
      return { tag: 'array', content: encoded.content.map(stateValueToJson) };
    default:
      throw new Error(`state value tag not supported by the harness yet: ${encoded.tag}`);
  }
};

const keyToJson = (key) =>
  key.tag === 'value' ? { tag: 'value', value: alignedValueToJson(key.value) } : { tag: 'stack' };

const opToJson = (op) => {
  if (typeof op === 'string') return op;
  if ('noop' in op) return { noop: { n: op.noop.n } };
  if ('popeq' in op)
    return { popeq: { cached: op.popeq.cached, result: alignedValueToJson(op.popeq.result) } };
  if ('addi' in op) return { addi: { immediate: op.addi.immediate } };
  if ('subi' in op) return { subi: { immediate: op.subi.immediate } };
  if ('push' in op)
    return { push: { storage: op.push.storage, value: stateValueToJson(op.push.value) } };
  if ('branch' in op) return { branch: { skip: op.branch.skip } };
  if ('jmp' in op) return { jmp: { skip: op.jmp.skip } };
  if ('concat' in op) return { concat: { cached: op.concat.cached, n: op.concat.n } };
  if ('rem' in op) return { rem: { cached: op.rem.cached } };
  if ('dup' in op) return { dup: { n: op.dup.n } };
  if ('swap' in op) return { swap: { n: op.swap.n } };
  if ('idx' in op)
    return {
      idx: {
        cached: op.idx.cached,
        pushPath: op.idx.pushPath,
        path: op.idx.path.map(keyToJson),
      },
    };
  if ('ins' in op) return { ins: { cached: op.ins.cached, n: op.ins.n } };
  throw new Error(`unhandled transcript op: ${JSON.stringify(op)}`);
};

// Normalized state channel (mirror report.rs::state_report_json): the bare
// StateValue as readable JSON plus the hex serialization of a fresh
// ContractState wrapping it (no operations, default maintenance authority).
const stateReport = (stateValue) => {
  const cs = new ocrt.ContractState();
  cs.data = new ocrt.ChargedState(stateValue);
  return {
    data: stateValueToJson(stateValue.encode()),
    serialized: hex(cs.serialize()),
  };
};

// --- tagged case values (mirror tagged.rs) ---------------------------------

const taggedToJs = (tagged) => {
  const keys = Object.keys(tagged);
  if (keys.length !== 1) throw new Error(`tagged value must have one key: ${JSON.stringify(tagged)}`);
  const [tag] = keys;
  const body = tagged[tag];
  switch (tag) {
    case 'field':
    case 'uint':
      return BigInt(body);
    case 'enum':
      return Number(body);
    case 'bool':
      return Boolean(body);
    case 'bytes':
      return Uint8Array.from(Buffer.from(body, 'hex'));
    case 'string':
      return String(body);
    case 'vector':
      return body.map(taggedToJs);
    default:
      throw new Error(`unknown value tag: ${tag}`);
  }
};

// --- scripted witnesses -----------------------------------------------------

// The Contract instance binds its witness functions once, so the driver keeps
// one mutable script slot the per-step queues are swapped into.
const makeWitnessHarness = (witnessNames) => {
  let queues = new Map();
  const witnesses = {};
  for (const name of witnessNames) {
    witnesses[name] = (witnessContext) => {
      const queue = queues.get(name);
      if (!queue || queue.length === 0) {
        throw new Error(`witness ${name}: script exhausted`);
      }
      return [witnessContext.privateState, taggedToJs(queue.shift())];
    };
  }
  return {
    witnesses,
    load(script) {
      queues = new Map(Object.entries(script ?? {}).map(([k, v]) => [k, [...v]]));
    },
    assertDrained(where) {
      for (const [name, queue] of queues) {
        if (queue.length > 0) {
          throw new Error(`${where}: witness ${name} has ${queue.length} unconsumed value(s)`);
        }
      }
    },
  };
};

// --- case execution ---------------------------------------------------------

const runCase = async (fixture, caseName, caseJson) => {
  const moduleUrl = pathToFileURL(join(fixturesDir, fixture, 'contract', 'index.js'));
  const mod = await import(moduleUrl);

  // Witness names: union of the scripts used anywhere in the case.
  const names = new Set();
  for (const step of caseJson.steps ?? []) {
    for (const name of Object.keys(step.witnesses ?? {})) names.add(name);
  }
  for (const name of Object.keys(caseJson.constructor?.witnesses ?? {})) names.add(name);

  const harness = makeWitnessHarness([...names]);
  const contract = new mod.Contract(harness.witnesses);

  // Constructor.
  harness.load(caseJson.constructor?.witnesses);
  const ctorArgs = (caseJson.constructor?.args ?? []).map(taggedToJs);
  const ctorResult = contract.initialState(
    rt.createConstructorContext(null, COIN_PUBLIC_KEY),
    ...ctorArgs,
  );
  harness.assertDrained(`${fixture}/${caseName} constructor`);
  let stateValue = ctorResult.currentContractState.data.state;

  const report = {
    fixture,
    case: caseName,
    constructor: { state: stateReport(stateValue) },
    steps: [],
  };

  for (const step of caseJson.steps ?? []) {
    harness.load(step.witnesses);
    const context = rt.createCircuitContext(
      ocrt.dummyContractAddress(),
      COIN_PUBLIC_KEY,
      new ocrt.ChargedState(stateValue),
      null,
      undefined,
      undefined,
      BLOCK_TIME,
    );
    const circuit = contract.circuits[step.circuit];
    if (!circuit) throw new Error(`${fixture} has no circuit ${step.circuit}`);
    const args = (step.args ?? []).map(taggedToJs);
    const result = circuit(context, ...args);
    harness.assertDrained(`${fixture}/${caseName} step ${step.circuit}`);

    const zswapOutputs = result.context.currentZswapLocalState.outputs;
    if (zswapOutputs.length > 0) {
      throw new Error(`${fixture}/${caseName}: zswap outputs not supported by the harness yet`);
    }

    stateValue = result.context.currentQueryContext.state.state;
    report.steps.push({
      circuit: step.circuit,
      input: alignedValueToJson(result.proofData.input),
      output: alignedValueToJson(result.proofData.output ?? { value: [], alignment: [] }),
      publicTranscript: result.proofData.publicTranscript.map(opToJson),
      privateTranscriptOutputs: result.proofData.privateTranscriptOutputs.map(alignedValueToJson),
      state: stateReport(stateValue),
      zswapOutputs: [],
    });
  }

  return report;
};

// --- main -------------------------------------------------------------------

const main = async () => {
  const only = process.argv[2]; // optional "<fixture>/<case>" filter
  let wrote = 0;
  for (const fixture of readdirSync(casesDir).sort()) {
    for (const file of readdirSync(join(casesDir, fixture)).sort()) {
      if (!file.endsWith('.json')) continue;
      const caseName = file.replace(/\.json$/, '');
      if (only && only !== `${fixture}/${caseName}`) continue;
      const caseJson = JSON.parse(readFileSync(join(casesDir, fixture, file), 'utf8'));
      const report = await runCase(fixture, caseName, caseJson);
      const outDir = join(expectedDir, fixture);
      mkdirSync(outDir, { recursive: true });
      const outPath = join(outDir, file);
      writeFileSync(outPath, `${JSON.stringify(report, null, 2)}\n`);
      console.log(`wrote ${outPath}`);
      wrote += 1;
    }
  }
  if (wrote === 0) {
    console.error('no cases matched');
    process.exit(1);
  }
};

await main();
