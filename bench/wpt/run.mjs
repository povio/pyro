// WPT testharness runner over CDP (Playwright connectOverCDP), identical for every target.
// Usage: bun run.mjs --wpt DIR --bin label=/path/to/serve-compatible-binary --out results.json
//          [--base http://127.0.0.1:9415]   (default: start serve.mjs on a free port)
//          [--dirs dom,url,...] [--jobs 6] [--timeout 15000] [--long-timeout 45000]
//          [--sample-encoding N] [--match REGEX] [--tests FILE] [--list]
//          [--baseline baseline.json [--gate]] [--shard I/N] [--reuse-context] [--repeat N]
// --tests FILE: only these URLs (a JSON array of URLs, or a results or baseline file).
// --repeat N: run every selected page N times (the gate's reruns), one result each; copies run side by side.
// --baseline: per-page durations order the run longest first and balance --shard I/N (0-based).
// --gate: the PR gate's plan (lib.mjs plan()): skip pages with no coverage to protect, and bound the timeout of a
//   page that completed in the baseline by 10x its duration (at least 10 s).
// Every page gets a new browser context, so a page's result does not depend on the page that ran before it in the
// same worker (window.open popups, focus, storage), which changes with sharding; --reuse-context keeps one context
// per worker, as this runner used to.
// Reads window.report (bench/wpt/testharnessreport.js, served by serve.mjs) once the harness completes.
import { createRequire } from 'node:module';
import { spawn, execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import readline from 'node:readline';
import { fileURLToPath } from 'node:url';
import { DEFAULT_DIRS, discover, plan, shard, readJson } from './lib.mjs';
const HERE = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(HERE, '..', 'package.json'));
const { chromium } = require('playwright-core');

const argv = process.argv.slice(2);
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const WPT = path.resolve(opt('--wpt'));
const [LABEL, BIN] = (opt('--bin', 'x=') ?? '').split(/=(.*)/s);
const OUT = opt('--out');
const DIRS = opt('--dirs', DEFAULT_DIRS.join(',')).split(',');
const JOBS = Number(opt('--jobs', 6));
const TIMEOUT = Number(opt('--timeout', 15000));
const LONG = Number(opt('--long-timeout', 45000));
const SAMPLE = Number(opt('--sample-encoding', 1));
const GATE = argv.includes('--gate');
const BASELINE = opt('--baseline') && readJson(opt('--baseline'));
const [SHARD, SHARDS] = (opt('--shard', '0/1')).split('/').map(Number);
const REUSE = argv.includes('--reuse-context');
const REPEAT = Number(opt('--repeat', 1));
// A run over part of the suite: the gate does not report baseline pages it did not run as missing.
const RESTRICTED = DIRS.join(',') !== DEFAULT_DIRS.join(',') || !!opt('--match') || !!opt('--tests') || SAMPLE !== 1;
if (GATE && !BASELINE) throw new Error('--gate needs --baseline');
if (!(SHARD >= 0 && SHARD < SHARDS)) throw new Error(`--shard ${opt('--shard')}: expected I/N with 0 <= I < N`);
if (!(REPEAT >= 1)) throw new Error(`--repeat ${opt('--repeat')}: expected N >= 1`);

function selectTests() {
  const { tests, excluded } = discover(WPT, DIRS, { sampleEncoding: SAMPLE });
  let sel = tests;
  const M = opt('--match') && new RegExp(opt('--match'));
  if (M) sel = sel.filter((t) => M.test(t.url));
  if (opt('--tests')) {
    const j = readJson(opt('--tests'));
    const urls = new Set(Array.isArray(j) ? j : j.results ? j.results.map((t) => t.url) : Object.keys(j.tests ?? {}));
    sel = sel.filter((t) => urls.has(t.url));
  }
  const p = plan(sel, BASELINE, { gate: GATE, timeout: TIMEOUT, longTimeout: LONG });
  const mine = shard(p.tests, SHARD, SHARDS);
  // Copies interleaved (a b c a b c): with few pages and enough jobs, a page's copies run side by side.
  return { tests: Array.from({ length: REPEAT }, () => mine.map((t) => ({ ...t }))).flat(), excluded, skipped: p.skipped, discovered: sel.length };
}
const { tests, excluded, skipped, discovered } = selectTests();
if (argv.includes('--list')) {
  const by = {};
  for (const t of tests) by[t.dir] = (by[t.dir] ?? 0) + 1;
  const est = tests.reduce((a, t) => a + t.est, 0);
  console.log(JSON.stringify({ total: tests.length, by, excluded, gateSkipped: skipped.length, estimatedMs: est }, null, 1));
  process.exit(0);
}
const gitRev = spawnSync('git', ['-C', WPT, 'rev-parse', 'HEAD'], { encoding: 'utf8' });
const WPT_REVISION = gitRev.status === 0 ? gitRev.stdout.trim() : null;

// ---------- WPT server ----------
let BASE = opt('--base');
let server;
if (!BASE) {
  server = spawn(process.execPath, [path.join(HERE, 'serve.mjs'), WPT, '0'], { stdio: ['ignore', 'pipe', 'inherit'] });
  const line = await new Promise((r, j) => {
    readline.createInterface({ input: server.stdout }).once('line', r);
    server.once('exit', () => j(new Error('serve.mjs exited')));
  });
  BASE = `http://127.0.0.1:${line.match(/(\d+)$/)[1]}`;
}

// ---------- browser ----------
async function freePort() { return new Promise((r) => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); }); }
let child, endpoint, generation = 0, browserCpuS = 0;
// CPU seconds a child process has used, from ps (the browser is SIGKILLed, so its rusage never reaches us).
function cpuOf(pid) {
  try {
    const [h, m, s] = ('0:0:' + execFileSync('ps', ['-o', 'time=', '-p', String(pid)]).toString().trim()).split(':').slice(-3).map(Number);
    return h * 3600 + m * 60 + s;
  } catch { return 0; }
}
function killBrowser() {
  if (!child) return;
  browserCpuS += cpuOf(child.pid);
  try { process.kill(-child.pid, 'SIGKILL'); } catch {}
}
async function startBrowser() {
  generation++;
  killBrowser();
  const port = await freePort();
  child = spawn(BIN, ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error', '--load-resources', 'stylesheet', '--load-resources', 'iframe', '--cdp-max-connections', '64'],
    { stdio: 'ignore', detached: true, env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' } });
  for (let i = 0; i < 1000; i++) {
    try { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) { endpoint = `http://127.0.0.1:${port}`; return; } } catch {}
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error('browser did not start');
}
process.on('exit', () => { try { process.kill(-child.pid, 'SIGKILL'); } catch {} try { server?.kill('SIGKILL'); } catch {} });

// Runs in the page: resolves once the harness completes (an add_completion_callback, no polling), or after `ms`.
// `report` is the shim's object; until testharness.js and the shim have loaded, look again every 5 ms.
function waitForReport(ms) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (complete) => {
      if (settled) return;
      settled = true;
      resolve({ complete, log: typeof report === 'object' && report ? report.log : null });
    };
    setTimeout(() => finish(false), ms);
    const hook = () => {
      if (typeof report !== 'object' || !report) return setTimeout(hook, 5);
      if (report.complete) return finish(true);
      add_completion_callback(() => finish(true));
    };
    hook();
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function openPage(slot) {
  if (slot.ctx && !REUSE) {
    const ctx = slot.ctx;
    slot.ctx = slot.page = null;
    let closed = false;
    await Promise.race([ctx.close().then(() => { closed = true; }, () => {}), sleep(5000)]);
    if (!closed) await resetSlot(slot);
  }
  if (!slot.browser) slot.browser = await chromium.connectOverCDP(endpoint, { timeout: 15000 });
  if (!slot.page) {
    slot.ctx = await slot.browser.newContext();
    slot.page = await slot.ctx.newPage();
    slot.page.on('dialog', (d) => d.dismiss().catch(() => {}));
  }
  return slot.page;
}

async function runOne(slot, t) {
  const page = await openPage(slot);
  const t0 = Date.now();
  await page.goto(`${BASE}/${t.url}`, { waitUntil: 'commit', timeout: t.limit });
  for (;;) {
    const left = t.limit - (Date.now() - t0);
    // A navigation inside the test destroys the context the wait runs in: wait again in the new one.
    const r = left > 0 ? await page.evaluate(waitForReport, left).catch(() => null) : null;
    if (r?.complete) return { status: 'complete', log: r.log, ms: Date.now() - t0 };
    if (Date.now() - t0 >= t.limit) {
      const partial = r?.log ?? (await page.evaluate(() => (typeof report === 'object' && report ? report.log : null)).catch(() => null));
      return { status: 'timeout', log: partial, ms: Date.now() - t0 };
    }
    await sleep(20);
  }
}
// A page that spins the browser's main thread never answers `evaluate`, and closing its connection
// never returns either: past a hard deadline the page is recorded as `hang` and the browser is replaced.
async function guarded(slot, t) {
  const hard = t.limit + 15000;
  let timer;
  const hang = new Promise((r) => { timer = setTimeout(() => r({ status: 'hang', ms: hard }), hard); });
  const run = runOne(slot, t);
  run.catch(() => {});
  try { return await Promise.race([run, hang]); } finally { clearTimeout(timer); }
}
let restarting = null;
async function restartBrowser(gen = generation) {
  if (gen !== generation) return restarting;
  restarting = startBrowser().catch(() => {});
  return restarting;
}
async function resetSlot(slot) {
  try { await Promise.race([slot.browser?.close(), sleep(5000)]); } catch {}
  slot.browser = slot.ctx = slot.page = null;
}

// The shim reports `name|status|message` per subtest. A name may itself contain '|' (selectors such as [a|b], URL
// tests): the status is the first testharness status that follows a '|' and ends at a '|' or the end of the line.
const LINE = /^(.*?)\|(Pass|Fail|Timeout|Not Run|Optional Feature Unsupported)(?:\|(.*))?$/s;
function parse(log) {
  const subs = [];
  for (const line of (log ?? '').split('\n')) {
    if (!line) continue;
    const m = line.match(LINE);
    if (m) { subs.push({ name: m[1], status: m[2], msg: (m[3] ?? '').slice(0, 200) }); continue; }
    const parts = line.split('|');
    if (parts.length < 2) continue;
    subs.push({ name: parts[0], status: parts[1], msg: parts.slice(2).join('|').slice(0, 200) });
  }
  return subs;
}

const PLAN = tests.map((t) => t.url);
await startBrowser();
const results = [];
let next = 0, done = 0, restarts = 0;
const t0 = Date.now();
async function worker() {
  const slot = {};
  while (next < tests.length) {
    const t = tests[next++];
    let r;
    if (restarting) await restarting;
    const gen = generation;
    try { r = await guarded(slot, t); }
    catch (e) {
      r = { status: 'error', error: String(e.message ?? e).split('\n')[0].slice(0, 200) };
      await resetSlot(slot);
      if (child.exitCode != null || child.signalCode) { restarts++; await restartBrowser(); }
      // The browser was replaced under this page (another slot's page hung it): run the page again.
      if ((gen !== generation || /ECONNREFUSED|has been closed/.test(r.error)) && (t.retried ?? 0) < 3) { t.retried = (t.retried ?? 0) + 1; tests.push(t); continue; }
    }
    if (r.status === 'hang') { slot.browser = slot.ctx = slot.page = null; restarts++; await restartBrowser(gen); }
    if (r.status === 'timeout') await resetSlot(slot);
    const { est, limit, retried, ...meta } = t;
    results.push({ ...meta, status: r.status, ms: r.ms, error: r.error, subtests: parse(r.log) });
    if (++done % 100 === 0) {
      if (OUT) fs.writeFileSync(OUT + '.partial', JSON.stringify({ label: LABEL, results }));
      console.error(`${LABEL}: ${done}/${PLAN.length} (${((Date.now() - t0) / 1000).toFixed(0)} s)`);
    }
  }
  await resetSlot(slot);
}
await Promise.all(Array.from({ length: JOBS }, worker));
const wallMs = Date.now() - t0;
killBrowser();
const serverCpuS = server ? cpuOf(server.pid) : 0;
const ru = process.resourceUsage();
const run = {
  label: LABEL, bin: BIN, mode: GATE ? 'gate' : 'full', shard: `${SHARD}/${SHARDS}`, dirs: DIRS, jobs: JOBS,
  restricted: RESTRICTED, reuseContext: REUSE, wptRevision: WPT_REVISION, discovered, plan: PLAN, skipped,
  wallMs, browserCpuS, runnerCpuS: (ru.userCPUTime + ru.systemCPUTime) / 1e6, serverCpuS, restarts, excluded, results,
};
if (OUT) { fs.writeFileSync(OUT, JSON.stringify(run)); try { fs.unlinkSync(OUT + '.partial'); } catch {} }
console.error(`${LABEL}: done ${PLAN.length} pages in ${(wallMs / 1000).toFixed(1)} s (CPU: browser ${browserCpuS.toFixed(1)} s, runner ${run.runnerCpuS.toFixed(1)} s, server ${serverCpuS.toFixed(1)} s), restarts ${restarts}`);
process.exit(0);
