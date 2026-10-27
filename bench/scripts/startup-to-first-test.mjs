// Spawn-to-first-assertion timing for `lightningpanda serve`, two setups:
//   server:  a front-end build served by a separate static HTTP server, auth
//            injected per context through Playwright storageState + init script
//            (what the TOS fixtures do today);
//   preload: the same build served by `--static ORIGIN=DIR`, auth preloaded
//            with `--local-storage FILE`; nothing listens on the app origin.
// Phases (ms from spawn): ready (/json/version 200), connected (connectOverCDP),
// context (newContext resolved), page (newPage), loaded (goto app, `load`),
// asserted (first assertion on app state rendered from localStorage).
// Usage: node startup-to-first-test.mjs --bin PATH [--runs 8] [--assets N] [--kind css|js|both] [--size KB] [--out file.json] [--only server|overlay|preload]
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_CORE_PATH ?? 'playwright-core');
const args = process.argv.slice(2);
const bin = args[args.indexOf('--bin') + 1];
const runs = Number(args[args.indexOf('--runs') + 1] || 8);
const out = args.includes('--out') ? args[args.indexOf('--out') + 1] : null;
const only = args.includes('--only') ? args[args.indexOf('--only') + 1] : null;
// Extra scripts and stylesheets per page, to mimic a real build's request count.
const assets = Number(args.includes('--assets') ? args[args.indexOf('--assets') + 1] : 0);
// Which extra assets to emit: css, js or both (default).
const kind = args.includes('--kind') ? args[args.indexOf('--kind') + 1] : 'both';
// Size of each extra asset in KB (realistic code: many functions / rules).
const sizeKb = Number(args.includes('--size') ? args[args.indexOf('--size') + 1] : 2);
function jsChunk(i, kb) {
  let out = `globalThis.chunks = (globalThis.chunks || 0) + 1;\n`;
  let n = 0;
  while (out.length < kb * 1024) {
    out += `function f${i}_${n}(a, b) { const c = a * ${n} + b; if (c > ${n}) { return { c, s: 'v' + c, t: [a, b, c].map((x) => x + 1) }; } return c; }\n`;
    n++;
  }
  return out + `globalThis.f${i} = f${i}_0;\n`;
}
function cssChunk(i, kb) {
  let out = '';
  let n = 0;
  while (out.length < kb * 1024) {
    out += `.c${i}-${n} > .x:hover, #id${n} .y[data-k="${n}"] { color: rgb(${n % 255}, 0, 0); margin: ${n % 9}px 1em; display: flex; }\n@media (min-width: ${n % 1200}px) { .m${i}-${n} { padding: ${n % 7}px; } }\n`;
    n++;
  }
  return out;
}
if (!bin || args.indexOf('--bin') < 0) { console.error('usage: --bin <path> [--runs N] [--out file] [--only server|preload]'); process.exit(2); }
const FLAGS = ['--log-level', 'error', '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'];
const free = () => new Promise((r, j) => { const s = net.createServer(); s.on('error', j); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });

// A small single-page app: shell + script + stylesheet + a JSON asset. The
// script renders from localStorage like a token-gated front end.
const dist = fs.mkdtempSync(path.join(os.tmpdir(), 'lp-first-test-'));
fs.mkdirSync(path.join(dist, 'assets'));
let extraTags = '';
for (let i = 0; i < assets; i++) {
  fs.writeFileSync(path.join(dist, 'assets', `chunk-${i}.js`), jsChunk(i, sizeKb));
  fs.writeFileSync(path.join(dist, 'assets', `chunk-${i}.css`), cssChunk(i, sizeKb));
  if (kind !== 'js') extraTags += `<link rel="stylesheet" href="/assets/chunk-${i}.css">`;
  if (kind !== 'css') extraTags += `<script src="/assets/chunk-${i}.js"></script>`;
}
fs.writeFileSync(path.join(dist, 'index.html'),
  `<!doctype html><html><head><link rel="stylesheet" href="/assets/style.css">${extraTags}<script type="module" src="/assets/app.js"></script></head><body><div id="app">booting</div></body></html>`);
fs.writeFileSync(path.join(dist, 'assets', 'app.js'),
  `const token = localStorage.getItem('auth-accessToken');
   const user = localStorage.getItem('auth-user');
   const el = document.getElementById('app');
   fetch('/assets/config.json').then((r) => r.json()).then((cfg) => {
     el.textContent = token ? 'ready:' + user + ':' + cfg.env : 'login';
   });`);
fs.writeFileSync(path.join(dist, 'assets', 'style.css'), '#app { color: rgb(1, 2, 3); }');
fs.writeFileSync(path.join(dist, 'assets', 'config.json'), JSON.stringify({ env: 'e2e' }));
const entries = [{ name: 'auth-accessToken', value: 'x'.repeat(400) }, { name: 'auth-user', value: 'e2e' }];
const expected = 'ready:e2e:e2e';
const seedPath = path.join(dist, '..', path.basename(dist) + '-seed.json');
process.on('exit', () => { try { fs.rmSync(dist, { recursive: true, force: true }); fs.rmSync(seedPath, { force: true }); } catch {} });

const MIME = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json' };
function staticServer(port) {
  const server = http.createServer((req, res) => {
    const p = decodeURIComponent(new URL(req.url, 'http://x').pathname);
    let file = path.join(dist, p);
    if (!fs.existsSync(file) || fs.statSync(file).isDirectory()) file = path.join(dist, 'index.html');
    res.writeHead(200, { 'content-type': MIME[path.extname(file)] ?? 'application/octet-stream' });
    res.end(fs.readFileSync(file));
  });
  return new Promise((r) => server.listen(port, '127.0.0.1', () => r(server)));
}

async function once(mode) {
  const port = await free();
  const appPort = await free();
  const origin = `http://127.0.0.1:${appPort}`;
  let server = null;
  const extra = [];
  if (mode === 'server') {
    server = await staticServer(appPort);
  } else {
    fs.writeFileSync(seedPath, JSON.stringify({ [origin]: Object.fromEntries(entries.map((e) => [e.name, e.value])) }));
    extra.push('--static', `${origin}=${dist}`, '--local-storage', seedPath);
  }
  const t0 = performance.now();
  const env = { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true', LIGHTNINGPANDA_STATIC_DIRECT: mode === 'overlay' ? '0' : '1' };
  const child = spawn(bin, ['serve', '--host', '127.0.0.1', '--port', String(port), ...FLAGS, ...extra], { stdio: 'ignore', env });
  const rec = { ready: null, connected: null, context: null, page: null, loaded: null, asserted: null, reload: null, error: null };
  let browser;
  try {
    for (;;) {
      try { if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {}
      if (performance.now() - t0 > 15000) throw new Error('not ready');
      await new Promise((r) => setTimeout(r, 2));
    }
    rec.ready = performance.now() - t0;
    browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`, { timeout: 15000 });
    rec.connected = performance.now() - t0;
    const ctx = mode === 'server'
      ? await browser.newContext({ storageState: { cookies: [], origins: [{ origin, localStorage: entries }] } })
      : await browser.newContext();
    if (mode === 'server') {
      // The TOS fixture also re-applies the entries from an init script.
      await ctx.addInitScript((entries) => { for (const { name, value } of entries) localStorage.setItem(name, value); }, entries);
    }
    rec.context = performance.now() - t0;
    const page = await ctx.newPage();
    rec.page = performance.now() - t0;
    await page.goto(origin + '/dashboard', { waitUntil: 'load', timeout: 10000 });
    rec.loaded = performance.now() - t0;
    const deadline = performance.now() + 5000;
    for (;;) {
      const text = await page.evaluate(() => document.getElementById('app').textContent);
      if (text === expected) break;
      if (performance.now() > deadline) throw new Error(`app text ${JSON.stringify(text)}`);
      await new Promise((r) => setTimeout(r, 1));
    }
    rec.asserted = performance.now() - t0;
    // A second navigation in the same context: what a later test in the
    // same browser process pays (duration of the goto alone).
    const t1 = performance.now();
    await page.goto(origin + '/dashboard/again', { waitUntil: 'load', timeout: 10000 });
    rec.reload = performance.now() - t1;
    await browser.close().catch(() => {});
  } catch (e) {
    rec.error = String(e.message).split('\n')[0];
  } finally {
    child.kill('SIGKILL');
    await new Promise((r) => (child.exitCode !== null || child.signalCode !== null ? r() : child.once('exit', r)));
    if (server) await new Promise((r) => server.close(r));
  }
  for (const k of ['ready', 'connected', 'context', 'page', 'loaded', 'asserted', 'reload']) if (rec[k] !== null) rec[k] = +rec[k].toFixed(1);
  return rec;
}

const median = (xs) => { const s = xs.filter((x) => x !== null).sort((a, b) => a - b); return s.length ? s[Math.floor(s.length / 2)] : null; };
// server: external static server; overlay: --static answered on the network
// I/O thread; preload: --static answered on the browser thread from memory.
const modes = only ? [only] : ['server', 'overlay', 'preload'];
const results = Object.fromEntries(modes.map((m) => [m, []]));
for (let i = 0; i < runs; i++) for (const m of modes) results[m].push(await once(m));
const phases = ['ready', 'connected', 'context', 'page', 'loaded', 'asserted', 'reload'];
const summary = Object.fromEntries(modes.map((m) => [m, Object.fromEntries(phases.map((k) => [k, median(results[m].map((r) => r[k]))]))]));
const report = { bin, runs, assets, kind, sizeKb, flags: FLAGS, summary, results };
if (out) fs.writeFileSync(out, JSON.stringify(report, null, 2));
console.log(JSON.stringify(summary, null, 2));
const failed = Object.values(results).flat().filter((r) => r.asserted === null);
if (failed.length) console.error(JSON.stringify(failed, null, 2));
process.exit(failed.length ? 1 : 0);
