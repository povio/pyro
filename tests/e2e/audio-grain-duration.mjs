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
const offlineQuantum = batch === 'offline-quantum';
const compressor = batch === 'compressor';
const destinationCount = batch === 'destination-count';
const reverse = batch === 'reverse-loop';
const detuning = batch === 'osc-detune';
const resampling = batch === 'resampling';
const batchDirectory = destinationCount ? '/webaudio/the-audio-api/the-destinationnode-interface/' : reverse || detuning ? '/webaudio/the-audio-api/the-oscillatornode-interface/' : compressor ? '/webaudio/the-audio-api/' : offlineClose || offlineQuantum ? '/webaudio/the-audio-api/the-offlineaudiocontext-interface/' : directory;
const batchPages = compressor ? ['the-dynamicscompressornode-interface/dynamicscompressor-basic.html', 'the-dynamicscompressornode-interface/ctor-dynamicscompressor.html', 'the-audioparam-interface/k-rate-dynamics-compressor-connections.html'] : offlineQuantum ? ['offlineaudiocontext-rendersizehint.html', 'offlineaudiocontext-suspend-rendersizehint.https.html', 'offlineaudiocontext-close.html'] : offlineClose ? ['ctor-offlineaudiocontext.html', 'offlineaudiocontext-close.html'] : pages;
if (reverse) batchPages.splice(0, batchPages.length, 'osc-basic-waveform.html');
if (destinationCount) batchPages.splice(0, batchPages.length, 'destination.html');
if (detuning) batchPages.splice(0, batchPages.length, 'osc-basic-waveform.html', 'detune-limiting.html');
if (resampling) batchPages.splice(0, batchPages.length, 'buffer-resampling.html', 'sub-sample-buffer-stitching.html');
if (batch === 'buffer-assignment') batchPages.splice(0, batchPages.length, 'audiobuffersource-channels.html', 'audiobuffersource-null.html');
const allowed = new Set([...batchPages.map(name => batchDirectory + name), '/resources/testharness.js', '/resources/testharnessreport.js', '/webaudio/resources/audit-util.js', '/webaudio/resources/audit.js', ...(periodic || offlineClose ? ['/webaudio/resources/audionodeoptions.js'] : [])]);
const browser = await chromium.launch({ executablePath });
if (hold) allowed.add('/webaudio/resources/audio-param.js');
if (realtime || offlineQuantum) allowed.add('/webaudio/resources/rendersizehint-testing.js');
if (retro) allowed.add(directory + 'retrospective-test.js');
try {
  const page = await browser.newPage();
  if (modulePath) {
    const source = await readFile(modulePath, 'utf8');
    await page.addInitScript(`{ const audio = (${source})({activated: () => ${realtime}, timer: (delay, fn) => setTimeout(fn, delay), clearTimer: clearTimeout}); ${periodic ? 'delete globalThis.PeriodicWave;' : ''} ${compressor ? 'delete globalThis.DynamicsCompressorNode;' : ''} Object.assign(globalThis, audio); globalThis.installedAudio = audio; }`);
  }
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (!allowed.has(path)) return route.abort();
    let body = await readFile(join(wptRoot, path), 'utf8');
    if (path === '/resources/testharness.js') body += '\nglobalThis.results = new Promise(resolve => add_completion_callback((tests, status) => resolve({status:status.status, tests:tests.map(test => test.status ? [test.name, test.status, test.message] : [test.name, test.status])})));';
    await route.fulfill({contentType: path.endsWith('.html') ? 'text/html' : 'application/javascript', body});
  });
  const outcomes = {};
  for (const name of batchPages) {
    await page.goto('https://wpt.test' + batchDirectory + name);
    if (modulePath) assert.ok(await page.evaluate(() => globalThis.installedAudio?.OfflineAudioContext === OfflineAudioContext && globalThis.installedAudio?.AudioBufferSourceNode === AudioBufferSourceNode), 'Web Audio module installation failed');
    if (modulePath && periodic) assert.ok(await page.evaluate(() => installedAudio.AudioContext === AudioContext && installedAudio.OscillatorNode === OscillatorNode && installedAudio.PeriodicWave === globalThis.PeriodicWave));
    if (modulePath && realtime) assert.ok(await page.evaluate(() => installedAudio.AudioContext === AudioContext));
    if (modulePath && resampling) assert.ok(await page.evaluate(() => installedAudio.OscillatorNode === OscillatorNode && installedAudio.AudioBuffer === AudioBuffer && installedAudio.ChannelMergerNode === ChannelMergerNode));
    if (modulePath && detuning) assert.ok(await page.evaluate(() => installedAudio.OscillatorNode === OscillatorNode && installedAudio.PeriodicWave === PeriodicWave && installedAudio.ChannelMergerNode === ChannelMergerNode));
    if (modulePath && compressor) assert.ok(await page.evaluate(() => installedAudio.AudioContext === AudioContext && installedAudio.DynamicsCompressorNode === globalThis.DynamicsCompressorNode));
    if (modulePath && destinationCount) assert.ok(await page.evaluate(() => installedAudio.AudioContext === AudioContext && installedAudio.AudioDestinationNode === AudioDestinationNode));
    if (modulePath && retro) assert.ok(await page.evaluate(() => installedAudio.AudioParam === AudioParam && new OfflineAudioContext(1, 128, 44100).createGain().gain instanceof AudioParam));
    await page.waitForFunction(() => globalThis.results);
    outcomes[name] = await page.evaluate(() => globalThis.results);
  }
  if (offlineQuantum) outcomes['stereo-analyser-pcm'] = await page.evaluate(async () => {
    const tests = [];
    for (const size of [441, 33001]) {
      const rate = 48000, pause = Math.ceil(32769 / size) * size, length = pause + 147;
      const context = new OfflineAudioContext({numberOfChannels: 2, length, sampleRate: rate, renderSizeHint: size});
      const source = context.createBufferSource(), analyser = context.createAnalyser();
      analyser.fftSize = 32;
      const sample = (frame, channel) => ((frame % 17) - 8) * (channel + 1) / 32;
      source.buffer = context.createBuffer(2, length, rate);
      for (let channel = 0; channel < 2; channel++) {
        const data = source.buffer.getChannelData(channel);
        for (let frame = 0; frame < length; frame++) data[frame] = sample(frame, channel);
      }
      source.connect(analyser).connect(context.destination);
      source.start();
      const suspended = context.suspend((pause - 0.5) / rate), rendered = context.startRendering();
      await suspended;
      const history = new Float32Array(32);
      analyser.getFloatTimeDomainData(history);
      tests.push([`mono history at frame ${pause}, quantum ${size}`, history.every((value, index) => value === (sample(pause - 32 + index, 0) + sample(pause - 32 + index, 1)) / 2) ? 0 : 1]);
      await context.resume();
      const buffer = await rendered;
      tests.push([`stereo PCM through final frame, quantum ${size}`, buffer.length === length && [0, 1].every(channel => buffer.getChannelData(channel).every((value, frame) => value === sample(frame, channel))) ? 0 : 1]);
    }
    return {status: 0, tests};
  });
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
  if (detuning) outcomes['detuned-waveforms'] = await page.evaluate(async () => {
    const tests = [];
    for (const type of ['sine', 'triangle', 'sawtooth', 'square']) {
      for (const sign of [-1, 1]) {
        const rate = 32768, context = new OfflineAudioContext(1, 32, rate);
        const osc = new OscillatorNode(context, {frequency: sign * 2048, type});
        osc.detune.setValueAtTime(4200, 3 / rate);
        osc.detune.setValueAtTime(0, 8 / rate);
        osc.connect(context.destination);
        osc.start();
        const data = (await context.startRendering()).getChannelData(0);
        const pass = data.subarray(0, 3).some(value => value !== 0) &&
          data.subarray(3, 8).every(value => value === 0) && data.subarray(8).some(value => value !== 0);
        tests.push([`${type}, frequency sign ${sign}: audible, silent, audible`, pass ? 0 : 1]);
      }
    }
    return {status: 0, tests};
  });
  if (reverse) {
    if (modulePath) assert.ok(await page.evaluate(() => installedAudio.OscillatorNode === OscillatorNode && installedAudio.PeriodicWave === PeriodicWave && installedAudio.AudioBuffer === AudioBuffer));
    outcomes['reverse-loop-pcm'] = await page.evaluate(async () => {
      const tests = [], pcm = {};
      for (const [name, offset, speed, loop, expected] of [
        ['reverse lead-in', 7, -1, true, [7,6,5,4,3,2,4,3]],
        ['reverse below loop', 1, -1, true, [2,4,3,2]],
        ['reverse inside loop', 3, -1, true, [3,2,4,3]],
        ['reverse at duration', 8, -1, false, [0,7,6,5]],
        ['reverse past duration', 10, -1, false, [0,7,6,5]],
        ['forward lead-in', 1, 1, true, [1,2,3,4,2]],
        ['reverse fractional lead-in', 6.5, -1, true, [6.5,5.5,3,3.5,2.5]],
      ]) {
        const rate = 32768, context = new OfflineAudioContext(1, expected.length, rate);
        const buffer = context.createBuffer(1, 8, rate);
        buffer.getChannelData(0).set([0,1,2,3,4,5,6,7]);
        const src = new AudioBufferSourceNode(context, {buffer, playbackRate: speed, loop, loopStart: 2/rate, loopEnd: 5/rate});
        src.connect(context.destination); src.start(0, offset/rate);
        const data = Array.from((await context.startRendering()).getChannelData(0));
        pcm[name] = data;
        tests.push([name, expected.every((value, frame) => data[frame] === value) ? 0 : 1]);
      }
      return {status: 0, tests, pcm};
    });
  }
  console.log(JSON.stringify(outcomes));
  for (const {status, tests} of Object.values(outcomes)) {
    assert.equal(status, 0);
    assert.ok(tests.length && tests.every(([, outcome]) => outcome === 0));
  }
} finally { await browser.close(); }
