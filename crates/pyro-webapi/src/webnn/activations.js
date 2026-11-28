// WebNN activation operators (https://www.w3.org/TR/webnn/#api-mlgraphbuilder-relu and the following sections):
// relu, sigmoid, tanh, elu, gelu, hardSigmoid, hardSwish, leakyRelu, linear, prelu, softplus, softsign, softmax
// and clamp. Kernels compute in double precision and round once when the result is stored into the output
// type's typed array. One expression evaluating to `(api) => {...}` (see the registry notes at the top of core.js).
(api) => {
  const { defineOp, tl, FLOAT_TYPES, SIGNED_TYPES, ALL_TYPES, broadcastShapes, mapBinary, roundTo, isBigInt, alloc } = api;

  const NUMBER_RANGES = {
    int8: [-128, 127], uint8: [0, 255], int32: [-(2 ** 31), 2 ** 31 - 1], uint32: [0, 2 ** 32 - 1],
  };
  const BIGINT_RANGES = {
    int64: [-(2n ** 63n), 2n ** 63n - 1n], uint64: [0n, 2n ** 64n - 1n],
  };

  // ---- erfc in double precision (gelu needs 1 + erf(x / sqrt 2) = erfc(-x / sqrt 2) without cancellation) --------
  const SQRT_PI = Math.sqrt(Math.PI);
  // erf(z) for 0 <= z < 2: the all-positive-terms series exp(-z^2) * sum 2^n z^(2n+1) / (1 * 3 * ... * (2n + 1)).
  const erfSmall = (z) => {
    const z2 = z * z;
    let term = z;
    let sum = z;
    for (let n = 0; n < 200; n++) {
      term *= (2 * z2) / (2 * n + 3);
      sum += term;
      if (term < sum * 1e-17) break;
    }
    return (2 / SQRT_PI) * Math.exp(-z2) * sum;
  };
  // erfc(z) for z >= 2: the continued fraction exp(-z^2) / sqrt(pi) / (z + (1/2) / (z + 1 / (z + (3/2) / (z + ...)))).
  const erfcLarge = (z) => {
    if (z > 27) return 0;
    let f = z;
    for (let k = 120; k >= 1; k--) f = z + k / 2 / f;
    return Math.exp(-z * z) / SQRT_PI / f;
  };
  const erfc = (x) => {
    if (x !== x) return x;
    if (x < 0) return 2 - erfc(-x);
    return x < 2 ? 1 - erfSmall(x) : erfcLarge(x);
  };

  // ---- shared pieces ---------------------------------------------------------------------------------------
  const sameShapeOf = (input) => ({ dataType: input.dataType, shape: input.shape.slice() });

  // Float-only element-wise operator with an optional `alpha` / `beta` option pair (IDL `float`, so a non-finite
  // value is a TypeError during conversion). `make(options)` returns the scalar function of one double.
  const floatUnary = (name, make, members) => {
    defineOp(name, {
      args: ['input:operand', 'options'],
      options: members || {},
      limits: { input: tl(FLOAT_TYPES), output: tl(FLOAT_TYPES) },
      validate: (ctx, input) => sameShapeOf(input),
      run([input, options], rt) {
        const f = make(options);
        const src = input.data;
        const out = alloc(rt.outputs[0].dataType, src.length);
        for (let i = 0; i < src.length; i++) out[i] = f(src[i]);
        return out;
      },
    });
  };

  floatUnary('sigmoid', () => (x) => 1 / (1 + Math.exp(-x)));
  floatUnary('tanh', () => Math.tanh);
  floatUnary('softsign', () => (x) => x / (1 + Math.abs(x)));
  // ln(1 + e^x) without overflow for large x and without losing small results for very negative x.
  floatUnary('softplus', () => (x) => (x > 0 ? x + Math.log1p(Math.exp(-x)) : Math.log1p(Math.exp(x))));
  floatUnary('gelu', () => (x) => x * 0.5 * erfc(-x / Math.SQRT2));
  floatUnary('hardSwish', () => (x) => (x * Math.max(0, Math.min(6, x + 3))) / 6);
  floatUnary('elu', ({ alpha }) => (x) => (x >= 0 ? x : alpha * Math.expm1(x)), { alpha: { type: 'float', default: 1 } });
  floatUnary('leakyRelu', ({ alpha }) => (x) => (x >= 0 ? x : alpha * x), { alpha: { type: 'float', default: 0.01 } });
  floatUnary('linear', ({ alpha, beta }) => (x) => alpha * x + beta, {
    alpha: { type: 'float', default: 1 }, beta: { type: 'float', default: 0 },
  });
  floatUnary('hardSigmoid', ({ alpha, beta }) => (x) => Math.max(0, Math.min(1, alpha * x + beta)), {
    alpha: { type: 'float', default: 0.2 }, beta: { type: 'float', default: 0.5 },
  });

  // relu: floats and signed integers (NaN passes through).
  defineOp('relu', {
    args: ['input:operand', 'options'],
    limits: { input: tl([...FLOAT_TYPES, ...SIGNED_TYPES]), output: tl([...FLOAT_TYPES, ...SIGNED_TYPES]) },
    validate: (ctx, input) => sameShapeOf(input),
    run([input], rt) {
      const src = input.data;
      const dt = rt.outputs[0].dataType;
      const out = alloc(dt, src.length);
      const zero = isBigInt(dt) ? 0n : 0;
      for (let i = 0; i < src.length; i++) out[i] = src[i] < zero ? zero : src[i];
      return out;
    },
  });

  // prelu: the slope is broadcast to the input's shape (unidirectionally: the output has the input's shape).
  defineOp('prelu', {
    args: ['input:operand', 'slope:operand', 'options'],
    limits: {
      input: tl([...FLOAT_TYPES, ...SIGNED_TYPES]), slope: tl([...FLOAT_TYPES, ...SIGNED_TYPES]),
      output: tl([...FLOAT_TYPES, ...SIGNED_TYPES]),
    },
    validate(ctx, input, slope) {
      if (input.dataType !== slope.dataType) ctx.fail(`The data type of slope (${slope.dataType}) must match the data type of input (${input.dataType})`);
      // Chrome (and the tests' "expanded output shape" case) broadcast both ways: the output is the broadcast shape.
      const shape = broadcastShapes(input.shape, slope.shape);
      if (!shape) ctx.fail(`The shape of slope [${slope.shape}] is not broadcastable to the shape of input [${input.shape}]`);
      return { dataType: input.dataType, shape };
    },
    run([input, slope], rt) {
      const out = rt.outputs[0];
      const zero = isBigInt(out.dataType) ? 0n : 0;
      // int32 products can exceed 2^53: Math.imul keeps the exact low 32 bits.
      const mul = out.dataType === 'int32' ? Math.imul : (a, b) => a * b;
      return mapBinary(input, slope, out.shape, out.dataType, (x, s) => (x >= zero ? x : mul(x, s)));
    },
  });

  // softmax along `axis` (rank >= 1), computed as exp(x - max) / sum for stability.
  defineOp('softmax', {
    args: ['input:operand', 'axis:ul', 'options'],
    limits: { input: tl(FLOAT_TYPES, 1), output: tl(FLOAT_TYPES, 1) },
    validate(ctx, input, axis) {
      if (axis >= input.rank) ctx.fail(`The axis ${axis} must be smaller than the rank of input (${input.rank})`);
      return sameShapeOf(input);
    },
    run([input, axis], rt) {
      const shape = input.shape;
      const src = input.data;
      const out = alloc(rt.outputs[0].dataType, src.length);
      let inner = 1;
      for (let i = axis + 1; i < shape.length; i++) inner *= shape[i];
      const size = shape[axis];
      const outer = src.length / (inner * size);
      const tmp = new Float64Array(size);
      for (let o = 0; o < outer; o++) {
        for (let j = 0; j < inner; j++) {
          const base = o * size * inner + j;
          let max = -Infinity;
          for (let k = 0; k < size; k++) {
            const v = src[base + k * inner];
            if (v > max || v !== v) max = v;
          }
          let sum = 0;
          for (let k = 0; k < size; k++) {
            const e = Math.exp(src[base + k * inner] - max);
            tmp[k] = e;
            sum += e;
          }
          for (let k = 0; k < size; k++) out[base + k * inner] = tmp[k] / sum;
        }
      }
      return out;
    },
  });

  // ---- clamp -------------------------------------------------------------------------------------------------
  // An MLNumber bound is cast to the input's data type: a BigInt only suits int64/uint64, a double is truncated
  // toward zero for integer types, and every integer conversion saturates (so +-Infinity become the type's limits).
  const castBound = (ctx, v, dataType, which) => {
    const big = typeof v === 'bigint';
    if (dataType === 'float32' || dataType === 'float16') {
      if (big) ctx.fail(`options.${which} is a BigInt, which is only valid for int64 and uint64 input, not ${dataType}`);
      // A NaN bound compares false against every element: it clamps nothing (the conformance tests expect that).
      return roundTo(dataType)(v);
    }
    if (isBigInt(dataType)) {
      const [lo, hi] = BIGINT_RANGES[dataType];
      let b;
      if (big) b = v;
      else if (v !== v) return which === 'minValue' ? lo : hi;
      else if (v === Infinity) b = hi;
      else if (v === -Infinity) b = lo;
      else b = BigInt(Math.trunc(v));
      return b < lo ? lo : b > hi ? hi : b;
    }
    if (big) ctx.fail(`options.${which} is a BigInt, which is only valid for int64 and uint64 input, not ${dataType}`);
    const [lo, hi] = NUMBER_RANGES[dataType];
    if (v !== v) return which === 'minValue' ? lo : hi;
    const n = Math.trunc(v) + 0;
    return n < lo ? lo : n > hi ? hi : n;
  };

  defineOp('clamp', {
    args: ['input:operand', 'options'],
    options: { minValue: 'mlnumber', maxValue: 'mlnumber' },
    limits: { input: tl(ALL_TYPES), output: tl(ALL_TYPES) },
    validate(ctx, input, options) {
      const dt = input.dataType;
      const lowest = isBigInt(dt) ? BIGINT_RANGES[dt][0] : NUMBER_RANGES[dt] ? NUMBER_RANGES[dt][0] : -Infinity;
      const highest = isBigInt(dt) ? BIGINT_RANGES[dt][1] : NUMBER_RANGES[dt] ? NUMBER_RANGES[dt][1] : Infinity;
      const min = options.minValue === undefined ? lowest : castBound(ctx, options.minValue, dt, 'minValue');
      const max = options.maxValue === undefined ? highest : castBound(ctx, options.maxValue, dt, 'maxValue');
      if (min > max) ctx.fail(`options.minValue (${min}) must not be greater than options.maxValue (${max})`);
      return { outputs: sameShapeOf(input), attrs: { min, max } };
    },
    run([input], rt) {
      const { min, max } = rt.attrs;
      const src = input.data;
      const out = alloc(rt.outputs[0].dataType, src.length);
      for (let i = 0; i < src.length; i++) {
        const x = src[i];
        out[i] = x < min ? min : x > max ? max : x;
      }
      return out;
    },
  });
}
