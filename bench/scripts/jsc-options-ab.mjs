#!/usr/bin/env bun
// A/B of process environments (JSC_* options, LIGHTNINGPANDA_* switches) on SPA page loads: what each costs in
// instructions and CPU time (every thread of the process, JSC's helpers included) and what it holds in memory.
//
//   bun scripts/jsc-options-ab.mjs --bin /abs/lightningpanda --spa /abs/dist --sampler /abs/rss-sampler
//       [--fork] [--rounds 3] [--pages 5] [--out file.json] [--stderr dir] [--quiet-ms 800] [--hold-ms 0] [--no-sw]
//       --variant base --variant nocjit:JSC_useConcurrentJIT=false --variant both:JSC_a=1,JSC_b=2 ...
//       [--variant other@/abs/other-binary[:K=V,...]]
//
// --stderr keeps each server's stderr as <dir>/<label>-r<round>.err (for JSC_logGC=1 and the like).
//
// A variant is a label, optionally another binary (`label@/abs/binary`), and the environment it adds. Every
// round starts one fresh server per variant; the order of the variants rotates round to round, so host drift
// hits each alike. Compare instructions; CPU time and wall are context on a busy machine.
//
// A page counts as loaded when its controls are there and the SPA has asked this script's file server for
// nothing during --quiet-ms: the ToS build's service worker precaches every chunk (1,386 requests, 17 MB)
// after the first render, so a page closed right at its controls has done an amount of work that depends on
// the host's load. --quiet-ms 0 closes at the controls. A round whose pages asked for fewer files than the
// others (a pause that long on a starved host) is left out of the medians. --no-sw answers 404 for the
// service worker's script: the page alone (167 requests, 3.5 MB), one VM, and far less spread between rounds.
// --hold-ms keeps every loaded page open that long with no command before it is read and closed: what a quiet
// process does with an open page (the collection of a quiet VM) is then in every page's count.
//
// Plain serve, one Playwright connection: a warm-up page, then --pages times (new context, load the SPA,
// close the context), the last one read while open, then idle 1.5 s so that what a close
// leaves to the idle loop is counted. Per page: instructions, CPU ms. Memory: `ps` RSS / phys_footprint with
// the page open (at once and after 1.2 s), after the closes, after the disconnect, and the peak RSS (20 ms
// samples).
//
// --fork: one connection per page, so one forked process per page, as lightningpanda-test drives it. Per
// page: the child's own account at its last step (LIGHTNINGPANDA_FORK_STATS: instructions, CPU, faults,
// footprint) and the kernel's at its reaping (peak RSS, CPU with the exit). Tree RSS / footprint with the last
// page open.
import { spawn, execFileSync } from 'node:child_process';
import http from 'node:http';
import net from 'node:net';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { chromium } from 'playwright-core';

const argv = process.argv.slice(2);
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const many = (k) => argv.flatMap((a, i) => (a === k ? [argv[i + 1]] : []));
const BIN = opt('--bin');
const SPA = opt('--spa');
const SAMPLER = opt('--sampler');
const FORK = argv.includes('--fork');
const ROUNDS = Number(opt('--rounds', 3));
const PAGES = Number(opt('--pages', 5));
const OUT = opt('--out');
const STDERR = opt('--stderr');
const QUIET = Number(opt('--quiet-ms', 800));
const HOLD = Number(opt('--hold-ms', 0));
const NO_SW = argv.includes('--no-sw');
if (STDERR) fs.mkdirSync(STDERR, { recursive: true });
if (!BIN || !SPA || !SAMPLER) { console.error('usage: jsc-options-ab.mjs --bin B --spa DIR --sampler S [--fork] [--rounds N] [--pages N] [--out F] --variant label[@bin][:K=V,...] ...'); process.exit(2); }
const VARIANTS = many('--variant').map((spec) => {
  const colon = spec.indexOf(':');
  const head = colon >= 0 ? spec.slice(0, colon) : spec;
  const [label, bin] = head.split('@');
  const env = Object.fromEntries((colon >= 0 ? spec.slice(colon + 1) : '').split(',').filter(Boolean).map((kv) => kv.split(/=(.*)/s).slice(0, 2)));
  return { label, bin: bin ?? BIN, env };
});
if (!VARIANTS.length) VARIANTS.push({ label: 'base', bin: BIN, env: {} });
const FLAGS = ['--log-level', 'error', '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const median = (a) => { const s = a.filter((x) => Number.isFinite(x)).sort((x, y) => x - y); return s.length ? (s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2) : NaN; };

const root = path.resolve(SPA);
const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2' };
let lastRequest = 0;
let requests = 0;
const server = http.createServer((req, res) => {
  lastRequest = performance.now(); requests++;
  let f = path.join(root, decodeURIComponent(new URL(req.url, 'http://x').pathname));
  if (!f.startsWith(root)) { res.writeHead(403); res.end(); return; }
  if (NO_SW && path.basename(f) === 'sw.js') { res.writeHead(404); res.end(); return; }
  if (!fs.existsSync(f) || fs.statSync(f).isDirectory()) f = path.join(root, 'index.html');
  res.writeHead(200, { 'content-type': types[path.extname(f)] ?? 'application/octet-stream' });
  fs.createReadStream(f).pipe(res);
});
server.keepAliveTimeout = 600_000;
const spa = await new Promise((r) => server.listen(0, '127.0.0.1', () => r(`http://127.0.0.1:${server.address().port}/`)));
const freePort = () => new Promise((r) => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });

// rss-sampler: epoch_ms resident_kib footprint_kib user_ns system_ns monotonic_ns instructions
function usage(pid) {
  try {
    const c = execFileSync(SAMPLER, [String(pid), '-', '--once', '--instructions'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim().split(/\s+/).map(Number);
    return { rssMB: c[1] / 1024, physMB: c[2] / 1024, cpuMs: (c[3] + c[4]) / 1e6, instr: c[6] };
  } catch { return null; }
}
function tree(rootPid) {
  const rows = execFileSync('ps', ['-A', '-o', 'pid=,ppid=,rss='], { encoding: 'utf8' }).trim().split('\n').map((l) => l.trim().split(/\s+/).map(Number));
  const pids = [rootPid];
  for (let i = 0; i < pids.length; i++) for (const [p, pp] of rows) if (pp === pids[i]) pids.push(p);
  return pids;
}
function treeUsage(rootPid) {
  const all = tree(rootPid).map(usage).filter(Boolean);
  return { procs: all.length, rssMB: all.reduce((a, u) => a + u.rssMB, 0), physMB: all.reduce((a, u) => a + u.physMB, 0), largestPhysMB: Math.max(...all.map((u) => u.physMB)) };
}
async function loadSpa(browser) {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto(spa, { waitUntil: 'load', timeout: 60000 });
  await page.waitForFunction(() => document.querySelectorAll('input,button').length > 0, null, { timeout: 60000 });
  if (QUIET) { const until = performance.now() + 60000; while (performance.now() - lastRequest < QUIET && performance.now() < until) await sleep(20); }
  return context;
}

async function round(variant, index) {
  const port = await freePort();
  const stats = FORK ? path.join(os.tmpdir(), `jsc-ab-${process.pid}-${Date.now()}.jsonl`) : null;
  const env = { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true', ...variant.env, ...(stats ? { LIGHTNINGPANDA_FORK_STATS: stats } : {}) };
  const child = spawn(variant.bin, ['serve', '--host', '127.0.0.1', '--port', String(port), ...FLAGS, ...(FORK ? ['--fork'] : [])], { stdio: ['ignore', 'ignore', STDERR ? fs.openSync(path.join(STDERR, `${variant.label}-r${index + 1}.err`), 'w') : 'ignore'], detached: true, env });
  const kill = () => { try { process.kill(-child.pid, 'SIGKILL'); } catch {} };
  process.on('exit', kill);
  for (;;) { try { if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {} await sleep(2); }
  const r = {};
  try {
    if (!FORK) {
      const samples = path.join(os.tmpdir(), `jsc-ab-${process.pid}-${Date.now()}.rss`);
      const sampler = spawn(SAMPLER, [String(child.pid), samples, '20'], { stdio: 'ignore' });
      const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 30000 });
      await (await loadSpa(browser)).close();
      const before = usage(child.pid);
      const requestsBefore = requests;
      const t0 = performance.now();
      let wall = 0;
      for (let i = 0; i < PAGES; i++) {
        const context = await loadSpa(browser);
        if (i === PAGES - 1) {
          wall = (performance.now() - t0) / PAGES;
          const open = usage(child.pid);
          r.openRssMB = open.rssMB; r.openPhysMB = open.physMB;
          await sleep(Math.max(1200, HOLD));
          const settled = usage(child.pid);
          r.openSettledRssMB = settled.rssMB; r.openSettledPhysMB = settled.physMB;
        } else if (HOLD) await sleep(HOLD);
        await context.close();
      }
      await sleep(1500);
      const after = usage(child.pid);
      r.instrM = (after.instr - before.instr) / PAGES / 1e6;
      r.cpuMs = (after.cpuMs - before.cpuMs) / PAGES;
      r.wallMs = wall;
      r.requests = (requests - requestsBefore) / PAGES;
      r.closedRssMB = after.rssMB; r.closedPhysMB = after.physMB;
      await browser.close().catch(() => {});
      await sleep(1500);
      const gone = usage(child.pid);
      r.disconnectedRssMB = gone.rssMB; r.disconnectedPhysMB = gone.physMB;
      sampler.kill();
      await sleep(50);
      r.peakRssMB = Math.max(...fs.readFileSync(samples, 'utf8').trim().split('\n').map((l) => Number(l.split(' ')[1]))) / 1024;
      fs.rmSync(samples, { force: true });
    } else {
      const walls = [];
      let requestsBefore = 0;
      for (let i = 0; i <= PAGES; i++) {
        if (i === 1) requestsBefore = requests;
        const t0 = performance.now();
        const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 30000 });
        const context = await loadSpa(browser);
        if (i > 0) walls.push(performance.now() - t0);
        if (HOLD) await sleep(HOLD);
        if (i === PAGES) {
          const open = treeUsage(child.pid);
          r.openRssMB = open.rssMB; r.openPhysMB = open.physMB; r.openPagePhysMB = open.largestPhysMB;
        }
        await context.close();
        await browser.close().catch(() => {});
      }
      await sleep(800);
      const idle = treeUsage(child.pid);
      r.closedRssMB = idle.rssMB; r.closedPhysMB = idle.physMB;
      r.wallMs = median(walls);
      r.requests = (requests - requestsBefore) / PAGES;
      const lines = fs.readFileSync(stats, 'utf8').trim().split('\n').map((l) => { try { return JSON.parse(l); } catch { return null; } }).filter(Boolean);
      fs.rmSync(stats, { force: true });
      // Children that served a connection, in the order they finished; the first is the warm-up.
      const served = lines.filter((l) => l.e === 'child' && l.steps.length > 2 && l.steps.at(-1).minstr - l.steps[0].minstr > 50).slice(1);
      const reaped = new Map(lines.filter((l) => l.e === 'reaped').map((l) => [l.pid, l]));
      const last = (l) => l.steps.at(-1);
      r.pagesSeen = served.length;
      r.instrM = median(served.map((l) => last(l).minstr - l.steps[0].minstr));
      r.cpuMs = median(served.map((l) => (last(l).user_us + last(l).sys_us - l.steps[0].user_us - l.steps[0].sys_us) / 1000));
      r.faults = median(served.map((l) => last(l).faults - l.steps[0].faults));
      r.cowFaults = median(served.map((l) => last(l).cow_faults - l.steps[0].cow_faults));
      r.endPhysMB = median(served.map((l) => last(l).footprint_kb / 1024));
      const reapedOf = served.map((l) => reaped.get(l.pid)).filter(Boolean);
      r.peakRssMB = median(reapedOf.map((x) => x.maxrss_kb / 1024));
      r.reapedCpuMs = median(reapedOf.map((x) => (x.user_us + x.sys_us) / 1000));
      const forks = lines.filter((l) => l.e === 'fork');
      r.forkUs = median(forks.map((l) => l.fork_us));
    }
  } catch (e) { r.error = String(e.message).split('\n')[0]; }
  kill();
  await sleep(300);
  return r;
}

const all = Object.fromEntries(VARIANTS.map((v) => [v.label, []]));
for (let i = 0; i < ROUNDS; i++) {
  const order = VARIANTS.map((_, k) => VARIANTS[(k + i) % VARIANTS.length]);
  if (i % 2) order.reverse();
  for (const v of order) {
    const r = await round(v, i);
    all[v.label].push(r);
    console.error(`round ${i + 1} ${v.label} ${JSON.stringify(Object.fromEntries(Object.entries(r).map(([k, x]) => [k, typeof x === 'number' ? +x.toFixed(1) : x])))}`);
  }
}
const summary = {};
const full = Math.max(...Object.values(all).flat().map((r) => r.requests ?? 0));
for (const [label, every] of Object.entries(all)) {
  const rs = every.filter((r) => (r.requests ?? 0) >= full - 3);
  summary[label] = { rounds: rs.length };
  for (const k of Object.keys(rs[0] ?? {})) if (typeof rs[0][k] === 'number') summary[label][k] = median(rs.map((x) => x[k]));
  summary[label].instrMin = Math.min(...rs.map((x) => x.instrM ?? NaN));
  summary[label].instrMax = Math.max(...rs.map((x) => x.instrM ?? NaN));
}
const base = summary[VARIANTS[0].label];
const keys = FORK
  ? ['instrM', 'cpuMs', 'reapedCpuMs', 'wallMs', 'requests', 'faults', 'cowFaults', 'endPhysMB', 'peakRssMB', 'openRssMB', 'openPhysMB', 'forkUs']
  : ['instrM', 'cpuMs', 'wallMs', 'requests', 'openRssMB', 'openPhysMB', 'openSettledRssMB', 'openSettledPhysMB', 'closedRssMB', 'closedPhysMB', 'disconnectedRssMB', 'peakRssMB'];
const pct = (a, b) => (Number.isFinite(a) && Number.isFinite(b) && b ? `${a >= b ? '+' : ''}${((a / b - 1) * 100).toFixed(1)}%` : '');
console.log(`# ${FORK ? 'serve --fork, one connection per page' : 'serve, one connection'}; ${ROUNDS} rounds x ${PAGES} SPA pages; medians; (delta to ${VARIANTS[0].label})`);
console.log(`| variant | ${keys.join(' | ')} | instr min..max | rounds |`);
console.log(`|---|${keys.map(() => '---').join('|')}|---|---|`);
for (const v of VARIANTS) {
  const s = summary[v.label];
  console.log(`| ${v.label} | ${keys.map((k) => `${s[k]?.toFixed(k === 'instrM' || k.endsWith('MB') || k.endsWith('Ms') ? 1 : 0)}${v === VARIANTS[0] || !['instrM', 'cpuMs', 'reapedCpuMs'].includes(k) ? '' : ` (${pct(s[k], base[k])})`}`).join(' | ')} | ${s.instrMin.toFixed(0)}..${s.instrMax.toFixed(0)} | ${s.rounds} |`);
}
if (OUT) fs.writeFileSync(OUT, JSON.stringify({ fork: FORK, rounds: ROUNDS, pages: PAGES, variants: VARIANTS, all, summary }, null, 1));
process.exit(0);
