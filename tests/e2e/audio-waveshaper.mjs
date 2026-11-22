import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const { chromium } = createRequire(new URL('../../bench/package.json', import.meta.url))('playwright-core');
const browser = await chromium.connectOverCDP(process.argv[2]);
try {
  const result = await (await browser.newPage()).evaluate(async () => {
    if (typeof WaveShaperNode === 'undefined') return { supported: false };
    const context = new OfflineAudioContext(2, 128, 8192);
    const buffer = context.createBuffer(2, 128, 8192);
    buffer.getChannelData(0).set([-2, -0.5, 0, 0.25, 2]);
    buffer.getChannelData(1).set([2, 0.5, 0, -0.25, -2]);
    const source = context.createBufferSource();
    source.buffer = buffer;
    const table = Float32Array.of(2, -1, 3);
    const shaper = context.createWaveShaper();
    shaper.curve = table;
    table.fill(0);
    shaper.curve.fill(0);
    source.connect(shaper).connect(context.destination);
    source.start();
    const output = await context.startRendering();
    const channels = [0, 1].map(channel => Array.from(output.getChannelData(channel).subarray(0, 5)));
    async function spectrum(oversample) {
      const frames = 2048, rate = 8192, tone = 1536;
      const ctx = new OfflineAudioContext(1, frames, rate);
      const wave = ctx.createBuffer(1, frames, rate);
      for (let frame = 0; frame < frames; frame++) wave.getChannelData(0)[frame] = Math.sin(2 * Math.PI * tone * frame / rate);
      const input = ctx.createBufferSource();
      input.buffer = wave;
      const curve = Array.from({length: 257}, (_, index) => (index / 128 - 1) ** 3);
      input.connect(new WaveShaperNode(ctx, {curve, oversample})).connect(ctx.destination);
      input.start();
      const samples = (await ctx.startRendering()).getChannelData(0);
      return [tone, rate - 3 * tone].map(frequency => {
        let real = 0, imag = 0;
        for (let frame = 1024; frame < frames; frame++) {
          const phase = 2 * Math.PI * frequency * frame / rate;
          real += samples[frame] * Math.cos(phase);
          imag += samples[frame] * Math.sin(phase);
        }
        return Math.hypot(real, imag) / 512;
      });
    }
    const policy = context.createWaveShaper();
    function assign(value) {
      try { policy.curve = value; return 'accepted'; } catch (error) { return error.name; }
    }
    const curveAssignments = [assign(new Float32Array(1)), assign(table), assign(null), assign(table)];
    return {channels, none: await spectrum('none'), twice: await spectrum('2x'), four: await spectrum('4x'), curveAssignments};
  });
  console.log(JSON.stringify(result));
  assert.notEqual(result.supported, false, 'WaveShaperNode is missing');
  assert.deepEqual(result.channels, [[2, 0.5, -1, 0, 3], [3, 1, -1, -0.25, 2]]);
  assert.ok(Math.abs(result.none[1] - 0.25) < 0.001);
  for (const mode of ['none', 'twice', 'four']) assert.ok(Math.abs(result[mode][0] - 0.75) < 0.02, mode);
  for (const mode of ['twice', 'four']) assert.ok(result[mode][1] < 0.06, mode);
  if (process.argv.includes('--check-spec-assignments')) {
    assert.deepEqual(result.curveAssignments, ['InvalidStateError', 'accepted', 'accepted', 'InvalidStateError']);
  }
} finally { await browser.close(); }
