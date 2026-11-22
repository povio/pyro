import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';

const { chromium } = createRequire(new URL('../../bench/package.json', import.meta.url))('playwright-core');
const [executablePath, wptRoot, modulePath] = process.argv.slice(2);
const directory = '/webaudio/the-audio-api/the-audiobuffersourcenode-interface/';
const pages = ['sub-sample-scheduling.html', 'audiobuffersource-duration-loop.html', 'audiobuffersource-duration-loop-playbackrate.html'];
const allowed = new Set([...pages.map(name => directory + name), '/resources/testharness.js', '/resources/testharnessreport.js', '/webaudio/resources/audit-util.js', '/webaudio/resources/audit.js']);
const browser = await chromium.launch({ executablePath });
try {
  const page = await browser.newPage();
  if (modulePath) {
    const source = await readFile(modulePath, 'utf8');
    await page.addInitScript(`Object.assign(globalThis, (${source})({activated: () => false}))`);
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
    await page.waitForFunction(() => globalThis.results);
    outcomes[name] = await page.evaluate(() => globalThis.results);
  }
  console.log(JSON.stringify(outcomes));
  for (const {status, tests} of Object.values(outcomes)) {
    assert.equal(status, 0);
    assert.ok(tests.length && tests.every(([, outcome]) => outcome === 0));
  }
} finally { await browser.close(); }
