(multiply, maxElements, maxMacs) => {
  'use strict';
  // Experiment-only. The declared support table follows the implemented CPU
  // kernels. Unsupported required data types/operators remain observable gaps.
  const key = {};
  const states = new WeakMap();
  const F32 = Float32Array;
  const maxRank = 5;
  const limits = () => {
    const types = (low, high, dataTypes = ['float32']) => ({ dataTypes, rankRange: { min: low, max: high } });
    const unavailable = [
      [
        'argMin argMax cast clamp cumulativeSum abs ceil cos erf exp floor identity log neg reciprocal roundEven sin sign sqrt tan elu expand gelu hardSigmoid hardSwish leakyRelu linear pad averagePool2d l2Pool2d maxPool2d reduceL1 reduceL2 reduceLogSum reduceLogSumExp reduceMax reduceMean reduceMin reduceProduct reduceSum reduceSumSquare resample2d reshape reverse sigmoid slice softmax softplus softsign tanh tile transpose triangular',
        'input output',
      ],
      ['batchNormalization', 'input mean variance scale bias output'],
      ['concat', 'inputs output'],
      ['conv2d convTranspose2d', 'input filter bias output'],
      [
        'sub mul div max min pow equal notEqual greater greaterOrEqual lesser lesserOrEqual logicalAnd logicalOr logicalXor',
        'a b output',
      ],
      ['logicalNot isNaN isInfinite', 'a output'],
      ['dequantizeLinear quantizeLinear', 'input scale zeroPoint output'],
      ['gather gatherElements gatherND', 'input indices output'],
      ['gru', 'input weight recurrentWeight bias recurrentBias initialHiddenState output0 output1'],
      ['gruCell', 'input weight recurrentWeight hiddenState bias recurrentBias output'],
      ['instanceNormalization layerNormalization', 'input scale bias output'],
      [
        'lstm',
        'input weight recurrentWeight bias recurrentBias peepholeWeight initialHiddenState initialCellState output0 output1 output2',
      ],
      [
        'lstmCell',
        'input weight recurrentWeight hiddenState cellState bias recurrentBias peepholeWeight output0 output1',
      ],
      ['prelu', 'input slope output'],
      ['scatterElements scatterND', 'input indices updates output'],
      ['split', 'input outputs'],
      ['where', 'condition trueValue falseValue output'],
    ];
    const all = {};
    for (const [operators, fields] of unavailable) {
      for (const operator of operators.split(' ')) {
        all[operator] = Object.fromEntries(fields.split(' ').map((name) => [name, types(0, 0, [])]));
      }
    }
    return {
      ...all,
      preferredInputLayout: 'nchw',
      maxTensorByteLength: maxElements * 4,
      input: types(0, maxRank),
      output: types(0, maxRank),
      constant: types(0, maxRank),
      relu: { input: types(0, maxRank), output: types(0, maxRank) },
      add: { a: types(0, maxRank), b: types(0, maxRank), output: types(0, maxRank) },
      matmul: { a: types(2, 2), b: types(2, 2), output: types(2, 2) },
      gemm: { a: types(2, 2), b: types(2, 2), c: types(0, 2), output: types(2, 2) },
    };
  };
  const invalid = (message) => new DOMException(message, 'InvalidStateError');
  const unsupported = (message) => new DOMException(message, 'NotSupportedError');
  function state(object, kind) {
    const s = states.get(object);
    if (!s || s.kind !== kind) throw new TypeError('Incorrect WebNN object');
    return s;
  }
  function own(object, token, kind, fields) {
    if (token !== key) throw new TypeError('Illegal constructor');
    states.set(object, { kind, ...fields });
  }
  function descriptor(d) {
    if (!d || d.dataType !== 'float32') throw new TypeError('Only float32 operands are supported');
    const dimensions = d.shape;
    if (dimensions == null || typeof dimensions[Symbol.iterator] !== 'function')
      throw new TypeError('Expected an iterable shape');
    const shape = Array.from(dimensions, (n) => {
      const x = Math.trunc(+n);
      if (!Number.isSafeInteger(x) || x <= 0 || x > 0xffffffff) throw new TypeError('Invalid dimension');
      return x;
    });
    const size = shape.reduce((a, b) => a * b, 1);
    if (shape.length > maxRank || size > maxElements) throw new TypeError('Unsupported tensor dimensions');
    return { shape: Object.freeze(shape), size };
  }
  const typed = Object.getPrototypeOf(F32.prototype);
  const getter = (proto, name) => Object.getOwnPropertyDescriptor(proto, name).get;
  const viewName = getter(typed, Symbol.toStringTag);
  const viewBuffer = getter(typed, 'buffer');
  const viewOffset = getter(typed, 'byteOffset');
  const viewLength = getter(typed, 'byteLength');
  const bufferLength = getter(ArrayBuffer.prototype, 'byteLength');
  const sharedLength =
    typeof SharedArrayBuffer === 'undefined' ? null : getter(SharedArrayBuffer.prototype, 'byteLength');
  function sourceBytes(v) {
    if (ArrayBuffer.isView(v)) {
      const name = viewName.call(v);
      if (name !== 'Float32Array' && name !== 'Uint8Array')
        throw new TypeError('Expected float32 or byte data');
      return new Uint8Array(viewBuffer.call(v), viewOffset.call(v), viewLength.call(v));
    }
    try {
      bufferLength.call(v);
    } catch {
      if (!sharedLength) throw new TypeError('Expected an ArrayBuffer or view');
      sharedLength.call(v);
    }
    return new Uint8Array(v);
  }
  function snapshot(v, size) {
    const bytes = sourceBytes(v);
    if (bytes.byteLength !== size * 4)
      throw new TypeError('Tensor data length does not match its descriptor');
    return new F32(bytes.slice().buffer);
  }
  const equal = (a, b) => a.length === b.length && a.every((n, i) => n === b[i]);
  function broadcast(a, b) {
    const rank = Math.max(a.length, b.length),
      shape = [];
    for (let i = 0; i < rank; i++) {
      const x = a[a.length - rank + i] ?? 1,
        y = b[b.length - rank + i] ?? 1;
      if (x !== y && x !== 1 && y !== 1) throw new TypeError('Incompatible broadcast dimensions');
      shape.push(Math.max(x, y));
    }
    return shape;
  }
  function indexIn(linear, target, shape) {
    let result = 0,
      stride = 1;
    for (let i = shape.length - 1, j = target.length - 1; i >= 0; --i, --j) {
      if (shape[i] !== 1) result += (linear % target[j]) * stride;
      linear = Math.floor(linear / target[j]);
      stride *= shape[i];
    }
    return result;
  }
  function usable(obj, kind) {
    const s = state(obj, kind);
    if (s.dead || (s.context && state(s.context, 'context').dead)) throw invalid(kind + ' is destroyed');
    return s;
  }
  function enqueue(ctx, work) {
    const s = usable(ctx, 'context');
    const promise = s.timeline.then(() => {
      usable(ctx, 'context');
      return work();
    });
    s.timeline = promise.catch(() => {});
    return promise;
  }
  // The context must release live device-side storage when lost. Weak
  // registrations allow independently unreachable resources to be collected.
  function registerResource(object, context) {
    const owner = state(context, 'context');
    const ref = new WeakRef(object);
    owner.resources.add(ref);
    owner.registry.register(object, ref, object);
    states.get(object).registration = ref;
  }
  function disposeResource(object) {
    const s = states.get(object);
    if (s.dead) return;
    s.dead = true;
    const owner = state(s.context, 'context');
    owner.registry.unregister(object);
    owner.resources.delete(s.registration);
    if (s.kind === 'tensor') {
      s.data = null;
      for (const reject of s.pending) reject(invalid('Tensor destroyed during a pending read'));
      s.pending.clear();
    } else {
      s.ops = null;
    }
  }
  function lose(context, message) {
    const owner = state(context, 'context');
    if (owner.dead) return;
    owner.dead = true;
    for (const ref of owner.resources) {
      const resource = ref.deref();
      if (resource) disposeResource(resource);
    }
    owner.resources.clear();
    owner.notify({ message });
  }
  class ML {
    constructor(token = undefined) {
      own(this, token, 'ml', {});
    }
    async createContext(options = {}) {
      state(this, 'ml');
      if (options != null && typeof options !== 'object' && typeof options !== 'function')
        throw new TypeError('Expected MLContextOptions');
      const requested = options?.accelerated;
      const accelerated = requested === undefined ? true : !!requested;
      const preference = options?.powerPreference;
      if (
        preference !== undefined &&
        !['default', 'low-power', 'high-performance'].includes(String(preference))
      )
        throw new TypeError('Unknown power preference');
      if (accelerated) throw unsupported('This experiment can create only unaccelerated CPU contexts');
      let notify;
      const lost = new Promise((resolve) => {
        notify = resolve;
      });
      const resources = new Set();
      const registry = new FinalizationRegistry((ref) => resources.delete(ref));
      return new MLContext(key, {
        lost,
        notify,
        dead: false,
        timeline: Promise.resolve(),
        resources,
        registry,
      });
    }
  }
  class MLContext {
    constructor(token = undefined, fields) {
      own(this, token, 'context', fields);
    }
    get accelerated() {
      state(this, 'context');
      return false;
    }
    get lost() {
      return state(this, 'context').lost;
    }
    opSupportLimits() {
      usable(this, 'context');
      return limits();
    }
    destroy() {
      lose(this, 'Context destroyed');
    }
    async createTensor(d) {
      usable(this, 'context');
      const desc = descriptor(d);
      return new MLTensor(key, {
        ...desc,
        context: this,
        pending: new Set(),
        data: new F32(desc.size),
        readable: !!d.readable,
        writable: !!d.writable,
        constant: false,
      });
    }
    async createConstantTensor(d, v) {
      usable(this, 'context');
      const desc = descriptor(d);
      return new MLTensor(key, {
        ...desc,
        context: this,
        pending: new Set(),
        data: snapshot(v, desc.size),
        readable: false,
        writable: false,
        constant: true,
      });
    }
    writeTensor(t, v) {
      const st = tensor(this, t);
      if (!st.writable) throw new TypeError('Tensor is not writable');
      const copy = snapshot(v, st.size),
        dest = st.data;
      enqueue(this, () => dest.set(copy));
    }
    async readTensor(t, dest) {
      const st = tensor(this, t);
      if (!st.readable) throw new TypeError('Tensor is not readable');
      const src = st.data;
      if (dest !== undefined && sourceBytes(dest).byteLength !== src.byteLength)
        throw new TypeError('Destination size mismatch');
      return new Promise((resolve, reject) => {
        st.pending.add(reject);
        enqueue(this, () => {
          if (!st.pending.delete(reject)) return;
          try {
            if (dest === undefined) resolve(src.slice().buffer);
            else {
              sourceBytes(dest).set(new Uint8Array(src.buffer));
              resolve();
            }
          } catch (e) {
            reject(e);
          }
        }).catch((e) => {
          st.pending.delete(reject);
          reject(e);
        });
      });
    }
    dispatch(graph, inputs, outputs) {
      usable(this, 'context');
      const g = usable(graph, 'graph');
      if (g.context !== this) throw new TypeError('Graph context mismatch');
      const bind = (expected, provided) => {
        const records = new Map(Object.entries(provided));
        if (records.size !== expected.length) throw new TypeError('Binding count mismatch');
        return expected.map(([name, slot]) => {
          const t = tensor(this, records.get(name));
          if (t.constant || !equal(t.shape, g.ops[slot].shape)) throw new TypeError('Binding shape mismatch');
          return t;
        });
      };
      const inStates = bind(g.inputs, inputs),
        outStates = bind(g.outputs, outputs);
      const bound = [...inStates, ...outStates];
      if (new Set(bound).size !== bound.length || outStates.some((t) => t.constant))
        throw new TypeError('Invalid output alias or constant output');
      const src = inStates.map((t) => t.data),
        dest = outStates.map((t) => t.data),
        ops = g.ops;
      enqueue(this, () => {
        const values = [];
        for (let i = 0; i < ops.length; i++) {
          const op = ops[i],
            args = op.parents.map((j) => values[j]);
          switch (op.type) {
            case 'input':
              values[i] = src[op.binding];
              break;
            case 'constant':
              values[i] = op.data;
              break;
            case 'relu':
              values[i] = args[0].map((n) => Math.max(0, n));
              break;
            case 'add': {
              const [l, r] = op.parents.map((j) => ops[j].shape);
              const value = new F32(op.size);
              for (let n = 0; n < value.length; n++)
                value[n] = args[0][indexIn(n, op.shape, l)] + args[1][indexIn(n, op.shape, r)];
              values[i] = value;
              break;
            }
            case 'matrix': {
              const value = new F32(multiply(args[0], args[1], op.m, op.n, op.k, op.at, op.bt));
              const c = op.parents.length === 3 ? ops[op.parents[2]].shape : null;
              for (let n = 0; n < value.length; n++)
                value[n] = op.alpha * value[n] + (c ? op.beta * args[2][indexIn(n, op.shape, c)] : 0);
              values[i] = value;
              break;
            }
          }
        }
        g.outputs.forEach(([, slot], i) => dest[i].set(values[slot]));
      }).catch((error) => {
        lose(this, String(error));
      });
    }
  }
  function tensor(ctx, t) {
    usable(ctx, 'context');
    const s = state(t, 'tensor');
    if (s.dead || s.context !== ctx) throw new TypeError('Tensor context mismatch');
    return s;
  }
  class MLTensor {
    constructor(token = undefined, fields) {
      own(this, token, 'tensor', fields);
      registerResource(this, fields.context);
    }
    get shape() {
      return state(this, 'tensor').shape;
    }
    get dataType() {
      state(this, 'tensor');
      return 'float32';
    }
    get writable() {
      return state(this, 'tensor').writable;
    }
    get readable() {
      return state(this, 'tensor').readable;
    }
    get constant() {
      return state(this, 'tensor').constant;
    }
    destroy() {
      state(this, 'tensor');
      disposeResource(this);
    }
  }
  class MLOperand {
    constructor(token = undefined, fields) {
      own(this, token, 'operand', fields);
    }
    get shape() {
      return state(this, 'operand').shape;
    }
    get dataType() {
      state(this, 'operand');
      return 'float32';
    }
  }
  class MLGraph {
    constructor(token = undefined, fields) {
      own(this, token, 'graph', fields);
      registerResource(this, fields.context);
    }
    destroy() {
      state(this, 'graph');
      disposeResource(this);
    }
  }
  function building(builder) {
    const s = state(builder, 'builder');
    usable(s.context, 'context');
    if (s.built) throw invalid('Graph builder was already used');
    return s;
  }
  function node(builder, d, fields) {
    building(builder);
    return new MLOperand(key, { ...descriptor(d), builder, parents: [], ...fields });
  }
  function operand(builder, v) {
    building(builder);
    const s = state(v, 'operand');
    if (s.builder !== builder) throw new TypeError('Operand belongs to another builder');
    return s;
  }
  function computed(builder, shape, type, parents, fields) {
    return node(builder, { dataType: 'float32', shape }, { type, parents, ...fields });
  }
  function matrix(builder, a, b, o) {
    const l = operand(builder, a),
      r = operand(builder, b);
    if (l.shape.length !== 2 || r.shape.length !== 2)
      throw new TypeError('Only rank-two matrices are supported');
    const at = !!o.aTranspose,
      bt = !!o.bTranspose;
    const m = l.shape[at ? 1 : 0],
      k = l.shape[at ? 0 : 1],
      n = r.shape[bt ? 0 : 1];
    if (k !== r.shape[bt ? 1 : 0]) throw new TypeError('Matrix inner dimensions differ');
    const parents = [a, b],
      shape = [m, n],
      c = o.c;
    if (c !== undefined) {
      if (!equal(broadcast(operand(builder, c).shape, shape), shape))
        throw new TypeError('Invalid bias shape');
      parents.push(c);
    }
    const alpha = o.alpha === undefined ? 1 : Math.fround(+o.alpha),
      beta = o.beta === undefined ? 1 : Math.fround(+o.beta);
    if (!Number.isFinite(alpha) || !Number.isFinite(beta))
      throw new TypeError('Expected finite GEMM coefficients');
    return computed(builder, shape, 'matrix', parents, { at, bt, m, k, n, alpha, beta });
  }
  class MLGraphBuilder {
    constructor(context) {
      usable(context, 'context');
      states.set(this, { kind: 'builder', context, built: false });
    }
    input(name, d) {
      name = String(name);
      if (!name) throw new TypeError('Input name is empty');
      return node(this, d, { type: 'input', name });
    }
    constant(d, v) {
      const builder = building(this);
      if (d instanceof MLTensor) {
        const t = tensor(builder.context, d);
        if (!t.constant) throw new TypeError('Expected a constant tensor');
        return computed(this, t.shape, 'constant', [], { data: t.data.slice(), constantTensor: d });
      }
      if (typeof d === 'string')
        return node(this, { dataType: d, shape: [] }, { type: 'constant', data: new F32([Number(v)]) });
      const desc = descriptor(d);
      return node(
        this,
        { dataType: 'float32', shape: desc.shape },
        { type: 'constant', data: snapshot(v, desc.size) },
      );
    }
    relu(x) {
      return computed(this, operand(this, x).shape, 'relu', [x]);
    }
    add(a, b) {
      return computed(this, broadcast(operand(this, a).shape, operand(this, b).shape), 'add', [a, b]);
    }
    matmul(a, b) {
      return matrix(this, a, b, {});
    }
    gemm(a, b, o = {}) {
      return matrix(this, a, b, o ?? {});
    }
    async build(outputs) {
      const s = building(this),
        entries = Object.entries(outputs);
      if (!entries.length) throw new TypeError('Expected graph outputs');
      const slots = new Map(),
        ops = [],
        inputs = [],
        names = new Set();
      let elements = 0,
        macs = 0;
      function visit(builder, value, depth = 0) {
        if (slots.has(value)) return slots.get(value);
        if (depth >= 128) throw unsupported('Graph exceeds CPU experiment depth');
        const op = operand(builder, value),
          parents = op.parents.map((x) => visit(builder, x, depth + 1));
        const slot = ops.length,
          copy = { ...op, parents };
        delete copy.builder;
        if (op.constantTensor) {
          tensor(s.context, op.constantTensor);
          delete copy.constantTensor;
        }
        if (op.type === 'input') {
          if (names.has(op.name)) throw new TypeError('Duplicate input name');
          names.add(op.name);
          copy.binding = inputs.length;
          inputs.push([op.name, slot]);
        }
        elements += op.size;
        if (op.type === 'matrix') macs += op.m * op.k * op.n;
        if (ops.length >= 128 || elements > 4 * maxElements || macs > maxMacs)
          throw unsupported('Graph exceeds CPU experiment budget');
        slots.set(value, slot);
        ops.push(copy);
        return slot;
      }
      const bindings = entries.map(([name, value]) => {
        if (!name) throw new TypeError('Output name is empty');
        const op = operand(this, value);
        if (op.type === 'input' || op.type === 'constant')
          throw new TypeError('Graph output must be an operator result');
        return [name, visit(this, value)];
      });
      s.built = true;
      return new MLGraph(key, { context: s.context, inputs, ops, outputs: bindings });
    }
  }
  const factory = new ML(key);
  const nav = Object.getPrototypeOf(navigator);
  Object.defineProperty(nav, 'ml', {
    configurable: true,
    enumerable: true,
    get() {
      return factory;
    },
  });
  for (const C of [ML, MLContext, MLGraphBuilder, MLOperand, MLGraph, MLTensor]) {
    Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
    Object.defineProperty(globalThis, C.name, { value: C, configurable: true, writable: true });
  }
};
