// Reductions (reduceL1 ... reduceSumSquare), argMin/argMax, cumulativeSum and triangular. Kernels accumulate in double
// precision (BigInt for int64/uint64) and round once, when the result is stored into the output type's array.
(api) => {
  const { defineOp, tl, ALL_TYPES, FLOAT_TYPES, sizeOf, stridesOf, isBigInt, alloc } = api;
  const MAXR = api.MAX_RANK ?? 8;

  // `axes` of a reduction: default all dimensions; each must be < rank and unique.
  const resolveAxes = (ctx, rank, axes) => {
    if (axes === undefined) return Array.from({ length: rank }, (_, i) => i);
    const seen = new Set();
    for (const a of axes) {
      if (a >= rank) ctx.fail(`The value ${a} in axes is out of range for an input of rank ${rank}`);
      if (seen.has(a)) ctx.fail(`The axes contain the duplicate value ${a}`);
      seen.add(a);
    }
    return axes.slice();
  };

  // For every input element, the index of the output element it reduces into (`contrib[d]` = stride of dimension d in the
  // output, 0 for a reduced dimension).
  const targetMap = (shape, contrib) => {
    const n = sizeOf(shape);
    const out = new Uint32Array(n);
    const rank = shape.length;
    const idx = new Array(rank).fill(0);
    let t = 0;
    for (let i = 0; i < n; i++) {
      out[i] = t;
      for (let d = rank - 1; d >= 0; d--) {
        idx[d]++;
        t += contrib[d];
        if (idx[d] < shape[d]) break;
        t -= contrib[d] * shape[d];
        idx[d] = 0;
      }
    }
    return out;
  };

  // ---- reduceXxx -------------------------------------------------------------------------------------
  const NUM = {
    l1: { init: 0, step: (a, x) => a + Math.abs(x) },
    sum: { init: 0, step: (a, x) => a + x },
    sumSquare: { init: 0, step: (a, x) => a + x * x, stepInt: (a, x) => a + Math.imul(x, x) },
    product: { init: 1, step: (a, x) => a * x, stepInt: (a, x) => Math.imul(a, x) },
    max: { init: -Infinity, step: (a, x) => (x !== x || a !== a ? NaN : x > a ? x : a) },
    min: { init: Infinity, step: (a, x) => (x !== x || a !== a ? NaN : x < a ? x : a) },
  };
  const BIG = {
    l1: { init: 0n, step: (a, x) => a + (x < 0n ? -x : x) },
    sum: { init: 0n, step: (a, x) => a + x },
    sumSquare: { init: 0n, step: (a, x) => a + x * x },
    product: { init: 1n, step: (a, x) => a * x },
    max: { init: -(2n ** 64n), step: (a, x) => (x > a ? x : a) },
    min: { init: 2n ** 64n, step: (a, x) => (x < a ? x : a) },
  };

  const reduceOp = (name, types, kernel) => {
    defineOp(name, {
      args: ['input:operand', 'options'],
      options: { axes: 'seq<ul>', keepDimensions: { type: 'boolean', default: false } },
      limits: { input: tl(types, 0, MAXR), output: tl(types, 0, MAXR) },
      validate(ctx, input, options) {
        const axes = resolveAxes(ctx, input.rank, options.axes);
        const shape = [];
        for (let d = 0; d < input.rank; d++) {
          if (!axes.includes(d)) shape.push(input.shape[d]);
          else if (options.keepDimensions) shape.push(1);
        }
        return { outputs: { dataType: input.dataType, shape }, attrs: { axes } };
      },
      run([input], rt) {
        const out = rt.outputs[0];
        const shape = input.shape;
        const outCount = sizeOf(out.shape);
        const axes = rt.attrs.axes;
        // Strides of the kept dimensions in the (keepDimensions=false) reduced shape.
        const kept = [];
        for (let d = 0; d < shape.length; d++) if (!axes.includes(d)) kept.push(shape[d]);
        const keptStrides = stridesOf(kept);
        const contrib = [];
        let k = 0;
        for (let d = 0; d < shape.length; d++) contrib.push(axes.includes(d) ? 0 : keptStrides[k++]);
        const map = targetMap(shape, contrib);
        const count = sizeOf(shape) / outCount;
        return kernel(input, out, map, outCount, count);
      },
    });
  };

  // Kinds shared by the integer/float types: l1, sum, sumSquare, product, max, min.
  const generic = (kind) => (input, out, map, outCount) => {
    const x = input.data;
    const res = alloc(out.dataType, outCount);
    if (isBigInt(input.dataType)) {
      const { init, step } = BIG[kind];
      const acc = new Array(outCount).fill(init);
      for (let i = 0; i < x.length; i++) acc[map[i]] = step(acc[map[i]], x[i]);
      for (let i = 0; i < outCount; i++) res[i] = acc[i];
      return res;
    }
    const ints = input.dataType !== 'float32' && input.dataType !== 'float16';
    const spec = NUM[kind];
    const step = ints && spec.stepInt ? spec.stepInt : spec.step;
    const acc = new Float64Array(outCount).fill(spec.init);
    for (let i = 0; i < x.length; i++) acc[map[i]] = step(acc[map[i]], x[i]);
    for (let i = 0; i < outCount; i++) res[i] = acc[i];
    return res;
  };
  // Floating-point-only kinds. `fin(sum-like accumulator, count)`.
  const floating = (pre, fin) => (input, out, map, outCount, count) => {
    const x = input.data;
    const acc = new Float64Array(outCount);
    for (let i = 0; i < x.length; i++) acc[map[i]] += pre(x[i]);
    const res = alloc(out.dataType, outCount);
    for (let i = 0; i < outCount; i++) res[i] = fin(acc[i], count);
    return res;
  };

  reduceOp('reduceL1', ALL_TYPES, generic('l1'));
  reduceOp('reduceSum', ALL_TYPES, generic('sum'));
  reduceOp('reduceSumSquare', ALL_TYPES, generic('sumSquare'));
  reduceOp('reduceProduct', ALL_TYPES, generic('product'));
  reduceOp('reduceMax', ALL_TYPES, generic('max'));
  reduceOp('reduceMin', ALL_TYPES, generic('min'));
  reduceOp('reduceL2', FLOAT_TYPES, floating((v) => v * v, (s) => Math.sqrt(s)));
  reduceOp('reduceMean', FLOAT_TYPES, floating((v) => v, (s, n) => s / n));
  reduceOp('reduceLogSum', FLOAT_TYPES, floating((v) => v, (s) => Math.log(s)));
  // log(sum(exp(x))) computed as max + log(sum(exp(x - max))), which cannot overflow.
  reduceOp('reduceLogSumExp', FLOAT_TYPES, (input, out, map, outCount) => {
    const x = input.data;
    const max = new Float64Array(outCount).fill(-Infinity);
    for (let i = 0; i < x.length; i++) {
      const m = map[i];
      if (x[i] !== x[i] || max[m] !== max[m]) max[m] = NaN;
      else if (x[i] > max[m]) max[m] = x[i];
    }
    const sum = new Float64Array(outCount);
    for (let i = 0; i < x.length; i++) {
      const m = max[map[i]];
      sum[map[i]] += Number.isFinite(m) ? Math.exp(x[i] - m) : 0;
    }
    const res = alloc(out.dataType, outCount);
    for (let i = 0; i < outCount; i++) res[i] = Number.isFinite(max[i]) ? max[i] + Math.log(sum[i]) : max[i];
    return res;
  });

  // ---- argMin / argMax -------------------------------------------------------------------------------
  const outerLenInner = (shape, axis) => {
    let outer = 1;
    let inner = 1;
    for (let d = 0; d < axis; d++) outer *= shape[d];
    for (let d = axis + 1; d < shape.length; d++) inner *= shape[d];
    return [outer, shape[axis], inner];
  };
  const argOp = (name, better) => {
    defineOp(name, {
      args: ['input:operand', 'axis:ul', 'options'],
      options: { keepDimensions: { type: 'boolean', default: false }, outputDataType: { type: 'dtype', default: 'int32' } },
      limits: { input: tl(ALL_TYPES, 1, MAXR), output: tl(['int32', 'int64'], 0, MAXR) },
      validate(ctx, input, axis, options) {
        if (axis >= input.rank) ctx.fail(`The axis ${axis} is out of range for an input of rank ${input.rank}`);
        const shape = [];
        for (let d = 0; d < input.rank; d++) {
          if (d !== axis) shape.push(input.shape[d]);
          else if (options.keepDimensions) shape.push(1);
        }
        if (options.outputDataType !== 'int32' && options.outputDataType !== 'int64') {
          ctx.fail(`The outputDataType must be int32 or int64, not ${options.outputDataType}`);
        }
        return { outputs: { dataType: options.outputDataType, shape }, attrs: { axis } };
      },
      run([input], rt) {
        const out = rt.outputs[0];
        const [outer, len, inner] = outerLenInner(input.shape, rt.attrs.axis);
        const x = input.data;
        const res = alloc(out.dataType, outer * inner);
        const big = out.dataType === 'int64';
        for (let o = 0; o < outer; o++) {
          for (let i = 0; i < inner; i++) {
            const base = o * len * inner + i;
            let best = 0;
            let bestV = x[base];
            for (let k = 1; k < len; k++) {
              const v = x[base + k * inner];
              if (better(v, bestV)) {
                best = k;
                bestV = v;
              }
            }
            res[o * inner + i] = big ? BigInt(best) : best;
          }
        }
        return res;
      },
    });
  };
  // Ties keep the first index; a NaN never wins against a number.
  argOp('argMax', (v, best) => v > best);
  argOp('argMin', (v, best) => v < best);

  // ---- cumulativeSum ---------------------------------------------------------------------------------
  defineOp('cumulativeSum', {
    args: ['input:operand', 'axis:ul', 'options'],
    options: { exclusive: { type: 'boolean', default: false }, reversed: { type: 'boolean', default: false } },
    limits: { input: tl(ALL_TYPES, 1, MAXR), output: tl(ALL_TYPES, 1, MAXR) },
    validate(ctx, input, axis) {
      if (axis >= input.rank) ctx.fail(`The axis ${axis} is out of range for an input of rank ${input.rank}`);
      return { outputs: { dataType: input.dataType, shape: input.shape.slice() }, attrs: { axis } };
    },
    run([input, , options], rt) {
      const out = rt.outputs[0];
      const [outer, len, inner] = outerLenInner(input.shape, rt.attrs.axis);
      const x = input.data;
      const res = alloc(out.dataType, x.length);
      const big = isBigInt(input.dataType);
      const { exclusive, reversed } = options;
      for (let o = 0; o < outer; o++) {
        for (let i = 0; i < inner; i++) {
          const base = o * len * inner + i;
          let acc = big ? 0n : 0;
          for (let s = 0; s < len; s++) {
            const k = reversed ? len - 1 - s : s;
            const at = base + k * inner;
            if (exclusive) {
              res[at] = acc;
              acc += x[at];
            } else {
              acc += x[at];
              res[at] = acc;
            }
          }
        }
      }
      return res;
    },
  });

  // ---- triangular ------------------------------------------------------------------------------------
  defineOp('triangular', {
    args: ['input:operand', 'options'],
    options: { diagonal: { type: 'long', default: 0 }, upper: { type: 'boolean', default: true } },
    limits: { input: tl(ALL_TYPES, 2, MAXR), output: tl(ALL_TYPES, 2, MAXR) },
    validate: (ctx, input) => ({ dataType: input.dataType, shape: input.shape.slice() }),
    run([input, options], rt) {
      const shape = input.shape;
      const rows = shape[shape.length - 2];
      const cols = shape[shape.length - 1];
      const x = input.data;
      const res = alloc(rt.outputs[0].dataType, x.length);
      const { upper, diagonal } = options;
      const matrices = x.length / (rows * cols);
      for (let m = 0; m < matrices; m++) {
        for (let r = 0; r < rows; r++) {
          for (let c = 0; c < cols; c++) {
            const keep = upper ? c - r >= diagonal : c - r <= diagonal;
            if (keep) res[(m * rows + r) * cols + c] = x[(m * rows + r) * cols + c];
          }
        }
      }
      return res;
    },
  });
}
