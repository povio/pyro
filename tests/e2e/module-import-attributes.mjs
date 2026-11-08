// Real Playwright 1.59.1 (connectOverCDP) against `lightningpanda serve`: JSON
// and CSS module scripts through import attributes (patched JavaScriptCore,
// patch 0020-module-import-attributes). Static and dynamic imports with
// `with { type: 'json' | 'css' }`, the CSSStyleSheet a CSS module exports
// adopted by the document, the (URL, type) module map, strict MIME checks
// and the TypeErrors HTML gives for unknown or mismatched types, and a module
// worker importing JSON while a CSS import fails there without a fetch.
//
// Usage: bun tests/e2e/module-import-attributes.mjs /abs/path/to/lightningpanda
// Requires `bun install` in bench/ for the pinned playwright-core.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');

const bin = process.argv[2];
if (!bin) {
  console.error('usage: bun tests/e2e/module-import-attributes.mjs /abs/path/to/lightningpanda');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`module-import-attributes: deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

const PAGE = `<!doctype html><html><head>
<script type="module">
import config from './config.json' with { type: 'json' };
import sheet from './theme.css' with { type: 'css' };
document.adoptedStyleSheets = [sheet];
window.staticImports = { config, isSheet: sheet instanceof CSSStyleSheet, rules: sheet.cssRules.length };
</script>
</head><body><p id="t">text</p></body></html>`;

const FILES = {
  '/': ['text/html', PAGE],
  '/config.json': ['application/json', '{"name": "lp", "list": [1, 2, 3]}'],
  '/other.json': ['application/vnd.api+json; charset=utf-8', '{"other": true}'],
  '/broken.json': ['application/json', '{"name": '],
  '/plain.json': ['text/plain', '{"n": 1}'],
  // Module scripts are UTF-8 whatever the charset parameter says.
  '/utf8.json': ['application/json; charset=windows-1252', '{"s": "\u015b\u0107\u0105"}'],
  '/utf8.css': ['text/css; charset=shift-jis', '#t::after { content: "\u015b"; }'],
  '/theme.css': ['text/css; charset=utf-8', '#t { color: rgb(0, 128, 0); }'],
  '/code.js': ['text/javascript', 'export default "js";'],
  '/worker.js': ['text/javascript', `
    import config from './config.json' with { type: 'json' };
    const css = await import('./worker-sheet.css', { with: { type: 'css' } }).then(() => 'loaded', (e) => e.constructor.name);
    postMessage({ name: config.name, css });`],
  '/worker-sheet.css': ['text/css', 'p { color: red }'],
};
const requested = [];

const port = await freePort();
const child = spawn(
  bin,
  ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error'],
  { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' } },
);
let stderr = '';
child.stderr.on('data', (d) => { stderr = (stderr + d).slice(-4000); });
process.on('exit', () => { try { child.kill('SIGKILL'); } catch {} });

const started = performance.now();
for (;;) {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) });
    if (r.ok) break;
  } catch {}
  if (child.exitCode !== null) { console.error(`lightningpanda exited early (${child.exitCode}): ${stderr}`); process.exit(1); }
  if (performance.now() - started > 10_000) { console.error(`startup timed out: ${stderr}`); process.exit(1); }
  await new Promise((r) => setTimeout(r, 10));
}

const server = http.createServer((req, res) => {
  const url = req.url.split('?')[0];
  requested.push(url);
  const file = FILES[url];
  if (!file) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { 'content-type': file[0] });
  res.end(file[1]);
});
await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
const origin = `http://127.0.0.1:${server.address().port}`;

async function check(name, fn) {
  const t = performance.now();
  try {
    await fn();
    results.push({ name, ok: true, ms: +(performance.now() - t).toFixed(1) });
  } catch (e) {
    results.push({ name, ok: false, error: String(e?.stack ?? e).split('\n').slice(0, 4).join('\n') });
  }
}

const browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
const context = await browser.newContext();
const page = await context.newPage();
await page.goto(`${origin}/`);

await check('static JSON and CSS imports; the sheet is adoptable', async () => {
  await page.waitForFunction(() => window.staticImports);
  const got = await page.evaluate(() => ({
    ...window.staticImports,
    color: getComputedStyle(document.getElementById('t')).color,
    adopted: document.adoptedStyleSheets.length,
  }));
  assert.deepEqual(got, { config: { name: 'lp', list: [1, 2, 3] }, isSheet: true, rules: 1, color: 'rgb(0, 128, 0)', adopted: 1 });
});

await check('dynamic import() with the options bag', async () => {
  const got = await page.evaluate(async () => {
    const json = await import('./other.json', { with: { type: 'json' } });
    const css = await import('./theme.css', { with: { type: 'css' } });
    const again = await import('./config.json', { with: { type: 'json' } });
    const utf8 = await import('./utf8.json', { with: { type: 'json' } });
    const utf8css = await import('./utf8.css', { with: { type: 'css' } });
    return {
      utf8: utf8.default.s,
      utf8css: utf8css.default.cssRules[0].cssText,
      other: json.default,
      keys: Object.keys(json),
      cssText: css.default.cssRules[0].cssText,
      sameSheet: css.default === document.adoptedStyleSheets[0],
      sameJson: again.default === window.staticImports.config,
    };
  });
  assert.deepEqual(got, {
    utf8: '\u015b\u0107\u0105',
    utf8css: '#t::after { content: "\u015b"; }',
    other: { other: true },
    keys: ['default'],
    cssText: '#t { color: rgb(0, 128, 0); }',
    sameSheet: true,
    sameJson: true,
  });
});

await check('mismatched and unknown types are TypeErrors; bad JSON is a SyntaxError', async () => {
  const got = await page.evaluate(async () => {
    const name = (p) => p.then(() => 'ok', (e) => e.constructor.name);
    return Promise.all([
      name(import('./config.json')),
      name(import('./theme.css')),
      name(import('./plain.json', { with: { type: 'json' } })),
      name(import('./code.js', { with: { type: 'json' } })),
      name(import('./config.json', { with: { type: 'css' } })),
      name(import('./missing.json', { with: { type: 'json' } })),
      name(import('./code.js', { with: { type: 'javascript' } })),
      name(import('./code.js', { with: { type: 'text' } })),
      name(import('./code.js', { with: { other: 'x' } })),
      name(import('./code.js', { with: 1 })),
      name(import('./broken.json', { with: { type: 'json' } })),
      name(import('./code.js', { with: {} })),
    ]);
  });
  assert.deepEqual(got, [
    'TypeError', 'TypeError', 'TypeError', 'TypeError', 'TypeError', 'TypeError',
    'TypeError', 'TypeError', 'TypeError', 'TypeError', 'SyntaxError', 'ok',
  ]);
});

await check('module worker: JSON loads, CSS fails without a fetch', async () => {
  const got = await page.evaluate(() => new Promise((resolve) => {
    const w = new Worker('./worker.js', { type: 'module' });
    w.onmessage = (e) => { resolve(e.data); w.terminate(); };
    w.onerror = (e) => resolve({ error: e.message });
  }));
  assert.deepEqual(got, { name: 'lp', css: 'TypeError' });
  assert.ok(!requested.includes('/worker-sheet.css'), JSON.stringify(requested));
});

await check('close', async () => {
  await page.close();
  await context.close();
  await browser.close();
});

clearTimeout(deadline);
child.kill('SIGTERM');
server.closeAllConnections();
await new Promise((resolve) => server.close(resolve));
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify({ passed: results.length - failed.length, total: results.length, results }, null, 2));
process.exit(failed.length ? 1 : 0);
