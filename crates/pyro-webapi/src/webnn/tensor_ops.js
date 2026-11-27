// WebNN tensor manipulation family (https://www.w3.org/TR/webnn/): identity, reshape, transpose, concat, split, slice,
// pad, expand, tile, reverse, gather, gatherElements, gatherND, scatterElements, scatterND, resample2d.
//
// All data movement is exact: elements are copied through an unsigned integer view of the same width (a float16 or
// float32 NaN keeps its bit pattern, int64/uint64 never go through a double). Only resample2d computes: in double
// precision, rounded once to the output type. One expression evaluating to `(api) => {...}`, no trailing semicolon.
(api) => {
  const { defineOp, tl, ALL_TYPES, typedArrayFor, stridesOf, sizeOf, conv } = api;
  const MAX_DIM = 2 ** 32 - 1;
  const MAX_SPLITS = 8192; // most outputs of split() and most inputs of concat()
  const INDEX_TYPES = ['int32', 'uint32', 'int64'];
  const R = (min = 0, max = 8) => tl(ALL_TYPES, min, max);
  const IDX = (min) => tl(INDEX_TYPES, min, 8);
  const fresh = (dataType, n) => new (typedArrayFor(dataType))(n);

  // `splits` is `(unsigned long or sequence<unsigned long>)`: core's declarative signature has no union type, so
  // register one converter under a private name (core's `api.conv` is the table its type strings are looked up in).
  conv.pyroSplits = (v) => {
    if ((typeof v === 'object' && v !== null) || typeof v === 'function') {
      if (typeof v[Symbol.iterator] === 'function') return Array.from(v, conv.ul);
    }
    return conv.ul(v);
  };

  // An unsigned view over the same memory as `a`: copying through it preserves every bit pattern.
  const bits = (a) => {
    switch (a.BYTES_PER_ELEMENT) {
      case 1: return a instanceof Uint8Array ? a : new Uint8Array(a.buffer, a.byteOffset, a.length);
      case 2: return new Uint16Array(a.buffer, a.byteOffset, a.length);
      case 4: return new Uint32Array(a.buffer, a.byteOffset, a.length);
      default: return new BigUint64Array(a.buffer, a.byteOffset, a.length);
    }
  };

  // dst[dOff + sum(i_k * dStr[k])] = src[sOff + sum(i_k * sStr[k])] for every index i inside `shape`; strides may be 0
  // (broadcast) or negative (reverse).
  const copyND = (src, sOff, sStr, dst, dOff, dStr, shape) => {
    const rank = shape.length;
    if (rank === 0) {
      dst[dOff] = src[sOff];
      return;
    }
    const last = rank - 1;
    const n = shape[last];
    const s1 = sStr[last];
    const d1 = dStr[last];
    const rec = (dim, s, d) => {
      if (dim === last) {
        if (s1 === 1 && d1 === 1 && n > 8) dst.set(src.subarray(s, s + n), d);
        else for (let i = 0; i < n; i++) dst[d + i * d1] = src[s + i * s1];
        return;
      }
      const len = shape[dim];
      const ss = sStr[dim];
      const ds = dStr[dim];
      for (let i = 0; i < len; i++) rec(dim + 1, s + i * ss, d + i * ds);
    };
    rec(0, sOff, dOff);
  };
  // Row-major walk over `shape` calling fn(flatIndex, coordinates).
  const eachCoord = (shape, fn) => {
    const n = sizeOf(shape);
    const rank = shape.length;
    const c = new Array(rank).fill(0);
    for (let p = 0; p < n; p++) {
      fn(p, c);
      for (let d = rank - 1; d >= 0; d--) {
        if (++c[d] < shape[d]) break;
        c[d] = 0;
      }
    }
  };
  // A (possibly negative, possibly out of range) index of an axis of size `dim`: negative counts from the end, the
  // result is clamped into [0, dim - 1] (the behaviour of the gather/scatter conformance tests).
  const clampIndex = (v, dim) => {
    let i = typeof v === 'bigint' ? Number(v) : v;
    if (i < 0) i += dim;
    return i < 0 ? 0 : i > dim - 1 ? dim - 1 : i;
  };
  const sameShape = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);
  const copyOf = (input, outShape) => {
    const out = fresh(input.dataType, sizeOf(outShape));
    out.set(input.data);
    return out;
  };
  const checkAxis = (ctx, axis, rank, what = 'axis') => {
    if (axis >= rank) ctx.fail(`The ${what} ${axis} must be smaller than the input rank ${rank}`);
  };
  const checkDistinctAxes = (ctx, axes, rank, what) => {
    const seen = new Set();
    for (const a of axes) {
      if (a >= rank) ctx.fail(`The ${what} value ${a} must be smaller than the input rank ${rank}`);
      if (seen.has(a)) ctx.fail(`The ${what} contain duplicate values`);
      seen.add(a);
    }
  };

  // ---- identity, reshape -------------------------------------------------------------------------------------
  defineOp('identity', {
    args: ['input:operand', 'options'],
    limits: { input: R(), output: R() },
    validate: (ctx, input) => ({ dataType: input.dataType, shape: input.shape.slice() }),
    run: ([input]) => input.data,
  });

  defineOp('reshape', {
    args: ['input:operand', 'newShape:seq<ul>', 'options'],
    limits: { input: R(), output: R() },
    validate(ctx, input, newShape) {
      for (const d of newShape) if (d === 0) ctx.fail('The new shape must not contain a zero dimension');
      if (sizeOf(newShape) !== sizeOf(input.shape)) {
        ctx.fail(`The number of elements of the new shape [${newShape}] differs from the input's (${sizeOf(input.shape)})`);
      }
      return { dataType: input.dataType, shape: newShape.slice() };
    },
    run: ([input]) => input.data,
  });

  // ---- transpose ---------------------------------------------------------------------------------------------
  defineOp('transpose', {
    args: ['input:operand', 'options'],
    options: { permutation: 'seq<ul>' },
    limits: { input: R(), output: R() },
    validate(ctx, input, options) {
      const rank = input.rank;
      let perm = options.permutation;
      if (perm === undefined) perm = Array.from({ length: rank }, (_, i) => rank - 1 - i);
      if (perm.length !== rank) ctx.fail(`The permutation length ${perm.length} must equal the input rank ${rank}`);
      checkDistinctAxes(ctx, perm, rank, 'permutation');
      return { outputs: { dataType: input.dataType, shape: perm.map((p) => input.shape[p]) }, attrs: { perm } };
    },
    run([input], rt) {
      const { perm } = rt.attrs;
      const outShape = rt.outputs[0].shape;
      const inStr = stridesOf(input.shape);
      const out = fresh(input.dataType, input.data.length);
      copyND(bits(input.data), 0, perm.map((p) => inStr[p]), bits(out), 0, stridesOf(outShape), outShape);
      return out;
    },
  });

  // ---- concat, split -----------------------------------------------------------------------------------------
  defineOp('concat', {
    args: ['inputs:seq<operand>', 'axis:ul', 'options'],
    limits: { inputs: R(1), output: R(1) },
    validate(ctx, inputs, axis) {
      if (inputs.length === 0) ctx.fail('The inputs must not be empty');
      if (inputs.length > MAX_SPLITS) ctx.fail(`The number of inputs ${inputs.length} exceeds the limit ${MAX_SPLITS}`);
      const first = inputs[0];
      checkAxis(ctx, axis, first.rank);
      let total = 0;
      for (const x of inputs) {
        if (x.dataType !== first.dataType) ctx.fail(`The data types of the inputs differ (${first.dataType} and ${x.dataType})`);
        if (x.rank !== first.rank) ctx.fail(`The ranks of the inputs differ (${first.rank} and ${x.rank})`);
        for (let d = 0; d < first.rank; d++) {
          if (d !== axis && x.shape[d] !== first.shape[d]) ctx.fail(`The input shapes [${first.shape}] and [${x.shape}] differ in a dimension other than the axis`);
        }
        total += x.shape[axis];
      }
      const shape = first.shape.slice();
      shape[axis] = total;
      return { outputs: { dataType: first.dataType, shape }, attrs: { axis } };
    },
    run([inputs], rt) {
      const { axis } = rt.attrs;
      const outShape = rt.outputs[0].shape;
      const outStr = stridesOf(outShape);
      const out = fresh(rt.outputs[0].dataType, sizeOf(outShape));
      const dst = bits(out);
      let at = 0;
      for (const x of inputs) {
        copyND(bits(x.data), 0, stridesOf(x.shape), dst, at * outStr[axis], outStr, x.shape);
        at += x.shape[axis];
      }
      return out;
    },
  });

  defineOp('split', {
    args: ['input:operand', 'splits:pyroSplits', 'options'],
    options: { axis: { type: 'ul', default: 0 } },
    limits: { input: R(1), outputs: R(1) },
    validate(ctx, input, splits, options) {
      const { axis } = options;
      checkAxis(ctx, axis, input.rank);
      const dim = input.shape[axis];
      let sizes;
      if (typeof splits === 'number') {
        if (splits === 0) ctx.fail('The number of splits must not be 0');
        if (splits > MAX_SPLITS) ctx.fail(`The number of splits ${splits} exceeds the limit ${MAX_SPLITS}`);
        if (dim % splits !== 0) ctx.fail(`The dimension ${dim} of the input along axis ${axis} is not divisible by ${splits}`);
        sizes = new Array(splits).fill(dim / splits);
      } else {
        if (splits.length > MAX_SPLITS) ctx.fail(`The number of splits ${splits.length} exceeds the limit ${MAX_SPLITS}`);
        let sum = 0;
        for (const s of splits) {
          if (s === 0) ctx.fail('A split size must not be 0');
          sum += s;
        }
        if (sum !== dim) ctx.fail(`The sum of the split sizes ${sum} differs from the dimension ${dim} of the input along axis ${axis}`);
        sizes = splits.slice();
      }
      const outputs = sizes.map((s) => {
        const shape = input.shape.slice();
        shape[axis] = s;
        return { dataType: input.dataType, shape };
      });
      return { outputs, attrs: { axis, sizes } };
    },
    run([input], rt) {
      const { axis, sizes } = rt.attrs;
      const inStr = stridesOf(input.shape);
      const src = bits(input.data);
      let at = 0;
      return rt.outputs.map((o, i) => {
        const out = fresh(o.dataType, sizeOf(o.shape));
        copyND(src, at * inStr[axis], inStr, bits(out), 0, stridesOf(o.shape), o.shape);
        at += sizes[i];
        return out;
      });
    },
  });

  // ---- slice, reverse ----------------------------------------------------------------------------------------
  defineOp('slice', {
    args: ['input:operand', 'starts:seq<ul>', 'sizes:seq<ul>', 'options'],
    options: { strides: 'seq<ul>' },
    limits: { input: R(), output: R() },
    validate(ctx, input, starts, sizes, options) {
      const rank = input.rank;
      if (starts.length !== rank) ctx.fail(`The length of starts (${starts.length}) must equal the input rank ${rank}`);
      if (sizes.length !== rank) ctx.fail(`The length of sizes (${sizes.length}) must equal the input rank ${rank}`);
      const strides = options.strides === undefined ? new Array(rank).fill(1) : options.strides;
      if (strides.length !== rank) ctx.fail(`The length of strides (${strides.length}) must equal the input rank ${rank}`);
      const shape = new Array(rank);
      for (let i = 0; i < rank; i++) {
        const dim = input.shape[i];
        if (starts[i] >= dim) ctx.fail(`The start ${starts[i]} of axis ${i} must be smaller than the input size ${dim}`);
        if (sizes[i] === 0) ctx.fail(`The size of axis ${i} must not be 0`);
        if (starts[i] + sizes[i] > dim) ctx.fail(`The slice [${starts[i]}, ${starts[i] + sizes[i]}) of axis ${i} exceeds the input size ${dim}`);
        if (strides[i] < 1) ctx.fail(`The stride of axis ${i} must be at least 1`);
        if (strides[i] > sizes[i]) ctx.fail(`The stride ${strides[i]} of axis ${i} must not exceed the size ${sizes[i]}`);
        shape[i] = Math.ceil(sizes[i] / strides[i]);
      }
      return { outputs: { dataType: input.dataType, shape }, attrs: { starts, strides } };
    },
    run([input], rt) {
      const { starts, strides } = rt.attrs;
      const outShape = rt.outputs[0].shape;
      const inStr = stridesOf(input.shape);
      let off = 0;
      for (let i = 0; i < starts.length; i++) off += starts[i] * inStr[i];
      const out = fresh(input.dataType, sizeOf(outShape));
      copyND(bits(input.data), off, inStr.map((s, i) => s * strides[i]), bits(out), 0, stridesOf(outShape), outShape);
      return out;
    },
  });

  defineOp('reverse', {
    args: ['input:operand', 'options'],
    options: { axes: 'seq<ul>' },
    limits: { input: R(), output: R() },
    validate(ctx, input, options) {
      const axes = options.axes === undefined ? Array.from({ length: input.rank }, (_, i) => i) : options.axes;
      checkDistinctAxes(ctx, axes, input.rank, 'axes');
      return { outputs: { dataType: input.dataType, shape: input.shape.slice() }, attrs: { axes } };
    },
    run([input], rt) {
      const inStr = stridesOf(input.shape);
      const sStr = inStr.slice();
      let off = 0;
      for (const a of rt.attrs.axes) {
        off += (input.shape[a] - 1) * inStr[a];
        sStr[a] = -inStr[a];
      }
      const out = fresh(input.dataType, input.data.length);
      copyND(bits(input.data), off, sStr, bits(out), 0, inStr, input.shape);
      return out;
    },
  });

  // ---- expand, tile ------------------------------------------------------------------------------------------
  defineOp('expand', {
    args: ['input:operand', 'newShape:seq<ul>', 'options'],
    limits: { input: R(), output: R() },
    validate(ctx, input, newShape) {
      const off = newShape.length - input.rank;
      if (off < 0) ctx.fail(`The rank of the new shape [${newShape}] is smaller than the input rank ${input.rank}`);
      for (let i = 0; i < newShape.length; i++) if (newShape[i] === 0) ctx.fail('The new shape must not contain a zero dimension');
      for (let i = 0; i < input.rank; i++) {
        const d = input.shape[i];
        if (d !== 1 && d !== newShape[i + off]) ctx.fail(`The input shape [${input.shape}] is not broadcastable to the new shape [${newShape}]`);
      }
      return { dataType: input.dataType, shape: newShape.slice() };
    },
    run([input], rt) {
      const outShape = rt.outputs[0].shape;
      const inStr = stridesOf(input.shape);
      const off = outShape.length - input.shape.length;
      const sStr = outShape.map((_, i) => (i >= off && input.shape[i - off] !== 1 ? inStr[i - off] : 0));
      const out = fresh(input.dataType, sizeOf(outShape));
      copyND(bits(input.data), 0, sStr, bits(out), 0, stridesOf(outShape), outShape);
      return out;
    },
  });

  defineOp('tile', {
    args: ['input:operand', 'repetitions:seq<ul>', 'options'],
    limits: { input: R(), output: R() },
    validate(ctx, input, repetitions) {
      if (repetitions.length !== input.rank) ctx.fail(`The length of repetitions (${repetitions.length}) must equal the input rank ${input.rank}`);
      for (const r of repetitions) if (r === 0) ctx.fail('A repetition must not be 0');
      return { dataType: input.dataType, shape: input.shape.map((d, i) => d * repetitions[i]) };
    },
    run([input, repetitions], rt) {
      // Each axis i becomes the pair (repetition i, input extent i) of a 2*rank-dimensional copy: the repetition
      // axis walks the output with stride extent*outStride and the input not at all.
      const outShape = rt.outputs[0].shape;
      const inStr = stridesOf(input.shape);
      const outStr = stridesOf(outShape);
      const shape = [];
      const sStr = [];
      const dStr = [];
      for (let i = 0; i < input.shape.length; i++) {
        shape.push(repetitions[i], input.shape[i]);
        sStr.push(0, inStr[i]);
        dStr.push(input.shape[i] * outStr[i], outStr[i]);
      }
      const out = fresh(input.dataType, sizeOf(outShape));
      copyND(bits(input.data), 0, sStr, bits(out), 0, dStr, shape);
      return out;
    },
  });

  // ---- pad ---------------------------------------------------------------------------------------------------
  // The pad value as an element of `dataType` (MLNumber: a BigInt for the 64-bit types, a number for the rest).
  const padValue = (dataType, v) => {
    if (dataType === 'int64' || dataType === 'uint64') {
      let b = typeof v === 'bigint' ? v : Number.isFinite(v) ? BigInt(Math.trunc(v)) : 0n;
      b = dataType === 'int64' ? BigInt.asIntN(64, b) : BigInt.asUintN(64, b);
      return b;
    }
    return typeof v === 'bigint' ? Number(v) : v;
  };
  defineOp('pad', {
    args: ['input:operand', 'beginningPadding:seq<ul>', 'endingPadding:seq<ul>', 'options'],
    options: { mode: { type: 'enum:constant|edge|reflection', default: 'constant' }, value: { type: 'mlnumber', default: 0 } },
    limits: { input: R(), output: R() },
    validate(ctx, input, begin, end, options) {
      const rank = input.rank;
      if (begin.length !== rank) ctx.fail(`The length of beginningPadding (${begin.length}) must equal the input rank ${rank}`);
      if (end.length !== rank) ctx.fail(`The length of endingPadding (${end.length}) must equal the input rank ${rank}`);
      const shape = new Array(rank);
      for (let i = 0; i < rank; i++) {
        const dim = input.shape[i];
        if (options.mode === 'reflection' && (begin[i] >= dim || end[i] >= dim)) {
          ctx.fail(`The padding [${begin[i]}, ${end[i]}] of axis ${i} must be smaller than the input size ${dim} in reflection mode`);
        }
        shape[i] = begin[i] + dim + end[i];
        if (shape[i] > MAX_DIM) ctx.fail(`The padded size of axis ${i} is too large`);
      }
      return { outputs: { dataType: input.dataType, shape }, attrs: { begin, mode: options.mode } };
    },
    run([input, , , options], rt) {
      const { begin, mode } = rt.attrs;
      const outShape = rt.outputs[0].shape;
      const inShape = input.shape;
      const outStr = stridesOf(outShape);
      const inStr = stridesOf(inShape);
      const out = fresh(input.dataType, sizeOf(outShape));
      const dst = bits(out);
      const src = bits(input.data);
      if (mode === 'constant') {
        out.fill(padValue(input.dataType, options.value));
        let off = 0;
        for (let i = 0; i < begin.length; i++) off += begin[i] * outStr[i];
        copyND(src, 0, inStr, dst, off, outStr, inShape);
        return out;
      }
      // edge / reflection: per axis, the source offset of every output coordinate.
      const maps = outShape.map((n, d) => {
        const dim = inShape[d];
        const m = new Array(n);
        for (let o = 0; o < n; o++) {
          let x = o - begin[d];
          if (mode === 'edge') x = x < 0 ? 0 : x > dim - 1 ? dim - 1 : x;
          else if (x < 0) x = -x;
          else if (x >= dim) x = 2 * (dim - 1) - x;
          m[o] = x * inStr[d];
        }
        return m;
      });
      const last = outShape.length - 1;
      let at = 0;
      const rec = (dim, s) => {
        if (dim === last) {
          const m = maps[dim];
          for (let i = 0; i < m.length; i++) dst[at++] = src[s + m[i]];
          return;
        }
        const m = maps[dim];
        for (let i = 0; i < m.length; i++) rec(dim + 1, s + m[i]);
      };
      if (outShape.length === 0) dst[0] = src[0];
      else rec(0, 0);
      return out;
    },
  });

  // ---- gather ------------------------------------------------------------------------------------------------
  defineOp('gather', {
    args: ['input:operand', 'indices:operand', 'options'],
    options: { axis: { type: 'ul', default: 0 } },
    limits: { input: R(1), indices: IDX(0), output: R() },
    validate(ctx, input, indices, options) {
      const { axis } = options;
      checkAxis(ctx, axis, input.rank);
      const shape = input.shape.slice(0, axis).concat(indices.shape, input.shape.slice(axis + 1));
      return { outputs: { dataType: input.dataType, shape }, attrs: { axis } };
    },
    run([input, indices], rt) {
      const { axis } = rt.attrs;
      const dim = input.shape[axis];
      let outer = 1;
      let inner = 1;
      for (let i = 0; i < axis; i++) outer *= input.shape[i];
      for (let i = axis + 1; i < input.shape.length; i++) inner *= input.shape[i];
      const n = indices.data.length;
      const out = fresh(input.dataType, sizeOf(rt.outputs[0].shape));
      const dst = bits(out);
      const src = bits(input.data);
      for (let o = 0; o < outer; o++) {
        for (let k = 0; k < n; k++) {
          const s = (o * dim + clampIndex(indices.data[k], dim)) * inner;
          const d = (o * n + k) * inner;
          for (let q = 0; q < inner; q++) dst[d + q] = src[s + q];
        }
      }
      return out;
    },
  });

  defineOp('gatherElements', {
    args: ['input:operand', 'indices:operand', 'options'],
    options: { axis: { type: 'ul', default: 0 } },
    limits: { input: R(1), indices: IDX(1), output: R(1) },
    validate(ctx, input, indices, options) {
      const { axis } = options;
      checkAxis(ctx, axis, input.rank);
      if (indices.rank !== input.rank) ctx.fail(`The indices rank ${indices.rank} must equal the input rank ${input.rank}`);
      for (let d = 0; d < input.rank; d++) {
        if (d !== axis && indices.shape[d] !== input.shape[d]) ctx.fail(`The indices shape [${indices.shape}] must equal the input shape [${input.shape}] except along the axis`);
      }
      return { outputs: { dataType: input.dataType, shape: indices.shape.slice() }, attrs: { axis } };
    },
    run([input, indices], rt) {
      const { axis } = rt.attrs;
      const inStr = stridesOf(input.shape);
      const out = fresh(input.dataType, indices.data.length);
      const dst = bits(out);
      const src = bits(input.data);
      eachCoord(indices.shape, (p, c) => {
        let s = 0;
        for (let d = 0; d < c.length; d++) s += (d === axis ? clampIndex(indices.data[p], input.shape[axis]) : c[d]) * inStr[d];
        dst[p] = src[s];
      });
      return out;
    },
  });

  defineOp('gatherND', {
    args: ['input:operand', 'indices:operand', 'options'],
    limits: { input: R(1), indices: IDX(1), output: R() },
    validate(ctx, input, indices) {
      const k = indices.shape[indices.rank - 1];
      if (k > input.rank) ctx.fail(`The last dimension ${k} of indices must not exceed the input rank ${input.rank}`);
      const shape = indices.shape.slice(0, -1).concat(input.shape.slice(k));
      return { outputs: { dataType: input.dataType, shape }, attrs: { k } };
    },
    run([input, indices], rt) {
      const { k } = rt.attrs;
      const inStr = stridesOf(input.shape);
      let slice = 1;
      for (let i = k; i < input.shape.length; i++) slice *= input.shape[i];
      const batches = indices.data.length / k;
      const out = fresh(input.dataType, sizeOf(rt.outputs[0].shape));
      const dst = bits(out);
      const src = bits(input.data);
      for (let b = 0; b < batches; b++) {
        let s = 0;
        for (let j = 0; j < k; j++) s += clampIndex(indices.data[b * k + j], input.shape[j]) * inStr[j];
        for (let q = 0; q < slice; q++) dst[b * slice + q] = src[s + q];
      }
      return out;
    },
  });

  // ---- scatter -----------------------------------------------------------------------------------------------
  defineOp('scatterElements', {
    args: ['input:operand', 'indices:operand', 'updates:operand', 'options'],
    options: { axis: { type: 'ul', default: 0 } },
    limits: { input: R(1), indices: IDX(1), updates: R(1), output: R(1) },
    validate(ctx, input, indices, updates, options) {
      const { axis } = options;
      checkAxis(ctx, axis, input.rank);
      if (updates.dataType !== input.dataType) ctx.fail(`The updates data type ${updates.dataType} must equal the input data type ${input.dataType}`);
      if (indices.rank !== input.rank) ctx.fail(`The indices rank ${indices.rank} must equal the input rank ${input.rank}`);
      if (!sameShape(updates.shape, indices.shape)) ctx.fail(`The updates shape [${updates.shape}] must equal the indices shape [${indices.shape}]`);
      for (let d = 0; d < input.rank; d++) {
        if (d !== axis && indices.shape[d] !== input.shape[d]) ctx.fail(`The indices shape [${indices.shape}] must equal the input shape [${input.shape}] except along the axis`);
      }
      return { outputs: { dataType: input.dataType, shape: input.shape.slice() }, attrs: { axis } };
    },
    run([input, indices, updates], rt) {
      const { axis } = rt.attrs;
      const inStr = stridesOf(input.shape);
      const out = copyOf(input, input.shape);
      const dst = bits(out);
      const upd = bits(updates.data);
      eachCoord(indices.shape, (p, c) => {
        let d = 0;
        for (let i = 0; i < c.length; i++) d += (i === axis ? clampIndex(indices.data[p], input.shape[axis]) : c[i]) * inStr[i];
        dst[d] = upd[p];
      });
      return out;
    },
  });

  defineOp('scatterND', {
    args: ['input:operand', 'indices:operand', 'updates:operand', 'options'],
    limits: { input: R(1), indices: IDX(1), updates: R(0), output: R(1) },
    validate(ctx, input, indices, updates) {
      if (updates.dataType !== input.dataType) ctx.fail(`The updates data type ${updates.dataType} must equal the input data type ${input.dataType}`);
      const k = indices.shape[indices.rank - 1];
      if (k > input.rank) ctx.fail(`The last dimension ${k} of indices must not exceed the input rank ${input.rank}`);
      const expected = indices.shape.slice(0, -1).concat(input.shape.slice(k));
      if (!sameShape(updates.shape, expected)) ctx.fail(`The updates shape [${updates.shape}] must be [${expected}]`);
      return { outputs: { dataType: input.dataType, shape: input.shape.slice() }, attrs: { k } };
    },
    run([input, indices, updates], rt) {
      const { k } = rt.attrs;
      const inStr = stridesOf(input.shape);
      let slice = 1;
      for (let i = k; i < input.shape.length; i++) slice *= input.shape[i];
      const batches = indices.data.length / k;
      const out = copyOf(input, input.shape);
      const dst = bits(out);
      const upd = bits(updates.data);
      for (let b = 0; b < batches; b++) {
        let d = 0;
        for (let j = 0; j < k; j++) d += clampIndex(indices.data[b * k + j], input.shape[j]) * inStr[j];
        for (let q = 0; q < slice; q++) dst[d + q] = upd[b * slice + q];
      }
      return out;
    },
  });

  // ---- resample2d --------------------------------------------------------------------------------------------
  // Resamples `src` (row-major, `shape`) along `axis` to `outN` samples into a Float64Array. Half-pixel centres:
  // the input coordinate of output sample o is (o + 0.5) * inN / outN - 0.5 (the ratio of the sizes, so a scale
  // whose product with the input size was floored gives the same positions as the resulting size), clamped to the
  // input; `nearest` takes ceil(coordinate - 0.5) (ties pick the smaller index), `linear` blends the two neighbours.
  const resampleAxis = (src, shape, axis, outN, linear) => {
    const inN = shape[axis];
    let outer = 1;
    let inner = 1;
    for (let i = 0; i < axis; i++) outer *= shape[i];
    for (let i = axis + 1; i < shape.length; i++) inner *= shape[i];
    const i0 = new Int32Array(outN);
    const i1 = new Int32Array(outN);
    const w = new Float64Array(outN);
    for (let o = 0; o < outN; o++) {
      if (linear) {
        let c = ((o + 0.5) * inN) / outN - 0.5;
        c = c < 0 ? 0 : c > inN - 1 ? inN - 1 : c;
        const f = Math.floor(c);
        i0[o] = f;
        i1[o] = f + 1 < inN ? f + 1 : inN - 1;
        w[o] = c - f;
      } else {
        // ceil(clamp(c) - 0.5) with c = (o + 0.5) * inN / outN - 0.5 is ceil((2o+1)*inN / (2*outN)) - 1, in exact
        // integer arithmetic so that ties (which pick the smaller index) are exact.
        const num = (2 * o + 1) * inN;
        const den = 2 * outN;
        const k = Math.floor((num + den - 1) / den) - 1;
        i0[o] = k < 0 ? 0 : k > inN - 1 ? inN - 1 : k;
      }
    }
    const out = new Float64Array(outer * outN * inner);
    for (let a = 0; a < outer; a++) {
      for (let o = 0; o < outN; o++) {
        const d = (a * outN + o) * inner;
        const s0 = (a * inN + i0[o]) * inner;
        if (!linear) {
          for (let q = 0; q < inner; q++) out[d + q] = src[s0 + q];
        } else {
          const s1 = (a * inN + i1[o]) * inner;
          const t = w[o];
          for (let q = 0; q < inner; q++) out[d + q] = src[s0 + q] * (1 - t) + src[s1 + q] * t;
        }
      }
    }
    return out;
  };
  defineOp('resample2d', {
    args: ['input:operand', 'options'],
    options: {
      mode: { type: 'enum:nearest-neighbor|linear', default: 'nearest-neighbor' },
      scales: 'seq<float>',
      sizes: 'seq<ul>',
      axes: 'seq<ul>',
    },
    limits: { input: tl(['float32', 'float16'].filter((t) => ALL_TYPES.includes(t)), 4, 4), output: tl(['float32', 'float16'].filter((t) => ALL_TYPES.includes(t)), 4, 4) },
    validate(ctx, input, options) {
      const axes = options.axes === undefined ? [2, 3] : options.axes;
      if (axes.length !== 2) ctx.fail(`The length of axes (${axes.length}) must be 2`);
      checkDistinctAxes(ctx, axes, input.rank, 'axes');
      const scales = options.scales === undefined ? [1, 1] : options.scales;
      if (scales.length !== 2) ctx.fail(`The length of scales (${scales.length}) must be 2`);
      for (const s of scales) if (!(s > 0)) ctx.fail('Every scale must be greater than 0');
      const sizes = options.sizes;
      if (sizes !== undefined) {
        if (sizes.length !== 2) ctx.fail(`The length of sizes (${sizes.length}) must be 2`);
        for (const s of sizes) if (s === 0) ctx.fail('Every size must be greater than 0');
      }
      const shape = input.shape.slice();
      for (let i = 0; i < 2; i++) {
        const out = sizes !== undefined ? sizes[i] : Math.floor(scales[i] * input.shape[axes[i]]);
        if (out < 1 || out > MAX_DIM) ctx.fail(`The output size ${out} of axis ${axes[i]} is not a valid dimension`);
        shape[axes[i]] = out;
      }
      return { outputs: { dataType: input.dataType, shape }, attrs: { axes, linear: options.mode === 'linear' } };
    },
    run([input], rt) {
      const { axes, linear } = rt.attrs;
      const outShape = rt.outputs[0].shape;
      let cur = input.data;
      const shape = input.shape.slice();
      for (let i = 0; i < 2; i++) {
        const axis = axes[i];
        cur = resampleAxis(cur, shape, axis, outShape[axis], linear);
        shape[axis] = outShape[axis];
      }
      const out = fresh(input.dataType, sizeOf(outShape));
      out.set(cur);
      return out;
    },
  });
}
