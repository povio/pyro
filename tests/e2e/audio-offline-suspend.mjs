import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';

const { chromium } = createRequire(new URL('../../bench/package.json', import.meta.url))('playwright-core');
const [executablePath, wptRoot, modulePath] = process.argv.slice(2);
const path = '/webaudio/the-audio-api/the-audionode-interface/audionode-disconnect-audioparam.html';
const browser = await chromium.launch({ executablePath });
try {
  const page = await browser.newPage();
  if (modulePath) {
    const source = await readFile(modulePath, 'utf8');
    await page.addInitScript(`Object.assign(globalThis, (${source})({activated: () => false}))`);
  }
  const allowed = [path, '/resources/testharness.js', '/resources/testharnessreport.js', '/webaudio/resources/audit-util.js', '/webaudio/resources/audit.js'];
  await page.route('**/*', async route => {
    const name = new URL(route.request().url()).pathname;
    if (!allowed.includes(name)) return route.abort();
    let body = await readFile(join(wptRoot, name), 'utf8');
    if (name === '/resources/testharness.js') body += '\nglobalThis.results = new Promise(resolve => add_completion_callback((tests, status) => resolve({status:status.status, tests:tests.map(test => [test.name, test.status])})));';
    await route.fulfill({ contentType: name === path ? 'text/html' : 'application/javascript', body });
  });
  await page.goto('https://wpt.test' + path);
  await page.waitForFunction(() => globalThis.results);
  const harness = await page.evaluate(() => globalThis.results);
  const controls = await page.evaluate(async () => {
    const measured = [];
    for (const requested of [0, 127, 128, 129, 128.5]) {
      const context = new OfflineAudioContext(1, 512, 8192);
      const source = context.createConstantSource();
      source.connect(context.destination);
      source.start();
      const pause = context.suspend(requested / context.sampleRate);
      const render = context.startRendering();
      await pause;
      const at = context.currentTime * context.sampleRate;
      source.disconnect();
      await context.resume();
      const samples = (await render).getChannelData(0);
      measured.push([requested, at, samples.indexOf(0)]);
    }
    return measured;
  });
  console.log(JSON.stringify({harness, controls}));
  for (const [requested, at, transition] of controls) {
    const expected = !modulePath && requested === 128.5 ? 128 : Math.ceil(requested / 128) * 128;
    assert.equal(at, expected);
    assert.equal(transition, at);
  }
  assert.equal(harness.status, 0);
  assert.ok(harness.tests.length && harness.tests.every(([, status]) => status === 0));
} finally { await browser.close(); }
