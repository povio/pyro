import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';

const { chromium } = createRequire(new URL('../../bench/package.json', import.meta.url))('playwright-core');
const [executablePath, wptRoot, modulePath, batch] = process.argv.slice(2);
const periodic = batch === 'periodic';
const hold = batch === 'hold';
const realtime = batch === 'realtime-quantum';
const retro = batch === 'retrospective';
const directory = '/webaudio/the-audio-api/' + (realtime ? 'the-audiocontext-interface/' : hold || retro ? 'the-audioparam-interface/' : periodic ? '' : 'the-audiobuffersourcenode-interface/');
const pages = retro ? ['setValueAtTime', 'setTargetAtTime', 'linearRampToValueAtTime', 'exponentialRampToValueAtTime', 'setValueCurveAtTime'].map(method => `retrospective-${method}.html`) : realtime ? ['audiocontext-rendersizehint.html'] : hold ? ['audioparam-cancel-and-hold.html', 'cancel-scheduled-values.html'] : periodic ? ['the-periodicwave-interface/periodicWave.html', 'the-oscillatornode-interface/ctor-oscillator.html'] : ['sub-sample-scheduling.html', 'audiobuffersource-duration-loop.html', 'audiobuffersource-duration-loop-playbackrate.html'];
const offlineClose = batch === 'offline-close';
const batchDirectory = offlineClose ? '/webaudio/the-audio-api/the-offlineaudiocontext-interface/' : directory;
const batchPages = offlineClose ? ['ctor-offlineaudiocontext.html', 'offlineaudiocontext-close.html'] : pages;
const allowed = new Set([...batchPages.map(name => batchDirectory + name), '/resources/testharness.js', '/resources/testharnessreport.js', '/webaudio/resources/audit-util.js', '/webaudio/resources/audit.js', ...(periodic || offlineClose ? ['/webaudio/resources/audionodeoptions.js'] : [])]);
const browser = await chromium.launch({ executablePath });
if (hold) allowed.add('/webaudio/resources/audio-param.js');
if (realtime) allowed.add('/webaudio/resources/rendersizehint-testing.js');
if (retro) allowed.add(directory + 'retrospective-test.js');
try {
  const page = await browser.newPage();
  if (modulePath) {
    const source = await readFile(modulePath, 'utf8');
    await page.addInitScript(`{ const audio = (${source})({activated: () => ${realtime}, timer: (delay, fn) => setTimeout(fn, delay), clearTimer: clearTimeout}); ${periodic ? 'delete globalThis.PeriodicWave;' : ''} Object.assign(globalThis, audio); globalThis.installedAudio = audio; }`);
  }
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (!allowed.has(path)) return route.abort();
    let body = await readFile(join(wptRoot, path), 'utf8');
    if (path === '/resources/testharness.js') body += '\nglobalThis.results = new Promise(resolve => add_completion_callback((tests, status) => resolve({status:status.status, tests:tests.map(test => [test.name, test.status])})));';
    await route.fulfill({contentType: path.endsWith('.html') ? 'text/html' : 'application/javascript', body});
  });
  const outcomes = {};
  for (const name of batchPages) {
    await page.goto('https://wpt.test' + batchDirectory + name);
    if (modulePath) assert.ok(await page.evaluate(() => globalThis.installedAudio?.OfflineAudioContext === OfflineAudioContext && globalThis.installedAudio?.AudioBufferSourceNode === AudioBufferSourceNode), 'Web Audio module installation failed');
    if (modulePath && periodic) assert.ok(await page.evaluate(() => installedAudio.AudioContext === AudioContext && installedAudio.OscillatorNode === OscillatorNode && installedAudio.PeriodicWave === globalThis.PeriodicWave));
    if (modulePath && realtime) assert.ok(await page.evaluate(() => installedAudio.AudioContext === AudioContext));
    if (modulePath && retro) assert.ok(await page.evaluate(() => installedAudio.AudioParam === AudioParam && new OfflineAudioContext(1, 128, 44100).createGain().gain instanceof AudioParam));
    await page.waitForFunction(() => globalThis.results);
    outcomes[name] = await page.evaluate(() => globalThis.results);
  }
  if (realtime) outcomes['source-clock'] = await page.evaluate(async () => {
    const tests = [];
    for (const quantum of [13, 500, 4096]) {
      const context = new AudioContext({sampleRate: 48000, renderSizeHint: quantum});
      try {
        await context.resume();
        const source = context.createOscillator();
        source.connect(context.destination);
        const ended = new Promise(resolve => { source.onended = () => resolve(true); });
        const stop = context.currentTime + 0.05;
        source.start();
        source.stop(stop);
        const delivered = await Promise.race([ended, new Promise(resolve => setTimeout(() => resolve(false), 2000))]);
        const frame = Math.round(context.currentTime * context.sampleRate);
        tests.push([`scheduled oscillator end and ${quantum}-frame clock`, delivered && context.renderQuantumSize === quantum && frame % quantum === 0 && frame + quantum >= Math.floor(stop * context.sampleRate) ? 0 : 1]);
      } finally { await context.close(); }
    }
    return {status: 0, tests};
  });
  if (hold) outcomes['hold-pcm'] = await page.evaluate(async () => {
    const tests = [];
    for (const event of ['set', 'target', 'ramp', 'none', 'curveEnd']) {
      const context = new OfflineAudioContext(1, 384, 8192), source = context.createConstantSource();
      const param = source.offset, cut = 128 / context.sampleRate;
      param.value = 0.25;
      if (event === 'set') param.setValueAtTime(0.75, cut);
      if (event === 'target') param.setTargetAtTime(0.75, cut, cut);
      if (event === 'ramp') param.linearRampToValueAtTime(0.75, cut);
      if (event === 'curveEnd') param.setValueCurveAtTime([0, 0.75], 0, cut);
      param.cancelAndHoldAtTime(cut).linearRampToValueAtTime(1, 2 * cut);
      source.connect(context.destination);
      source.start();
      const samples = (await context.startRendering()).getChannelData(0);
      const pass = samples.every((sample, frame) => {
        if (frame >= 256) return sample === 1;
        if (event === 'target' || event === 'none') return sample === 0.25 + 0.75 * frame / 256;
        if (frame >= 128) return sample === 0.75 + 0.25 * (frame - 128) / 128;
        if (event === 'set') return sample === 0.25;
        return sample === (event === 'curveEnd' ? 0.75 * frame / 128 : 0.25 + 0.5 * frame / 128);
      });
      tests.push([event, pass ? 0 : 1]);
    }
    return {status: 0, tests};
  });
  if (periodic) outcomes['periodic-wave-pcm'] = await page.evaluate(async () => {
    const tests = [];
    for (const normalize of [true, false]) {
      const context = new OfflineAudioContext(1, 256, 8192);
      const name = `copied coefficients, DC excluded, normalize=${normalize}`;
      if (typeof context.createPeriodicWave !== 'function') { tests.push([name, 1]); continue; }
      const real = [10, 1.5, 0, 0.5];
      const wave = context.createPeriodicWave(real, [5, 0, 0, 0], {disableNormalization: !normalize});
      real[1] = 0;
      const osc = new OscillatorNode(context, {frequency: 64, periodicWave: wave});
      osc.connect(context.destination);
      osc.start();
      const data = (await context.startRendering()).getChannelData(0);
      tests.push([name, data.every((sample, frame) => {
        const angle = 2 * Math.PI * frame / 128;
        return Math.abs(sample - (normalize ? 1 : 2) * (0.75 * Math.cos(angle) + 0.25 * Math.cos(3 * angle))) < 0.002;
      }) ? 0 : 1]);
    }
    return {status: 0, tests};
  });
  console.log(JSON.stringify(outcomes));
  for (const {status, tests} of Object.values(outcomes)) {
    assert.equal(status, 0);
    assert.ok(tests.length && tests.every(([, outcome]) => outcome === 0));
  }
} finally { await browser.close(); }
