// Element-wise operators (https://www.w3.org/TR/webnn/#api-mlgraphbuilder-binary, -unary, -logical):
//   binary   add sub mul div max min pow            (numpy/WebNN bidirectional broadcasting, same data type)
//   unary    abs ceil cos erf exp floor log neg reciprocal roundEven sign sin sqrt tan
//   logical  equal notEqual greater greaterOrEqual lesser lesserOrEqual (any type in, uint8 out),
//            logicalAnd logicalOr logicalXor logicalNot (uint8 in/out), isNaN isInfinite (float in, uint8 out)
// Kernels compute in double precision (BigInt for int64/uint64) and round once to the output data type by the
// typed-array store (float32: fround, float16: round-to-nearest-even, integers: wrap-around). `identity` belongs to
// tensor_ops.js.
(api) => {
  const { defineOp, tl, ALL_TYPES, FLOAT_TYPES, broadcastShapes, mapBinary, isBigInt } = api;

  const SIGNED_INT_TYPES = ['int32', 'int8', 'int64'];
  const SIGNED_AND_FLOAT = FLOAT_TYPES.concat(SIGNED_INT_TYPES);

  // ---- binary arithmetic ----------------------------------------------------------------------------------
  // `num(x, y)` for float and up-to-32-bit integer types, `big(x, y)` for int64/uint64 (BigInt operands). A
  // `num32` override handles int32/uint32 where the exact result may exceed 2^53 (mul).
  const arithmetic = (name, types, num, big, num32) => {
    defineOp(name, {
      args: ['a:operand', 'b:operand', 'options'],
      limits: { a: tl(types), b: tl(types), output: tl(types) },
      validate(ctx, a, b) {
        if (a.dataType !== b.dataType) ctx.fail(`The data types of the two inputs differ (${a.dataType} and ${b.dataType})`);
        const shape = broadcastShapes(a.shape, b.shape);
        if (!shape) ctx.fail(`The shapes [${a.shape}] and [${b.shape}] are not broadcastable`);
        return { dataType: a.dataType, shape };
      },
      run([a, b], rt) {
        const out = rt.outputs[0];
        const dt = out.dataType;
        const f = isBigInt(dt) ? big : (dt === 'int32' || dt === 'uint32') && num32 ? num32 : num;
        return mapBinary(a, b, out.shape, dt, f);
      },
    });
  };
  arithmetic('add', ALL_TYPES, (x, y) => x + y, (x, y) => x + y);
  arithmetic('sub', ALL_TYPES, (x, y) => x - y, (x, y) => x - y);
  arithmetic('mul', ALL_TYPES, (x, y) => x * y, (x, y) => x * y, Math.imul);
  // Integer division truncates toward zero; a zero divisor has no defined result and yields 0 (as a wrapped
  // infinity/NaN conversion does for the other integer types).
  arithmetic('div', ALL_TYPES, (x, y) => x / y, (x, y) => (y === 0n ? 0n : x / y));
  // IEEE max/min: a NaN operand gives NaN, -0 is smaller than +0 (Math.max/min already do both).
  arithmetic('max', ALL_TYPES, Math.max, (x, y) => (x > y ? x : y));
  arithmetic('min', ALL_TYPES, Math.min, (x, y) => (x < y ? x : y));

  // pow: floating point as C99 pow (pow(1, y) = 1 and pow(-1, +-inf) = 1 for every y, unlike ECMAScript),
  // integers by exact modular exponentiation (mod 2^64, the store narrows): a negative exponent gives the
  // truncated reciprocal power (x^-n is 0 unless |x| = 1).
  const powFloat = (x, y) => {
    if (x === 1 || y === 0) return 1;
    if (x === -1 && (y === Infinity || y === -Infinity)) return 1;
    return Math.pow(x, y);
  };
  const powBig = (x, y) => {
    if (y < 0n) {
      if (x === 1n) return 1n;
      if (x === -1n) return y % 2n === 0n ? 1n : -1n;
      return 0n;
    }
    let result = 1n;
    let base = BigInt.asUintN(64, x);
    let e = y;
    while (e > 0n) {
      if (e & 1n) result = BigInt.asUintN(64, result * base);
      e >>= 1n;
      if (e > 0n) base = BigInt.asUintN(64, base * base);
    }
    return result;
  };
  const POW_TYPES = FLOAT_TYPES.concat(['int32', 'int8', 'uint8', 'uint32', 'int64', 'uint64']);
  defineOp('pow', {
    args: ['a:operand', 'b:operand', 'options'],
    limits: { a: tl(POW_TYPES), b: tl(POW_TYPES), output: tl(POW_TYPES) },
    validate(ctx, a, b) {
      if (a.dataType !== b.dataType) ctx.fail(`The data types of the two inputs differ (${a.dataType} and ${b.dataType})`);
      const shape = broadcastShapes(a.shape, b.shape);
      if (!shape) ctx.fail(`The shapes [${a.shape}] and [${b.shape}] are not broadcastable`);
      return { dataType: a.dataType, shape };
    },
    run([a, b], rt) {
      const out = rt.outputs[0];
      const dt = out.dataType;
      let f;
      if (dt === 'float32' || dt === 'float16') f = powFloat;
      else if (isBigInt(dt)) f = powBig;
      else f = (x, y) => Number(BigInt.asIntN(32, powBig(BigInt(x), BigInt(y))));
      return mapBinary(a, b, out.shape, dt, f);
    },
  });

  // ---- comparisons and logical operators ------------------------------------------------------------------
  // The output is uint8 0/1. Equality of NaN is false (and notEqual true), -0 equals +0: the JS operators agree.
  const comparison = (name, fn) => {
    defineOp(name, {
      args: ['a:operand', 'b:operand', 'options'],
      limits: { a: tl(ALL_TYPES), b: tl(ALL_TYPES), output: tl(['uint8']) },
      validate(ctx, a, b) {
        if (a.dataType !== b.dataType) ctx.fail(`The data types of the two inputs differ (${a.dataType} and ${b.dataType})`);
        const shape = broadcastShapes(a.shape, b.shape);
        if (!shape) ctx.fail(`The shapes [${a.shape}] and [${b.shape}] are not broadcastable`);
        return { dataType: 'uint8', shape };
      },
      run: ([a, b], rt) => mapBinary(a, b, rt.outputs[0].shape, 'uint8', fn),
    });
  };
  comparison('equal', (x, y) => (x == y ? 1 : 0)); // eslint-disable-line eqeqeq
  comparison('notEqual', (x, y) => (x != y ? 1 : 0)); // eslint-disable-line eqeqeq
  comparison('greater', (x, y) => (x > y ? 1 : 0));
  comparison('greaterOrEqual', (x, y) => (x >= y ? 1 : 0));
  comparison('lesser', (x, y) => (x < y ? 1 : 0));
  comparison('lesserOrEqual', (x, y) => (x <= y ? 1 : 0));

  // Logical operators: uint8 operands, non-zero is true.
  const logical = (name, fn) => {
    defineOp(name, {
      args: ['a:operand', 'b:operand', 'options'],
      limits: { a: tl(['uint8']), b: tl(['uint8']), output: tl(['uint8']) },
      validate(ctx, a, b) {
        if (a.dataType !== b.dataType) ctx.fail(`The data types of the two inputs differ (${a.dataType} and ${b.dataType})`);
        const shape = broadcastShapes(a.shape, b.shape);
        if (!shape) ctx.fail(`The shapes [${a.shape}] and [${b.shape}] are not broadcastable`);
        return { dataType: 'uint8', shape };
      },
      run: ([a, b], rt) => mapBinary(a, b, rt.outputs[0].shape, 'uint8', fn),
    });
  };
  logical('logicalAnd', (x, y) => (x !== 0 && y !== 0 ? 1 : 0));
  logical('logicalOr', (x, y) => (x !== 0 || y !== 0 ? 1 : 0));
  logical('logicalXor', (x, y) => ((x !== 0) !== (y !== 0) ? 1 : 0));

  // Unary predicates, output uint8 of the input's shape.
  const predicate = (name, types, test) => {
    defineOp(name, {
      args: ['a:operand', 'options'],
      limits: { a: tl(types), output: tl(['uint8']) },
      validate: (ctx, a) => ({ dataType: 'uint8', shape: a.shape.slice() }),
      run([a]) {
        const d = a.data;
        const out = new Uint8Array(d.length);
        for (let i = 0; i < out.length; i++) out[i] = test(d[i]) ? 1 : 0;
        return out;
      },
    });
  };
  predicate('logicalNot', ['uint8'], (x) => x === 0);
  predicate('isNaN', FLOAT_TYPES, (x) => x !== x);
  predicate('isInfinite', FLOAT_TYPES, (x) => x === Infinity || x === -Infinity);

  // ---- unary ----------------------------------------------------------------------------------------------
  // erf in double precision: Maclaurin series for small |x|, the continued fraction of erfc beyond.
  const erf = (x) => {
    if (x !== x) return x;
    const ax = Math.abs(x);
    if (ax >= 6) return x > 0 ? 1 : -1;
    let r;
    if (ax < 2.5) {
      // erf(x) = 2/sqrt(pi) * sum_n (-1)^n x^(2n+1) / (n! (2n+1))
      let term = ax;
      let sum = ax;
      const x2 = ax * ax;
      for (let n = 1; n < 100; n++) {
        term *= -x2 / n;
        const t = term / (2 * n + 1);
        sum += t;
        if (Math.abs(t) < 1e-18 * Math.abs(sum)) break;
      }
      r = (2 / Math.sqrt(Math.PI)) * sum;
    } else {
      // erfc(x) = exp(-x^2)/(x sqrt(pi)) * 1/(1+ 1/(2x^2)/(1+ 2/(2x^2)/(1+ ...))) evaluated bottom-up (Lentz-free, fixed depth)
      const x2 = ax * ax;
      let f = 0;
      for (let k = 60; k >= 1; k--) f = (k / 2) / (ax + f);
      r = 1 - Math.exp(-x2) / (Math.sqrt(Math.PI) * (ax + f));
    }
    return x < 0 ? -r : r;
  };
  const roundEven = (x) => {
    if (!isFinite(x)) return x;
    const f = Math.floor(x);
    const d = x - f;
    let r;
    if (d < 0.5) r = f;
    else if (d > 0.5) r = f + 1;
    else r = f % 2 === 0 ? f : f + 1;
    return r === 0 && (x < 0 || Object.is(x, -0)) ? -0 : r;
  };
  const sign = (x) => (x > 0 ? 1 : x < 0 ? -1 : x);

  const unary = (name, types, num, big) => {
    defineOp(name, {
      args: ['input:operand', 'options'],
      limits: { input: tl(types), output: tl(types) },
      validate: (ctx, input) => ({ dataType: input.dataType, shape: input.shape.slice() }),
      run([input], rt) {
        const d = input.data;
        const out = new d.constructor(d.length);
        const f = isBigInt(rt.outputs[0].dataType) ? big : num;
        for (let i = 0; i < d.length; i++) out[i] = f(d[i]);
        return out;
      },
    });
  };
  unary('abs', SIGNED_AND_FLOAT, Math.abs, (x) => (x < 0n ? -x : x));
  unary('neg', SIGNED_AND_FLOAT, (x) => -x, (x) => -x);
  unary('sign', SIGNED_AND_FLOAT, sign, (x) => (x > 0n ? 1n : x < 0n ? -1n : 0n));
  unary('ceil', FLOAT_TYPES, Math.ceil);
  unary('floor', FLOAT_TYPES, Math.floor);
  unary('roundEven', FLOAT_TYPES, roundEven);
  unary('cos', FLOAT_TYPES, Math.cos);
  unary('sin', FLOAT_TYPES, Math.sin);
  unary('tan', FLOAT_TYPES, Math.tan);
  unary('erf', FLOAT_TYPES, erf);
  unary('exp', FLOAT_TYPES, Math.exp);
  unary('log', FLOAT_TYPES, Math.log);
  unary('reciprocal', FLOAT_TYPES, (x) => 1 / x);
  unary('sqrt', FLOAT_TYPES, Math.sqrt);
}
