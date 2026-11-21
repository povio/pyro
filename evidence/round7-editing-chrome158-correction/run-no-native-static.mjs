// WPT runner over CDP (Playwright connectOverCDP), identical for every target: testharness pages, reftests with
// --kind reftest (reftest.mjs: each reftest is reported as one subtest, "reftest"), or crashtests with --kind crashtest
// (crashtest.mjs: one subtest, "crashtest", passing when the page loaded and the browser neither crashed, panicked
// nor hung).
// Usage: bun run.mjs --wpt DIR --bin label=/path/to/serve-compatible-binary --out results.json
//          [--kind testharness|reftest|crashtest] [--base http://127.0.0.1:9415]   (default: start serve.mjs on a free port)
//          [--server static|wptserve] [--dirs dom,url,...|all] [--jobs 6] [--timeout 15000] [--long-timeout 45000]
//          [--sample-encoding N] [--match REGEX] [--tests FILE] [--list]
//          [--baseline baseline.json [--gate]] [--shard I/N] [--reuse-context] [--repeat N]
// --server wptserve: WPT's own server (wptserve.mjs) instead of serve.mjs, every context behind its host-mapping
//   proxy; discovery then keeps the pages that need wptserve (names, Python handlers, pipes). README "wptserve".
// --testdriver: with --server wptserve, testdriver pages too (testdriver.mjs).
// --tests FILE: only these URLs (a JSON array of URLs, or a results or baseline file).
// --repeat N: run every selected page N times (the gate's reruns), one result each; copies run side by side.
// --baseline: per-page durations order the run longest first and balance --shard I/N (0-based).
// --slow-jobs N: the baseline's timed-out pages run on N more workers beside --jobs (see SLOW_JOBS).
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
import { DEFAULT_DIRS, REFTEST_DIRS, allDirs, discover, discoverReftests, plan, shard, readBaselineAt, readJson } from './lib.mjs';
import { REFTEST_CONTEXT, ahemEnv, reftestEnv, runReftest } from './reftest.mjs';
import { CRASHTEST_DIRS, discoverCrashtests, runCrashtest } from './crashtest.mjs';
import { BROWSER_HOST, startProxy, startWptserve } from './wptserve.mjs';
import { installTestdriver } from './testdriver.mjs';
const HERE = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(HERE, '..', 'package.json'));
const { chromium } = require('playwright-core');

const argv = process.argv.slice(2);
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const WPT = path.resolve(opt('--wpt'));
const [LABEL, BIN] = (opt('--bin', 'x=') ?? '').split(/=(.*)/s);
const OUT = opt('--out');
const KIND = opt('--kind', 'testharness');
if (!['testharness', 'reftest', 'crashtest'].includes(KIND)) throw new Error(`--kind ${KIND}: expected testharness, reftest or crashtest`);
const KIND_DIRS = KIND === 'reftest' ? REFTEST_DIRS : KIND === 'crashtest' ? CRASHTEST_DIRS : DEFAULT_DIRS;
// Reftests and crashtests render: the reftest environment and viewport, and a page whose browser died runs again alone.
const RENDERS = KIND !== 'testharness';
// --dirs all: every directory of the checkout (fetch-wpt.sh --all), the kind's own first.
const DIRS = opt('--dirs') === 'all' ? (KIND_DIRS.includes('.') ? KIND_DIRS : allDirs(WPT, KIND_DIRS)) : opt('--dirs', KIND_DIRS.join(',')).split(',');
const JOBS = Number(opt('--jobs', 6));
const TIMEOUT = Number(opt('--timeout', 15000));
const LONG = Number(opt('--long-timeout', 45000));
const SAMPLE = Number(opt('--sample-encoding', 1));
const GATE = argv.includes('--gate');
const SERVER = opt('--server', 'static');
if (!['static', 'wptserve'].includes(SERVER)) throw new Error(`--server ${SERVER}: expected static or wptserve`);
const WPTSERVE = SERVER === 'wptserve';
// --testdriver (with --server wptserve): testdriver pages run, testdriver-vendor.js driving the browser through the
// runner (testdriver.mjs). README "testdriver".
const TESTDRIVER = argv.includes('--testdriver');
const workerGlobals = argv.includes('--worker-globals');
if (workerGlobals && (!WPTSERVE || KIND !== 'testharness')) throw new Error('--worker-globals needs testharness with --server wptserve');
if (TESTDRIVER && !WPTSERVE) throw new Error('--testdriver needs --server wptserve');
const BASELINE = opt('--baseline') && readBaselineAt(opt('--baseline'));
const [SHARD, SHARDS] = (opt('--shard', '0/1')).split('/').map(Number);
const REUSE = argv.includes('--reuse-context');
const REPEAT = Number(opt('--repeat', 1));
// A run over part of the suite: the gate does not report baseline pages it did not run as missing.
// --dirs all is every page there is: not a restricted run (a whole-suite baseline can start from it).
const RESTRICTED = (opt('--dirs') !== 'all' && DIRS.join(',') !== KIND_DIRS.join(',')) || !!opt('--match') || !!opt('--tests') || SAMPLE !== 1;
if (GATE && !BASELINE) throw new Error('--gate needs --baseline');
if (!(SHARD >= 0 && SHARD < SHARDS)) throw new Error(`--shard ${opt('--shard')}: expected I/N with 0 <= I < N`);
if (!(REPEAT >= 1)) throw new Error(`--repeat ${opt('--repeat')}: expected N >= 1`);

function selectTests() {
  const { tests, excluded, info } = KIND === 'reftest' ? discoverReftests(WPT, DIRS, { wptserve: WPTSERVE, testdriver: TESTDRIVER }) : KIND === 'crashtest' ? discoverCrashtests(WPT, DIRS, { wptserve: WPTSERVE, testdriver: TESTDRIVER }) : discover(WPT, DIRS, { sampleEncoding: SAMPLE, wptserve: WPTSERVE, testdriver: TESTDRIVER, workerGlobals });
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
  return { tests: Array.from({ length: REPEAT }, () => mine.map((t) => ({ ...t }))).flat(), excluded, info, skipped: p.skipped, discovered: sel.length };
}
const { tests, excluded, info, skipped, discovered } = selectTests();
if (argv.includes('--list')) {
  const by = {};
  for (const t of tests) by[t.dir] = (by[t.dir] ?? 0) + 1;
  const est = tests.reduce((a, t) => a + t.est, 0);
  console.log(JSON.stringify({ total: tests.length, by, excluded, ...(info ? { info } : {}), gateSkipped: skipped.length, estimatedMs: est }, null, 1));
  process.exit(0);
}
const gitRev = spawnSync('git', ['-C', WPT, 'rev-parse', 'HEAD'], { encoding: 'utf8' });
const WPT_REVISION = gitRev.status === 0 ? gitRev.stdout.trim() : null;

// ---------- WPT server ----------
let BASE = opt('--base');
let finishing = false;
let server, wptserve, proxy;
// A run whose server dies or stops answering fails as a whole (exit 3, no results file): its pages' error documents
// must not be judged, and the gate must not record a baseline from it. (Each page's own main document response is
// checked as well: lib.mjs documentError.)
const SERVER_LOST = 3;
function serverLost(why) {
  console.error(`${LABEL}: the WPT server ${why}; the run fails`);
  process.exit(SERVER_LOST);
}
// Ended by a signal or an uncaught error: the servers and the browser go with the runner (the 'exit' handlers).
for (const [sig, code] of [['SIGINT', 130], ['SIGTERM', 143], ['SIGHUP', 129]]) process.on(sig, () => process.exit(code));
process.on('uncaughtException', (e) => { console.error(e); process.exit(1); });
process.on('unhandledRejection', (e) => { console.error(e); process.exit(1); });
if (!BASE && WPTSERVE) {
  wptserve = await startWptserve(WPT);
  proxy = await startProxy();
  BASE = wptserve.base;
  wptserve.onExit((code) => serverLost(`(wptserve) exited (${code}); log: ${wptserve.logFile}`));
  // Liveness: this run's token over http every 10 s; three misses in a row (about 30 s and more) end the run.
  let misses = 0, probing = false;
  setInterval(async () => {
    if (probing) return;
    probing = true;
    misses = (await wptserve.alive()) ? 0 : misses + 1;
    probing = false;
    if (misses >= 3) serverLost(`(wptserve) stopped answering; log: ${wptserve.logFile}`);
  }, 10000).unref();
} else if (!BASE) {
  server = spawn(process.execPath, [path.join(HERE, 'serve.mjs'), WPT, '0'], { stdio: ['ignore', 'pipe', 'inherit'] });
  const line = await new Promise((r, j) => {
    readline.createInterface({ input: server.stdout }).once('line', r);
    server.once('exit', () => j(new Error('serve.mjs exited')));
  });
  BASE = `http://127.0.0.1:${line.match(/(\d+)$/)[1]}`;
  server.once('exit', (code) => { if (!finishing) serverLost(`(serve.mjs) exited (${code})`); });
}

// The origin a page loads from, as wptrunner picks it from the file name under wptserve: `.https.` (and
// `.serviceworker.`) pages over https, `.www.` pages from the www subdomain. h2 is not served (wptserve --no-h2).
function baseFor(url) {
  if (!wptserve) return BASE;
  const name = url.replace(/[?#].*$/, '').split('/').pop();
  const https = /\.(https|serviceworker)\./.test(name);
  const host = /\.www\./.test(name) ? `www.${BROWSER_HOST}` : BROWSER_HOST;
  return https ? `https://${host}:${wptserve.ports.https}` : `http://${host}:${wptserve.ports.http}`;
}

// ---------- browser ----------
// A free port. With the machine out of ephemeral ports (a stash-polling page, see drainPorts) listening fails too:
// try again for up to 2 minutes rather than leave the runner without a browser.
function freePortOnce() {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.once('error', reject);
    s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => resolve(p)); });
  });
}
async function freePort() {
  for (let i = 0; ; i++) {
    try { return await freePortOnce(); } catch (e) {
      if (i >= 60) throw e;
      if (i === 0) console.error(`${LABEL}: no free port (${e.code}); waiting`);
      await new Promise((r) => setTimeout(r, 2000));
    }
  }
}
let child, endpoint, generation = 0, browserCpuS = 0;
// Crashtests read the browser's stderr: a Rust panic the process survives (caught at a binding, or on a worker
// thread) prints `thread '...' panicked at <file>:<line>:<col>:` and its message on the next line. `panics` counts
// them, `lastPanic` is the newest, `stderrTail` the last lines of the current browser (a crash's last words).
// CRASHTEST_STDERR=FILE appends every line there (with RUST_BACKTRACE=1, the panics' backtraces).
let panics = 0, lastPanic = '', stderrTail = [];
const STDERR_LOG = process.env.CRASHTEST_STDERR && fs.createWriteStream(process.env.CRASHTEST_STDERR, { flags: 'a' });
function watchStderr(stream) {
  const tail = stderrTail = [];
  let head = null;
  readline.createInterface({ input: stream }).on('line', (line) => {
    STDERR_LOG?.write(line + '\n');
    tail.push(line.slice(0, 300));
    if (tail.length > 12) tail.shift();
    if (head != null) { lastPanic = `${head} ${line.trim()}`.slice(0, 400); head = null; }
    if (/panicked at /.test(line)) { panics++; head = line.trim(); lastPanic = head.slice(0, 400); }
  });
}
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
    // PYRO_PARENT_PID: the browser exits with this runner however it ends. It runs in a process group of its own, so a
    // runner killed with its CI job (a cancelled or superseded run) left it serving on the self-hosted machine.
    { stdio: KIND === 'crashtest' ? ['ignore', 'ignore', 'pipe'] : 'ignore', detached: true, env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true', PYRO_PARENT_PID: String(process.pid), ...(RENDERS ? reftestEnv(WPT) : ahemEnv(WPT)), ...(KIND === 'crashtest' ? { RUST_BACKTRACE: process.env.RUST_BACKTRACE ?? '0' } : {}) } });
  if (child.stderr) watchStderr(child.stderr);
  for (let i = 0; i < 1000; i++) {
    try { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) { endpoint = `http://127.0.0.1:${port}`; return; } } catch {}
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error('browser did not start');
}
process.on('exit', () => { try { process.kill(-child.pid, 'SIGKILL'); } catch {} try { server?.kill('SIGKILL'); } catch {} wptserve?.stop(); });

// Runs in the page: resolves once the harness completes (an add_completion_callback, no polling), or after `ms`.
// `report` is the shim's object; until testharness.js and the shim have loaded, look again every 5 ms.
function waitForReport(ms) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (complete) => {
      if (settled) return;
      settled = true;
      const current = typeof report === 'object' && report ? report : null;
      resolve({ complete, log: current?.log ?? null, harnessStatus: current?.harnessStatus });
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
// Playwright's default viewport, which the testharness pages without one of their own (lib.mjs discover()) run in.
const DEFAULT_VIEWPORT = { width: 1280, height: 720 };
async function openPage(slot, t) {
  // A connection to a browser that was replaced since (another slot's page crashed or hung it) is dead: drop it.
  if (slot.browser && slot.gen !== generation) await resetSlot(slot);
  if (slot.ctx && !REUSE) {
    const ctx = slot.ctx;
    slot.ctx = slot.page = null;
    let closed = false;
    await Promise.race([ctx.close().then(() => { closed = true; }, () => {}), sleep(5000)]);
    if (!closed) await resetSlot(slot);
  }
  if (!slot.browser) { slot.gen = generation; slot.browser = await chromium.connectOverCDP(endpoint, { timeout: 15000 }); }
  if (!slot.page) {
    const viewport = RENDERS ? REFTEST_CONTEXT : t.viewport ? { viewport: t.viewport, deviceScaleFactor: 1 } : {};
    // wptserve: its hosts through the proxy (no bypass: localhost too, as Chrome's host rules map every name), and
    // WPT's test certificates.
    const served = proxy ? { proxy: { server: `http://127.0.0.1:${proxy.port}`, bypass: '<-loopback>' }, ignoreHTTPSErrors: true } : {};
    slot.ctx = await slot.browser.newContext({ ...viewport, ...served });
    if (TESTDRIVER) await installTestdriver(slot.ctx);
    slot.page = await slot.ctx.newPage();
    slot.page.on('dialog', (d) => d.dismiss().catch(() => {}));
  }
  // A context kept from the page before (--reuse-context) may have that page's viewport.
  if (!RENDERS && REUSE) {
    const want = t.viewport ?? DEFAULT_VIEWPORT, now = slot.page.viewportSize();
    if (now?.width !== want.width || now?.height !== want.height) await slot.page.setViewportSize(want);
  }
  return slot.page;
}

// `gen`: the browser (`generation`) the page runs in.
async function runOne(slot, t, gen) {
  const page = await openPage(slot, t);
  if (KIND === 'reftest') return runReftest(page, t, baseFor(t.url), t.limit);
  if (KIND === 'crashtest') return runCrashtest(page, t, baseFor(t.url), t.limit);
  const t0 = Date.now();
  await page.goto(`${baseFor(t.url)}/${t.url}`, { waitUntil: 'commit', timeout: t.limit });
  // The wait also fails when the browser is gone, replaced because another slot's page hung it. It would fail until
  // the timeout and record a page that was about to complete as timed out with no result: the worker runs it again.
  const replaced = () => { if (gen !== generation) throw new Error('the browser was replaced under this page'); };
  for (;;) {
    const left = t.limit - (Date.now() - t0);
    // A navigation inside the test destroys the context the wait runs in: wait again in the new one.
    const r = left > 0 ? await page.evaluate(waitForReport, left).catch(() => null) : null;
    if (r?.complete) return { status: 'complete', log: r.log, harnessStatus: r.harnessStatus, ms: Date.now() - t0 };
    if (!r) replaced();
    if (Date.now() - t0 >= t.limit) {
      const partial = r?.log ?? (await page.evaluate(() => (typeof report === 'object' && report ? report.log : null)).catch(() => null));
      if (partial == null) replaced();
      return { status: 'timeout', log: partial, ms: Date.now() - t0 };
    }
    await sleep(20);
  }
}
// A page that spins the browser's main thread never answers `evaluate`, and closing its connection
// never returns either: past a hard deadline the page is recorded as `hang` and the browser is replaced.
async function guarded(slot, t, gen) {
  const hard = t.limit + 15000;
  let timer;
  const hang = new Promise((r) => { timer = setTimeout(() => r({ status: 'hang', ms: hard }), hard); });
  const run = runOne(slot, t, gen);
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
// A subtest name that holds a URL the page resolved (interpolation pages name their url() values) holds this
// server's origin, whose port differs from run to run: named by wptserve's origin instead, so the name is the same
// in every run (the baseline keys passes by name).
// Under wptserve every port is free-picked: each is named by wptserve's default for it.
const PORT_NAMES = wptserve ? [[wptserve.ports.http, 8000], [wptserve.ports.http2, 8001], [wptserve.ports.https, 8443], [wptserve.ports.https2, 8444], [wptserve.ports.ws, 8888], [wptserve.ports.wss, 8889]] : [];
const stableName = (s) => PORT_NAMES.reduce((x, [from, to]) => x.replaceAll(`:${from}`, `:${to}`), s.replaceAll(BASE, 'http://web-platform.test:8000'));
const LINE = /^(.*?)\|(Pass|Fail|Timeout|Not Run|Optional Feature Unsupported)(?:\|(.*))?$/s;
function parse(log) {
  const subs = [];
  for (const line of (log ?? '').split('\n')) {
    if (!line) continue;
    const m = line.match(LINE);
    if (m) { subs.push({ name: stableName(m[1]), status: m[2], msg: (m[3] ?? '').slice(0, 200) }); continue; }
    const parts = line.split('|');
    if (parts.length < 2) continue;
    subs.push({ name: stableName(parts[0]), status: parts[1], msg: parts.slice(2).join('|').slice(0, 200) });
  }
  return subs;
}

// Reftests and crashtests: a page whose browser died under it runs again alone (no other page in the browser), so the page that
// crashes the browser is told apart from the pages that ran beside it: one that loses its browser alone is recorded
// as `crash`, the others get their own result. (Testharness pages run again up to 3 times beside the others.)
// Crashtests also run a page again alone when a panic was printed while it ran beside others, and record it as
// `panic` when one is printed again while it runs alone.
let running = 0, solo = null;
const idle = [];
async function enter(t) {
  while (solo) await solo;
  const leave = () => { if (--running === 0) idle.splice(0).forEach((f) => f()); };
  if (!t.solo) { running++; return leave; }
  let release;
  solo = new Promise((r) => { release = r; });
  while (running > 0) await new Promise((r) => idle.push(r));
  running++;
  return () => { solo = null; release(); leave(); };
}

// Whether the browser process has exited, waiting up to 1 s for its exit to be reported.
async function exited(proc) {
  if (proc.exitCode != null || proc.signalCode) return true;
  await Promise.race([new Promise((r) => proc.once('exit', r)), sleep(1000)]);
  return proc.exitCode != null || !!proc.signalCode;
}

// Watchdog: every page in flight, by worker. guarded() bounds a page's own run; this catches a worker stuck anywhere
// else (a context that never closes, a browser that never starts) or a page whose guard did not fire. Past its
// limit plus WATCHDOG_MS the browser is killed and replaced, which fails every wait on it, and the stuck page is
// logged. WPT_TRACE=FILE appends `start|end <worker> <url> [status ms]` per page (find a hang after the fact).
const WATCHDOG_MS = 60000;
const inflight = new Map();
const TRACE = process.env.WPT_TRACE && fs.createWriteStream(process.env.WPT_TRACE, { flags: 'a' });
const trace = (line) => TRACE?.write(`${new Date().toISOString()} ${line}\n`);
let watchdogKills = 0;
const watchdog = setInterval(() => {
  const now = Date.now();
  for (const [w, p] of inflight) {
    // Again after another WATCHDOG_MS when the worker is still stuck after a replacement.
    if (now - (p.killedAt ?? p.since) < (p.killedAt ? WATCHDOG_MS : (p.limit ?? TIMEOUT) + 15000 + WATCHDOG_MS)) continue;
    p.killedAt = now;
    watchdogKills++;
    console.error(`${LABEL}: watchdog: worker ${w} stuck ${((now - p.since) / 1000).toFixed(0)} s on ${p.url}; replacing the browser`);
    trace(`watchdog ${w} ${p.url}`);
    killBrowser();
    restartBrowser(generation);
  }
  // The runner as a whole: no page finished for STALL_MS (every worker stuck, the browser unable to start) ends the
  // run with what it has, the pages without a result recorded as errors, rather than holding the CI job (a shard
  // once sat 2 h after a failed browser start). The gate then sees every planned page.
  if (now - lastFinish > STALL_MS && results.length < PLAN.length) {
    console.error(`${LABEL}: no page finished for ${STALL_MS / 60000} min; ending the run`);
    trace('stalled');
    finish('runner stalled');
  }
}, 5000);
watchdog.unref();
const STALL_MS = Number(process.env.WPT_STALL_MS ?? 10 * 60000);
let lastFinish = Date.now();
// Progress at least every 30 s (a slow run looks the same as a hung one otherwise).
let lastProgress = Date.now();

const PLAN = tests.map((t) => t.url);
await startBrowser();
const results = [];
let done = 0, restarts = 0;
// The runner's event loop delay, sampled every 100 ms: one loop serves every worker (CDP messages, a reftest's 2.6 MB
// base64 screenshot, pixel comparisons), so a busy loop holds pages that the browser has finished.
const lags = [];
let lagAt = Date.now();
const lagTimer = setInterval(() => { const n = Date.now(); lags.push(n - lagAt - 100); lagAt = n; }, 100);
lagTimer.unref();
const t0 = Date.now();
// Resolves once a TCP connect to the proxy (or the server) succeeds again, after at most 90 s.
let portsDrained = null;
async function drainPorts() {
  const port = proxy?.port ?? Number(new URL(BASE).port);
  const t0 = Date.now();
  while (Date.now() - t0 < 90000) {
    const ok = await new Promise((resolve) => {
      const c = net.connect(port, '127.0.0.1', () => { c.destroy(); resolve(true); });
      c.on('error', () => resolve(false));
    });
    if (ok) { await sleep(5000); return; }
    await sleep(2000);
  }
}
// --slow-jobs N: pages that timed out in the baseline (`known: 'timeout'`, lib.mjs plan()) or took 5 s or more in it
// (est, mostly testharness's own 10 s timeout ending the page) wait nearly idle. In a nightly testharness shard
// (8,890 pages) they were 6% of the pages and 60% of the page time. They go to a queue of their own served by N more
// workers, beside the --jobs workers, so they neither hold those workers nor get a shorter cap: they still run to
// their timeout and stay in the results as they come out.
const SLOW_JOBS = Number(opt('--slow-jobs', 0));
const isSlow = (t) => SLOW_JOBS > 0 && !!t.known && (t.known === 'timeout' || t.est >= 5000);
const queues = { main: { list: tests.filter((t) => !isSlow(t)), next: 0 }, slow: { list: tests.filter(isSlow), next: 0 } };
let workers = 0;
async function worker(q) {
  const slot = {};
  const w = workers++;
  while (q.next < q.list.length) {
    const t = q.list[q.next++];
    let r;
    if (restarting) await restarting;
    const leave = RENDERS ? await enter(t) : () => {};
    if (restarting) await restarting;
    const gen = generation, panicsBefore = panics, tail = stderrTail;
    // In flight from here: waiting for a solo page or a restart above is not being stuck.
    const startedAt = Date.now();
    inflight.set(w, { url: t.url, since: startedAt, limit: t.limit });
    trace(`start ${w} ${t.url}`);
    try { r = await guarded(slot, t, gen); }
    catch (e) {
      r = { status: 'error', error: String(e.message ?? e).split('\n')[0].slice(0, 200) };
      await resetSlot(slot);
      // Out of ephemeral ports: a page polling a wptserve stash handler in a tight loop (the scroll-to-text-fragment
      // pages wait for a popup's results that never come) makes ~1,000 requests a second, each a new connection from
      // wptserve to its stash manager, and macOS's 16,384 ports sit in TIME_WAIT for 30 s. Every connect then fails
      // (88 pages in a row in the first nightly, as ERR_PROXY_CONNECTION_FAILED). Wait until a connect works again,
      // then run the page again.
      if (false) { // Static matched correction: do not retry any URL.

        console.error(`${LABEL}: ${t.url}: connections fail (ports exhausted?); waiting for them to drain`);
        inflight.delete(w);
        await (portsDrained ??= drainPorts().finally(() => { portsDrained = null; }));
        t.retried = (t.retried ?? 0) + 1; q.list.push(t); continue;
      }
      const lost = gen !== generation || /ECONNREFUSED|has been closed/.test(r.error);
      // Whether this page's browser died (or was replaced): a lost connection can arrive before its exit does.
      const died = gen !== generation || (lost ? await exited(child) : child.exitCode != null || !!child.signalCode);
      // This page's browser died and no slot has replaced it yet. (After a replacement `child` may still be the
      // browser that was killed for it, until the new one is spawned.)
      if (gen === generation && died) { restarts++; await restartBrowser(gen); }
      if (RENDERS && lost) {
        if (!t.solo) { t.solo = true; q.list.push(t); continue; }
        if (died) {
          r.status = 'crash';
          // The browser's last words (a panic that aborted, an assertion), read once its stderr has drained.
          if (KIND === 'crashtest') { await sleep(50); r.error = `browser died: ${tail.slice(-4).join(' | ') || 'nothing on stderr'}`.slice(0, 600); }
        }
      }
      // The browser was replaced under this page (another slot's page hung it): run the page again.
      else if (lost || died) { r.status = 'error'; r.error = 'Browser connection lost; observation withheld without replay.'; }
    } finally { leave(); inflight.delete(w); }
    if (KIND === 'crashtest' && r.status !== 'crash' && r.status !== 'hang') {
      // A panic's lines may still be in the pipe when the page's last answer arrives.
      await sleep(10);
      if (panics !== panicsBefore) {
        if (!t.solo) { t.solo = true; q.list.push(t); await resetSlot(slot); continue; }
        r = { status: 'panic', ms: r.ms, error: lastPanic, subtests: [] };
        await resetSlot(slot);
      }
    }
    if (r.status === 'hang') { slot.browser = slot.ctx = slot.page = null; restarts++; await restartBrowser(gen); }
    if (r.status === 'timeout') await resetSlot(slot);
    const { est, limit, retried, solo: alone, refs, fuzzy, viewport, ...meta } = t;
    inflight.delete(w);
    trace(`end ${w} ${t.url} ${r.status} ${r.ms ?? ''}`);
    // A reftest's stage timings, and the worker's own time around the page (`gap`: from the previous page's end to
    // this page's start, i.e. queueing, solo waits and context resets).
    results.push({ ...meta, status: r.status, harnessStatus: r.harnessStatus, ms: r.ms, error: r.error, subtests: r.subtests ?? parse(r.log), ...(r.stages ? { stages: { ...r.stages, gap: startedAt - (slot.lastEnd ?? startedAt) } } : {}) });
    slot.lastEnd = Date.now();
    lastFinish = Date.now();
    if (++done % 100 === 0 || Date.now() - lastProgress > 30000) {
      lastProgress = Date.now();
      if (OUT) fs.writeFileSync(OUT + '.partial', JSON.stringify({ label: LABEL, results }));
      console.error(`${LABEL}: ${done}/${PLAN.length} (${((Date.now() - t0) / 1000).toFixed(0)} s)`);
    }
  }
  await resetSlot(slot);
}
if (queues.slow.list.length) console.error(`${LABEL}: ${queues.slow.list.length} slow page(s) (timed out or 5 s+ in the baseline) run on ${Math.min(SLOW_JOBS, queues.slow.list.length)} more worker(s)`);
await Promise.all([
  ...Array.from({ length: JOBS }, () => worker(queues.main)),
  ...Array.from({ length: Math.min(SLOW_JOBS, queues.slow.list.length) }, () => worker(queues.slow)),
]);
finish();
// Writes the results and exits. `stalled`: the reason pages that have no result are recorded as errors.
function finish(stalled) {
  finishing = true;
  if (stalled) {
    const have = new Set(results.map((t) => t.url));
    for (const t of tests) if (!have.has(t.url)) { have.add(t.url); results.push({ dir: t.dir, url: t.url, status: 'error', error: stalled, subtests: [] }); }
  }
const wallMs = Date.now() - t0;
clearInterval(lagTimer);
const lagSorted = lags.sort((a, b) => a - b), pct = (q) => lagSorted[Math.min(lagSorted.length - 1, Math.floor(q * lagSorted.length))] ?? 0;
const loopLagMs = { p50: pct(0.5), p99: pct(0.99), max: lagSorted.at(-1) ?? 0, samples: lagSorted.length };
killBrowser();
const serverCpuS = server ? cpuOf(server.pid) : 0;
const ru = process.resourceUsage();
const run = {
  label: LABEL, bin: BIN, kind: KIND, mode: GATE ? 'gate' : 'full', shard: `${SHARD}/${SHARDS}`, dirs: opt('--dirs') === 'all' ? ['all'] : DIRS, jobs: JOBS,
  restricted: RESTRICTED, reuseContext: REUSE, server: SERVER, testdriver: TESTDRIVER, workerGlobals, wptRevision: WPT_REVISION, discovered, plan: PLAN, skipped,
  wallMs, loopLagMs, browserCpuS, runnerCpuS: (ru.userCPUTime + ru.systemCPUTime) / 1e6, serverCpuS, restarts, watchdogKills, excluded, ...(info ? { info } : {}), results,
};
if (OUT) { fs.writeFileSync(OUT, JSON.stringify(run)); try { fs.unlinkSync(OUT + '.partial'); } catch {} }
console.error(`${LABEL}: done ${PLAN.length} pages in ${(wallMs / 1000).toFixed(1)} s (CPU: browser ${browserCpuS.toFixed(1)} s, runner ${run.runnerCpuS.toFixed(1)} s, server ${serverCpuS.toFixed(1)} s), restarts ${restarts}, event loop lag p50 ${loopLagMs.p50} ms p99 ${loopLagMs.p99} ms`);
process.exit(0);
}
