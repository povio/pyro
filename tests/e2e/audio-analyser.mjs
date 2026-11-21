import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const { chromium } = createRequire(new URL('../../bench/package.json', import.meta.url))('playwright-core');
const browser = await chromium.connectOverCDP(process.argv[2]);
try {
  const page = await browser.newPage();
  const result = await page.evaluate(async () => {
    const context = new OfflineAudioContext(2, 512, 8192);
    const through = new AnalyserNode(context, { fftSize: 256, smoothingTimeConstant: 0.5 });
    const tap = context.createAnalyser();
    tap.fftSize = 32;
    const source = context.createBufferSource();
    const input = context.createBuffer(2, 512, 8192);
    input.getChannelData(0).fill(0.75);
    input.getChannelData(1).fill(-0.25);
    source.buffer = input;
    source.connect(through).connect(context.destination);
    source.connect(tap);
    source.start();
    const paused = context.suspend(256 / 8192);
    const rendered = context.startRendering();
    await paused;
    tap.fftSize = 256;
    const history = new Float32Array(258).fill(-7);
    tap.getFloatTimeDomainData(history);
    const samples = new Uint8Array(10);
    tap.getByteTimeDomainData(samples);
    const bins = new Float32Array(1), again = new Float32Array(1);
    through.getFloatFrequencyData(bins);
    through.getFloatFrequencyData(again);
    await context.resume();
    const output = await rendered;
    return { history: history.subarray(0, 256).every(value => value === 0.25), suffix: history[256], bytes: [...samples],
      stereo: [0.75, -0.25].every((value, channel) => output.getChannelData(channel).every(actual => actual === value)),
      dc: bins[0], repeat: again[0] };
  });
  console.log(JSON.stringify(result));
  assert.ok(result.history && result.stereo);
  assert.equal(result.suffix, -7);
  assert.deepEqual(result.bytes, Array(10).fill(160));
  assert.ok(Math.abs(result.dc - 20 * Math.log10(0.25 * 0.42 * 0.5)) < 0.001);
  assert.equal(result.dc, result.repeat);
} finally { await browser.close(); }
