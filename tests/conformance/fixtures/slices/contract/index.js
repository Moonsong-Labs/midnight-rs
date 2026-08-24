import * as __compactRuntime from '@midnight-ntwrk/compact-runtime';
__compactRuntime.checkRuntimeVersion('0.18.107');

const _descriptor_0 = __compactRuntime.CompactTypeField;

const _descriptor_1 = new __compactRuntime.CompactTypeUnsignedInteger(255n, 1);

const _descriptor_2 = new __compactRuntime.CompactTypeUnsignedInteger(65535n, 2);

const _descriptor_3 = new __compactRuntime.CompactTypeVector(6, _descriptor_0);

const _descriptor_4 = new __compactRuntime.CompactTypeBytes(8);

const _descriptor_5 = new __compactRuntime.CompactTypeBytes(4);

const _descriptor_6 = new __compactRuntime.CompactTypeVector(3, _descriptor_0);

const _descriptor_7 = new __compactRuntime.CompactTypeUnsignedInteger(18446744073709551615n, 8);

const _descriptor_8 = __compactRuntime.CompactTypeBoolean;

const _descriptor_9 = new __compactRuntime.CompactTypeBytes(32);

class _Either_0 {
  alignment() {
    return _descriptor_8.alignment().concat(_descriptor_9.alignment().concat(_descriptor_9.alignment()));
  }
  fromValue(value_0) {
    return {
      is_left: _descriptor_8.fromValue(value_0),
      left: _descriptor_9.fromValue(value_0),
      right: _descriptor_9.fromValue(value_0)
    }
  }
  toValue(value_0) {
    return _descriptor_8.toValue(value_0.is_left).concat(_descriptor_9.toValue(value_0.left).concat(_descriptor_9.toValue(value_0.right)));
  }
}

const _descriptor_10 = new _Either_0();

const _descriptor_11 = new __compactRuntime.CompactTypeUnsignedInteger(340282366920938463463374607431768211455n, 16);

class _ContractAddress_0 {
  alignment() {
    return _descriptor_9.alignment();
  }
  fromValue(value_0) {
    return {
      bytes: _descriptor_9.fromValue(value_0)
    }
  }
  toValue(value_0) {
    return _descriptor_9.toValue(value_0.bytes);
  }
}

const _descriptor_12 = new _ContractAddress_0();

const _descriptor_13 = new __compactRuntime.CompactTypeUnsignedInteger(4294967295n, 4);

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
      index_bytes: async (...args_1) => {
        if (args_1.length !== 2) {
          throw new __compactRuntime.CompactError(`index_bytes: expected 2 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const b_0 = args_1[1];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.callContext.currentQueryContext != undefined)) {
          __compactRuntime.typeError('index_bytes',
                                     'argument 1 (as invoked from Typescript)',
                                     'slices.compact line 16 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(b_0.buffer instanceof ArrayBuffer && b_0.BYTES_PER_ELEMENT === 1 && b_0.length === 8)) {
          __compactRuntime.typeError('index_bytes',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'slices.compact line 16 char 1',
                                     'Bytes<8>',
                                     b_0)
        }
        const context = __compactRuntime.copyCircuitContext(contextOrig_0);
        const partialProofData = {
          input: {
            value: _descriptor_4.toValue(b_0),
            alignment: _descriptor_4.alignment()
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = await this._index_bytes_0(context,
                                                   partialProofData,
                                                   b_0);
        partialProofData.output = { value: _descriptor_2.toValue(result_0), alignment: _descriptor_2.alignment() };
        __compactRuntime.finalizeCallProofData(context, partialProofData);
        return { result: result_0, context: context, gasCost: context.callContext.currentGasCost };
      },
      slice_bytes_const: async (...args_1) => {
        if (args_1.length !== 2) {
          throw new __compactRuntime.CompactError(`slice_bytes_const: expected 2 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const b_0 = args_1[1];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.callContext.currentQueryContext != undefined)) {
          __compactRuntime.typeError('slice_bytes_const',
                                     'argument 1 (as invoked from Typescript)',
                                     'slices.compact line 23 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(b_0.buffer instanceof ArrayBuffer && b_0.BYTES_PER_ELEMENT === 1 && b_0.length === 8)) {
          __compactRuntime.typeError('slice_bytes_const',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'slices.compact line 23 char 1',
                                     'Bytes<8>',
                                     b_0)
        }
        const context = __compactRuntime.copyCircuitContext(contextOrig_0);
        const partialProofData = {
          input: {
            value: _descriptor_4.toValue(b_0),
            alignment: _descriptor_4.alignment()
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = await this._slice_bytes_const_0(context,
                                                         partialProofData,
                                                         b_0);
        partialProofData.output = { value: _descriptor_5.toValue(result_0), alignment: _descriptor_5.alignment() };
        __compactRuntime.finalizeCallProofData(context, partialProofData);
        return { result: result_0, context: context, gasCost: context.callContext.currentGasCost };
      },
      slice_bytes_dynamic: async (...args_1) => {
        if (args_1.length !== 2) {
          throw new __compactRuntime.CompactError(`slice_bytes_dynamic: expected 2 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const b_0 = args_1[1];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.callContext.currentQueryContext != undefined)) {
          __compactRuntime.typeError('slice_bytes_dynamic',
                                     'argument 1 (as invoked from Typescript)',
                                     'slices.compact line 30 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(b_0.buffer instanceof ArrayBuffer && b_0.BYTES_PER_ELEMENT === 1 && b_0.length === 8)) {
          __compactRuntime.typeError('slice_bytes_dynamic',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'slices.compact line 30 char 1',
                                     'Bytes<8>',
                                     b_0)
        }
        const context = __compactRuntime.copyCircuitContext(contextOrig_0);
        const partialProofData = {
          input: {
            value: _descriptor_4.toValue(b_0),
            alignment: _descriptor_4.alignment()
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = await this._slice_bytes_dynamic_0(context,
                                                           partialProofData,
                                                           b_0);
        partialProofData.output = { value: _descriptor_5.toValue(result_0), alignment: _descriptor_5.alignment() };
        __compactRuntime.finalizeCallProofData(context, partialProofData);
        return { result: result_0, context: context, gasCost: context.callContext.currentGasCost };
      },
      slice_then_index: async (...args_1) => {
        if (args_1.length !== 2) {
          throw new __compactRuntime.CompactError(`slice_then_index: expected 2 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const b_0 = args_1[1];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.callContext.currentQueryContext != undefined)) {
          __compactRuntime.typeError('slice_then_index',
                                     'argument 1 (as invoked from Typescript)',
                                     'slices.compact line 38 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(b_0.buffer instanceof ArrayBuffer && b_0.BYTES_PER_ELEMENT === 1 && b_0.length === 8)) {
          __compactRuntime.typeError('slice_then_index',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'slices.compact line 38 char 1',
                                     'Bytes<8>',
                                     b_0)
        }
        const context = __compactRuntime.copyCircuitContext(contextOrig_0);
        const partialProofData = {
          input: {
            value: _descriptor_4.toValue(b_0),
            alignment: _descriptor_4.alignment()
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = await this._slice_then_index_0(context,
                                                        partialProofData,
                                                        b_0);
        partialProofData.output = { value: _descriptor_2.toValue(result_0), alignment: _descriptor_2.alignment() };
        __compactRuntime.finalizeCallProofData(context, partialProofData);
        return { result: result_0, context: context, gasCost: context.callContext.currentGasCost };
      },
      slice_vector_const: async (...args_1) => {
        if (args_1.length !== 2) {
          throw new __compactRuntime.CompactError(`slice_vector_const: expected 2 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const xs_0 = args_1[1];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.callContext.currentQueryContext != undefined)) {
          __compactRuntime.typeError('slice_vector_const',
                                     'argument 1 (as invoked from Typescript)',
                                     'slices.compact line 46 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(Array.isArray(xs_0) && xs_0.length === 6 && xs_0.every((t) => typeof(t) === 'bigint' && t >= 0 && t <= __compactRuntime.MAX_FIELD))) {
          __compactRuntime.typeError('slice_vector_const',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'slices.compact line 46 char 1',
                                     'Vector<6, Field>',
                                     xs_0)
        }
        const context = __compactRuntime.copyCircuitContext(contextOrig_0);
        const partialProofData = {
          input: {
            value: _descriptor_3.toValue(xs_0),
            alignment: _descriptor_3.alignment()
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = await this._slice_vector_const_0(context,
                                                          partialProofData,
                                                          xs_0);
        partialProofData.output = { value: _descriptor_0.toValue(result_0), alignment: _descriptor_0.alignment() };
        __compactRuntime.finalizeCallProofData(context, partialProofData);
        return { result: result_0, context: context, gasCost: context.callContext.currentGasCost };
      },
      slice_tuple_const: async (...args_1) => {
        if (args_1.length !== 4) {
          throw new __compactRuntime.CompactError(`slice_tuple_const: expected 4 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const a_0 = args_1[1];
        const b_0 = args_1[2];
        const c_0 = args_1[3];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.callContext.currentQueryContext != undefined)) {
          __compactRuntime.typeError('slice_tuple_const',
                                     'argument 1 (as invoked from Typescript)',
                                     'slices.compact line 55 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(typeof(a_0) === 'bigint' && a_0 >= 0n && a_0 <= 255n)) {
          __compactRuntime.typeError('slice_tuple_const',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'slices.compact line 55 char 1',
                                     'Uint<0..256>',
                                     a_0)
        }
        if (!(typeof(b_0) === 'bigint' && b_0 >= 0n && b_0 <= 65535n)) {
          __compactRuntime.typeError('slice_tuple_const',
                                     'argument 2 (argument 3 as invoked from Typescript)',
                                     'slices.compact line 55 char 1',
                                     'Uint<0..65536>',
                                     b_0)
        }
        if (!(typeof(c_0) === 'bigint' && c_0 >= 0 && c_0 <= __compactRuntime.MAX_FIELD)) {
          __compactRuntime.typeError('slice_tuple_const',
                                     'argument 3 (argument 4 as invoked from Typescript)',
                                     'slices.compact line 55 char 1',
                                     'Field',
                                     c_0)
        }
        const context = __compactRuntime.copyCircuitContext(contextOrig_0);
        const partialProofData = {
          input: {
            value: _descriptor_1.toValue(a_0).concat(_descriptor_2.toValue(b_0).concat(_descriptor_0.toValue(c_0))),
            alignment: _descriptor_1.alignment().concat(_descriptor_2.alignment().concat(_descriptor_0.alignment()))
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = await this._slice_tuple_const_0(context,
                                                         partialProofData,
                                                         a_0,
                                                         b_0,
                                                         c_0);
        partialProofData.output = { value: _descriptor_0.toValue(result_0), alignment: _descriptor_0.alignment() };
        __compactRuntime.finalizeCallProofData(context, partialProofData);
        return { result: result_0, context: context, gasCost: context.callContext.currentGasCost };
      },
      slice_vector_dynamic: async (...args_1) => {
        if (args_1.length !== 2) {
          throw new __compactRuntime.CompactError(`slice_vector_dynamic: expected 2 arguments (as invoked from Typescript), received ${args_1.length}`);
        }
        const contextOrig_0 = args_1[0];
        const xs_0 = args_1[1];
        if (!(typeof(contextOrig_0) === 'object' && contextOrig_0.callContext.currentQueryContext != undefined)) {
          __compactRuntime.typeError('slice_vector_dynamic',
                                     'argument 1 (as invoked from Typescript)',
                                     'slices.compact line 65 char 1',
                                     'CircuitContext',
                                     contextOrig_0)
        }
        if (!(Array.isArray(xs_0) && xs_0.length === 6 && xs_0.every((t) => typeof(t) === 'bigint' && t >= 0 && t <= __compactRuntime.MAX_FIELD))) {
          __compactRuntime.typeError('slice_vector_dynamic',
                                     'argument 1 (argument 2 as invoked from Typescript)',
                                     'slices.compact line 65 char 1',
                                     'Vector<6, Field>',
                                     xs_0)
        }
        const context = __compactRuntime.copyCircuitContext(contextOrig_0);
        const partialProofData = {
          input: {
            value: _descriptor_3.toValue(xs_0),
            alignment: _descriptor_3.alignment()
          },
          output: undefined,
          publicTranscript: [],
          privateTranscriptOutputs: []
        };
        const result_0 = await this._slice_vector_dynamic_0(context,
                                                            partialProofData,
                                                            xs_0);
        partialProofData.output = { value: _descriptor_0.toValue(result_0), alignment: _descriptor_0.alignment() };
        __compactRuntime.finalizeCallProofData(context, partialProofData);
        return { result: result_0, context: context, gasCost: context.callContext.currentGasCost };
      }
    };
    this.impureCircuits = {
      index_bytes: this.circuits.index_bytes,
      slice_bytes_const: this.circuits.slice_bytes_const,
      slice_bytes_dynamic: this.circuits.slice_bytes_dynamic,
      slice_then_index: this.circuits.slice_then_index,
      slice_vector_const: this.circuits.slice_vector_const,
      slice_tuple_const: this.circuits.slice_tuple_const,
      slice_vector_dynamic: this.circuits.slice_vector_dynamic
    };
    this.provableCircuits = {
      index_bytes: this.circuits.index_bytes,
      slice_bytes_const: this.circuits.slice_bytes_const,
      slice_bytes_dynamic: this.circuits.slice_bytes_dynamic,
      slice_then_index: this.circuits.slice_then_index,
      slice_vector_const: this.circuits.slice_vector_const,
      slice_tuple_const: this.circuits.slice_tuple_const,
      slice_vector_dynamic: this.circuits.slice_vector_dynamic
    };
  }
  async initialState(...args_0) {
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
    stateValue_0 = stateValue_0.arrayPush(__compactRuntime.StateValue.newNull());
    stateValue_0 = stateValue_0.arrayPush(__compactRuntime.StateValue.newNull());
    state_0.data = new __compactRuntime.ChargedState(stateValue_0);
    state_0.setOperation('index_bytes', new __compactRuntime.ContractOperation());
    state_0.setOperation('slice_bytes_const', new __compactRuntime.ContractOperation());
    state_0.setOperation('slice_bytes_dynamic', new __compactRuntime.ContractOperation());
    state_0.setOperation('slice_then_index', new __compactRuntime.ContractOperation());
    state_0.setOperation('slice_vector_const', new __compactRuntime.ContractOperation());
    state_0.setOperation('slice_tuple_const', new __compactRuntime.ContractOperation());
    state_0.setOperation('slice_vector_dynamic', new __compactRuntime.ContractOperation());
    const context = __compactRuntime.createCircuitContext('constructor', __compactRuntime.dummyContractAddress(), constructorContext_0.initialZswapLocalState.coinPublicKey, state_0.data, constructorContext_0.initialPrivateState);
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
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(0n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_2.toValue(0n),
                                                                                              alignment: _descriptor_2.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(1n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_0.toValue(0n),
                                                                                              alignment: _descriptor_0.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(2n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_0.toValue(0n),
                                                                                              alignment: _descriptor_0.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(3n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_5.toValue(new Uint8Array(4)),
                                                                                              alignment: _descriptor_5.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    state_0.data = new __compactRuntime.ChargedState(context.callContext.currentQueryContext.state.state);
    return {
      currentContractState: state_0,
      currentPrivateState: context.callContext.currentPrivateState,
      currentZswapLocalState: context.callContext.currentZswapLocalState
    }
  }
  _pack3_0(v_0) {
    return __compactRuntime.addField(__compactRuntime.addField(__compactRuntime.mulField(v_0[0],
                                                                                         1000000n),
                                                               __compactRuntime.mulField(v_0[1],
                                                                                         1000n)),
                                     v_0[2]);
  }
  async _index_bytes_0(context, partialProofData, b_0) {
    const packed_0 = BigInt(b_0[2n]) * 256n + BigInt(b_0[5n]);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(0n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_2.toValue(packed_0),
                                                                                              alignment: _descriptor_2.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return packed_0;
  }
  async _slice_bytes_const_0(context, partialProofData, b_0) {
    const tail_0 = ((e, i) => e.slice(i, i+4))(b_0, Number(3n));
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(3n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_5.toValue(tail_0),
                                                                                              alignment: _descriptor_5.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return tail_0;
  }
  async _slice_bytes_dynamic_0(context, partialProofData, b_0) {
    const start_0 = 1n;
    const tail_0 = ((e, i) => e.slice(i, i+4))(b_0, Number(start_0));
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(3n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_5.toValue(tail_0),
                                                                                              alignment: _descriptor_5.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return tail_0;
  }
  async _slice_then_index_0(context, partialProofData, b_0) {
    const tail_0 = ((e, i) => e.slice(i, i+4))(b_0, Number(3n));
    const packed_0 = BigInt(tail_0[0n]) * 256n + BigInt(tail_0[3n]);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(0n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_2.toValue(packed_0),
                                                                                              alignment: _descriptor_2.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return packed_0;
  }
  async _slice_vector_const_0(context, partialProofData, xs_0) {
    const mid_0 = ((e) => e.slice(2, 5))(xs_0);
    const packed_0 = this._pack3_0(mid_0);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(1n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_0.toValue(packed_0),
                                                                                              alignment: _descriptor_0.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return packed_0;
  }
  async _slice_tuple_const_0(context, partialProofData, a_0, b_0, c_0) {
    const row_0 = [a_0, b_0, c_0, 7n];
    const mid_0 = ((e) => e.slice(1, 3))(row_0);
    const packed_0 = __compactRuntime.addField(__compactRuntime.mulField(mid_0[0],
                                                                         1000n),
                                               mid_0[1]);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(2n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_0.toValue(packed_0),
                                                                                              alignment: _descriptor_0.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return packed_0;
  }
  async _slice_vector_dynamic_0(context, partialProofData, xs_0) {
    const start_0 = 1n;
    const mid_0 = ((e, i) => e.slice(i, i+3))(xs_0, Number(start_0));
    const packed_0 = this._pack3_0(mid_0);
    __compactRuntime.queryLedgerState(context,
                                      partialProofData,
                                      [
                                       { push: { storage: false,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_1.toValue(1n),
                                                                                              alignment: _descriptor_1.alignment() }).encode() } },
                                       { push: { storage: true,
                                                 value: __compactRuntime.StateValue.newCell({ value: _descriptor_0.toValue(packed_0),
                                                                                              alignment: _descriptor_0.alignment() }).encode() } },
                                       { ins: { cached: false, n: 1 } }]);
    return packed_0;
  }
}
export function ledger(stateOrChargedState) {
  const state = stateOrChargedState instanceof __compactRuntime.StateValue ? stateOrChargedState : stateOrChargedState.state;
  const chargedState = stateOrChargedState instanceof __compactRuntime.StateValue ? new __compactRuntime.ChargedState(stateOrChargedState) : stateOrChargedState;
  const context = {
    callContext: { currentQueryContext: new __compactRuntime.QueryContext(chargedState, __compactRuntime.dummyContractAddress()), currentGasCost: __compactRuntime.emptyRunningCost() },
    costModel: __compactRuntime.CostModel.initialCostModel()
  };
  const partialProofData = {
    input: { value: [], alignment: [] },
    output: undefined,
    publicTranscript: [],
    privateTranscriptOutputs: []
  };
  return {
    get byte_pair() {
      return _descriptor_2.fromValue(__compactRuntime.queryLedgerState(context,
                                                                       partialProofData,
                                                                       [
                                                                        { dup: { n: 0 } },
                                                                        { idx: { cached: false,
                                                                                 pushPath: false,
                                                                                 path: [
                                                                                        { tag: 'value',
                                                                                          value: { value: _descriptor_1.toValue(0n),
                                                                                                   alignment: _descriptor_1.alignment() } }] } },
                                                                        { popeq: { cached: false,
                                                                                   result: undefined } }]).value);
    },
    get vector_digest() {
      return _descriptor_0.fromValue(__compactRuntime.queryLedgerState(context,
                                                                       partialProofData,
                                                                       [
                                                                        { dup: { n: 0 } },
                                                                        { idx: { cached: false,
                                                                                 pushPath: false,
                                                                                 path: [
                                                                                        { tag: 'value',
                                                                                          value: { value: _descriptor_1.toValue(1n),
                                                                                                   alignment: _descriptor_1.alignment() } }] } },
                                                                        { popeq: { cached: false,
                                                                                   result: undefined } }]).value);
    },
    get tuple_digest() {
      return _descriptor_0.fromValue(__compactRuntime.queryLedgerState(context,
                                                                       partialProofData,
                                                                       [
                                                                        { dup: { n: 0 } },
                                                                        { idx: { cached: false,
                                                                                 pushPath: false,
                                                                                 path: [
                                                                                        { tag: 'value',
                                                                                          value: { value: _descriptor_1.toValue(2n),
                                                                                                   alignment: _descriptor_1.alignment() } }] } },
                                                                        { popeq: { cached: false,
                                                                                   result: undefined } }]).value);
    },
    get tail_bytes() {
      return _descriptor_5.fromValue(__compactRuntime.queryLedgerState(context,
                                                                       partialProofData,
                                                                       [
                                                                        { dup: { n: 0 } },
                                                                        { idx: { cached: false,
                                                                                 pushPath: false,
                                                                                 path: [
                                                                                        { tag: 'value',
                                                                                          value: { value: _descriptor_1.toValue(3n),
                                                                                                   alignment: _descriptor_1.alignment() } }] } },
                                                                        { popeq: { cached: false,
                                                                                   result: undefined } }]).value);
    }
  };
}
const _emptyContext = {
  callContext: { currentQueryContext: new __compactRuntime.QueryContext(new __compactRuntime.ContractState().data, __compactRuntime.dummyContractAddress()), currentGasCost: __compactRuntime.emptyRunningCost() }
};
const _dummyContract = new Contract({ });
export const pureCircuits = {};
export const contractReferenceLocations =
  { tag: 'publicLedgerArray', indices: { } };
export const expectedVk = {};

//# sourceMappingURL=index.js.map
