// Web Neural Network API (https://www.w3.org/TR/webnn/): `navigator.ml`, ML, MLContext, MLTensor,
// MLGraphBuilder, MLOperand, MLGraph. A real CPU implementation: graphs are built, validated and then
// executed eagerly on typed arrays when `MLContext.dispatch()` runs. Kernels compute in double precision and
// round once to the output data type. Every context is CPU-backed whatever `deviceType` asks for (the device
// type is a hint); `accelerated` is therefore false. Nothing here depends on WebGPU.
//
// ==========================================================================================================
// HOW THE FILES FIT TOGETHER
//
// webnn.rs evaluates this file (the factory below, called with `native`) and, as `native.families`, one
// function per file listed in FAMILIES in webnn.rs, in that order: core_ops.js first, then one file per
// operator family (elementwise, activations, reduce, conv, norm, recurrent, tensor_ops, linear). Each family
// file is ONE EXPRESSION that evaluates to `(api) => { ... }`, for example
//
//     (api) => {
//       const { defineOp, tl, FLOAT_TYPES } = api;
//       defineOp('relu', { ... });
//     }
//
// The factory calls every family function with the same `api` object (documented below), in order, before
// the module is returned, and installs one method on `MLGraphBuilder.prototype` for every registered op. An
// op that is not registered has no method (and no `opSupportLimits()` entry). Defining a name again replaces
// the earlier definition (the last one wins), so a family file simply defines its ops; core_ops.js holds
// throw-away copies of add/sub/mul/identity that the elementwise and tensor_ops families override.
// Family files never edit core.js or webnn.rs; they may only use what `api` hands them. webnn.rs pastes the
// files into one script (`[file1, file2, ...]`, then this factory), so a family file must be exactly one
// expression: no `'use strict'`, no `export`, and NO trailing semicolon after the closing brace.
//
// ==========================================================================================================
// THE REGISTRY: api.defineOp(name, spec)
//
//   spec.args     Array of strings, the IDL arguments of the builder method in order, each `'name:type'`,
//                 or `'name?:type'` for an optional one (it stays `undefined` when absent), and finally the
//                 literal string `'options'` for a trailing optional dictionary (whose members `spec.options`
//                 describes; `label` is always present). Types: see TYPES below. Example for
//                 `argMax(input, axis, options)`:  args: ['input:operand', 'axis:ul', 'options'].
//                 Too few arguments throw TypeError (as WebIDL does) and so does every failed conversion.
//   spec.options  { member: type | {type, default, required} } for the dictionary. A type string is enough
//                 for members without default; members are converted in alphabetical order, `label` first.
//                 Absent members without default are `undefined` in the object the hooks receive.
//   spec.limits   The `opSupportLimits()[name]` entry, e.g. {a: tl(FLOAT_TYPES, 0, 8), b: ..., output: ...}.
//                 Key names are the IDL names of the support-limits dictionary and MUST be the names of the
//                 arguments / option members they describe (`input`, `a`, `b`, `scale`, `inputs`...): the core
//                 checks every operand (or every operand of a sequence) it finds under such a name against the
//                 entry (data type in `dataTypes`, rank in `rankRange`, else TypeError) before `validate`
//                 runs, so the limits can never disagree with the builder. The key `output` (or `outputs`)
//                 is checked against every output descriptor `validate` returns. Set `autoLimits: false` to
//                 do the check yourself at a point of your choosing with `ctx.checkLimits()`.
//   spec.validate(ctx, ...args)
//                 The ONLY place that infers output shapes and checks arguments. `args` are the converted
//                 arguments in order (the dictionary, if any, last; never undefined: defaults applied). An
//                 operand argument is an internal operand object with read-only `dataType`, `shape` (a frozen
//                 array, the same one the public MLOperand shows) and `rank`. Operands of this builder only
//                 (the core has checked that). Return the output descriptor `{dataType, shape}` (shape: a
//                 plain array of positive integers), or an array of descriptors for an op with several
//                 outputs (the builder method then returns an array of operands), or `{outputs, attrs}`
//                 where `attrs` is any value the kernel will get back as `rt.attrs` (normalised axes,
//                 resolved paddings...). Fail with `ctx.fail(message)`: it throws the TypeError whose message
//                 carries the `[label]` prefix the tests look for. (A bare TypeError thrown here gets the
//                 prefix added.) After you return, the core rejects an output whose byte length exceeds
//                 `maxTensorByteLength` or whose data type or rank is outside `limits.output`.
//   spec.run(args, rt)
//                 The kernel. `args` mirrors the converted arguments exactly, but each operand is replaced by
//                 its runtime tensor `{dataType, shape, data}` where `data` is a typed array of the data
//                 type's class (api.typedArrayFor(dataType): Float32Array, Float16Array, Int32Array,
//                 Uint32Array, Int8Array, Uint8Array, BigInt64Array, BigUint64Array) with
//                 `data.length === sizeOf(shape)`, row-major. NEVER write into an input's `data`.
//                 `rt = {op, attrs, outputs}`: `outputs[i] = {dataType, shape}` is the i-th output descriptor.
//                 Return a freshly allocated typed array of the output type and length (or an array of them
//                 for several outputs); returning an input's own array (identity, reshape) is allowed, the
//                 core copies when that would alias a graph input or constant. Numbers are doubles, 64-bit
//                 types are BigInts. Round once to the output type: assigning into the typed array does it
//                 (Math.fround for float32, round-to-nearest-even for Float16Array, wrap-around for ints),
//                 or use api.roundTo(dataType).
//
// api (everything a family may use):
//   defineOp                         as above.
//   tl(dataTypes, min = 0, max = 8)  builds {dataTypes: [...], rankRange: {min, max}} for `limits`.
//   ALL_TYPES, FLOAT_TYPES, SIGNED_TYPES, INTEGER_TYPES, MAX_RANK   the data types this engine implements
//                                    (float16 is left out of every list when Float16Array does not exist).
//   sizeOf(shape) bytesPer(dataType) typedArrayFor(dataType) isFloat(dt) isBigInt(dt) alloc(dataType, n)
//   stridesOf(shape)                 row-major strides.
//   broadcastShapes(a, b)            numpy/WebNN bidirectional broadcast; null when not broadcastable.
//   broadcastStrides(shape, outShape) strides of `shape` read as `outShape` (0 on broadcast axes).
//   mapBinary(a, b, outShape, outType, fn)   broadcasting element-wise loop over two tensors.
//   roundTo(dataType)                (x) => x rounded/wrapped to the type (BigInt for 64-bit types).
//   fail(label, message)             RETURNS the `[label]`-prefixed TypeError (the caller throws it), for code without a ctx.
//   domError(name, message)          a DOMException (InvalidStateError, NotSupportedError...).
//   conv                             the IDL converters by type name (see TYPES), for validate() code that
//                                    needs to convert something the declarative signature cannot express.
//   maxTensorByteLength              2 ** 31.
// ctx (first argument of validate): {op, label, builder, fail(msg), checkLimits(), limits, maxTensorByteLength,
//   context}. `ctx.fail` never returns.
//
// TYPES for args / options: 'operand' (an MLOperand of this builder), 'ul' ([EnforceRange] unsigned long),
//   'long' ([EnforceRange] long), 'double' (finite), 'udouble' (unrestricted double), 'float', 'ufloat',
//   'boolean', 'string' (DOMString), 'usv' (USVString), 'dtype' (MLOperandDataType), 'mlnumber' (MLNumber:
//   a BigInt stays a BigInt, anything else becomes a double), 'enum:a|b|c', and 'seq<T>' of any of these
//   (an array of converted values; 'seq<operand>' yields an array of operands).
//
// Errors: argument problems are TypeError (ctx.fail); lifecycle problems are DOMExceptions made by the core
// (a destroyed context, an already built builder: InvalidStateError). Kernels never throw for valid graphs;
// an exception out of a kernel is an engine failure and loses the context (spec: execution failure).
// ==========================================================================================================
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const A = Array;
  const PromiseC = Promise;
  const TE = TypeError;
  const DOMExceptionC = DOMException;
  const isArray = A.isArray;
  const defProp = O.defineProperty;
  const NumberC = Number;
  const MathC = Math;
  const WM = WeakMap;
  const MAX_RANK = 8;
  const MAX_TENSOR_BYTES = 2 ** 31;
  const HAS_F16 = typeof Float16Array === 'function';

  // ---- data types ------------------------------------------------------------------------------------
  const ARRAYS = {
    float32: Float32Array,
    int32: Int32Array,
    uint32: Uint32Array,
    int64: BigInt64Array,
    uint64: BigUint64Array,
    int8: Int8Array,
    uint8: Uint8Array,
  };
  const BYTES = { float32: 4, float16: 2, int32: 4, uint32: 4, int64: 8, uint64: 8, int8: 1, uint8: 1 };
  const ARRAY_NAMES = {
    float32: 'Float32Array', float16: 'Float16Array', int32: 'Int32Array', uint32: 'Uint32Array',
    int64: 'BigInt64Array', uint64: 'BigUint64Array', int8: 'Int8Array', uint8: 'Uint8Array',
  };
  if (HAS_F16) ARRAYS.float16 = Float16Array;
  const ALL_TYPES = O.freeze(['float32'].concat(HAS_F16 ? ['float16'] : [], ['int32', 'uint32', 'int64', 'uint64', 'int8', 'uint8']));
  const FLOAT_TYPES = O.freeze(HAS_F16 ? ['float32', 'float16'] : ['float32']);
  const SIGNED_TYPES = O.freeze(['int32', 'int64', 'int8']);
  const INTEGER_TYPES = O.freeze(['int32', 'uint32', 'int64', 'uint64', 'int8', 'uint8']);
  // The MLOperandDataType enum as the IDL (and Chrome) know it; what a context really supports is the limits' job.
  const ENUM_TYPES = ['float32', 'float16', 'int32', 'uint32', 'int64', 'uint64', 'int8', 'uint8', 'int4', 'uint4', 'bfloat16', 'float8e4m3'];

  const bytesPer = (dt) => BYTES[dt];
  const typedArrayFor = (dt) => ARRAYS[dt];
  const isFloat = (dt) => dt === 'float32' || dt === 'float16';
  const isBigInt = (dt) => dt === 'int64' || dt === 'uint64';
  const alloc = (dt, n) => new ARRAYS[dt](n);
  const sizeOf = (shape) => {
    let n = 1;
    for (let i = 0; i < shape.length; i++) n *= shape[i];
    return n;
  };
  const stridesOf = (shape) => {
    const s = new A(shape.length);
    let acc = 1;
    for (let i = shape.length - 1; i >= 0; i--) {
      s[i] = acc;
      acc *= shape[i];
    }
    return s;
  };
  const f16scratch = HAS_F16 ? new Float16Array(1) : null;
  const roundTo = (dt) => {
    switch (dt) {
      case 'float32': return MathC.fround;
      case 'float16': return (x) => { f16scratch[0] = x; return f16scratch[0]; };
      case 'int32': return (x) => x | 0;
      case 'uint32': return (x) => x >>> 0;
      case 'int8': return (x) => (x << 24) >> 24;
      case 'uint8': return (x) => x & 255;
      case 'int64': return (x) => BigInt.asIntN(64, typeof x === 'bigint' ? x : BigInt(MathC.trunc(x)));
      case 'uint64': return (x) => BigInt.asUintN(64, typeof x === 'bigint' ? x : BigInt(MathC.trunc(x)));
    }
    throw new TE(`unsupported data type ${dt}`);
  };
  const tl = (dataTypes, min = 0, max = MAX_RANK) => ({ dataTypes: A.from(dataTypes), rankRange: { min, max } });

  // ---- shapes ----------------------------------------------------------------------------------------
  const broadcastShapes = (a, b) => {
    const rank = MathC.max(a.length, b.length);
    const out = new A(rank);
    for (let i = 0; i < rank; i++) {
      const x = i < rank - a.length ? 1 : a[i - (rank - a.length)];
      const y = i < rank - b.length ? 1 : b[i - (rank - b.length)];
      if (x !== y && x !== 1 && y !== 1) return null;
      out[i] = x === 1 ? y : x;
    }
    return out;
  };
  const broadcastStrides = (shape, outShape) => {
    const own = stridesOf(shape);
    const out = new A(outShape.length).fill(0);
    const off = outShape.length - shape.length;
    for (let i = 0; i < shape.length; i++) out[i + off] = shape[i] === 1 && outShape[i + off] !== 1 ? 0 : own[i];
    return out;
  };
  const sameShape = (a, b) => {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    return true;
  };
  // Element-wise `fn(x, y)` over two tensors broadcast to `outShape`, stored (so rounded) into a new array.
  const mapBinary = (a, b, outShape, outType, fn) => {
    const n = sizeOf(outShape);
    const out = new ARRAYS[outType](n);
    const ad = a.data;
    const bd = b.data;
    if (sameShape(a.shape, b.shape)) {
      for (let i = 0; i < n; i++) out[i] = fn(ad[i], bd[i]);
    } else if (bd.length === 1 && sameShape(a.shape, outShape)) {
      const y = bd[0];
      for (let i = 0; i < n; i++) out[i] = fn(ad[i], y);
    } else if (ad.length === 1 && sameShape(b.shape, outShape)) {
      const x = ad[0];
      for (let i = 0; i < n; i++) out[i] = fn(x, bd[i]);
    } else {
      const sa = broadcastStrides(a.shape, outShape);
      const sb = broadcastStrides(b.shape, outShape);
      const rank = outShape.length;
      const idx = new A(rank).fill(0);
      let ia = 0;
      let ib = 0;
      for (let i = 0; i < n; i++) {
        out[i] = fn(ad[ia], bd[ib]);
        for (let d = rank - 1; d >= 0; d--) {
          idx[d]++;
          ia += sa[d];
          ib += sb[d];
          if (idx[d] < outShape[d]) break;
          ia -= sa[d] * outShape[d];
          ib -= sb[d] * outShape[d];
          idx[d] = 0;
        }
      }
    }
    return out;
  };

  // ---- errors ----------------------------------------------------------------------------------------
  const fail = (label, message) => new TE(label ? `[${label}] ${message}` : message);
  const domError = (name, message) => new DOMExceptionC(message, name);
  const illegal = (what) => new TE(`Illegal invocation${what ? ` (${what})` : ''}`);
  const toWellFormed = (s) => (typeof s.toWellFormed === 'function' ? s.toWellFormed() : s.replace(/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g, '�'));
  const isObject = (v) => (typeof v === 'object' && v !== null) || typeof v === 'function';

  // ---- WebIDL conversions ----------------------------------------------------------------------------
  const operandNodes = new WM(); // MLOperand -> Node
  const enforceRange = (lo, hi, name) => (v) => {
    let x = +v;
    if (!NumberC.isFinite(x)) throw new TE(`The value is not a finite number (${name})`);
    x = MathC.trunc(x) + 0;
    if (x < lo || x > hi) throw new TE(`The value ${x} is outside the range [${lo}, ${hi}] (${name})`);
    return x;
  };
  const toDomString = (v) => {
    if (typeof v === 'symbol') throw new TE('Cannot convert a Symbol to a string');
    return String(v);
  };
  const enumConv = (values) => (v) => {
    const s = toDomString(v);
    if (!values.includes(s)) throw new TE(`The provided value '${s}' is not a valid enum value`);
    return s;
  };
  const seqOf = (v, inner) => {
    if (!isObject(v) || typeof v[Symbol.iterator] !== 'function') throw new TE('The provided value cannot be converted to a sequence');
    const out = [];
    for (const x of v) out.push(inner(x));
    return out;
  };
  const dtypeConv = enumConv(ENUM_TYPES);
  const convs = {
    boolean: (v) => !!v,
    string: toDomString,
    usv: (v) => toWellFormed(toDomString(v)),
    ul: enforceRange(0, 2 ** 32 - 1, 'unsigned long'),
    long: enforceRange(-(2 ** 31), 2 ** 31 - 1, 'long'),
    udouble: (v) => +v,
    double: (v) => {
      const x = +v;
      if (!NumberC.isFinite(x)) throw new TE('The provided double value is non-finite');
      return x;
    },
    ufloat: (v) => MathC.fround(+v),
    float: (v) => {
      const x = MathC.fround(+v);
      if (!NumberC.isFinite(x)) throw new TE('The provided float value is non-finite');
      return x;
    },
    mlnumber: (v) => (typeof v === 'bigint' ? v : +v),
    dtype: dtypeConv,
    operand: (v) => {
      const node = isObject(v) ? operandNodes.get(v) : undefined;
      if (!node) throw new TE("The provided value is not of type 'MLOperand'");
      return node;
    },
  };
  const typeCache = new Map();
  const typeConv = (t) => {
    let c = typeCache.get(t);
    if (c) return c;
    if (t.startsWith('seq<') && t.endsWith('>')) {
      const inner = typeConv(t.slice(4, -1));
      c = (v) => seqOf(v, inner);
    } else if (t.startsWith('enum:')) {
      c = enumConv(t.slice(5).split('|'));
    } else {
      c = convs[t];
    }
    if (!c) throw new Error(`webnn: unknown argument type '${t}'`);
    typeCache.set(t, c);
    return c;
  };
  const parseMembers = (members) => {
    const names = O.keys(members).filter((k) => k !== 'label').sort();
    const list = [{ name: 'label', conv: convs.usv, required: false, hasDefault: true, def: '' }];
    for (const name of names) {
      const m = members[name];
      const o = typeof m === 'string' ? { type: m } : m;
      list.push({ name, conv: typeConv(o.type), required: !!o.required, hasDefault: 'default' in o, def: o.default });
    }
    return list;
  };
  const convertDictionary = (v, members) => {
    if (v !== undefined && v !== null && !isObject(v)) throw new TE('The provided value is not of type dictionary');
    const out = {};
    for (const m of members) {
      const raw = v === undefined || v === null ? undefined : v[m.name];
      if (raw === undefined) {
        if (m.required) throw new TE(`Required member '${m.name}' is undefined`);
        if (m.hasDefault) out[m.name] = isArray(m.def) ? m.def.slice() : m.def;
      } else {
        out[m.name] = m.conv(raw);
      }
    }
    return out;
  };
  const DESCRIPTOR = parseMembers({ dataType: { type: 'dtype', required: true }, shape: { type: 'seq<ul>', required: true } });
  const TENSOR_DESCRIPTOR = parseMembers({
    dataType: { type: 'dtype', required: true }, shape: { type: 'seq<ul>', required: true },
    readable: { type: 'boolean', default: false }, writable: { type: 'boolean', default: false },
  });
  // Dictionary members convert in lexicographical order, ancestors first; for these two that is dataType, shape,
  // then readable, writable; `label` of parseMembers is not part of them, so drop it.
  const descriptorMembers = DESCRIPTOR.slice(1);
  const tensorDescriptorMembers = TENSOR_DESCRIPTOR.slice(1);
  const convertRecord = (v, inner) => {
    if (!isObject(v)) throw new TE('The provided value is not of type record');
    const out = new Map();
    for (const key of O.keys(v)) out.set(toWellFormed(key), inner(v[key]));
    return out;
  };

  // ---- buffer sources --------------------------------------------------------------------------------
  const abByteLength = O.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
  const abDetached = O.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'detached')?.get;
  const sabByteLength = typeof SharedArrayBuffer === 'function' ? O.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, 'byteLength').get : null;
  const taProto = O.getPrototypeOf(Int8Array.prototype);
  const taTag = O.getOwnPropertyDescriptor(taProto, Symbol.toStringTag).get;
  const taBuffer = O.getOwnPropertyDescriptor(taProto, 'buffer').get;
  const taOffset = O.getOwnPropertyDescriptor(taProto, 'byteOffset').get;
  const taLength = O.getOwnPropertyDescriptor(taProto, 'byteLength').get;
  const dvBuffer = O.getOwnPropertyDescriptor(DataView.prototype, 'buffer').get;
  const dvOffset = O.getOwnPropertyDescriptor(DataView.prototype, 'byteOffset').get;
  const dvLength = O.getOwnPropertyDescriptor(DataView.prototype, 'byteLength').get;
  const tryCall = (f, v) => {
    try {
      return f.call(v);
    } catch {
      return undefined;
    }
  };
  const emptyBytes = new Uint8Array(0);
  const isDetached = (buffer) => (abDetached ? tryCall(abDetached, buffer) === true : false);
  // AllowSharedBufferSource -> {bytes: Uint8Array over the same memory, detached, kind: typed array name | null}.
  // A detached buffer (or a view of one) has no bytes, as WebIDL's "get a copy of the buffer source" sees it.
  const bufferSource = (v) => {
    if (isObject(v)) {
      if (tryCall(abByteLength, v) !== undefined) {
        const detached = isDetached(v);
        return { bytes: detached ? emptyBytes : new Uint8Array(v), detached, kind: null };
      }
      if (sabByteLength && tryCall(sabByteLength, v) !== undefined) return { bytes: new Uint8Array(v), detached: false, kind: null };
      const tag = tryCall(taTag, v);
      if (tag !== undefined) {
        const buffer = taBuffer.call(v);
        const detached = isDetached(buffer);
        return { bytes: detached ? emptyBytes : new Uint8Array(buffer, taOffset.call(v), taLength.call(v)), detached, kind: tag };
      }
      if (tryCall(dvLength, v) !== undefined) {
        const buffer = dvBuffer.call(v);
        return { bytes: new Uint8Array(buffer, dvOffset.call(v), dvLength.call(v)), detached: false, kind: null };
      }
    }
    throw new TE('The provided value is not of type AllowSharedBufferSource');
  };
  const copyBytesInto = (data, bytes) => new Uint8Array(data.buffer, data.byteOffset, data.byteLength).set(bytes);

  // ---- global limits ---------------------------------------------------------------------------------
  const GLOBAL_LIMITS = { input: tl(ALL_TYPES), constant: tl(ALL_TYPES), output: tl(ALL_TYPES) };
  const checkTensorLimit = (failFn, key, dataType, rank, lim) => {
    if (!lim.dataTypes.includes(dataType)) failFn(`Unsupported data type '${dataType}' for ${key}, must be one of [${lim.dataTypes.join(', ')}]`);
    if (rank < lim.rankRange.min || rank > lim.rankRange.max) failFn(`Unsupported rank ${rank} for ${key}, must be between ${lim.rankRange.min} and ${lim.rankRange.max}`);
  };
  // Descriptor checks shared by input(), constant(), createTensor(), createConstantTensor().
  const checkDescriptor = (failFn, key, desc, lim) => {
    for (const d of desc.shape) if (d === 0) failFn('The shape must not contain a zero dimension');
    if (sizeOf(desc.shape) * (BYTES[desc.dataType] ?? 1) > MAX_TENSOR_BYTES) failFn(`The tensor byte length exceeds the limit ${MAX_TENSOR_BYTES}`);
    checkTensorLimit(failFn, key, desc.dataType, desc.shape.length, lim);
  };
  const freezeShape = (shape) => O.freeze(shape.slice());
  const plainFail = (message) => {
    throw new TE(message);
  };

  // ---- graph model -----------------------------------------------------------------------------------
  // Node: one operand of a builder. kind 'input' | 'constant' | 'op'.
  class Node {
    constructor(builder, kind, dataType, shape) {
      this.builder = builder;
      this.kind = kind;
      this.dataType = dataType;
      this.shape = shape;
      this.call = null;
      this.name = null;
      this.tensor = null;
      this.operand = null;
    }
    get rank() {
      return this.shape.length;
    }
  }
  const operandOf = (node) => {
    if (!node.operand) {
      node.operand = new MLOperand(token);
      operandNodes.set(node.operand, node);
    }
    return node.operand;
  };
  const collectOperands = (v, out) => {
    if (v instanceof Node) out.push(v);
    else if (isArray(v)) for (const x of v) collectOperands(x, out);
    else if (v !== null && typeof v === 'object' && O.getPrototypeOf(v) === O.prototype) for (const k of O.keys(v)) collectOperands(v[k], out);
    return out;
  };
  const substitute = (v, values) => {
    if (v instanceof Node) return values.get(v);
    if (isArray(v)) return v.map((x) => substitute(x, values));
    if (v !== null && typeof v === 'object' && O.getPrototypeOf(v) === O.prototype) {
      const r = {};
      for (const k of O.keys(v)) r[k] = substitute(v[k], values);
      return r;
    }
    return v;
  };

  // ---- registry --------------------------------------------------------------------------------------
  const ops = new Map();
  const parseSignature = (name, spec) => {
    const sig = [];
    let required = 0;
    for (const entry of spec.args ?? []) {
      if (entry === 'options') {
        sig.push({ name: 'options', options: true });
        continue;
      }
      const m = /^(\w+)(\?)?:(.+)$/.exec(entry);
      if (!m) throw new Error(`webnn: bad argument '${entry}' of ${name}`);
      const optional = !!m[2];
      if (!optional) required++;
      sig.push({ name: m[1], optional, conv: typeConv(m[3]) });
    }
    return { sig, required, members: sig.some((s) => s.options) ? parseMembers(spec.options ?? {}) : null };
  };
  const defineOp = (name, spec) => {
    if (typeof name !== 'string' || !spec || typeof spec.validate !== 'function' || typeof spec.run !== 'function' || !spec.limits) {
      throw new Error(`webnn: bad op definition '${name}': need limits, validate and run`);
    }
    ops.set(name, { name, spec, ...parseSignature(name, spec) });
  };
  const cloneLimits = (v) => JSON.parse(JSON.stringify(v));

  // Builder state
  const builders = new WM(); // MLGraphBuilder -> state
  const contexts = new WM(); // MLContext -> state
  const tensors = new WM(); // MLTensor -> state
  const graphs = new WM(); // MLGraph -> state

  const checkOutputDescriptor = (ctx, desc, limit) => {
    if (!desc || !ENUM_TYPES.includes(desc.dataType) || !isArray(desc.shape)) throw new Error(`webnn: ${ctx.op}: validate returned a bad output descriptor`);
    for (const d of desc.shape) if (!NumberC.isInteger(d) || d < 1 || d > 2 ** 32 - 1) ctx.fail(`The output shape [${desc.shape}] is not valid`);
    if (sizeOf(desc.shape) * BYTES[desc.dataType] > MAX_TENSOR_BYTES) ctx.fail(`The output tensor byte length exceeds the limit ${MAX_TENSOR_BYTES}`);
    if (limit) checkTensorLimit(ctx.fail, 'output', desc.dataType, desc.shape.length, limit);
  };

  const invokeOp = (def, bs, methodThis, args) => {
    const { spec } = def;
    if (args.length < def.required) throw new TE(`${def.required} argument${def.required === 1 ? '' : 's'} required, but only ${args.length} present`);
    const converted = [];
    const named = {};
    for (let i = 0; i < def.sig.length; i++) {
      const s = def.sig[i];
      if (s.options) {
        const dict = convertDictionary(args[i], def.members);
        converted.push(dict);
        for (const k of O.keys(dict)) named[k] = dict[k];
      } else {
        const raw = args[i];
        const v = raw === undefined && s.optional ? undefined : s.conv(raw);
        converted.push(v);
        named[s.name] = v;
      }
    }
    const label = def.members ? converted[def.sig.findIndex((s) => s.options)].label : '';
    if (bs.hasBuilt) throw domError('InvalidStateError', 'The graph builder has already built a graph');
    if (bs.context.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
    const failFn = (message) => {
      throw fail(label, message);
    };
    const operands = collectOperands(converted, []);
    for (const node of operands) if (node.builder !== bs) failFn('The operand belongs to another MLGraphBuilder');
    const limits = spec.limits;
    const checkLimits = () => {
      for (const key of O.keys(limits)) {
        if (key === 'output' || key === 'outputs') continue;
        const v = named[key];
        if (v === undefined) continue;
        for (const node of collectOperands(v, [])) checkTensorLimit(failFn, key, node.dataType, node.rank, limits[key]);
      }
    };
    const ctx = { op: def.name, label, builder: methodThis, context: bs.contextObject, limits, maxTensorByteLength: MAX_TENSOR_BYTES, fail: failFn, checkLimits };
    if (spec.autoLimits !== false) checkLimits();
    let result;
    try {
      result = spec.validate(ctx, ...converted);
    } catch (e) {
      if (e instanceof TE && label && !e.message.includes(`[${label}]`)) throw fail(label, e.message);
      throw e;
    }
    let outs = result;
    let attrs;
    if (result && !isArray(result) && 'outputs' in result) {
      outs = result.outputs;
      attrs = result.attrs;
    }
    const multi = isArray(outs);
    const descs = multi ? outs : [outs];
    const outLimit = limits.output ?? limits.outputs;
    for (const d of descs) checkOutputDescriptor(ctx, d, outLimit);
    const call = { def, args: converted, attrs, inputs: [...new Set(operands)], outputs: [], rt: null };
    for (let i = 0; i < descs.length; i++) {
      const node = new Node(bs, 'op', descs[i].dataType, freezeShape(descs[i].shape));
      node.call = call;
      call.outputs.push(node);
    }
    call.rt = { op: def.name, attrs, outputs: call.outputs.map((n) => ({ dataType: n.dataType, shape: n.shape })) };
    const made = call.outputs.map(operandOf);
    return multi ? made : made[0];
  };

  const installOps = (proto) => {
    for (const def of ops.values()) {
      const { name } = def;
      const fn = {
        [name](...args) {
          const bs = isObject(this) ? builders.get(this) : undefined;
          if (!bs) throw illegal(name);
          return invokeOp(def, bs, this, args);
        },
      }[name];
      defProp(fn, 'length', { value: def.required, configurable: true });
      defProp(proto, name, { value: fn, writable: true, enumerable: true, configurable: true });
    }
  };

  // ---- tensors, execution ----------------------------------------------------------------------------
  const newStorage = (dt, shape) => new ARRAYS[dt](sizeOf(shape));
  const asyncOp = (f) => {
    try {
      return PromiseC.resolve(f());
    } catch (e) {
      return PromiseC.reject(e);
    }
  };
  const contextState = (v, what) => {
    const s = isObject(v) ? contexts.get(v) : undefined;
    if (!s) throw illegal(what);
    return s;
  };
  const tensorState = (v) => {
    const s = isObject(v) ? tensors.get(v) : undefined;
    if (!s) throw new TE("The provided value is not of type 'MLTensor'");
    return s;
  };
  const makeTensor = (cs, desc, flags, data) => {
    const t = new MLTensor(token);
    tensors.set(t, {
      context: cs, dataType: desc.dataType, shape: freezeShape(desc.shape), readable: !!flags.readable, writable: !!flags.writable,
      constant: !!flags.constant, data, destroyed: false, pending: new Set(), object: t,
    });
    return t;
  };
  const cancelRead = (rec) => {
    if (rec.settled) return;
    rec.settled = true;
    rec.tensor.pending.delete(rec);
    rec.tensor.context.pending.delete(rec);
    rec.reject(domError('InvalidStateError', 'The tensor was destroyed while it was being read'));
  };
  const finishRead = (rec) => {
    if (rec.settled) return;
    rec.settled = true;
    rec.tensor.pending.delete(rec);
    rec.tensor.context.pending.delete(rec);
    if (!rec.target) {
      rec.resolve(rec.snapshot);
      return;
    }
    try {
      const src = bufferSource(rec.target);
      if (src.detached || src.bytes.byteLength < rec.snapshot.byteLength) throw new TE('The output buffer was detached or is too small');
      src.bytes.set(new Uint8Array(rec.snapshot));
      rec.resolve(undefined);
    } catch (e) {
      rec.reject(e);
    }
  };
  const destroyTensor = (ts) => {
    if (ts.destroyed) return;
    ts.destroyed = true;
    ts.data = null;
    for (const rec of A.from(ts.pending)) cancelRead(rec);
  };
  const loseContext = (cs, message) => {
    if (cs.destroyed) return;
    cs.destroyed = true;
    for (const rec of A.from(cs.pending)) cancelRead(rec);
    cs.resolveLost({ message });
  };

  const executeGraph = (gs, inputStates, outputStates) => {
    const values = new Map();
    const persistent = new Set(); // buffers a result must not alias: graph inputs and constants
    for (const [name, node] of gs.inputs) {
      const ts = inputStates.get(name);
      values.set(node, { dataType: node.dataType, shape: node.shape, data: ts.data });
      persistent.add(ts.data.buffer);
    }
    for (const node of gs.constants) {
      values.set(node, node.tensor);
      persistent.add(node.tensor.data.buffer);
    }
    const uses = new Map(gs.uses);
    for (const call of gs.order) {
      const args = call.hasOperands ? substitute(call.args, values) : call.args;
      let res = call.def.spec.run(args, call.rt);
      if (call.outputs.length === 1 && !isArray(res)) res = [res];
      if (!isArray(res) || res.length !== call.outputs.length) throw new Error(`${call.def.name}: the kernel returned the wrong number of outputs`);
      for (let i = 0; i < res.length; i++) {
        const node = call.outputs[i];
        let data = res[i];
        if (data && !ArrayBuffer.isView(data) && data.data) data = data.data;
        if (!(data instanceof ARRAYS[node.dataType]) || data.length !== sizeOf(node.shape)) {
          throw new Error(`${call.def.name}: the kernel returned a bad ${node.dataType} result`);
        }
        values.set(node, { dataType: node.dataType, shape: node.shape, data });
      }
      for (const node of call.inputs) {
        const left = uses.get(node) - 1;
        uses.set(node, left);
        if (left === 0 && node.kind === 'op') values.delete(node);
      }
    }
    // Everything computed: now (and only now) write the outputs.
    const adopted = new Set();
    for (const [name, node] of gs.outputs) {
      const ts = outputStates.get(name);
      let data = values.get(node).data;
      if (persistent.has(data.buffer) || adopted.has(data.buffer) || data.byteOffset !== 0 || data.buffer.byteLength !== data.byteLength) {
        const copy = new ARRAYS[node.dataType](data.length);
        copy.set(data);
        data = copy;
      }
      adopted.add(data.buffer);
      ts.data = data;
    }
  };

  const buildGraph = (bs, outputs) => {
    const inputs = new Map();
    const constants = [];
    const order = [];
    const uses = new Map();
    const seenCalls = new Set();
    const seenLeaves = new Set();
    const bump = (node) => uses.set(node, (uses.get(node) ?? 0) + 1);
    for (const root of outputs.values()) {
      const stack = [[root, false]];
      while (stack.length) {
        const [node, expanded] = stack.pop();
        if (node.kind !== 'op') {
          if (seenLeaves.has(node)) continue;
          seenLeaves.add(node);
          if (node.kind === 'input') {
            // Input names must be unique among the inputs the graph uses (checked at build, as in Chrome).
            if (inputs.has(node.name)) throw new TE(`The input name '${node.name}' is used more than once`);
            inputs.set(node.name, node);
          }
          else constants.push(node);
          continue;
        }
        const call = node.call;
        if (seenCalls.has(call)) continue;
        if (expanded) {
          seenCalls.add(call);
          order.push(call);
          for (const input of call.inputs) bump(input);
          continue;
        }
        stack.push([node, true]);
        for (const input of call.inputs) if (input.kind !== 'op' || !seenCalls.has(input.call)) stack.push([input, false]);
      }
    }
    for (const call of order) call.hasOperands = call.inputs.length > 0;
    for (const node of outputs.values()) bump(node); // outputs stay alive until the end
    return { inputs, constants, order, uses, outputs, destroyed: false };
  };

  // ---- classes ---------------------------------------------------------------------------------------
  const namedInterface = (C, name) => {
    defProp(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
    for (const k of O.getOwnPropertyNames(C.prototype)) {
      if (k === 'constructor') continue;
      const d = O.getOwnPropertyDescriptor(C.prototype, k);
      d.enumerable = true;
      defProp(C.prototype, k, d);
    }
  };
  const illegalConstructor = () => new TE('Illegal constructor');

  class MLOperand {
    constructor(t) {
      if (t !== token) throw illegalConstructor();
    }
    get dataType() {
      const n = isObject(this) ? operandNodes.get(this) : undefined;
      if (!n) throw illegal('dataType');
      return n.dataType;
    }
    get shape() {
      const n = isObject(this) ? operandNodes.get(this) : undefined;
      if (!n) throw illegal('shape');
      return n.shape;
    }
  }
  namedInterface(MLOperand, 'MLOperand');

  class MLTensor {
    constructor(t) {
      if (t !== token) throw illegalConstructor();
    }
    get dataType() {
      return tensorBrand(this).dataType;
    }
    get shape() {
      return tensorBrand(this).shape;
    }
    get readable() {
      return tensorBrand(this).readable;
    }
    get writable() {
      return tensorBrand(this).writable;
    }
    get constant() {
      return tensorBrand(this).constant;
    }
    destroy() {
      destroyTensor(tensorBrand(this));
    }
  }
  const tensorBrand = (v) => {
    const s = isObject(v) ? tensors.get(v) : undefined;
    if (!s) throw illegal('MLTensor');
    return s;
  };
  namedInterface(MLTensor, 'MLTensor');

  class MLGraph {
    constructor(t) {
      if (t !== token) throw illegalConstructor();
    }
    get devices() {
      const gs = isObject(this) ? graphs.get(this) : undefined;
      if (!gs) throw illegal('devices');
      return gs.devices;
    }
    destroy() {
      const gs = isObject(this) ? graphs.get(this) : undefined;
      if (!gs) throw illegal('destroy');
      gs.destroyed = true;
    }
  }
  namedInterface(MLGraph, 'MLGraph');

  const unsupportedGpu = () => domError('NotSupportedError', 'Interoperability with WebGPU is not supported: this engine has no WebGPU');

  class MLContext {
    constructor(t) {
      if (t !== token) throw illegalConstructor();
    }
    get accelerated() {
      return contextState(this, 'accelerated').accelerated;
    }
    get lost() {
      return contextState(this, 'lost').lost;
    }
    destroy() {
      loseContext(contextState(this, 'destroy'), 'The context was destroyed');
    }
    opSupportLimits() {
      contextState(this, 'opSupportLimits');
      const out = {
        preferredInputLayout: 'nchw',
        maxTensorByteLength: MAX_TENSOR_BYTES,
        input: cloneLimits(GLOBAL_LIMITS.input),
        constant: cloneLimits(GLOBAL_LIMITS.constant),
        output: cloneLimits(GLOBAL_LIMITS.output),
      };
      for (const def of ops.values()) out[def.name] = cloneLimits(def.spec.limits);
      return out;
    }
    createTensor(descriptor) {
      return asyncOp(() => {
        const cs = contextState(this, 'createTensor');
        if (arguments.length < 1) throw new TE('1 argument required, but only 0 present');
        const desc = convertDictionary(descriptor, tensorDescriptorMembers);
        if (cs.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
        checkDescriptor(plainFail, 'tensor', desc, GLOBAL_LIMITS.input);
        return makeTensor(cs, desc, desc, newStorage(desc.dataType, desc.shape));
      });
    }
    createExportableTensor(descriptor, gpuDevice) {
      return asyncOp(() => {
        const cs = contextState(this, 'createExportableTensor');
        if (arguments.length < 2) throw new TE('2 arguments required');
        convertDictionary(descriptor, tensorDescriptorMembers);
        if (cs.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
        throw unsupportedGpu();
      });
    }
    createConstantTensor(descriptor, inputData) {
      return asyncOp(() => {
        const cs = contextState(this, 'createConstantTensor');
        if (arguments.length < 2) throw new TE('2 arguments required');
        const desc = convertDictionary(descriptor, descriptorMembers);
        const src = bufferSource(inputData);
        if (cs.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
        checkDescriptor(plainFail, 'constant', desc, GLOBAL_LIMITS.constant);
        if (src.detached || src.bytes.byteLength !== sizeOf(desc.shape) * BYTES[desc.dataType]) throw new TE('The byte length of the input data does not match the descriptor');
        const data = newStorage(desc.dataType, desc.shape);
        copyBytesInto(data, src.bytes);
        return makeTensor(cs, desc, { constant: true }, data);
      });
    }
    readTensor(tensor, outputData = undefined) {
      return asyncOp(() => {
        const cs = contextState(this, 'readTensor');
        if (arguments.length < 1) throw new TE('1 argument required, but only 0 present');
        const ts = tensorState(tensor);
        const target = arguments.length > 1 && outputData !== undefined ? outputData : null;
        const src = target ? bufferSource(target) : null;
        if (cs.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
        if (ts.context !== cs) throw new TE('The tensor belongs to another context');
        if (ts.destroyed) throw domError('InvalidStateError', 'The tensor is destroyed');
        if (!ts.readable) throw new TE('The tensor is not readable');
        const bytes = ts.data.byteLength;
        if (src && (src.detached || src.bytes.byteLength < bytes)) throw new TE('The output buffer is detached or too small for the tensor');
        // The tensor's contents at this point of the context's timeline; the promise settles later.
        const snapshot = ts.data.buffer.slice(ts.data.byteOffset, ts.data.byteOffset + bytes);
        return new PromiseC((resolve, reject) => {
          const rec = { tensor: ts, snapshot, target, resolve, reject, settled: false };
          ts.pending.add(rec);
          cs.pending.add(rec);
          PromiseC.resolve().then(() => finishRead(rec));
        });
      });
    }
    writeTensor(tensor, inputData) {
      const cs = contextState(this, 'writeTensor');
      if (arguments.length < 2) throw new TE('2 arguments required, but only ' + arguments.length + ' present');
      const ts = tensorState(tensor);
      const src = bufferSource(inputData);
      if (cs.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
      if (ts.context !== cs) throw new TE('The tensor belongs to another context');
      if (ts.destroyed) throw domError('InvalidStateError', 'The tensor is destroyed');
      if (!ts.writable) throw new TE('The tensor is not writable');
      if (src.detached || src.bytes.byteLength !== ts.data.byteLength) throw new TE('The byte length of the input data does not match the tensor');
      copyBytesInto(ts.data, src.bytes);
    }
    exportToGPU(tensor) {
      const cs = contextState(this, 'exportToGPU');
      const ts = tensorState(tensor);
      if (cs.destroyed || ts.destroyed) throw domError('InvalidStateError', 'The context or the tensor is destroyed');
      throw new TE('The tensor was not created with createExportableTensor()');
    }
    dispatch(graph, inputs, outputs) {
      const cs = contextState(this, 'dispatch');
      if (arguments.length < 3) throw new TE('3 arguments required, but only ' + arguments.length + ' present');
      const gs = isObject(graph) ? graphs.get(graph) : undefined;
      if (!gs) throw new TE("The provided value is not of type 'MLGraph'");
      const ins = convertRecord(inputs, (v) => tensorState(v));
      const outs = convertRecord(outputs, (v) => tensorState(v));
      if (cs.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
      if (gs.context !== cs) throw new TE('The graph belongs to another context');
      if (gs.destroyed) throw domError('InvalidStateError', 'The graph is destroyed');
      if (ins.size !== gs.inputs.size) throw new TE('The inputs do not match the graph inputs');
      for (const name of gs.inputs.keys()) if (!ins.has(name)) throw new TE(`Missing input '${name}'`);
      if (outs.size !== gs.outputs.size) throw new TE('The outputs do not match the graph outputs');
      for (const name of gs.outputs.keys()) if (!outs.has(name)) throw new TE(`Missing output '${name}'`);
      const match = (name, ts, node) => {
        if (ts.context !== cs) throw new TE(`The tensor '${name}' belongs to another context`);
        if (ts.destroyed) throw domError('InvalidStateError', `The tensor '${name}' is destroyed`);
        if (ts.dataType !== node.dataType || !sameShape(ts.shape, node.shape)) throw new TE(`The tensor '${name}' does not match the descriptor of the graph operand`);
      };
      for (const [name, node] of gs.inputs) match(name, ins.get(name), node);
      const seen = new Set();
      for (const [name, node] of gs.outputs) {
        const ts = outs.get(name);
        match(name, ts, node);
        if (ts.constant) throw new TE(`The output tensor '${name}' is a constant tensor`);
        if (seen.has(ts)) throw new TE('The same tensor is used for several outputs');
        seen.add(ts);
      }
      for (const ts of ins.values()) if (seen.has(ts)) throw new TE('A tensor is used as an input and an output');
      try {
        executeGraph(gs, ins, outs);
      } catch (e) {
        // Spec: a failed execution loses the context. A kernel throwing is an engine bug, so make it visible too.
        loseContext(cs, `Graph execution failed: ${e && e.message}`);
        if (typeof reportError === 'function') reportError(e);
      }
    }
  }
  namedInterface(MLContext, 'MLContext');

  class MLGraphBuilder {
    constructor(context) {
      if (arguments.length < 1) throw new TE('1 argument required, but only 0 present');
      const cs = isObject(context) ? contexts.get(context) : undefined;
      if (!cs) throw new TE("The provided value is not of type 'MLContext'");
      if (cs.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
      builders.set(this, { context: cs, contextObject: context, hasBuilt: false });
    }
    input(name, descriptor) {
      const bs = isObject(this) ? builders.get(this) : undefined;
      if (!bs) throw illegal('input');
      if (arguments.length < 2) throw new TE('2 arguments required, but only ' + arguments.length + ' present');
      const n = convs.usv(name);
      const desc = convertDictionary(descriptor, descriptorMembers);
      if (bs.hasBuilt) throw domError('InvalidStateError', 'The graph builder has already built a graph');
      if (bs.context.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
      if (n === '') throw new TE('The input name must not be empty');
      checkDescriptor(plainFail, 'input', desc, GLOBAL_LIMITS.input);
      const node = new Node(bs, 'input', desc.dataType, freezeShape(desc.shape));
      node.name = n;
      return operandOf(node);
    }
    constant(a, b = undefined) {
      const bs = isObject(this) ? builders.get(this) : undefined;
      if (!bs) throw illegal('constant');
      if (arguments.length < 1) throw new TE('1 argument required, but only 0 present');
      let tensor;
      if (isObject(a) && tensors.has(a)) {
        const ts = tensors.get(a);
        if (bs.hasBuilt) throw domError('InvalidStateError', 'The graph builder has already built a graph');
        if (bs.context.destroyed || ts.destroyed) throw domError('InvalidStateError', 'The context or the tensor is destroyed');
        if (ts.context !== bs.context) throw new TE('The tensor belongs to another context');
        if (!ts.constant) throw new TE('The tensor is not a constant tensor');
        checkTensorLimit(plainFail, 'constant', ts.dataType, ts.shape.length, GLOBAL_LIMITS.constant);
        tensor = { dataType: ts.dataType, shape: ts.shape, data: ts.data };
      } else if (typeof a === 'string') {
        // constant(dataType, value): a scalar
        if (arguments.length < 2) throw new TE('2 arguments required, but only 1 present');
        const dataType = dtypeConv(a);
        const value = convs.mlnumber(b);
        if (bs.hasBuilt) throw domError('InvalidStateError', 'The graph builder has already built a graph');
        if (bs.context.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
        checkTensorLimit(plainFail, 'constant', dataType, 0, GLOBAL_LIMITS.constant);
        const data = new ARRAYS[dataType](1);
        if (isBigInt(dataType)) {
          if (typeof value === 'bigint') data[0] = value;
          else if (NumberC.isFinite(value)) data[0] = BigInt(MathC.trunc(value));
          else throw new TE('The value is not a finite number');
        } else {
          if (typeof value === 'bigint') throw new TE(`A BigInt value cannot be used for ${dataType}`);
          data[0] = value;
        }
        tensor = { dataType, shape: freezeShape([]), data };
      } else {
        if (arguments.length < 2) throw new TE('2 arguments required, but only 1 present');
        const desc = convertDictionary(a, descriptorMembers);
        const src = bufferSource(b);
        if (bs.hasBuilt) throw domError('InvalidStateError', 'The graph builder has already built a graph');
        if (bs.context.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
        checkDescriptor(plainFail, 'constant', desc, GLOBAL_LIMITS.constant);
        if (src.kind !== null) {
          const ok = src.kind === ARRAY_NAMES[desc.dataType] || src.kind === 'Uint8Array' || (desc.dataType === 'float16' && src.kind === 'Uint16Array');
          if (!ok) throw new TE(`A ${src.kind} cannot hold ${desc.dataType} data`);
        }
        if (src.detached || src.bytes.byteLength !== sizeOf(desc.shape) * BYTES[desc.dataType]) throw new TE('The byte length of the buffer does not match the descriptor');
        // The data is copied now: later changes to (or detaching of) the caller's buffer do not reach the graph.
        const data = newStorage(desc.dataType, desc.shape);
        copyBytesInto(data, src.bytes);
        tensor = { dataType: desc.dataType, shape: freezeShape(desc.shape), data };
      }
      const node = new Node(bs, 'constant', tensor.dataType, tensor.shape);
      node.tensor = tensor;
      return operandOf(node);
    }
    build(outputs) {
      return asyncOp(() => {
        const bs = isObject(this) ? builders.get(this) : undefined;
        if (!bs) throw illegal('build');
        if (arguments.length < 1) throw new TE('1 argument required, but only 0 present');
        const named = convertRecord(outputs, convs.operand);
        if (bs.context.destroyed) throw domError('InvalidStateError', 'The context is destroyed');
        if (bs.hasBuilt) throw domError('InvalidStateError', 'The graph builder has already built a graph');
        if (named.size === 0) throw new TE('The outputs must not be empty');
        for (const [name, node] of named) {
          if (name === '') throw new TE('An output name must not be empty');
          if (node.builder !== bs) throw new TE(`The output '${name}' belongs to another MLGraphBuilder`);
          if (node.kind !== 'op') throw new TE(`The output '${name}' must not be an input or a constant`);
          checkTensorLimit(plainFail, `output '${name}'`, node.dataType, node.rank, GLOBAL_LIMITS.output);
        }
        const gs = buildGraph(bs, named);
        bs.hasBuilt = true;
        gs.context = bs.context;
        gs.devices = O.freeze(['cpu']);
        const graph = new MLGraph(token);
        graphs.set(graph, gs);
        return graph;
      });
    }
  }
  namedInterface(MLGraphBuilder, 'MLGraphBuilder');

  class ML {
    constructor(t) {
      if (t !== token) throw illegalConstructor();
    }
    createContext(options = undefined) {
      return asyncOp(() => {
        if (!isObject(this) || !(this instanceof ML)) throw illegal('createContext');
        if (typeof GPUDevice === 'function' && options instanceof GPUDevice) throw domError('NotSupportedError', 'WebGPU devices are not supported');
        const opts = convertDictionary(options, CONTEXT_OPTIONS);
        void opts;
        if (!native.active()) throw domError('InvalidStateError', 'The document is not fully active');
        if (!native.policy()) throw domError('SecurityError', 'Access to the WebNN API is disallowed by permissions policy');
        const context = new MLContext(token);
        let resolveLost;
        const lost = new PromiseC((r) => {
          resolveLost = r;
        });
        // Every context runs on the CPU: deviceType and powerPreference are hints, `accelerated` reports the truth.
        contexts.set(context, { destroyed: false, accelerated: false, lost, resolveLost, pending: new Set() });
        return context;
      });
    }
  }
  const CONTEXT_OPTIONS = [
    { name: 'accelerated', conv: convs.boolean, required: false, hasDefault: true, def: true },
    // MLDeviceType was dropped from the IDL, but Chrome still accepts and validates it.
    { name: 'deviceType', conv: enumConv(['cpu', 'gpu', 'npu']), required: false, hasDefault: false },
    { name: 'powerPreference', conv: enumConv(['default', 'high-performance', 'low-power']), required: false, hasDefault: true, def: 'default' },
  ];
  namedInterface(ML, 'ML');

  // ---- registry hand-over ----------------------------------------------------------------------------
  const api = {
    defineOp, tl, ALL_TYPES, FLOAT_TYPES, SIGNED_TYPES, INTEGER_TYPES, MAX_RANK,
    sizeOf, bytesPer, typedArrayFor, isFloat, isBigInt, alloc, stridesOf, broadcastShapes, broadcastStrides, mapBinary, roundTo,
    fail, domError, conv: convs, maxTensorByteLength: MAX_TENSOR_BYTES,
  };
  for (const family of native.families) family(api);
  installOps(MLGraphBuilder.prototype);

  return { ML, MLContext, MLGraph, MLGraphBuilder, MLOperand, MLTensor, ml: new ML(token) };
});
