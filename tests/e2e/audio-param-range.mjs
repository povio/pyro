import { createRequire } from 'node:module';
import assert from 'node:assert/strict';

const { chromium } = createRequire(new URL('../../bench/package.json', import.meta.url))('playwright-core');
const browser = await chromium.connectOverCDP(process.argv[2]);
try {
  const page = await browser.newPage();
  const result = await page.evaluate(async () => {
    async function probe(mode) {
      const context = new OfflineAudioContext(2, 128, 8192);
      const panner = new StereoPannerNode(context, { pan: mode === 'option' ? 2 : 0 });
      if (mode === 'setter') panner.pan.value = 2;
      if (mode === 'event') panner.pan.setValueAtTime(2, 0);
      if (mode === 'ramp') panner.pan.setValueAtTime(0, 0).linearRampToValueAtTime(2, 128 / 8192);
      const source = new ConstantSourceNode(context, { offset: 1 });
      const modulation = new ConstantSourceNode(context, { offset: -1 });
      source.connect(panner).connect(context.destination);
      modulation.connect(panner.pan);
      source.start(); modulation.start();
      const before = panner.pan.value;
      const rendered = await context.startRendering();
      return [before, panner.pan.value, rendered.getChannelData(0)[64], rendered.getChannelData(1)[64], rendered.getChannelData(0)[96]];
    }
    const context = new OfflineAudioContext(1, 128, 48000);
    const timeline = new OfflineAudioContext(1, 256, 8192);
    const signal = timeline.createConstantSource();
    signal.offset.setValueAtTime(0.25, 0).linearRampToValueAtTime(0.75, 256 / 8192);
    signal.connect(timeline.destination);
    signal.start();
    const intrinsic = [signal.offset.value];
    const suspended = timeline.suspend(128 / 8192);
    const rendering = timeline.startRendering();
    await suspended;
    intrinsic.push(signal.offset.value);
    await timeline.resume();
    await rendering;
    intrinsic.push(signal.offset.value);
    return { setter: await probe('setter'), option: await probe('option'), event: await probe('event'), ramp: await probe('ramp'),
      delay: new DelayNode(context, { maxDelayTime: 1.5, delayTime: 4 }).delayTime.value,
      oscillator: new OscillatorNode(context, { frequency: -48000 }).frequency.value,
      gainLimit: context.createBiquadFilter().gain.maxValue, intrinsic };
  });
  console.log(JSON.stringify(result));
  for (const mode of ['setter', 'option', 'event', 'ramp']) {
    const [before, after, ...samples] = result[mode];
    assert.equal(before, ['setter', 'option'].includes(mode) ? 1 : 0, mode);
    assert.equal(after, 1, mode);
    for (const sample of samples) assert.ok(Math.abs(sample - Math.SQRT1_2) < 1e-6, `${mode}: ${sample}`);
  }
  assert.deepEqual(result.intrinsic, [1, 0.498046875, 0.748046875]);
  assert.equal(result.delay, 1.5);
  assert.equal(result.oscillator, -24000);
  assert.equal(result.gainLimit, Math.fround(40 * Math.fround(Math.log10(3.4028234663852886e38))));
} finally { await browser.close(); }
