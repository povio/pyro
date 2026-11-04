// WPT testharness runner over CDP (Playwright connectOverCDP), identical for every target.
// Usage: node run.mjs --wpt DIR --base http://127.0.0.1:9415 --bin label=/path/to/serve-compatible-binary
//                     --out results.json [--dirs dom,url,...] [--jobs 6] [--timeout 15000] [--long-timeout 45000]
//                     [--sample-encoding N] [--match REGEX] [--list]
// Start bench/wpt/serve.mjs first. See bench/README.md.
// Reads window.report (lightpanda-io/wpt fork's testharnessreport.js) after report.complete.
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const HERE = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(HERE, '..', 'package.json'));
const { chromium } = require('playwright-core');

const argv = process.argv.slice(2);
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const WPT = path.resolve(opt('--wpt'));
const BASE = opt('--base', 'http://127.0.0.1:9415');
const [LABEL, BIN] = (opt('--bin', 'x=') ?? '').split(/=(.*)/s);
const OUT = opt('--out');
const DIRS = opt('--dirs', 'dom,html/dom,encoding,url,fetch/api/basic,xhr,css/selectors,uievents,shadow-dom,webstorage').split(',');
const JOBS = Number(opt('--jobs', 6));
const TIMEOUT = Number(opt('--timeout', 15000));
const LONG = Number(opt('--long-timeout', 45000));

// ---------- discovery ----------
function walk(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) { if (!['resources', 'support', 'reference', 'tools', 'crashtests'].includes(e.name)) walk(p, out); }
    else out.push(p);
  }
  return out;
}
const excluded = {};
const exclude = (why) => { excluded[why] = (excluded[why] ?? 0) + 1; };
function discover() {
  const tests = [];
  for (const d of DIRS) {
    for (const f of walk(path.join(WPT, d))) {
      const rel = path.relative(WPT, f);
      const base = path.basename(f);
      let src, url, variants = [''], long = false;
      if (/\.(any|window)\.js$/.test(base)) {
        src = fs.readFileSync(f, 'utf8');
        const g = src.match(/^\/\/\s*META:\s*global=(.*)$/m);
        if (base.endsWith('.any.js') && g && !g[1].split(',').map((s) => s.trim()).some((x) => x === 'window' || x === 'default')) { exclude('no window global'); continue; }
        url = rel.replace(/\.js$/, '.html');
        const v = [...src.matchAll(/^\/\/\s*META:\s*variant=(.*)$/gm)].map((m) => m[1].trim());
        if (v.length) variants = v;
        long = /META:\s*timeout=long/.test(src);
      } else if (/\.(html|htm|xhtml)$/.test(base)) {
        if (/-ref\.|-manual\.|^ref-/.test(base) || /\.(any|window|worker)\.html$/.test(base)) continue;
        src = fs.readFileSync(f, 'utf8');
        if (!src.includes('/resources/testharness.js')) continue;
        url = rel;
        const v = [...src.matchAll(/<meta\s+name=["']?variant["']?\s+content=["']([^"']*)["']/gi)].map((m) => m[1]);
        if (v.length) variants = v;
        long = /<meta\s+name=["']?timeout["']?\s+content=["']?long/i.test(src);
      } else continue;
      if (/\.sub\.|\.https\.|\.h2\.|\.www\./.test(base)) { exclude('needs wptserve substitution/https/h2'); continue; }
      if (/testdriver\.js|testdriver-vendor/.test(src)) { exclude('needs testdriver'); continue; }
      if (/\.py\b/.test(src)) { exclude('needs wptserve python handler'); continue; }
      for (const v of variants) tests.push({ dir: d, url: url + v, long });
    }
  }
  // --sample-encoding N: keep every Nth encoding/ page (identical selection for every browser)
  const N = Number(opt('--sample-encoding', 1));
  let k = 0;
  const M = opt('--match') && new RegExp(opt('--match'));
  return tests.filter((t) => t.dir !== 'encoding' || (k++ % N === 0)).filter((t) => !M || M.test(t.url));
}

// ---------- browser ----------
async function freePort() { return new Promise((r) => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); }); }
let child, endpoint;
async function startBrowser() {
  if (child) { try { process.kill(-child.pid, 'SIGKILL'); } catch {} }
  const port = await freePort();
  child = spawn(BIN, ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error', '--load-resources', 'stylesheet', '--load-resources', 'iframe', '--cdp-max-connections', '64'],
    { stdio: 'ignore', detached: true, env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' } });
  for (let i = 0; i < 1000; i++) {
    try { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) { endpoint = `http://127.0.0.1:${port}`; return; } } catch {}
    await new Promise((r) => setTimeout(r, 20));
  }
  throw new Error('browser did not start');
}
process.on('exit', () => { try { process.kill(-child.pid, 'SIGKILL'); } catch {} });

async function runOne(slot, t) {
  if (!slot.page) {
    slot.browser = await chromium.connectOverCDP(endpoint, { timeout: 15000 });
    slot.ctx = await slot.browser.newContext();
    slot.page = await slot.ctx.newPage();
    slot.page.on('dialog', (d) => d.dismiss().catch(() => {}));
  }
  const page = slot.page;
  const limit = t.long ? LONG : TIMEOUT;
  const t0 = Date.now();
  await page.goto(`${BASE}/${t.url}`, { waitUntil: 'commit', timeout: limit });
  for (;;) {
    const r = await page.evaluate(() => (typeof report === 'object' && report ? { complete: report.complete, log: report.complete ? report.log : null } : null)).catch(() => null);
    if (r?.complete) return { status: 'complete', log: r.log, ms: Date.now() - t0 };
    if (Date.now() - t0 > limit) {
      const partial = await page.evaluate(() => (typeof report === 'object' && report ? report.log : null)).catch(() => null);
      return { status: 'timeout', log: partial, ms: Date.now() - t0 };
    }
    await new Promise((r) => setTimeout(r, 100));
  }
}
async function resetSlot(slot) {
  try { await slot.browser?.close(); } catch {}
  slot.browser = slot.ctx = slot.page = null;
}

function parse(log) {
  const subs = [];
  for (const line of (log ?? '').split('\n')) {
    if (!line) continue;
    const parts = line.split('|');
    if (parts.length < 2) continue;
    subs.push({ name: parts[0], status: parts[1], msg: parts.slice(2).join('|').slice(0, 200) });
  }
  return subs;
}

const tests = discover();
if (argv.includes('--list')) {
  const by = {};
  for (const t of tests) by[t.dir] = (by[t.dir] ?? 0) + 1;
  console.log(JSON.stringify({ total: tests.length, by, excluded }, null, 1));
  process.exit(0);
}

await startBrowser();
const results = [];
let next = 0, done = 0, restarts = 0;
const t0 = Date.now();
async function worker() {
  const slot = {};
  while (next < tests.length) {
    const t = tests[next++];
    let r;
    try { r = await runOne(slot, t); }
    catch (e) {
      r = { status: 'error', error: String(e.message ?? e).split('\n')[0].slice(0, 200) };
      await resetSlot(slot);
      if (child.exitCode != null || child.signalCode) { restarts++; await startBrowser().catch(() => {}); }
    }
    if (r.status === 'timeout') await resetSlot(slot);
    results.push({ ...t, status: r.status, ms: r.ms, error: r.error, subtests: parse(r.log) });
    if (++done % 100 === 0) fs.writeFileSync(OUT + '.partial', JSON.stringify({ label: LABEL, results }));
    if (done % 100 === 0) console.error(`${LABEL}: ${done}/${tests.length} (${((Date.now() - t0) / 1000).toFixed(0)} s)`);
  }
  await resetSlot(slot);
}
await Promise.all(Array.from({ length: JOBS }, worker));
fs.writeFileSync(OUT, JSON.stringify({ label: LABEL, bin: BIN, wallMs: Date.now() - t0, restarts, excluded, results }));
console.error(`${LABEL}: done ${tests.length} files in ${((Date.now() - t0) / 1000).toFixed(0)} s, restarts ${restarts}`);
process.exit(0);
