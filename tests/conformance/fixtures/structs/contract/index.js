import * as __compactRuntime from '@midnight-ntwrk/compact-runtime';
__compactRuntime.checkRuntimeVersion('0.16.101');

const _descriptor_0 = new __compactRuntime.CompactTypeBytes(32);

const _descriptor_1 = new __compactRuntime.CompactTypeUnsignedInteger(16777215n, 3);

const _descriptor_2 = new __compactRuntime.CompactTypeUnsignedInteger(281474976710655n, 6);

const _descriptor_3 = new __compactRuntime.CompactTypeUnsignedInteger(999999n, 3);

class _Odd_0 {
  alignment() {
    return _descriptor_1.alignment().concat(_descriptor_2.alignment().concat(_descriptor_3.alignment()));
  }
  fromValue(value_0) {
    return {
      small: _descriptor_1.fromValue(value_0),
      medium: _descriptor_2.fromValue(value_0),
      ranged: _descriptor_3.fromValue(value_0)
    }
  }
  toValue(value_0) {
    return _descriptor_1.toValue(value_0.small).concat(_descriptor_2.toValue(value_0.medium).concat(_descriptor_3.toValue(value_0.ranged)));
  }
}

const _descriptor_4 = new _Odd_0();

const _descriptor_5 = new __compactRuntime.CompactTypeUnsignedInteger(4294967295n, 4);

const _descriptor_6 = __compactRuntime.CompactTypeBoolean;

class _Point_0 {
  alignment() {
    return _descriptor_5.alignment().concat(_descriptor_6.alignment().concat(_descriptor_0.alignment()));
  }
  fromValue(value_0) {
    return {
      x: _descriptor_5.fromValue(value_0),
      flag: _descriptor_6.fromValue(value_0),
      label: _descriptor_0.fromValue(value_0)
    }
  }
  toValue(value_0) {
    return _descriptor_5.toValue(value_0.x).concat(_descriptor_6.toValue(value_0.flag).concat(_descriptor_0.toValue(value_0.label)));
  }
}

const _descriptor_7 = new _Point_0();

const _descriptor_8 = __compactRuntime.CompactTypeField;

const _descriptor_9 = new __compactRuntime.CompactTypeUnsignedInteger(18446744073709551615n, 8);

class _Either_0 {
  alignment() {
    return _descriptor_6.alignment().concat(_descriptor_0.alignment().concat(_descriptor_0.alignment()));
  }
  fromValue(value_0) {
    return {
      is_left: _descriptor_6.fromValue(value_0),
      left: _descriptor_0.fromValue(value_0),
      right: _descriptor_0.fromValue(value_0)
    }
  }
  toValue(value_0) {
    return _descriptor_6.toValue(value_0.is_left).concat(_descriptor_0.toValue(value_0.left).concat(_descriptor_0.toValue(value_0.right)));
  }
}

const _descriptor_10 = new _Either_0();

const _descriptor_11 = new __compactRuntime.CompactTypeUnsignedInteger(340282366920938463463374607431768211455n, 16);

class _ContractAddress_0 {
  alignment() {
    return _descriptor_0.alignment();
  }
  fromValue(value_0) {
    return {
      bytes: _descriptor_0.fromValue(value_0)
    }
  }
  toValue(value_0) {
    return _descriptor_0.toValue(value_0.bytes);
  }
}

const _descriptor_12 = new _ContractAddress_0();

const _descriptor_13 = new __compactRuntime.CompactTypeUnsignedInteger(255n, 1);

export class Contract {
  witnesses;
  constructor(...args_0) {
    if (args_0.length !== 1) {
      throw new __compactRuntime.CompactError(`Contract constructor: expected 1 argument, received ${args_0.length}`);
    }
    const witnesses_0 = args_0[0];
    if (typeof(witnesses_0) !== 'object') {
      throw new __compactRuntime.CompactError('first (witnesses) argument to Contract constructor is not an object');
    }
    this.witnesses = witnesses_0;
    this.circuits = {
      hash_struct: (...args_1) => {
        if (args_1.length !== 2) {
          throw new __compactRuntime.CompactError(`hash_struct: expected 2 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const p_0 = args_1[1];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.currentQueryContext != undefined)) {
          __compactRuntime.typeError('hash_struct',
                                     'argument 1 (as invoked from Typescript)',
                                     'structs.compact line 23 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(typeof(p_0) === 'object' && typeof(p_0.x) === 'bigint' && p_0.x >= 0n && p_0.x <= 4294967295n && typeof(p_0.flag) === 'boolean' && p_0.label.buffer instanceof ArrayBuffer && p_0.label.BYTES_PER_ELEMENT === 1 && p_0.label.length === 32)) {
          __compactRuntime.typeError('hash_struct',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'structs.compact line 23 char 1',
                                     'struct Point<x: Uint<0..4294967296>, flag: Boolean, label: Bytes<32>>',
                                     p_0)
        }
        const context = { ...contextOrig_0, gasCost: __compactRuntime.emptyRunningCost() };
        const partialProofData = {
          input: {
            value: _descriptor_7.toValue(p_0),
            alignment: _descriptor_7.alignment()
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = this._hash_struct_0(context, partialProofData, p_0);
        partialProofData.output = { value: _descriptor_0.toValue(result_0), alignment: _descriptor_0.alignment() };
        return { result: result_0, context: context, proofData: partialProofData, gasCost: context.gasCost };
      },
      commit_struct: (...args_1) => {
        if (args_1.length !== 3) {
          throw new __compactRuntime.CompactError(`commit_struct: expected 3 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const p_0 = args_1[1];
        const r_0 = args_1[2];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.currentQueryContext != undefined)) {
          __compactRuntime.typeError('commit_struct',
                                     'argument 1 (as invoked from Typescript)',
                                     'structs.compact line 31 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(typeof(p_0) === 'object' && typeof(p_0.x) === 'bigint' && p_0.x >= 0n && p_0.x <= 4294967295n && typeof(p_0.flag) === 'boolean' && p_0.label.buffer instanceof ArrayBuffer && p_0.label.BYTES_PER_ELEMENT === 1 && p_0.label.length === 32)) {
          __compactRuntime.typeError('commit_struct',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'structs.compact line 31 char 1',
                                     'struct Point<x: Uint<0..4294967296>, flag: Boolean, label: Bytes<32>>',
                                     p_0)
        }
        if (!(typeof(r_0) === 'bigint' && r_0 >= 0 && r_0 <= __compactRuntime.MAX_FIELD)) {
          __compactRuntime.typeError('commit_struct',
                                     'argument 2 (argument 3 as invoked from Typescript)',
                                     'structs.compact line 31 char 1',
                                     'Field',
                                     r_0)
        }
        const context = { ...contextOrig_0, gasCost: __compactRuntime.emptyRunningCost() };
        const partialProofData = {
          input: {
            value: _descriptor_7.toValue(p_0).concat(_descriptor_8.toValue(r_0)),
            alignment: _descriptor_7.alignment().concat(_descriptor_8.alignment())
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = this._commit_struct_0(context,
                                               partialProofData,
                                               p_0,
                                               r_0);
        partialProofData.output = { value: _descriptor_8.toValue(result_0), alignment: _descriptor_8.alignment() };
        return { result: result_0, context: context, proofData: partialProofData, gasCost: context.gasCost };
      },
      hash_odd: (...args_1) => {
        if (args_1.length !== 2) {
          throw new __compactRuntime.CompactError(`hash_odd: expected 2 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const o_0 = args_1[1];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.currentQueryContext != undefined)) {
          __compactRuntime.typeError('hash_odd',
                                     'argument 1 (as invoked from Typescript)',
                                     'structs.compact line 38 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(typeof(o_0) === 'object' && typeof(o_0.small) === 'bigint' && o_0.small >= 0n && o_0.small <= 16777215n && typeof(o_0.medium) === 'bigint' && o_0.medium >= 0n && o_0.medium <= 281474976710655n && typeof(o_0.ranged) === 'bigint' && o_0.ranged >= 0n && o_0.ranged <= 999999n)) {
          __compactRuntime.typeError('hash_odd',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'structs.compact line 38 char 1',
                                     'struct Odd<small: Uint<0..16777216>, medium: Uint<0..281474976710656>, ranged: Uint<0..1000000>>',
                                     o_0)
        }
        const context = { ...contextOrig_0, gasCost: __compactRuntime.emptyRunningCost() };
        const partialProofData = {
          input: {
            value: _descriptor_4.toValue(o_0),
            alignment: _descriptor_4.alignment()
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = this._hash_odd_0(context, partialProofData, o_0);
        partialProofData.output = { value: _descriptor_0.toValue(result_0), alignment: _descriptor_0.alignment() };
        return { result: result_0, context: context, proofData: partialProofData, gasCost: context.gasCost };
      }
    };
    this.impureCircuits = {
      hash_struct: this.circuits.hash_struct,
      commit_struct: this.circuits.commit_struct,
      hash_odd: this.circuits.hash_odd
    };
    this.provableCircuits = {
      hash_struct: this.circuits.hash_struct,
      commit_struct: this.circuits.commit_struct,
      hash_odd: this.circuits.hash_odd
    };
  }
  initialState(...args_0) {
    if (args_0.length !== 1) {
      throw new __compactRuntime.CompactError(`Contract state constructor: expected 1 argument (as invoked from Typescript), received ${args_0.length}`);
    }
    const constructorContext_0 = args_0[0];
    if (typeof(constructorContext_0) !== 'object') {
      throw new __compactRuntime.CompactError(`Contract state constructor: expected 'constructorContext' in argument 1 (as invoked from Typescript) to be an object`);
    }
    if (!('initialZswapLocalState' in constructorContext_0)) {
      throw new __compactRuntime.CompactError(`Contract state constructor: expected 'initialZswapLocalState' in argument 1 (as invoked from Typescript)`);
    }
    if (typeof(constructorContext_0.initialZswapLocalState) !== 'object') {
      throw new __compactRuntime.CompactError(`Contract state constructor: expected 'initialZswapLocalState' in argument 1 (as invoked from Typescript) to be an object`);
    }
    const state_0 = new __compactRuntime.ContractState();
    let stateValue_0 = __compactRuntime.StateValue.newArray();
    stateValue_0 = stateValue_0.arrayPush(__compactRuntime.StateValue.newNull());
    stateValue_0 = stateValue_0.arrayPush(__compactRuntime.StateValue.newNull());
    state_0.data = new __compactRuntime.ChargedState(stateValue_0);
    state_0.setOperation('hash_struct', new __compactRuntime.ContractOperation());
    state_0.setOperation('commit_struct', new __compactRuntime.ContractOperation());
    state_0.setOperation('hash_odd', new __compactRuntime.ContractOperation());
    const context = __compactRuntime.createCircuitContext(__compactRuntime.dummyContractAddress(), constructorContext_0.initialZswapLocalState.coinPublicKey, state_0.data, constructorContext_0.initialPrivateState);
    const partialProofData = {
      input: { value: [], alignment: [] },
      output: undefined,
      publicTranscript: [],
      privateTranscriptOutputs: []
    };
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_13.toValue(0n),
                                                                                              alignment: _descriptor_13.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_0.toValue(new Uint8Array(32)),
                                                                                              alignment: _descriptor_0.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_13.toValue(1n),
                                                                                              alignment: _descriptor_13.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_8.toValue(0n),
                                                                                              alignment: _descriptor_8.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    state_0.data = new __compactRuntime.ChargedState(context.currentQueryContext.state.state);
    return {
      currentContractState: state_0,
      currentPrivateState: context.currentPrivateState,
      currentZswapLocalState: context.currentZswapLocalState
    }
  }
  _transientCommit_0(value_0, rand_0) {
    const result_0 = __compactRuntime.transientCommit(_descriptor_7,
                                                      value_0,
                                                      rand_0);
    return result_0;
  }
  _persistentHash_0(value_0) {
    const result_0 = __compactRuntime.persistentHash(_descriptor_7, value_0);
    return result_0;
  }
  _persistentHash_1(value_0) {
    const result_0 = __compactRuntime.persistentHash(_descriptor_4, value_0);
    return result_0;
  }
  _persistentCommit_0(value_0, rand_0) {
    const result_0 = __compactRuntime.persistentCommit(_descriptor_7,
                                                       value_0,
                                                       rand_0);
    return result_0;
  }
  _hash_struct_0(context, partialProofData, p_0) {
    const h_0 = this._persistentHash_0(p_0);
    const c_0 = this._persistentCommit_0(p_0, h_0);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_13.toValue(0n),
                                                                                              alignment: _descriptor_13.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_0.toValue(c_0),
                                                                                              alignment: _descriptor_0.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return c_0;
  }
  _commit_struct_0(context, partialProofData, p_0, r_0) {
    const c_0 = this._transientCommit_0(p_0, r_0);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_13.toValue(1n),
                                                                                              alignment: _descriptor_13.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_8.toValue(c_0),
                                                                                              alignment: _descriptor_8.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return c_0;
  }
  _hash_odd_0(context, partialProofData, o_0) {
    const h_0 = this._persistentHash_1(o_0);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_13.toValue(0n),
                                                                                              alignment: _descriptor_13.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_0.toValue(h_0),
                                                                                              alignment: _descriptor_0.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return h_0;
  }
}
export function ledger(stateOrChargedState) {
  const state = stateOrChargedState instanceof __compactRuntime.StateValue ? stateOrChargedState : stateOrChargedState.state;
  const chargedState = stateOrChargedState instanceof __compactRuntime.StateValue ? new __compactRuntime.ChargedState(stateOrChargedState) : stateOrChargedState;
  const context = {
    currentQueryContext: new __compactRuntime.QueryContext(chargedState, __compactRuntime.dummyContractAddress()),
    costModel: __compactRuntime.CostModel.initialCostModel()
  };
  const partialProofData = {
    input: { value: [], alignment: [] },
    output: undefined,
    publicTranscript: [],
    privateTranscriptOutputs: []
  };
  return {
    get tag_cell() {
      return _descriptor_0.fromValue(__compactRuntime.queryLedgerState(context,
                                                                       partialProofData,
                                                                       [
                                                                        { dup: { n: 0 } },
                                                                        { idx: { cached: false,
                                                                                 pushPath: false,
                                                                                 path: [
                                                                                        { tag: 'value',
                                                                                          value: { value: _descriptor_13.toValue(0n),
                                                                                                   alignment: _descriptor_13.alignment() } }] } },
                                                                        { popeq: { cached: false,
                                                                                   result: undefined } }]).value);
    },
    get scratch() {
      return _descriptor_8.fromValue(__compactRuntime.queryLedgerState(context,
                                                                       partialProofData,
                                                                       [
                                                                        { dup: { n: 0 } },
                                                                        { idx: { cached: false,
                                                                                 pushPath: false,
                                                                                 path: [
                                                                                        { tag: 'value',
                                                                                          value: { value: _descriptor_13.toValue(1n),
                                                                                                   alignment: _descriptor_13.alignment() } }] } },
                                                                        { popeq: { cached: false,
                                                                                   result: undefined } }]).value);
    }
  };
}
const _emptyContext = {
  currentQueryContext: new __compactRuntime.QueryContext(new __compactRuntime.ContractState().data, __compactRuntime.dummyContractAddress())
};
const _dummyContract = new Contract({ });
export const pureCircuits = {};
export const contractReferenceLocations =
  { tag: 'publicLedgerArray', indices: { } };
//# sourceMappingURL=index.js.map
