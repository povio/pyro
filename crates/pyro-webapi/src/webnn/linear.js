// WebNN operator family: matmul, gemm, quantizeLinear, dequantizeLinear, cast, where. Kernels accumulate in double
// precision and round once, by storing into the output type's typed array. One expression `(api) => {...}`.
(api) => {
  const { defineOp, tl, ALL_TYPES, FLOAT_TYPES, sizeOf, broadcastShapes, broadcastStrides, isBigInt } = api;

  const sameShape = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);
  // Iterate the output index space of `shape`, calling fn(outIndex, offsetA, offsetB, offsetC) with the offsets
  // of up to three broadcast operands (strides arrays, 0 on broadcast axes).
  const walk = (shape, strides, fn) => {
    const n = sizeOf(shape);
    const rank = shape.length;
    const k = strides.length;
    const offs = new Array(k).fill(0);
    const idx = new Array(rank).fill(0);
    for (let i = 0; i < n; i++) {
      fn(i, offs);
      for (let d = rank - 1; d >= 0; d--) {
        idx[d]++;
        for (let j = 0; j < k; j++) offs[j] += strides[j][d];
        if (idx[d] < shape[d]) break;
        for (let j = 0; j < k; j++) offs[j] -= strides[j][d] * shape[d];
        idx[d] = 0;
      }
    }
  };

  // ---- matmul ------------------------------------------------------------------------------------------
  defineOp('matmul', {
    args: ['a:operand', 'b:operand', 'options'],
    limits: { a: tl(FLOAT_TYPES, 2), b: tl(FLOAT_TYPES, 2), output: tl(FLOAT_TYPES, 2) },
    validate(ctx, a, b) {
      if (a.dataType !== b.dataType) ctx.fail(`The data types of a (${a.dataType}) and b (${b.dataType}) must be the same`);
      const ra = a.shape.length;
      const rb = b.shape.length;
      const m = a.shape[ra - 2];
      const k = a.shape[ra - 1];
      const k2 = b.shape[rb - 2];
      const n = b.shape[rb - 1];
      if (k !== k2) ctx.fail(`The columns of a (${k}) must equal the rows of b (${k2})`);
      const batch = broadcastShapes(a.shape.slice(0, ra - 2), b.shape.slice(0, rb - 2));
      if (!batch) ctx.fail(`The batch shapes of a [${a.shape.slice(0, ra - 2)}] and b [${b.shape.slice(0, rb - 2)}] are not broadcastable`);
      return { dataType: a.dataType, shape: [...batch, m, n] };
    },
    run([a, b], rt) {
      const out = rt.outputs[0];
      const ra = a.shape.length;
      const rb = b.shape.length;
      const m = a.shape[ra - 2];
      const k = a.shape[ra - 1];
      const n = b.shape[rb - 1];
      const batch = out.shape.slice(0, out.shape.length - 2);
      const sa = broadcastStrides(a.shape.slice(0, ra - 2), batch).map((s) => s * m * k);
      const sb = broadcastStrides(b.shape.slice(0, rb - 2), batch).map((s) => s * k * n);
      const res = new (api.typedArrayFor(out.dataType))(sizeOf(out.shape));
      const ad = a.data;
      const bd = b.data;
      walk(batch, [sa, sb], (bi, offs) => {
        const oa = offs[0];
        const ob = offs[1];
        const oo = bi * m * n;
        for (let i = 0; i < m; i++) {
          for (let j = 0; j < n; j++) {
            let acc = 0;
            for (let p = 0; p < k; p++) acc += ad[oa + i * k + p] * bd[ob + p * n + j];
            res[oo + i * n + j] = acc;
          }
        }
      });
      return res;
    },
  });

  // ---- gemm: alpha * op(A) * op(B) + beta * C ------------------------------------------------------------
  defineOp('gemm', {
    args: ['a:operand', 'b:operand', 'options'],
    options: {
      alpha: { type: 'float', default: 1.0 },
      beta: { type: 'float', default: 1.0 },
      c: 'operand',
      aTranspose: { type: 'boolean', default: false },
      bTranspose: { type: 'boolean', default: false },
    },
    limits: { a: tl(FLOAT_TYPES, 2, 2), b: tl(FLOAT_TYPES, 2, 2), c: tl(FLOAT_TYPES, 0, 2), output: tl(FLOAT_TYPES, 2, 2) },
    validate(ctx, a, b, options) {
      if (a.dataType !== b.dataType) ctx.fail(`The data types of a (${a.dataType}) and b (${b.dataType}) must be the same`);
      const [ar, ac] = options.aTranspose ? [a.shape[1], a.shape[0]] : [a.shape[0], a.shape[1]];
      const [br, bc] = options.bTranspose ? [b.shape[1], b.shape[0]] : [b.shape[0], b.shape[1]];
      if (ac !== br) ctx.fail(`The columns of op(a) (${ac}) must equal the rows of op(b) (${br})`);
      const shape = [ar, bc];
      const c = options.c;
      if (c) {
        if (c.dataType !== a.dataType) ctx.fail(`The data type of c (${c.dataType}) must equal that of a (${a.dataType})`);
        const bs = broadcastShapes(c.shape, shape);
        if (!bs || !sameShape(bs, shape)) ctx.fail(`The shape of c [${c.shape}] is not unidirectionally broadcastable to [${shape}]`);
      }
      return { dataType: a.dataType, shape };
    },
    run([a, b, options], rt) {
      const out = rt.outputs[0];
      const [m, n] = out.shape;
      const k = options.aTranspose ? a.shape[0] : a.shape[1];
      const alpha = options.alpha;
      const beta = options.beta;
      const ad = a.data;
      const bd = b.data;
      const res = new (api.typedArrayFor(out.dataType))(m * n);
      const c = options.c;
      const cs = c ? broadcastStrides(c.shape, out.shape) : null;
      const aStrideI = options.aTranspose ? 1 : k;
      const aStrideP = options.aTranspose ? m : 1;
      const bStrideP = options.bTranspose ? 1 : n;
      const bStrideJ = options.bTranspose ? k : 1;
      for (let i = 0; i < m; i++) {
        for (let j = 0; j < n; j++) {
          let acc = 0;
          for (let p = 0; p < k; p++) acc += ad[i * aStrideI + p * aStrideP] * bd[p * bStrideP + j * bStrideJ];
          let v = alpha * acc;
          if (c) v += beta * c.data[i * cs[0] + j * cs[1]];
          res[i * n + j] = v;
        }
      }
      return res;
    },
  });

  // ---- quantizeLinear / dequantizeLinear ------------------------------------------------------------------
  const roundEven = (x) => {
    const f = Math.floor(x);
    const d = x - f;
    if (d < 0.5) return f;
    if (d > 0.5) return f + 1;
    return f % 2 === 0 ? f : f + 1;
  };
  const INT_RANGE = { int8: [-128, 127], uint8: [0, 255], int32: [-(2 ** 31), 2 ** 31 - 1] };
  // Scale (and zeroPoint) blocks: every dimension of scale must divide that of the input (rank must match).
  const checkBlocks = (ctx, input, scale, zeroPoint) => {
    if (scale.shape.length !== input.shape.length) ctx.fail(`The rank of scale (${scale.shape.length}) must equal the rank of input (${input.shape.length})`);
    if (!sameShape(zeroPoint.shape, scale.shape)) ctx.fail(`The shape of zeroPoint [${zeroPoint.shape}] must equal the shape of scale [${scale.shape}]`);
    for (let i = 0; i < input.shape.length; i++) {
      if (input.shape[i] % scale.shape[i] !== 0) ctx.fail(`The size of scale dimension ${i} (${scale.shape[i]}) must be a factor of the input size (${input.shape[i]})`);
    }
  };
  // Offsets into the scale tensor for each input element (block-wise indexing).
  const blockOffsets = (inputShape, scaleShape) => {
    const n = sizeOf(inputShape);
    const rank = inputShape.length;
    const own = api.stridesOf(scaleShape);
    const block = inputShape.map((v, d) => v / scaleShape[d]);
    const idx = new Array(rank).fill(0);
    const offs = new Int32Array(n);
    for (let i = 0; i < n; i++) {
      let o = 0;
      for (let d = 0; d < rank; d++) o += Math.floor(idx[d] / block[d]) * own[d];
      offs[i] = o;
      for (let d = rank - 1; d >= 0; d--) {
        if (++idx[d] < inputShape[d]) break;
        idx[d] = 0;
      }
    }
    return offs;
  };

  defineOp('quantizeLinear', {
    args: ['input:operand', 'scale:operand', 'zeroPoint:operand', 'options'],
    limits: {
      input: tl(FLOAT_TYPES), scale: tl(FLOAT_TYPES), zeroPoint: tl(['int8', 'uint8', 'int32']), output: tl(['int8', 'uint8', 'int32']),
    },
    validate(ctx, input, scale, zeroPoint) {
      if (input.dataType !== scale.dataType) ctx.fail(`The data types of input (${input.dataType}) and scale (${scale.dataType}) must be the same`);
      checkBlocks(ctx, input, scale, zeroPoint);
      return { dataType: zeroPoint.dataType, shape: input.shape.slice() };
    },
    run([input, scale, zeroPoint], rt) {
      const out = rt.outputs[0];
      const [lo, hi] = INT_RANGE[out.dataType];
      const n = input.data.length;
      const res = new (api.typedArrayFor(out.dataType))(n);
      const offs = blockOffsets(input.shape, scale.shape);
      const x = input.data;
      const s = scale.data;
      const z = zeroPoint.data;
      for (let i = 0; i < n; i++) {
        const o = offs[i];
        const v = roundEven(x[i] / s[o]) + z[o];
        res[i] = Number.isNaN(v) ? 0 : v < lo ? lo : v > hi ? hi : v;
      }
      return res;
    },
  });

  defineOp('dequantizeLinear', {
    args: ['input:operand', 'scale:operand', 'zeroPoint:operand', 'options'],
    limits: {
      input: tl(['int8', 'uint8', 'int32']), scale: tl(FLOAT_TYPES), zeroPoint: tl(['int8', 'uint8', 'int32']), output: tl(FLOAT_TYPES),
    },
    validate(ctx, input, scale, zeroPoint) {
      if (input.dataType !== zeroPoint.dataType) ctx.fail(`The data type of zeroPoint (${zeroPoint.dataType}) must equal that of input (${input.dataType})`);
      checkBlocks(ctx, input, scale, zeroPoint);
      return { dataType: scale.dataType, shape: input.shape.slice() };
    },
    run([input, scale, zeroPoint], rt) {
      const out = rt.outputs[0];
      const n = input.data.length;
      const res = new (api.typedArrayFor(out.dataType))(n);
      const offs = blockOffsets(input.shape, scale.shape);
      const x = input.data;
      const s = scale.data;
      const z = zeroPoint.data;
      for (let i = 0; i < n; i++) {
        const o = offs[i];
        res[i] = (x[i] - z[o]) * s[o];
      }
      return res;
    },
  });

  // ---- cast ---------------------------------------------------------------------------------------------------
  const TWO63 = 2 ** 63;
  const TWO64 = 2 ** 64;
  const INT64_MIN = -(2n ** 63n);
  const INT64_MAX = 2n ** 63n - 1n;
  const UINT64_MAX = 2n ** 64n - 1n;
  // Number (a double) -> the integer type: truncate toward zero, saturate, NaN becomes 0.
  const numberToInt = (to) => {
    if (to === 'int64') return (x) => (x !== x ? 0n : x >= TWO63 ? INT64_MAX : x <= -TWO63 ? INT64_MIN : BigInt(Math.trunc(x)));
    if (to === 'uint64') return (x) => (x !== x || x <= 0 ? 0n : x >= TWO64 ? UINT64_MAX : BigInt(Math.trunc(x)));
    const [lo, hi] = to === 'uint32' ? [0, 2 ** 32 - 1] : INT_RANGE[to];
    return (x) => {
      if (x !== x) return 0;
      const t = Math.trunc(x);
      return t < lo ? lo : t > hi ? hi : t;
    };
  };
  const bigintToInt = (to) => {
    if (to === 'int64') return (x) => x;
    if (to === 'uint64') return (x) => (x < 0n ? 0n : x);
    const [lo, hi] = to === 'uint32' ? [0, 2 ** 32 - 1] : INT_RANGE[to];
    const blo = BigInt(lo);
    const bhi = BigInt(hi);
    return (x) => Number(x < blo ? blo : x > bhi ? bhi : x);
  };

  defineOp('cast', {
    args: ['input:operand', 'type:dtype', 'options'],
    limits: { input: tl(ALL_TYPES), output: tl(ALL_TYPES) },
    validate(ctx, input, type) {
      return { dataType: type, shape: input.shape.slice() };
    },
    run([input, type], rt) {
      const from = input.dataType;
      const to = rt.outputs[0].dataType;
      const src = input.data;
      const n = src.length;
      if (from === to) return src.slice();
      const res = new (api.typedArrayFor(to))(n);
      const toFloat = to === 'float32' || to === 'float16';
      if (isBigInt(from)) {
        if (toFloat) for (let i = 0; i < n; i++) res[i] = Number(src[i]);
        else {
          const f = bigintToInt(to);
          if (isBigInt(to)) for (let i = 0; i < n; i++) res[i] = f(BigInt(src[i]));
          else for (let i = 0; i < n; i++) res[i] = f(src[i]);
        }
      } else if (toFloat) {
        for (let i = 0; i < n; i++) res[i] = src[i];
      } else {
        const f = numberToInt(to);
        for (let i = 0; i < n; i++) res[i] = f(src[i]);
      }
      return res;
    },
  });

  // ---- where ----------------------------------------------------------------------------------------------------
  defineOp('where', {
    args: ['condition:operand', 'trueValue:operand', 'falseValue:operand', 'options'],
    limits: { condition: tl(['uint8']), trueValue: tl(ALL_TYPES), falseValue: tl(ALL_TYPES), output: tl(ALL_TYPES) },
    validate(ctx, condition, trueValue, falseValue) {
      if (trueValue.dataType !== falseValue.dataType) ctx.fail(`The data types of trueValue (${trueValue.dataType}) and falseValue (${falseValue.dataType}) must be the same`);
      const tf = broadcastShapes(trueValue.shape, falseValue.shape);
      if (!tf) ctx.fail(`The shapes of trueValue [${trueValue.shape}] and falseValue [${falseValue.shape}] are not broadcastable`);
      const shape = broadcastShapes(condition.shape, tf);
      if (!shape) ctx.fail(`The shape of condition [${condition.shape}] is not broadcastable with [${tf}]`);
      return { dataType: trueValue.dataType, shape };
    },
    run([condition, t, f], rt) {
      const out = rt.outputs[0];
      const res = new (api.typedArrayFor(out.dataType))(sizeOf(out.shape));
      const strides = [broadcastStrides(condition.shape, out.shape), broadcastStrides(t.shape, out.shape), broadcastStrides(f.shape, out.shape)];
      const cd = condition.data;
      const td = t.data;
      const fd = f.data;
      walk(out.shape, strides, (i, o) => {
        res[i] = cd[o[0]] !== 0 ? td[o[1]] : fd[o[2]];
      });
      return res;
    },
  });
}
