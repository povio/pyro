import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';

const { chromium } = createRequire(new URL('../../bench/package.json', import.meta.url))('playwright-core');
const [executablePath, wptRoot, modulePath, batch] = process.argv.slice(2);
const periodic = batch === 'periodic';
const directory = '/webaudio/the-audio-api/' + (periodic ? '' : 'the-audiobuffersourcenode-interface/');
const pages = periodic ? ['the-periodicwave-interface/periodicWave.html', 'the-oscillatornode-interface/ctor-oscillator.html'] : ['sub-sample-scheduling.html', 'audiobuffersource-duration-loop.html', 'audiobuffersource-duration-loop-playbackrate.html'];
const allowed = new Set([...pages.map(name => directory + name), '/resources/testharness.js', '/resources/testharnessreport.js', '/webaudio/resources/audit-util.js', '/webaudio/resources/audit.js', ...(periodic ? ['/webaudio/resources/audionodeoptions.js'] : [])]);
const browser = await chromium.launch({ executablePath });
try {
  const page = await browser.newPage();
  if (modulePath) {
    const source = await readFile(modulePath, 'utf8');
    await page.addInitScript(`{ const audio = (${source})({activated: () => false}); ${periodic ? 'delete globalThis.PeriodicWave;' : ''} Object.assign(globalThis, audio); globalThis.installedAudio = audio; }`);
  }
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (!allowed.has(path)) return route.abort();
    let body = await readFile(join(wptRoot, path), 'utf8');
    if (path === '/resources/testharness.js') body += '\nglobalThis.results = new Promise(resolve => add_completion_callback((tests, status) => resolve({status:status.status, tests:tests.map(test => [test.name, test.status])})));';
    await route.fulfill({contentType: path.endsWith('.html') ? 'text/html' : 'application/javascript', body});
  });
  const outcomes = {};
  for (const name of pages) {
    await page.goto('https://wpt.test' + directory + name);
    if (modulePath) assert.ok(await page.evaluate(() => globalThis.installedAudio?.OfflineAudioContext === OfflineAudioContext && globalThis.installedAudio?.AudioBufferSourceNode === AudioBufferSourceNode), 'Web Audio module installation failed');
    if (modulePath && periodic) assert.ok(await page.evaluate(() => installedAudio.AudioContext === AudioContext && installedAudio.OscillatorNode === OscillatorNode && installedAudio.PeriodicWave === globalThis.PeriodicWave));
    await page.waitForFunction(() => globalThis.results);
    outcomes[name] = await page.evaluate(() => globalThis.results);
  }
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
