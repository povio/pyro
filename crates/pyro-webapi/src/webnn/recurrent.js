// WebNN operator family: gru, gruCell, lstm, lstmCell. The gate arithmetic is computed here directly, in double
// precision (the hidden and cell state stay doubles between steps), and rounded once into the output type.
// One expression `(api) => {...}`.
(api) => {
  const { defineOp, tl, FLOAT_TYPES, sizeOf, typedArrayFor, bytesPer, maxTensorByteLength } = api;

  const ACTIVATIONS = {
    relu: (x) => (x > 0 ? x : 0),
    sigmoid: (x) => 1 / (1 + Math.exp(-x)),
    tanh: Math.tanh,
  };
  const ACT_ENUM = 'seq<enum:relu|sigmoid|tanh>';
  const sameShape = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);
  const dot = (x, xo, w, wo, n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) acc += x[xo + i] * w[wo + i];
    return acc;
  };

  // ---- validation helpers --------------------------------------------------------------------------------
  const requirePositive = (ctx, name, v) => {
    if (v === 0) ctx.fail(`${name} must be greater than 0`);
  };
  const requireType = (ctx, name, operand, dataType) => {
    if (operand && operand.dataType !== dataType) ctx.fail(`The data type of ${name} (${operand.dataType}) must equal that of input (${dataType})`);
  };
  const requireShape = (ctx, name, operand, expected) => {
    if (operand && !sameShape(operand.shape, expected)) ctx.fail(`The shape of ${name} [${operand.shape}] must be [${expected}]`);
  };
  const checkActivations = (ctx, activations, count) => {
    if (activations !== undefined && activations.length !== count) ctx.fail(`options.activations must have ${count} elements, but has ${activations.length}`);
  };
  const checkBytes = (ctx, what, shape, dataType) => {
    if (sizeOf(shape) * bytesPer(dataType) > maxTensorByteLength) ctx.fail(`The ${what} tensor byte length exceeds the limit ${maxTensorByteLength}`);
  };
  const activationsOf = (list, defaults) => (list === undefined ? defaults : list).map((n) => ACTIVATIONS[n]);

  // ---- GRU ---------------------------------------------------------------------------------------------------
  const GRU_OPTIONS = {
    bias: 'operand',
    recurrentBias: 'operand',
    initialHiddenState: 'operand',
    resetAfter: { type: 'boolean', default: true },
    returnSequence: { type: 'boolean', default: false },
    direction: { type: 'enum:forward|backward|both', default: 'forward' },
    layout: { type: 'enum:zrn|rzn', default: 'zrn' },
    activations: ACT_ENUM,
  };
  const GRU_LIMITS = {
    input: tl(FLOAT_TYPES, 3, 3), weight: tl(FLOAT_TYPES, 3, 3), recurrentWeight: tl(FLOAT_TYPES, 3, 3), bias: tl(FLOAT_TYPES, 2, 2),
    recurrentBias: tl(FLOAT_TYPES, 2, 2), initialHiddenState: tl(FLOAT_TYPES, 3, 3), output0: tl(FLOAT_TYPES, 3, 3), output1: tl(FLOAT_TYPES, 4, 4),
  };
  const GRU_CELL_LIMITS = {
    input: tl(FLOAT_TYPES, 2, 2), weight: tl(FLOAT_TYPES, 2, 2), recurrentWeight: tl(FLOAT_TYPES, 2, 2), hiddenState: tl(FLOAT_TYPES, 2, 2),
    bias: tl(FLOAT_TYPES, 1, 1), recurrentBias: tl(FLOAT_TYPES, 1, 1), output: tl(FLOAT_TYPES, 2, 2),
  };

  // One GRU step for every batch row. W/R/B/RB are the flat typed arrays, `wb`/`rb`/`bb` the offsets of this
  // direction's slab, `h` the previous hidden state (batch x H doubles); returns the new hidden state.
  const gruStep = (p, xs, xo, h) => {
    const { W, wb, R, rb, B, bb, RB, rbb, batch, I, H, zIdx, rIdx, resetAfter, f, g } = p;
    const out = new Float64Array(batch * H);
    const z = new Float64Array(H);
    const r = new Float64Array(H);
    const rh = new Float64Array(H);
    for (let b = 0; b < batch; b++) {
      const xOff = xo + b * I;
      const hOff = b * H;
      for (let u = 0; u < H; u++) {
        const zr = (zIdx * H + u);
        const rr = (rIdx * H + u);
        let zs = dot(xs, xOff, W, wb + zr * I, I) + dot(h, hOff, R, rb + zr * H, H);
        let rs = dot(xs, xOff, W, wb + rr * I, I) + dot(h, hOff, R, rb + rr * H, H);
        if (B) {
          zs += B[bb + zr];
          rs += B[bb + rr];
        }
        if (RB) {
          zs += RB[rbb + zr];
          rs += RB[rbb + rr];
        }
        z[u] = f(zs);
        r[u] = f(rs);
        rh[u] = r[u] * h[hOff + u];
      }
      for (let u = 0; u < H; u++) {
        const nr = 2 * H + u;
        let ns = dot(xs, xOff, W, wb + nr * I, I);
        if (resetAfter) {
          let rec = dot(h, hOff, R, rb + nr * H, H);
          if (RB) rec += RB[rbb + nr];
          ns += (B ? B[bb + nr] : 0) + r[u] * rec;
        } else {
          ns += dot(rh, 0, R, rb + nr * H, H) + (RB ? RB[rbb + nr] : 0) + (B ? B[bb + nr] : 0);
        }
        const n = g(ns);
        out[hOff + u] = (1 - z[u]) * n + z[u] * h[hOff + u];
      }
    }
    return out;
  };
  const gruParams = (o, W, R, batch, I, H, dir) => ({
    W: W.data, wb: dir * 3 * H * I, R: R.data, rb: dir * 3 * H * H,
    B: o.bias ? o.bias.data : null, bb: dir * 3 * H, RB: o.recurrentBias ? o.recurrentBias.data : null, rbb: dir * 3 * H,
    batch, I, H, zIdx: o.layout === 'zrn' ? 0 : 1, rIdx: o.layout === 'zrn' ? 1 : 0, resetAfter: o.resetAfter,
    f: null, g: null,
  });

  defineOp('gru', {
    args: ['input:operand', 'weight:operand', 'recurrentWeight:operand', 'steps:ul', 'hiddenSize:ul', 'options'],
    options: GRU_OPTIONS,
    limits: GRU_LIMITS,
    validate(ctx, input, weight, recurrentWeight, steps, hiddenSize, o) {
      requirePositive(ctx, 'steps', steps);
      requirePositive(ctx, 'hiddenSize', hiddenSize);
      const dt = input.dataType;
      requireType(ctx, 'weight', weight, dt);
      requireType(ctx, 'recurrentWeight', recurrentWeight, dt);
      requireType(ctx, 'options.bias', o.bias, dt);
      requireType(ctx, 'options.recurrentBias', o.recurrentBias, dt);
      requireType(ctx, 'options.initialHiddenState', o.initialHiddenState, dt);
      if (input.shape[0] !== steps) ctx.fail(`The first dimension of input (${input.shape[0]}) must equal steps (${steps})`);
      const batch = input.shape[1];
      const inputSize = input.shape[2];
      const numDir = o.direction === 'both' ? 2 : 1;
      requireShape(ctx, 'weight', weight, [numDir, 3 * hiddenSize, inputSize]);
      requireShape(ctx, 'recurrentWeight', recurrentWeight, [numDir, 3 * hiddenSize, hiddenSize]);
      requireShape(ctx, 'options.bias', o.bias, [numDir, 3 * hiddenSize]);
      requireShape(ctx, 'options.recurrentBias', o.recurrentBias, [numDir, 3 * hiddenSize]);
      requireShape(ctx, 'options.initialHiddenState', o.initialHiddenState, [numDir, batch, hiddenSize]);
      checkActivations(ctx, o.activations, 2);
      const sequence = [steps, numDir, batch, hiddenSize];
      checkBytes(ctx, 'full-sequence output', sequence, dt);
      const outputs = [{ dataType: dt, shape: [numDir, batch, hiddenSize] }];
      if (o.returnSequence) outputs.push({ dataType: dt, shape: sequence });
      return outputs;
    },
    run([input, weight, recurrentWeight, steps, hiddenSize, o], rt) {
      const [, batch, I] = input.shape;
      const H = hiddenSize;
      const numDir = o.direction === 'both' ? 2 : 1;
      const [f, g] = activationsOf(o.activations, ['sigmoid', 'tanh']);
      const T = typedArrayFor(input.dataType);
      const last = new T(numDir * batch * H);
      const seq = o.returnSequence ? new T(steps * numDir * batch * H) : null;
      for (let d = 0; d < numDir; d++) {
        const backward = o.direction === 'backward' || (o.direction === 'both' && d === 1);
        const p = gruParams(o, weight, recurrentWeight, batch, I, H, d);
        p.f = f;
        p.g = g;
        let h = new Float64Array(batch * H);
        if (o.initialHiddenState) for (let i = 0; i < h.length; i++) h[i] = o.initialHiddenState.data[d * batch * H + i];
        for (let s = 0; s < steps; s++) {
          const t = backward ? steps - 1 - s : s;
          h = gruStep(p, input.data, t * batch * I, h);
          if (seq) for (let i = 0; i < h.length; i++) seq[(t * numDir + d) * batch * H + i] = h[i];
        }
        for (let i = 0; i < h.length; i++) last[d * batch * H + i] = h[i];
      }
      return seq ? [last, seq] : [last];
    },
  });

  defineOp('gruCell', {
    args: ['input:operand', 'weight:operand', 'recurrentWeight:operand', 'hiddenState:operand', 'hiddenSize:ul', 'options'],
    options: {
      bias: 'operand', recurrentBias: 'operand', resetAfter: { type: 'boolean', default: true },
      layout: { type: 'enum:zrn|rzn', default: 'zrn' }, activations: ACT_ENUM,
    },
    limits: GRU_CELL_LIMITS,
    validate(ctx, input, weight, recurrentWeight, hiddenState, hiddenSize, o) {
      requirePositive(ctx, 'hiddenSize', hiddenSize);
      const dt = input.dataType;
      requireType(ctx, 'weight', weight, dt);
      requireType(ctx, 'recurrentWeight', recurrentWeight, dt);
      requireType(ctx, 'hiddenState', hiddenState, dt);
      requireType(ctx, 'options.bias', o.bias, dt);
      requireType(ctx, 'options.recurrentBias', o.recurrentBias, dt);
      const batch = input.shape[0];
      requireShape(ctx, 'weight', weight, [3 * hiddenSize, input.shape[1]]);
      requireShape(ctx, 'recurrentWeight', recurrentWeight, [3 * hiddenSize, hiddenSize]);
      requireShape(ctx, 'hiddenState', hiddenState, [batch, hiddenSize]);
      requireShape(ctx, 'options.bias', o.bias, [3 * hiddenSize]);
      requireShape(ctx, 'options.recurrentBias', o.recurrentBias, [3 * hiddenSize]);
      checkActivations(ctx, o.activations, 2);
      return { dataType: dt, shape: [batch, hiddenSize] };
    },
    run([input, weight, recurrentWeight, hiddenState, hiddenSize, o], rt) {
      const [batch, I] = input.shape;
      const H = hiddenSize;
      const [f, g] = activationsOf(o.activations, ['sigmoid', 'tanh']);
      const p = gruParams(o, weight, recurrentWeight, batch, I, H, 0);
      p.f = f;
      p.g = g;
      const h = Float64Array.from(hiddenState.data);
      const out = gruStep(p, input.data, 0, h);
      const res = new (typedArrayFor(input.dataType))(batch * H);
      res.set(out);
      return res;
    },
  });

  // ---- LSTM --------------------------------------------------------------------------------------------------
  const LSTM_LAYOUT = { iofg: { i: 0, o: 1, f: 2, g: 3 }, ifgo: { i: 0, f: 1, g: 2, o: 3 } };
  // One LSTM step for every batch row; returns [hidden, cell] as Float64Arrays.
  const lstmStep = (p, xs, xo, h, c) => {
    const { W, wb, R, rb, B, bb, RB, rbb, P, pb, batch, I, H, gate, f, g, hAct } = p;
    const hOut = new Float64Array(batch * H);
    const cOut = new Float64Array(batch * H);
    const pre = (b, k, u) => {
      const row = k * H + u;
      let s = dot(xs, xo + b * I, W, wb + row * I, I) + dot(h, b * H, R, rb + row * H, H);
      if (B) s += B[bb + row];
      if (RB) s += RB[rbb + row];
      return s;
    };
    for (let b = 0; b < batch; b++) {
      for (let u = 0; u < H; u++) {
        const cPrev = c[b * H + u];
        const it = f(pre(b, gate.i, u) + (P ? P[pb + u] * cPrev : 0));
        const ft = f(pre(b, gate.f, u) + (P ? P[pb + 2 * H + u] * cPrev : 0));
        const gt = g(pre(b, gate.g, u));
        const ct = ft * cPrev + it * gt;
        const ot = f(pre(b, gate.o, u) + (P ? P[pb + H + u] * ct : 0));
        cOut[b * H + u] = ct;
        hOut[b * H + u] = ot * hAct(ct);
      }
    }
    return [hOut, cOut];
  };
  const lstmParams = (o, W, R, batch, I, H, dir) => ({
    W: W.data, wb: dir * 4 * H * I, R: R.data, rb: dir * 4 * H * H,
    B: o.bias ? o.bias.data : null, bb: dir * 4 * H, RB: o.recurrentBias ? o.recurrentBias.data : null, rbb: dir * 4 * H,
    P: o.peepholeWeight ? o.peepholeWeight.data : null, pb: dir * 3 * H, batch, I, H, gate: LSTM_LAYOUT[o.layout],
    f: null, g: null, hAct: null,
  });
  const setActs = (p, list) => {
    [p.f, p.g, p.hAct] = activationsOf(list, ['sigmoid', 'tanh', 'tanh']);
  };

  defineOp('lstm', {
    args: ['input:operand', 'weight:operand', 'recurrentWeight:operand', 'steps:ul', 'hiddenSize:ul', 'options'],
    options: {
      bias: 'operand', recurrentBias: 'operand', peepholeWeight: 'operand', initialHiddenState: 'operand', initialCellState: 'operand',
      returnSequence: { type: 'boolean', default: false },
      direction: { type: 'enum:forward|backward|both', default: 'forward' },
      layout: { type: 'enum:iofg|ifgo', default: 'iofg' },
      activations: ACT_ENUM,
    },
    limits: {
      input: tl(FLOAT_TYPES, 3, 3), weight: tl(FLOAT_TYPES, 3, 3), recurrentWeight: tl(FLOAT_TYPES, 3, 3), bias: tl(FLOAT_TYPES, 2, 2),
      recurrentBias: tl(FLOAT_TYPES, 2, 2), peepholeWeight: tl(FLOAT_TYPES, 2, 2), initialHiddenState: tl(FLOAT_TYPES, 3, 3),
      initialCellState: tl(FLOAT_TYPES, 3, 3), output0: tl(FLOAT_TYPES, 3, 3), output1: tl(FLOAT_TYPES, 3, 3), output2: tl(FLOAT_TYPES, 4, 4),
    },
    validate(ctx, input, weight, recurrentWeight, steps, hiddenSize, o) {
      requirePositive(ctx, 'steps', steps);
      requirePositive(ctx, 'hiddenSize', hiddenSize);
      const dt = input.dataType;
      requireType(ctx, 'weight', weight, dt);
      requireType(ctx, 'recurrentWeight', recurrentWeight, dt);
      for (const k of ['bias', 'recurrentBias', 'peepholeWeight', 'initialHiddenState', 'initialCellState']) requireType(ctx, `options.${k}`, o[k], dt);
      if (input.shape[0] !== steps) ctx.fail(`The first dimension of input (${input.shape[0]}) must equal steps (${steps})`);
      const batch = input.shape[1];
      const inputSize = input.shape[2];
      const numDir = o.direction === 'both' ? 2 : 1;
      requireShape(ctx, 'weight', weight, [numDir, 4 * hiddenSize, inputSize]);
      requireShape(ctx, 'recurrentWeight', recurrentWeight, [numDir, 4 * hiddenSize, hiddenSize]);
      requireShape(ctx, 'options.bias', o.bias, [numDir, 4 * hiddenSize]);
      requireShape(ctx, 'options.recurrentBias', o.recurrentBias, [numDir, 4 * hiddenSize]);
      requireShape(ctx, 'options.peepholeWeight', o.peepholeWeight, [numDir, 3 * hiddenSize]);
      requireShape(ctx, 'options.initialHiddenState', o.initialHiddenState, [numDir, batch, hiddenSize]);
      requireShape(ctx, 'options.initialCellState', o.initialCellState, [numDir, batch, hiddenSize]);
      checkActivations(ctx, o.activations, 3);
      const sequence = [steps, numDir, batch, hiddenSize];
      checkBytes(ctx, 'full-sequence output', sequence, dt);
      const outputs = [{ dataType: dt, shape: [numDir, batch, hiddenSize] }, { dataType: dt, shape: [numDir, batch, hiddenSize] }];
      if (o.returnSequence) outputs.push({ dataType: dt, shape: sequence });
      return outputs;
    },
    run([input, weight, recurrentWeight, steps, hiddenSize, o], rt) {
      const [, batch, I] = input.shape;
      const H = hiddenSize;
      const numDir = o.direction === 'both' ? 2 : 1;
      const T = typedArrayFor(input.dataType);
      const lastH = new T(numDir * batch * H);
      const lastC = new T(numDir * batch * H);
      const seq = o.returnSequence ? new T(steps * numDir * batch * H) : null;
      for (let d = 0; d < numDir; d++) {
        const backward = o.direction === 'backward' || (o.direction === 'both' && d === 1);
        const p = lstmParams(o, weight, recurrentWeight, batch, I, H, d);
        setActs(p, o.activations);
        let h = new Float64Array(batch * H);
        let c = new Float64Array(batch * H);
        if (o.initialHiddenState) for (let i = 0; i < h.length; i++) h[i] = o.initialHiddenState.data[d * batch * H + i];
        if (o.initialCellState) for (let i = 0; i < c.length; i++) c[i] = o.initialCellState.data[d * batch * H + i];
        for (let s = 0; s < steps; s++) {
          const t = backward ? steps - 1 - s : s;
          [h, c] = lstmStep(p, input.data, t * batch * I, h, c);
          if (seq) for (let i = 0; i < h.length; i++) seq[(t * numDir + d) * batch * H + i] = h[i];
        }
        for (let i = 0; i < h.length; i++) {
          lastH[d * batch * H + i] = h[i];
          lastC[d * batch * H + i] = c[i];
        }
      }
      return seq ? [lastH, lastC, seq] : [lastH, lastC];
    },
  });

  defineOp('lstmCell', {
    args: ['input:operand', 'weight:operand', 'recurrentWeight:operand', 'hiddenState:operand', 'cellState:operand', 'hiddenSize:ul', 'options'],
    options: {
      bias: 'operand', recurrentBias: 'operand', peepholeWeight: 'operand',
      layout: { type: 'enum:iofg|ifgo', default: 'iofg' }, activations: ACT_ENUM,
    },
    limits: {
      input: tl(FLOAT_TYPES, 2, 2), weight: tl(FLOAT_TYPES, 2, 2), recurrentWeight: tl(FLOAT_TYPES, 2, 2), hiddenState: tl(FLOAT_TYPES, 2, 2),
      cellState: tl(FLOAT_TYPES, 2, 2), bias: tl(FLOAT_TYPES, 1, 1), recurrentBias: tl(FLOAT_TYPES, 1, 1), peepholeWeight: tl(FLOAT_TYPES, 1, 1),
      output0: tl(FLOAT_TYPES, 2, 2), output1: tl(FLOAT_TYPES, 2, 2),
    },
    validate(ctx, input, weight, recurrentWeight, hiddenState, cellState, hiddenSize, o) {
      requirePositive(ctx, 'hiddenSize', hiddenSize);
      const dt = input.dataType;
      requireType(ctx, 'weight', weight, dt);
      requireType(ctx, 'recurrentWeight', recurrentWeight, dt);
      requireType(ctx, 'hiddenState', hiddenState, dt);
      requireType(ctx, 'cellState', cellState, dt);
      for (const k of ['bias', 'recurrentBias', 'peepholeWeight']) requireType(ctx, `options.${k}`, o[k], dt);
      const batch = input.shape[0];
      requireShape(ctx, 'weight', weight, [4 * hiddenSize, input.shape[1]]);
      requireShape(ctx, 'recurrentWeight', recurrentWeight, [4 * hiddenSize, hiddenSize]);
      requireShape(ctx, 'hiddenState', hiddenState, [batch, hiddenSize]);
      requireShape(ctx, 'cellState', cellState, [batch, hiddenSize]);
      requireShape(ctx, 'options.bias', o.bias, [4 * hiddenSize]);
      requireShape(ctx, 'options.recurrentBias', o.recurrentBias, [4 * hiddenSize]);
      requireShape(ctx, 'options.peepholeWeight', o.peepholeWeight, [3 * hiddenSize]);
      checkActivations(ctx, o.activations, 3);
      return [{ dataType: dt, shape: [batch, hiddenSize] }, { dataType: dt, shape: [batch, hiddenSize] }];
    },
    run([input, weight, recurrentWeight, hiddenState, cellState, hiddenSize, o], rt) {
      const [batch, I] = input.shape;
      const H = hiddenSize;
      const p = lstmParams(o, weight, recurrentWeight, batch, I, H, 0);
      setActs(p, o.activations);
      const [h, c] = lstmStep(p, input.data, 0, Float64Array.from(hiddenState.data), Float64Array.from(cellState.data));
      const T = typedArrayFor(input.dataType);
      const hOut = new T(batch * H);
      const cOut = new T(batch * H);
      hOut.set(h);
      cOut.set(c);
      return [hOut, cOut];
    },
  });
}
