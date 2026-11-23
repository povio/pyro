import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';

const { chromium } = createRequire(new URL('../../bench/package.json', import.meta.url))('playwright-core');
const [executablePath, wptRoot, modulePath] = process.argv.slice(2);
const pagePath = '/webaudio/the-audio-api/the-audiobuffer-interface/audiobuffer-copy-channel.html';
const scripts = ['/resources/testharness.js', '/resources/testharnessreport.js', '/webaudio/resources/audit-util.js', '/webaudio/resources/audit.js'];
const browser = await chromium.launch({ executablePath });
try {
  const page = await browser.newPage();
  if (modulePath) {
    const source = await readFile(modulePath, 'utf8');
    await page.addInitScript(`Object.assign(globalThis, (${source})({activated: () => false}))`);
  }
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (![pagePath, ...scripts].includes(path)) return route.abort();
    let body = await readFile(join(wptRoot, path), 'utf8');
    if (path === scripts[0]) body += '\nglobalThis.results = new Promise(resolve => add_completion_callback((tests, status) => resolve({status:status.status, tests:tests.map(test => [test.name, test.status])})));';
    await route.fulfill({ contentType: path === pagePath ? 'text/html' : 'application/javascript', body });
  });
  await page.goto('https://wpt.test' + pagePath);
  await page.waitForFunction(() => globalThis.results);
  const harness = await page.evaluate(() => globalThis.results);
  const controls = await page.evaluate(() => {
    const frame = document.body.appendChild(document.createElement('iframe'));
    const foreign = frame.contentWindow;
    const audio = new AudioBuffer({numberOfChannels: 2, length: 4, sampleRate: 8000});
    const source = foreign.Float32Array.of(99, 0.25, -0.75, 99).subarray(1, 3);
    const destination = new foreign.Float32Array(4).fill(-1);
    function outcome(operation) {
      try { operation(); return 'accepted'; } catch (error) { return error.name; }
    }
    const write = outcome(() => audio.copyToChannel(source, 1, 1));
    const read = outcome(() => audio.copyFromChannel(destination.subarray(1), 1, 1));
    frame.remove();
    return {write, read, signal: Array.from(destination), untouched: Array.from(audio.getChannelData(0))};
  });
  console.log(JSON.stringify({harness, controls}));
  assert.deepEqual(controls, {write: 'accepted', read: 'accepted', signal: [-1, 0.25, -0.75, 0], untouched: [0, 0, 0, 0]});
  assert.equal(harness.status, 0);
  assert.ok(harness.tests.length && harness.tests.every(([, status]) => status === 0));
} finally { await browser.close(); }
