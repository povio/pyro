// Playwright-over-CDP smoke test against a Lightpanda `serve` endpoint.
// Usage: node smoke.mjs <cdpEndpoint e.g. ws://127.0.0.1:9222> <siteBaseUrl e.g. http://127.0.0.1:8080> [out.json]
// Runs every step even if an earlier one failed (where meaningful) and writes a per-step report.
// Steps that cannot run because a prerequisite failed are recorded as skipped.
// Exit status: 0 only when every step passed; 1 otherwise; 2 for usage errors.
import { createRequire } from 'node:module';
import fs from 'node:fs';

const require = createRequire(import.meta.url);
// Pinned in bench/package.json to the Playwright version TOS uses (1.59.1).
const PW_PATH = process.env.PLAYWRIGHT_CORE_PATH ?? 'playwright-core';
const { chromium } = require(PW_PATH);

const [cdp, site, outPath] = process.argv.slice(2);
if (!cdp || !site) {
  console.error('usage: node smoke.mjs <cdpEndpoint> <siteBaseUrl> [out.json]');
  process.exit(2);
}
const STEP_TIMEOUT = 10_000;
const OVERALL_TIMEOUT = 120_000;
const steps = [];
const t0 = performance.now();
const overall = setTimeout(() => {
  console.error(`smoke: overall deadline of ${OVERALL_TIMEOUT} ms exceeded`);
  process.exit(1);
}, OVERALL_TIMEOUT);

function skip(name, reason) {
  steps.push({ step: name, ok: false, skipped: true, reason });
}

async function step(name, fn) {
  const s = performance.now();
  try {
    const detail = await fn();
    steps.push({ step: name, ok: true, ms: +(performance.now() - s).toFixed(1), detail: detail ?? null });
    return true;
  } catch (e) {
    steps.push({ step: name, ok: false, ms: +(performance.now() - s).toFixed(1), error: String(e?.message ?? e).split('\n').slice(0, 6).join('\n') });
    return false;
  }
}

let browser, context, page;
await step('connectOverCDP', async () => {
  browser = await chromium.connectOverCDP(cdp, { timeout: STEP_TIMEOUT });
  return { version: browser.version(), contexts: browser.contexts().length };
});
const PAGE_STEPS = ['goto', 'locator.fill', 'click+waitForResponse', 'evaluate', 'route.fulfill'];
if (browser) {
  await step('newContext', async () => { context = await browser.newContext(); });
  await step('newPage', async () => {
    page = await (context ?? browser.contexts()[0]).newPage();
    page.setDefaultTimeout(STEP_TIMEOUT);
  });
} else {
  skip('newContext', 'no browser connection');
  skip('newPage', 'no browser connection');
}
if (!page) for (const name of PAGE_STEPS) skip(name, 'no page');
if (page) {
  await step('goto', async () => {
    const r = await page.goto(site + '/', { waitUntil: 'load' });
    return { status: r?.status() ?? null, title: await page.title() };
  });
  await step('locator.fill', async () => {
    await page.locator('#name').fill('Alice');
    const v = await page.locator('#name').inputValue();
    if (v !== 'Alice') throw new Error(`inputValue after fill = ${JSON.stringify(v)}`);
    return { value: v };
  });
  await step('click+waitForResponse', async () => {
    const [resp] = await Promise.all([
      page.waitForResponse((r) => r.url().includes('/api/data.json')),
      page.locator('#submit').click(),
    ]);
    const body = await resp.json();
    if (resp.status() !== 200 || body?.greeting !== 'Hello, Alice') throw new Error(`unexpected response ${resp.status()} ${JSON.stringify(body)}`);
    return { url: resp.url(), status: resp.status(), body };
  });
  await step('evaluate', async () => {
    // fetch().then(json) may land just after the response event; poll briefly via evaluate.
    let txt = '';
    for (let i = 0; i < 50; i++) {
      txt = await page.evaluate(() => document.getElementById('out').textContent);
      if (txt) break;
      await new Promise((r) => setTimeout(r, 20));
    }
    if (txt !== 'Hello, Alice') throw new Error(`#out = ${JSON.stringify(txt)}`);
    const sum = await page.evaluate(() => [1, 2, 3].reduce((a, b) => a + b, 0));
    if (sum !== 6) throw new Error(`evaluate sum = ${sum}`);
    return { out: txt, sum };
  });
  await step('route.fulfill', async () => {
    await page.route('**/api/routed.json', (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ source: 'fulfilled' }) }));
    const [resp] = await Promise.all([
      page.waitForResponse((r) => r.url().includes('/api/routed.json')),
      page.locator('#routed').click(),
    ]);
    let txt = '';
    for (let i = 0; i < 50; i++) {
      txt = await page.evaluate(() => document.getElementById('routed-out').textContent);
      if (txt) break;
      await new Promise((r) => setTimeout(r, 20));
    }
    if (txt !== 'fulfilled') throw new Error(`#routed-out = ${JSON.stringify(txt)} (status ${resp.status()})`);
    return { out: txt, status: resp.status() };
  });
}
if (browser) {
  await step('close', async () => {
    if (page) await page.close();
    if (context) await context.close();
    await browser.close();
  });
} else {
  skip('close', 'no browser connection');
}

clearTimeout(overall);
const passed = steps.filter((s) => s.ok).length;
const skipped = steps.filter((s) => s.skipped).length;
const report = { cdp, site, wallMs: +(performance.now() - t0).toFixed(1), passed, skipped, total: steps.length, steps };
const json = JSON.stringify(report, null, 2);
if (outPath) fs.writeFileSync(outPath, json);
console.log(json);
process.exit(passed === steps.length ? 0 : 1);
