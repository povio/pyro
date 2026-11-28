// WebNN operator family: conv2d, convTranspose2d, averagePool2d, l2Pool2d, maxPool2d. Direct nested loops with
// double-precision accumulation, rounded once into the output type. One expression `(api) => {...}`.
(api) => {
  const { defineOp, tl, ALL_TYPES, FLOAT_TYPES, sizeOf, typedArrayFor, bytesPer, maxTensorByteLength } = api;

  const MAXU = 2 ** 32 - 1;
  const SEQ = 'seq<ul>';

  // ---- shared validation ---------------------------------------------------------------------------------
  // A list option of the given length (or its default); every element at least `min`.
  const listOption = (ctx, name, v, len, def, min) => {
    if (v === undefined) return def;
    if (v.length !== len) ctx.fail(`options.${name} must have ${len} elements, but has ${v.length}`);
    if (min !== undefined) for (const x of v) if (x < min) ctx.fail(`The elements of options.${name} must be at least ${min}`);
    return v;
  };
  // Dimensions of a 4-D tensor in the input layout: {n, c, h, w} and the strides that address them.
  const dimsOf = (layout, s) => (layout === 'nchw' ? { n: s[0], c: s[1], h: s[2], w: s[3] } : { n: s[0], h: s[1], w: s[2], c: s[3] });
  const shapeOf = (layout, n, c, h, w) => (layout === 'nchw' ? [n, c, h, w] : [n, h, w, c]);
  const stridesOfLayout = (layout, { n, c, h, w }) =>
    layout === 'nchw' ? { n: c * h * w, c: h * w, h: w, w: 1 } : { n: h * w * c, h: w * c, w: c, c: 1 };
  const sameType = (ctx, name, operand, dataType) => {
    if (operand && operand.dataType !== dataType) ctx.fail(`The data type of ${name} (${operand.dataType}) must equal that of input (${dataType})`);
  };
  // Shape checks shared by conv2d, convTranspose2d and the pools: the padded input must not exceed what the
  // implementation limits allow, and strides/dilations/effective window must fit in it.
  const checkWindow = (ctx, what, padded, eff, stride, dilation, axis) => {
    if (padded > MAXU) ctx.fail(`The padded input ${axis} (${padded}) is too large`);
    if (eff > MAXU) ctx.fail(`The effective ${what} ${axis} overflows`);
    if (stride > padded) ctx.fail(`The stride (${stride}) is larger than the padded input ${axis} (${padded})`);
    if (dilation > padded) ctx.fail(`The dilation (${dilation}) is larger than the padded input ${axis} (${padded})`);
    if (eff > padded) ctx.fail(`The effective ${what} ${axis} (${eff}) is larger than the padded input ${axis} (${padded})`);
  };
  const checkPaddedBytes = (ctx, n, c, ph, pw, dataType) => {
    if (n * c * ph * pw * bytesPer(dataType) > maxTensorByteLength) ctx.fail('The padded input tensor is too large');
  };

  // ---- conv2d --------------------------------------------------------------------------------------------
  // Filter dimensions {o, i, h, w} and strides by layout.
  const CONV_FILTER = {
    oihw: (s) => ({ o: s[0], i: s[1], h: s[2], w: s[3] }),
    hwio: (s) => ({ h: s[0], w: s[1], i: s[2], o: s[3] }),
    ohwi: (s) => ({ o: s[0], h: s[1], w: s[2], i: s[3] }),
    ihwo: (s) => ({ i: s[0], h: s[1], w: s[2], o: s[3] }),
  };
  const CONVT_FILTER = {
    iohw: (s) => ({ i: s[0], o: s[1], h: s[2], w: s[3] }),
    hwoi: (s) => ({ h: s[0], w: s[1], o: s[2], i: s[3] }),
    ohwi: (s) => ({ o: s[0], h: s[1], w: s[2], i: s[3] }),
  };
  const filterStrides = (letters, dims) => {
    // `letters` is the layout string; the stride of each letter is the product of the dims to its right.
    const out = {};
    let acc = 1;
    for (let k = letters.length - 1; k >= 0; k--) {
      out[letters[k]] = acc;
      acc *= dims[letters[k]];
    }
    return out;
  };

  const commonConvOptions = {
    padding: SEQ, strides: SEQ, dilations: SEQ, groups: { type: 'ul', default: 1 },
    inputLayout: { type: 'enum:nchw|nhwc', default: 'nchw' }, bias: 'operand',
  };
  const CONV_LIMITS = { input: tl(FLOAT_TYPES, 4, 4), filter: tl(FLOAT_TYPES, 4, 4), bias: tl(FLOAT_TYPES, 1, 1), output: tl(FLOAT_TYPES, 4, 4) };

  defineOp('conv2d', {
    args: ['input:operand', 'filter:operand', 'options'],
    options: { ...commonConvOptions, filterLayout: { type: 'enum:oihw|hwio|ohwi|ihwo', default: 'oihw' } },
    limits: CONV_LIMITS,
    validate(ctx, input, filter, o) {
      const dt = input.dataType;
      sameType(ctx, 'filter', filter, dt);
      sameType(ctx, 'options.bias', o.bias, dt);
      const padding = listOption(ctx, 'padding', o.padding, 4, [0, 0, 0, 0]);
      const strides = listOption(ctx, 'strides', o.strides, 2, [1, 1], 1);
      const dilations = listOption(ctx, 'dilations', o.dilations, 2, [1, 1], 1);
      const groups = o.groups;
      if (groups < 1) ctx.fail('options.groups must be at least 1');
      const inp = dimsOf(o.inputLayout, input.shape);
      const f = CONV_FILTER[o.filterLayout](filter.shape);
      if (inp.c % groups !== 0) ctx.fail(`The input channels (${inp.c}) must be a multiple of groups (${groups})`);
      if (f.i !== inp.c / groups) ctx.fail(`The filter input channels (${f.i}) must equal input channels / groups (${inp.c / groups})`);
      if (f.o % groups !== 0) ctx.fail(`The output channels (${f.o}) must be a multiple of groups (${groups})`);
      if (o.bias && o.bias.shape[0] !== f.o) ctx.fail(`The shape of options.bias [${o.bias.shape}] must be [${f.o}]`);
      const pH = inp.h + padding[0] + padding[1];
      const pW = inp.w + padding[2] + padding[3];
      const effH = (f.h - 1) * dilations[0] + 1;
      const effW = (f.w - 1) * dilations[1] + 1;
      checkWindow(ctx, 'filter', pH, effH, strides[0], dilations[0], 'height');
      checkWindow(ctx, 'filter', pW, effW, strides[1], dilations[1], 'width');
      checkPaddedBytes(ctx, inp.n, inp.c, pH, pW, dt);
      const outH = Math.floor((pH - effH) / strides[0]) + 1;
      const outW = Math.floor((pW - effW) / strides[1]) + 1;
      return { outputs: { dataType: dt, shape: shapeOf(o.inputLayout, inp.n, f.o, outH, outW) }, attrs: { padding, strides, dilations } };
    },
    run([input, filter, o], rt) {
      const { padding, strides, dilations } = rt.attrs;
      const out = rt.outputs[0];
      const groups = o.groups;
      const inp = dimsOf(o.inputLayout, input.shape);
      const outD = dimsOf(o.inputLayout, out.shape);
      const fd = CONV_FILTER[o.filterLayout](filter.shape);
      const is = stridesOfLayout(o.inputLayout, inp);
      const os = stridesOfLayout(o.inputLayout, outD);
      const fs = filterStrides(o.filterLayout, fd);
      const res = new (typedArrayFor(out.dataType))(sizeOf(out.shape));
      const x = input.data;
      const w = filter.data;
      const bias = o.bias ? o.bias.data : null;
      const opg = fd.o / groups;
      const cig = fd.i;
      for (let n = 0; n < inp.n; n++) {
        for (let oc = 0; oc < fd.o; oc++) {
          const g = Math.floor(oc / opg);
          for (let oy = 0; oy < outD.h; oy++) {
            for (let ox = 0; ox < outD.w; ox++) {
              let acc = bias ? bias[oc] : 0;
              for (let ky = 0; ky < fd.h; ky++) {
                const iy = oy * strides[0] - padding[0] + ky * dilations[0];
                if (iy < 0 || iy >= inp.h) continue;
                for (let kx = 0; kx < fd.w; kx++) {
                  const ix = ox * strides[1] - padding[2] + kx * dilations[1];
                  if (ix < 0 || ix >= inp.w) continue;
                  const xBase = n * is.n + iy * is.h + ix * is.w;
                  const wBase = oc * fs.o + ky * fs.h + kx * fs.w;
                  for (let ci = 0; ci < cig; ci++) acc += x[xBase + (g * cig + ci) * is.c] * w[wBase + ci * fs.i];
                }
              }
              res[n * os.n + oc * os.c + oy * os.h + ox * os.w] = acc;
            }
          }
        }
      }
      return res;
    },
  });

  // ---- convTranspose2d --------------------------------------------------------------------------------------
  defineOp('convTranspose2d', {
    args: ['input:operand', 'filter:operand', 'options'],
    options: {
      ...commonConvOptions, outputPadding: SEQ, outputSizes: SEQ,
      filterLayout: { type: 'enum:iohw|hwoi|ohwi', default: 'iohw' },
    },
    limits: CONV_LIMITS,
    validate(ctx, input, filter, o) {
      const dt = input.dataType;
      sameType(ctx, 'filter', filter, dt);
      sameType(ctx, 'options.bias', o.bias, dt);
      const padding = listOption(ctx, 'padding', o.padding, 4, [0, 0, 0, 0]);
      const strides = listOption(ctx, 'strides', o.strides, 2, [1, 1], 1);
      const dilations = listOption(ctx, 'dilations', o.dilations, 2, [1, 1], 1);
      const outputPadding = listOption(ctx, 'outputPadding', o.outputPadding, 2, [0, 0]);
      const outputSizes = listOption(ctx, 'outputSizes', o.outputSizes, 2, undefined, 1);
      const groups = o.groups;
      if (groups < 1) ctx.fail('options.groups must be at least 1');
      const inp = dimsOf(o.inputLayout, input.shape);
      const f = CONVT_FILTER[o.filterLayout](filter.shape);
      if (f.i !== inp.c) ctx.fail(`The filter input channels (${f.i}) must equal the input channels (${inp.c})`);
      const outC = f.o * groups;
      if (outC > MAXU) ctx.fail(`The output channels (${outC}) are too large`);
      if (o.bias && o.bias.shape[0] !== outC) ctx.fail(`The shape of options.bias [${o.bias.shape}] must be [${outC}]`);
      const sizes = [];
      const axes = [['height', inp.h, f.h, 0, 0, 1], ['width', inp.w, f.w, 2, 1, 1]];
      for (const [axis, inSize, fSize, padIdx, k] of axes) {
        const eff = (fSize - 1) * dilations[k] + 1;
        if (eff > MAXU) ctx.fail(`The effective filter ${axis} overflows`);
        const base = (inSize - 1) * strides[k] + eff - padding[padIdx] - padding[padIdx + 1];
        let size;
        if (outputSizes) {
          size = outputSizes[k];
          if (size < base || size >= base + strides[k]) ctx.fail(`outputSizes[${k}] (${size}) must be in [${base}, ${base + strides[k] - 1}]`);
        } else {
          if (outputPadding[k] >= strides[k]) ctx.fail(`options.outputPadding[${k}] must be smaller than the stride`);
          size = base + outputPadding[k];
        }
        if (!(size > 0) || size > MAXU) ctx.fail(`The output ${axis} (${size}) is not valid`);
        if (strides[k] > size) ctx.fail(`The stride (${strides[k]}) is larger than the output ${axis} (${size})`);
        if (dilations[k] > size) ctx.fail(`The dilation (${dilations[k]}) is larger than the output ${axis} (${size})`);
        sizes.push(size);
      }
      return { outputs: { dataType: dt, shape: shapeOf(o.inputLayout, inp.n, outC, sizes[0], sizes[1]) }, attrs: { padding, strides, dilations } };
    },
    run([input, filter, o], rt) {
      const { padding, strides, dilations } = rt.attrs;
      const out = rt.outputs[0];
      const groups = o.groups;
      const inp = dimsOf(o.inputLayout, input.shape);
      const outD = dimsOf(o.inputLayout, out.shape);
      const fd = CONVT_FILTER[o.filterLayout](filter.shape);
      const is = stridesOfLayout(o.inputLayout, inp);
      const os = stridesOfLayout(o.inputLayout, outD);
      const fs = filterStrides(o.filterLayout, fd);
      const acc = new Float64Array(sizeOf(out.shape));
      const x = input.data;
      const w = filter.data;
      const opg = fd.o;
      const cig = Math.floor(inp.c / groups);
      for (let n = 0; n < inp.n; n++) {
        for (let g = 0; g < groups; g++) {
          for (let ci = 0; ci < cig; ci++) {
            const ic = g * cig + ci;
            for (let iy = 0; iy < inp.h; iy++) {
              for (let ix = 0; ix < inp.w; ix++) {
                const v = x[n * is.n + ic * is.c + iy * is.h + ix * is.w];
                for (let ko = 0; ko < opg; ko++) {
                  const oc = g * opg + ko;
                  for (let ky = 0; ky < fd.h; ky++) {
                    const oy = iy * strides[0] - padding[0] + ky * dilations[0];
                    if (oy < 0 || oy >= outD.h) continue;
                    for (let kx = 0; kx < fd.w; kx++) {
                      const ox = ix * strides[1] - padding[2] + kx * dilations[1];
                      if (ox < 0 || ox >= outD.w) continue;
                      acc[n * os.n + oc * os.c + oy * os.h + ox * os.w] += v * w[ic * fs.i + ko * fs.o + ky * fs.h + kx * fs.w];
                    }
                  }
                }
              }
            }
          }
        }
      }
      const res = new (typedArrayFor(out.dataType))(acc.length);
      if (o.bias) {
        const b = o.bias.data;
        for (let n = 0; n < outD.n; n++) {
          for (let oc = 0; oc < outD.c; oc++) {
            for (let y = 0; y < outD.h; y++) for (let xx = 0; xx < outD.w; xx++) acc[n * os.n + oc * os.c + y * os.h + xx * os.w] += b[oc];
          }
        }
      }
      for (let i = 0; i < acc.length; i++) res[i] = acc[i];
      return res;
    },
  });

  // ---- pooling ---------------------------------------------------------------------------------------------
  const poolOptions = {
    windowDimensions: SEQ, padding: SEQ, strides: SEQ, dilations: SEQ,
    layout: { type: 'enum:nchw|nhwc', default: 'nchw' },
    outputShapeRounding: { type: 'enum:floor|ceil', default: 'floor' },
    outputSizes: SEQ,
  };
  const poolValidate = (ctx, input, o) => {
    const inp = dimsOf(o.layout, input.shape);
    const window = listOption(ctx, 'windowDimensions', o.windowDimensions, 2, [inp.h, inp.w], 1);
    const padding = listOption(ctx, 'padding', o.padding, 4, [0, 0, 0, 0]);
    const strides = listOption(ctx, 'strides', o.strides, 2, [1, 1], 1);
    const dilations = listOption(ctx, 'dilations', o.dilations, 2, [1, 1], 1);
    const outputSizes = listOption(ctx, 'outputSizes', o.outputSizes, 2, undefined, 1);
    if (window[0] * window[1] > MAXU) ctx.fail('The product of the window dimensions is too large');
    const sizes = [];
    for (const [axis, inSize, k, padIdx] of [['height', inp.h, 0, 0], ['width', inp.w, 1, 2]]) {
      const padded = inSize + padding[padIdx] + padding[padIdx + 1];
      const eff = (window[k] - 1) * dilations[k] + 1;
      checkWindow(ctx, 'window', padded, eff, strides[k], dilations[k], axis);
      const span = (padded - eff) / strides[k];
      const floorSize = Math.floor(span) + 1;
      const ceilSize = Math.ceil(span) + 1;
      let size = o.outputShapeRounding === 'ceil' ? ceilSize : floorSize;
      if (outputSizes) {
        size = outputSizes[k];
        if (size !== floorSize && size !== ceilSize) ctx.fail(`outputSizes[${k}] (${size}) must be ${floorSize}${ceilSize !== floorSize ? ` or ${ceilSize}` : ''}`);
      }
      sizes.push(size);
    }
    return { outputs: { dataType: input.dataType, shape: shapeOf(o.layout, inp.n, inp.c, sizes[0], sizes[1]) }, attrs: { window, padding, strides, dilations } };
  };
  // `fold(window values iterator)`: init(), step(acc, v), done(acc, count).
  const poolRun = (input, o, rt, kernel) => {
    const { window, padding, strides, dilations } = rt.attrs;
    const out = rt.outputs[0];
    const inp = dimsOf(o.layout, input.shape);
    const outD = dimsOf(o.layout, out.shape);
    const is = stridesOfLayout(o.layout, inp);
    const os = stridesOfLayout(o.layout, outD);
    const res = new (typedArrayFor(out.dataType))(sizeOf(out.shape));
    const x = input.data;
    for (let n = 0; n < inp.n; n++) {
      for (let c = 0; c < inp.c; c++) {
        for (let oy = 0; oy < outD.h; oy++) {
          for (let ox = 0; ox < outD.w; ox++) {
            let acc = kernel.init();
            let count = 0;
            for (let ky = 0; ky < window[0]; ky++) {
              const iy = oy * strides[0] - padding[0] + ky * dilations[0];
              if (iy < 0 || iy >= inp.h) continue;
              for (let kx = 0; kx < window[1]; kx++) {
                const ix = ox * strides[1] - padding[2] + kx * dilations[1];
                if (ix < 0 || ix >= inp.w) continue;
                acc = kernel.step(acc, x[n * is.n + c * is.c + iy * is.h + ix * is.w]);
                count++;
              }
            }
            // With outputShapeRounding "ceil" a window can lie wholly in the padding: it has no elements and
            // yields 0 (what Chrome does), not NaN or undefined.
            res[n * os.n + c * os.c + oy * os.h + ox * os.w] = count === 0 ? 0 : kernel.done(acc, count);
          }
        }
      }
    }
    return res;
  };
  const POOL_LIMITS = (types) => ({ input: tl(types, 4, 4), output: tl(types, 4, 4) });

  // Padding elements are not part of an average: the window mean is over the elements that exist.
  defineOp('averagePool2d', {
    args: ['input:operand', 'options'],
    options: poolOptions,
    limits: POOL_LIMITS(FLOAT_TYPES),
    validate: (ctx, input, o) => poolValidate(ctx, input, o),
    run: ([input, o], rt) => poolRun(input, o, rt, { init: () => 0, step: (a, v) => a + v, done: (a, n) => a / n }),
  });
  defineOp('l2Pool2d', {
    args: ['input:operand', 'options'],
    options: poolOptions,
    limits: POOL_LIMITS(FLOAT_TYPES),
    validate: (ctx, input, o) => poolValidate(ctx, input, o),
    run: ([input, o], rt) => poolRun(input, o, rt, { init: () => 0, step: (a, v) => a + v * v, done: (a) => Math.sqrt(a) }),
  });
  defineOp('maxPool2d', {
    args: ['input:operand', 'options'],
    options: poolOptions,
    limits: POOL_LIMITS(ALL_TYPES),
    validate: (ctx, input, o) => poolValidate(ctx, input, o),
    run: ([input, o], rt) => poolRun(input, o, rt, { init: () => undefined, step: (a, v) => (a === undefined || v > a || v !== v ? v : a), done: (a) => a }),
  });
}
