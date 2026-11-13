// Playwright (connectOverCDP) against `pyro serve --local-storage FILE --static ORIGIN=DIR`.
//
// Checks that every browser context starts with the preloaded localStorage
// entries (and that contexts stay isolated), and that a front-end build served
// from a directory loads without any HTTP server listening on its origin.
//
// Usage: node tests/e2e/startup-preload.mjs /abs/path/to/pyro [--skip-static]
// Requires `bun install` (or npm install) in bench/ for the pinned playwright-core.
// Exit status: 0 when every check passed, 1 on any failure, 2 for usage errors.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import net from 'node:net';
import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');

const bin = process.argv[2];
if (!bin) {
  console.error('usage: node tests/e2e/startup-preload.mjs /abs/path/to/pyro [--skip-static]');
  process.exit(2);
}
const skipStatic = process.argv.includes('--skip-static');

const deadline = setTimeout(() => {
  console.error(`startup-preload: overall deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);

async function freePort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.on('error', reject);
    s.listen(0, '127.0.0.1', () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });
}

// A real origin for the localStorage check: its page reads the preload.
let fixtureRequests = 0;
const fixture = http.createServer((req, res) => {
  fixtureRequests++;
  res.writeHead(200, { 'content-type': 'text/html' });
  res.end('<!doctype html><title>fixture</title><script>document.title = localStorage.getItem("token") ?? "none"</script>');
});
await new Promise((resolve, reject) => {
  fixture.once('error', reject);
  fixture.listen(0, '127.0.0.1', resolve);
});
const fixtureOrigin = `http://127.0.0.1:${fixture.address().port}`;

// The static origin: nothing listens there. Every file comes from `dist`.
const staticPort = await freePort();
const staticOrigin = `http://127.0.0.1:${staticPort}`;

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'pyro-preload-'));
const dist = path.join(tmp, 'dist');
fs.mkdirSync(path.join(dist, 'assets'), { recursive: true });
fs.writeFileSync(
  path.join(dist, 'index.html'),
  '<!doctype html><html><head><link rel="stylesheet" href="/assets/style.css"><script type="module" src="/assets/app.js"></script></head><body><div id="app">shell</div></body></html>',
);
fs.writeFileSync(
  path.join(dist, 'assets', 'app.js'),
  'document.getElementById("app").textContent = "token=" + localStorage.getItem("token") + " path=" + location.pathname; globalThis.appLoaded = true;',
);
fs.writeFileSync(path.join(dist, 'assets', 'style.css'), '#app { color: rgb(1, 2, 3); }');
fs.writeFileSync(path.join(dist, 'data.json'), JSON.stringify({ ok: true }));
const seedPath = path.join(tmp, 'local-storage.json');
fs.writeFileSync(
  seedPath,
  JSON.stringify({
    [fixtureOrigin]: { token: 'preloaded-fixture', settings: { theme: 'dark' } },
    [staticOrigin + '/some/path']: { token: 'preloaded-static' },
  }),
);

const port = await freePort();
const flags = ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error',
  '--experimental-features', 'cors', '--local-storage', seedPath];
if (!skipStatic) flags.push('--static', `${staticOrigin}=${dist}`);
const child = spawn(bin, flags, {
  stdio: ['ignore', 'ignore', 'pipe'],
  env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' },
});
let stderr = '';
child.stderr.on('data', (d) => { stderr = (stderr + d).slice(-4000); });
process.on('exit', () => {
  try { child.kill('SIGKILL'); } catch {}
  try { fs.rmSync(tmp, { recursive: true, force: true }); } catch {}
});

const results = [];
async function check(name, fn) {
  if (process.env.E2E_VERBOSE) console.error(`check: ${name}`);
  const t = performance.now();
  try {
    await fn();
    results.push({ name, ok: true, ms: +(performance.now() - t).toFixed(1) });
  } catch (e) {
    results.push({ name, ok: false, error: String(e?.stack ?? e).split('\n').slice(0, 4).join('\n') });
  }
}

const started = performance.now();
for (;;) {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) });
    if (r.ok) break;
  } catch {}
  if (child.exitCode !== null) {
    console.error(`pyro exited early (${child.exitCode}): ${stderr}`);
    process.exit(1);
  }
  if (performance.now() - started > 10_000) {
    console.error(`startup timed out: ${stderr}`);
    process.exit(1);
  }
  await new Promise((r) => setTimeout(r, 10));
}

const browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
const context = await browser.newContext();
const page = await context.newPage();

await check('localStorage preloaded on first navigation', async () => {
  await page.goto(fixtureOrigin + '/');
  assert.equal(await page.title(), 'preloaded-fixture');
  assert.equal(await page.evaluate(() => localStorage.length), 2);
  // A JSON object in the seed file is stored as its JSON text.
  assert.deepEqual(await page.evaluate(() => JSON.parse(localStorage.getItem('settings'))), { theme: 'dark' });
});
await check('writes stay in the writing context', async () => {
  await page.evaluate(() => localStorage.setItem('token', 'changed'));
  const other = await browser.newContext();
  const otherPage = await other.newPage();
  await otherPage.goto(fixtureOrigin + '/');
  assert.equal(await otherPage.title(), 'preloaded-fixture');
  assert.equal(await otherPage.evaluate(() => localStorage.getItem('token')), 'preloaded-fixture');
  await other.close();
  assert.equal(await page.evaluate(() => localStorage.getItem('token')), 'changed');
});
await check('no preload for other origins', async () => {
  // Same host, another port: a different origin.
  const other = await browser.newContext();
  const otherPage = await other.newPage();
  await otherPage.route('**/*', (route) => route.fulfill({ status: 200, contentType: 'text/html', body: '<p>x' }));
  await otherPage.goto('http://127.0.0.1:1/');
  assert.equal(await otherPage.evaluate(() => localStorage.length), 0);
  await other.close();
});

if (!skipStatic) {
  const staticPage = await (await browser.newContext()).newPage();
  const staticRequests = [];
  staticPage.on('request', (r) => staticRequests.push(r.url()));
  await check('static: document, script and stylesheet served from the directory', async () => {
    const response = await staticPage.goto(staticOrigin + '/', { waitUntil: 'load' });
    assert.equal(response.status(), 200);
    assert.match(response.headers()['content-type'], /^text\/html/);
    assert.equal(await staticPage.evaluate(() => globalThis.appLoaded), true);
    assert.equal(await staticPage.textContent('#app'), 'token=preloaded-static path=/');
    assert.equal(await staticPage.evaluate(() => getComputedStyle(document.getElementById('app')).color), 'rgb(1, 2, 3)');
    // Served on the browser thread, yet visible to CDP Network observers.
    for (const path of ['/', '/assets/app.js', '/assets/style.css']) {
      assert.ok(staticRequests.includes(staticOrigin + path), JSON.stringify(staticRequests));
    }
  });
  await check('static: index.html explicitly and a JSON asset', async () => {
    const response = await staticPage.goto(staticOrigin + '/index.html');
    assert.equal(response.status(), 200);
    const data = await staticPage.evaluate(async () => {
      const r = await fetch('/data.json');
      return [r.status, r.headers.get('content-type'), await r.json()];
    });
    assert.deepEqual(data, [200, 'application/json', { ok: true }]);
  });
  await check('static: SPA route falls back to index.html', async () => {
    const response = await staticPage.goto(staticOrigin + '/dashboard/settings?tab=1', { waitUntil: 'load' });
    assert.equal(response.status(), 200);
    assert.equal(await staticPage.textContent('#app'), 'token=preloaded-static path=/dashboard/settings');
  });
  await check('static: missing asset falls through to the (absent) server', async () => {
    const failed = await staticPage.evaluate(async () => {
      try {
        await fetch('/assets/missing.js');
        return 'resolved';
      } catch (e) {
        return 'rejected';
      }
    });
    assert.equal(failed, 'rejected');
  });
  await check('static: path traversal is not served', async () => {
    const status = await staticPage.evaluate(async () => {
      try {
        const r = await fetch('/assets/../../local-storage.json');
        return r.status;
      } catch {
        return 'rejected';
      }
    });
    // `..` is resolved by the URL parser; `%2e%2e` is rejected by the overlay and goes to the network.
    assert.equal(status, 'rejected');
    const encoded = await staticPage.evaluate(async () => {
      try {
        const r = await fetch('/assets/%2e%2e/%2e%2e/local-storage.json');
        return r.status;
      } catch {
        return 'rejected';
      }
    });
    assert.equal(encoded, 'rejected');
  });
  await check('static: cross-origin fetch from the fixture origin', async () => {
    await page.goto(fixtureOrigin + '/');
    const text = await page.evaluate(async (origin) => (await fetch(origin + '/data.json')).text(), staticOrigin);
    assert.equal(text, '{"ok":true}');
  });
}

await check('fixture server only saw the fixture pages', async () => {
  assert.ok(fixtureRequests >= 2, String(fixtureRequests));
});

await browser.close();
fixture.close();
clearTimeout(deadline);
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify({ ok: failed.length === 0, results }, null, 2));
if (failed.length) {
  console.error(`stderr tail:\n${stderr}`);
}
process.exit(failed.length ? 1 : 0);
