// Normalization: batchNormalization, instanceNormalization, layerNormalization. Statistics and the affine transform are
// computed in double precision; the result is rounded once when stored into the output array.
(api) => {
  const { defineOp, tl, FLOAT_TYPES, sizeOf, stridesOf, alloc } = api;

  // scale / bias of a normalization: a 1-D operand of the input's type whose size is `size`.
  const check1D = (ctx, input, operand, what, size) => {
    if (!operand) return;
    if (operand.dataType !== input.dataType) ctx.fail(`The ${what} data type ${operand.dataType} must match the input data type ${input.dataType}`);
    if (operand.rank !== 1) ctx.fail(`The ${what} must be a 1-D tensor, but its rank is ${operand.rank}`);
    if (operand.shape[0] !== size) ctx.fail(`The ${what} size ${operand.shape[0]} must equal ${size}`);
  };

  // ---- batchNormalization ----------------------------------------------------------------------------
  defineOp('batchNormalization', {
    args: ['input:operand', 'mean:operand', 'variance:operand', 'options'],
    options: { axis: { type: 'ul', default: 1 }, bias: 'operand', epsilon: { type: 'double', default: 1e-5 }, scale: 'operand' },
    limits: {
      input: tl(FLOAT_TYPES, 1, 5), mean: tl(FLOAT_TYPES, 1, 1), variance: tl(FLOAT_TYPES, 1, 1),
      scale: tl(FLOAT_TYPES, 1, 1), bias: tl(FLOAT_TYPES, 1, 1), output: tl(FLOAT_TYPES, 1, 5),
    },
    validate(ctx, input, mean, variance, options) {
      if (options.axis >= input.rank) ctx.fail(`The axis ${options.axis} is out of range for an input of rank ${input.rank}`);
      const size = input.shape[options.axis];
      check1D(ctx, input, mean, 'mean', size);
      check1D(ctx, input, variance, 'variance', size);
      check1D(ctx, input, options.scale, 'scale', size);
      check1D(ctx, input, options.bias, 'bias', size);
      return { dataType: input.dataType, shape: input.shape.slice() };
    },
    run([input, mean, variance, options], rt) {
      const shape = input.shape;
      const axis = options.axis;
      const channels = shape[axis];
      let inner = 1;
      for (let d = axis + 1; d < shape.length; d++) inner *= shape[d];
      const x = input.data;
      const out = alloc(rt.outputs[0].dataType, x.length);
      // Per channel: y = (x - mean) * scale / sqrt(variance + epsilon) + bias.
      const a = new Float64Array(channels);
      for (let c = 0; c < channels; c++) {
        const s = options.scale ? options.scale.data[c] : 1;
        const k = s / Math.sqrt(variance.data[c] + options.epsilon);
        a[c] = k;
      }
      for (let i = 0; i < x.length; i++) {
        const c = Math.floor(i / inner) % channels;
        out[i] = (x[i] - mean.data[c]) * a[c] + (options.bias ? options.bias.data[c] : 0);
      }
      return out;
    },
  });

  // ---- instanceNormalization -------------------------------------------------------------------------
  defineOp('instanceNormalization', {
    args: ['input:operand', 'options'],
    options: { bias: 'operand', epsilon: { type: 'double', default: 1e-5 }, layout: { type: 'enum:nchw|nhwc', default: 'nchw' }, scale: 'operand' },
    limits: { input: tl(FLOAT_TYPES, 4, 4), scale: tl(FLOAT_TYPES, 1, 1), bias: tl(FLOAT_TYPES, 1, 1), output: tl(FLOAT_TYPES, 4, 4) },
    validate(ctx, input, options) {
      const channels = input.shape[options.layout === 'nchw' ? 1 : 3];
      check1D(ctx, input, options.scale, 'scale', channels);
      check1D(ctx, input, options.bias, 'bias', channels);
      return { dataType: input.dataType, shape: input.shape.slice() };
    },
    run([input, options], rt) {
      const [n, d1, d2, d3] = input.shape;
      const nchw = options.layout === 'nchw';
      const channels = nchw ? d1 : d3;
      const spatial = nchw ? d2 * d3 : d1 * d2;
      const x = input.data;
      const out = alloc(rt.outputs[0].dataType, x.length);
      // Element (b, c, s) lives at nchw: (b*C + c)*S + s, nhwc: (b*S + s)*C + c.
      const at = nchw ? (b, c, s) => (b * channels + c) * spatial + s : (b, c, s) => (b * spatial + s) * channels + c;
      for (let b = 0; b < n; b++) {
        for (let c = 0; c < channels; c++) {
          let sum = 0;
          for (let s = 0; s < spatial; s++) sum += x[at(b, c, s)];
          const mean = sum / spatial;
          let sq = 0;
          for (let s = 0; s < spatial; s++) {
            const v = x[at(b, c, s)] - mean;
            sq += v * v;
          }
          const inv = 1 / Math.sqrt(sq / spatial + options.epsilon);
          const scale = options.scale ? options.scale.data[c] : 1;
          const bias = options.bias ? options.bias.data[c] : 0;
          for (let s = 0; s < spatial; s++) {
            const i = at(b, c, s);
            out[i] = (x[i] - mean) * inv * scale + bias;
          }
        }
      }
      return out;
    },
  });

  // ---- layerNormalization ----------------------------------------------------------------------------
  defineOp('layerNormalization', {
    args: ['input:operand', 'options'],
    options: { axes: 'seq<ul>', bias: 'operand', epsilon: { type: 'double', default: 1e-5 }, scale: 'operand' },
    limits: { input: tl(FLOAT_TYPES, 0, 8), scale: tl(FLOAT_TYPES, 0, 8), bias: tl(FLOAT_TYPES, 0, 8), output: tl(FLOAT_TYPES, 0, 8) },
    validate(ctx, input, options) {
      // Default axes: every dimension but the first (the batch), i.e. [1, ..., rank - 1]; none for a scalar.
      const axes = options.axes === undefined ? Array.from({ length: Math.max(input.rank - 1, 0) }, (_, i) => i + 1) : options.axes.slice();
      const seen = new Set();
      for (const a of axes) {
        if (a >= input.rank) ctx.fail(`The value ${a} in axes is out of range for an input of rank ${input.rank}`);
        if (seen.has(a)) ctx.fail(`The axes contain the duplicate value ${a}`);
        seen.add(a);
      }
      for (const [what, operand] of [['scale', options.scale], ['bias', options.bias]]) {
        if (!operand) continue;
        if (operand.dataType !== input.dataType) ctx.fail(`The ${what} data type ${operand.dataType} must match the input data type ${input.dataType}`);
        if (operand.rank !== axes.length) ctx.fail(`The ${what} rank ${operand.rank} must equal the number of axes ${axes.length}`);
        for (let j = 0; j < axes.length; j++) {
          if (operand.shape[j] !== input.shape[axes[j]]) ctx.fail(`The ${what} dimension ${j} (${operand.shape[j]}) must equal the input dimension ${axes[j]} (${input.shape[axes[j]]})`);
        }
      }
      return { outputs: { dataType: input.dataType, shape: input.shape.slice() }, attrs: { axes } };
    },
    run([input, options], rt) {
      const { axes } = rt.attrs;
      const shape = input.shape;
      const rank = shape.length;
      const x = input.data;
      const out = alloc(rt.outputs[0].dataType, x.length);
      const inAxes = new Array(rank).fill(false);
      for (const a of axes) inAxes[a] = true;
      const strides = stridesOf(shape);
      const outerDims = [];
      for (let d = 0; d < rank; d++) if (!inAxes[d]) outerDims.push(d);
      const outerCount = sizeOf(outerDims.map((d) => shape[d]));
      const normCount = sizeOf(axes.map((a) => shape[a]));
      // Offsets of the normalized elements relative to a group's base, in axes order (also the scale/bias order).
      const rel = new Int32Array(normCount);
      {
        const idx = new Array(axes.length).fill(0);
        let off = 0;
        for (let i = 0; i < normCount; i++) {
          rel[i] = off;
          for (let j = axes.length - 1; j >= 0; j--) {
            idx[j]++;
            off += strides[axes[j]];
            if (idx[j] < shape[axes[j]]) break;
            off -= strides[axes[j]] * shape[axes[j]];
            idx[j] = 0;
          }
        }
      }
      const sd = options.scale ? options.scale.data : null;
      const bd = options.bias ? options.bias.data : null;
      const idx = new Array(outerDims.length).fill(0);
      let base = 0;
      for (let g = 0; g < outerCount; g++) {
        let sum = 0;
        for (let i = 0; i < normCount; i++) sum += x[base + rel[i]];
        const mean = sum / normCount;
        let sq = 0;
        for (let i = 0; i < normCount; i++) {
          const v = x[base + rel[i]] - mean;
          sq += v * v;
        }
        const inv = 1 / Math.sqrt(sq / normCount + options.epsilon);
        for (let i = 0; i < normCount; i++) {
          const p = base + rel[i];
          out[p] = (x[p] - mean) * inv * (sd ? sd[i] : 1) + (bd ? bd[i] : 0);
        }
        for (let j = outerDims.length - 1; j >= 0; j--) {
          const d = outerDims[j];
          idx[j]++;
          base += strides[d];
          if (idx[j] < shape[d]) break;
          base -= strides[d] * shape[d];
          idx[j] = 0;
        }
      }
      return out;
    },
  });
}
