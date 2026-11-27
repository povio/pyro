// The operators the core's own tests need: add, sub, mul, identity, logicalAnd/Or/Xor/Not. Real implementations (broadcasting,
// every data type, wrap-around integer arithmetic), but only stand-ins: elementwise.js and tensor_ops.js
// define the same names later (logical ops: elementwise.js) and replace them (the last definition wins), after which the matching
// definitions here can be deleted. One expression evaluating to `(api) => {...}`, no trailing semicolon.
(api) => {
  const { defineOp, tl, ALL_TYPES, sizeOf, broadcastShapes, mapBinary } = api;

  const binary = (name, fn, intFn) => {
    defineOp(name, {
      args: ['a:operand', 'b:operand', 'options'],
      limits: { a: tl(ALL_TYPES), b: tl(ALL_TYPES), output: tl(ALL_TYPES) },
      validate(ctx, a, b) {
        if (a.dataType !== b.dataType) ctx.fail(`The data types of the two inputs differ (${a.dataType} and ${b.dataType})`);
        const shape = broadcastShapes(a.shape, b.shape);
        if (!shape) ctx.fail(`The shapes [${a.shape}] and [${b.shape}] are not broadcastable`);
        return { dataType: a.dataType, shape };
      },
      run(args, rt) {
        const [a, b] = args;
        const out = rt.outputs[0];
        // Integer products of two 32-bit values exceed 2^53: take the low 32 bits with imul; the typed-array
        // store then wraps to the exact type. All other cases are exact in a double, then rounded once.
        const f = (out.dataType === 'int32' || out.dataType === 'uint32') && intFn ? intFn : fn;
        return mapBinary(a, b, out.shape, out.dataType, f);
      },
    });
  };
  binary('add', (x, y) => x + y);
  binary('sub', (x, y) => x - y);
  binary('mul', (x, y) => x * y, Math.imul);

  defineOp('identity', {
    args: ['input:operand', 'options'],
    limits: { input: tl(ALL_TYPES), output: tl(ALL_TYPES) },
    validate: (ctx, input) => ({ dataType: input.dataType, shape: input.shape.slice() }),
    run: (args) => args[0].data,
  });

  // Logical ops: uint8 operands, non-zero is true, the result is 0 or 1.
  const logical = (name, fn) => {
    defineOp(name, {
      args: ['a:operand', 'b:operand', 'options'],
      limits: { a: tl(['uint8']), b: tl(['uint8']), output: tl(['uint8']) },
      validate(ctx, a, b) {
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
  defineOp('logicalNot', {
    args: ['a:operand', 'options'],
    limits: { a: tl(['uint8']), output: tl(['uint8']) },
    validate: (ctx, a) => ({ dataType: 'uint8', shape: a.shape.slice() }),
    run([a]) {
      const out = new Uint8Array(a.data.length);
      for (let i = 0; i < out.length; i++) out[i] = a.data[i] === 0 ? 1 : 0;
      return out;
    },
  });
}
