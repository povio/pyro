import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';

const { chromium } = createRequire(new URL('../../bench/package.json', import.meta.url))('playwright-core');
const [executablePath, modulePath, wptRoot] = process.argv.slice(2);
const browser = await chromium.launch({executablePath});
try {
  const page = await browser.newPage();
  if (modulePath) {
    const source = await readFile(modulePath, 'utf8');
    await page.addInitScript(`{ const audio = (${source})({timer: (delay, fn) => setTimeout(fn, delay), clearTimer: clearTimeout}); Object.assign(globalThis, audio); globalThis.installedAudio = audio; }`);
  }
  await page.goto('about:blank');
  const guard = async () => { if (modulePath) assert.ok(await page.evaluate(() => installedAudio.OfflineAudioContext === OfflineAudioContext && installedAudio.AudioBufferSourceNode === AudioBufferSourceNode)); };
  await guard();
  const results = await page.evaluate(async () => {
    const context = new OfflineAudioContext(1, 405, 8192);
    const source = context.createBufferSource();
    source.buffer = context.createBuffer(1, 405, 8192);
    const ramp = source.buffer.getChannelData(0);
    for (let frame = 0; frame < ramp.length; frame++) ramp[frame] = frame / 512;
    source.connect(context.destination);
    source.start();
    const events = [];
    context.oncomplete = event => events.push(event.renderedBuffer);
    const first = context.startRendering(1);
    const running = context.state === 'running';
    const second = context.startRendering(129);
    const final = context.startRendering();
    const extra = context.startRendering(1).catch(error => error.name);
    const outcomes = await Promise.allSettled([first, second, final]);
    const chunks = outcomes.filter(result => result.status === 'fulfilled').map(result => result.value);
    await new Promise(resolve => setTimeout(resolve, 20));
    const samples = chunks.flatMap(buffer => Array.from(buffer.getChannelData(0)));
    const invalid = new OfflineAudioContext(1, 1, 8192);
    const zero = await invalid.startRendering(0).then(() => 'accepted', error => error.name);
    return {running, chunkLengths: String(chunks.map(buffer => buffer.length)) === '128,256,21', overcommit: await extra === 'InvalidStateError', pcm: samples.length === 405 && samples.every((sample, frame) => sample === frame / 512), completion: events.length === 1 && events[0] === chunks.at(-1), closed: context.state === 'closed', clock: context.currentTime * 8192 === 512, zero: zero === 'NotSupportedError'};
  });
  if (wptRoot) {
    const files = ['current-time-block-size.html', 'offlineaudiocontext-incremental-rendering-data-integrity.html'];
    const directory = '/webaudio/the-audio-api/the-offlineaudiocontext-interface/';
    const allowed = new Set(['/resources/testharness.js', '/resources/testharnessreport.js', ...files.map(file => directory + file)]);
    await page.route('**/*', async route => {
      const path = new URL(route.request().url()).pathname;
      if (!allowed.has(path)) return route.abort();
      let body = await readFile(join(wptRoot, path), 'utf8');
      if (path === '/resources/testharness.js') body += '\nglobalThis.results = new Promise(resolve => add_completion_callback((tests, status) => resolve(status.status === 0 && tests.length === 1 && tests[0].status === 0)));';
      await route.fulfill({body, contentType: path.endsWith('.html') ? 'text/html' : 'application/javascript'});
    });
    for (const file of files) {
      await page.goto('https://wpt.test' + directory + file);
      await guard();
      results[file] = await page.evaluate(() => globalThis.results);
    }
  }
  console.log(JSON.stringify(results));
  if (!Object.values(results).every(Boolean)) process.exitCode = 1;
} finally { await browser.close(); }
