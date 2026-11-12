#!/usr/bin/env bun
// Memory of a `serve` process tree, state by state (macOS).
//
//   bun scripts/mem-states.mjs --bin /abs/lightningpanda [--fork] [--spa /abs/dist] [--pages 5]
//                              [--flow speed|pages] [--out file.json] [--raw dir] [--heap prefix]
//                              [--settle ms] [--watch ms] [--spa-port N] [-- extra serve args]
//
// Starts the binary, drives it with playwright-core and prints one row per state:
//   rss    `ps` RSS summed over the tree. This is what bench harnesses usually report. It counts clean file
//          pages (the binary, the JSC dylib) and, with --fork, every page a child still shares with its parent
//          once per process.
//   phys   phys_footprint summed over the tree (`footprint <pid>`): private dirty + compressed memory. Shared
//          copy-on-write pages are charged to one process, so this is what the tree really costs.
//   top    the largest footprint categories, summed over the tree.
// Flows:
//   speed  what bench-work/final/speed.mjs does between its idle and after readings (default)
//   pages  idle, connected, one blank page, N pages of the SPA (or big.html) open, all closed, disconnected
// --heap <prefix> (a `--features heap-profile` build only) dumps the sampled Rust heap of every process at
// every state; read the dumps with heap-profile.mjs, and the allocator pages next to them with heap-areas.mjs.
// --raw keeps the full `footprint` and `vmmap --summary` text of every process at every state.
import { spawn, execFileSync } from 'node:child_process';
import http from 'node:http';
import net from 'node:net';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';

const here = path.dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const dd = argv.indexOf('--');
const own = dd >= 0 ? argv.slice(0, dd) : argv;
const extra = dd >= 0 ? argv.slice(dd + 1) : [];
const opt = (k, d) => { const i = own.indexOf(k); return i >= 0 ? own[i + 1] : d; };
const BIN = opt('--bin');
if (!BIN) { console.error('usage: mem-states.mjs --bin /abs/binary [--fork] [--spa dir] [--pages N] [--flow speed|pages] [--out f] [--raw dir]'); process.exit(2); }
const FORK = own.includes('--fork');
const SPA = opt('--spa');
const N = Number(opt('--pages', 5));
const FLOW = opt('--flow', 'speed');
const OUT = opt('--out');
const RAW = opt('--raw');
const HEAP = opt('--heap');
const SETTLE = Number(opt('--settle', 1200));
const WATCH = Number(opt('--watch', 0));
const FLAGS = ['--log-level', 'error', '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const pagesDir = path.join(here, '../playwright-suite/pages');
function serveFrom(root, fallback) {
  return (req, res) => {
    const u = new URL(req.url, 'http://x');
    if (u.pathname === '/api/echo') { let b = ''; req.on('data', (d) => (b += d)); req.on('end', () => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify({ body: b })); }); return; }
    let f = path.join(root, decodeURIComponent(u.pathname));
    if (!f.startsWith(root)) { res.writeHead(403); res.end(); return; }
    if (!fs.existsSync(f) || fs.statSync(f).isDirectory()) f = fallback ? path.join(root, 'index.html') : path.join(f, 'index.html');
    if (!fs.existsSync(f)) { res.writeHead(404); res.end(); return; }
    const t = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2' }[path.extname(f)] ?? 'application/octet-stream';
    res.writeHead(200, { 'content-type': t });
    fs.createReadStream(f).pipe(res);
  };
}
const listen = (h, port = 0) => new Promise((r) => { const s = http.createServer(h); s.keepAliveTimeout = 600_000; s.listen(port, '127.0.0.1', () => r(`http://127.0.0.1:${s.address().port}`)); });
const site = await listen(serveFrom(pagesDir, false));
// --spa-port: module URLs are part of what a bytecode cache keys on, so a warm cache needs the same port.
const spa = SPA ? await listen(serveFrom(path.resolve(SPA), true), Number(opt('--spa-port', 0))) : null;
const freePort = () => new Promise((r) => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });

function tree(root) {
  const out = execFileSync('ps', ['-A', '-o', 'pid=,ppid=,rss='], { encoding: 'utf8' });
  const rows = out.trim().split('\n').map((l) => l.trim().split(/\s+/).map(Number));
  const pids = [root];
  for (let i = 0; i < pids.length; i++) for (const [p, pp] of rows) if (pp === pids[i]) pids.push(p);
  return pids.map((p) => ({ pid: p, rssKb: rows.find((r) => r[0] === p)?.[2] ?? 0 }));
}
// `footprint -f bytes <pid>`: "dirty clean reclaimable regions category" rows, then phys_footprint.
function footprintOf(pid, tag) {
  let text;
  try { text = execFileSync('footprint', ['-f', 'bytes', String(pid)], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }); } catch { return null; }
  if (RAW && tag) {
    fs.writeFileSync(path.join(RAW, `${tag}.${pid}.footprint.txt`), text);
    try { fs.writeFileSync(path.join(RAW, `${tag}.${pid}.vmmap.txt`), execFileSync('vmmap', ['--summary', String(pid)], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })); } catch {}
  }
  const cats = {};
  for (const line of text.split('\n')) {
    const m = line.match(/^\s*(\d+)(?: B)?\s+(\d+)(?: B)?\s+(\d+)(?: B)?\s+(\d+)\s+(.+?)\s*$/);
    if (m && m[5] !== 'TOTAL') cats[m[5]] = (cats[m[5]] ?? 0) + Number(m[1]);
  }
  const phys = Number(text.match(/phys_footprint: (\d+)/)?.[1] ?? 0);
  return { phys, cats };
}
const MB = (b) => (b / 1048576).toFixed(1);
const states = [];
let serverPid = 0;
// --watch <ms>: before a state's reading, the tree's RSS and its largest process's footprint every 250 ms
// for that long (how fast memory comes back after a close).
async function watch(label) {
  const t0 = Date.now();
  const row = [];
  while (Date.now() - t0 < WATCH) {
    const procs = tree(serverPid);
    const big = procs.reduce((a, p) => (p.rssKb > a.rssKb ? p : a));
    const t = Date.now() - t0;
    const f = footprintOf(big.pid, null);
    row.push(`${(t / 1000).toFixed(1)}s ${MB(procs.reduce((a, p) => a + p.rssKb, 0) * 1024)}/${f ? MB(f.phys) : '?'}`);
    await sleep(250);
  }
  console.log(`${label.padEnd(34)} watch (tree rss/largest phys): ${row.join('  ')}`);
}
async function snap(label) {
  if (WATCH) await watch(label);
  await sleep(SETTLE);
  const procs = tree(serverPid);
  if (HEAP) {
    // heap-profile builds only: the signal ends any other process. The dump is written by the process's
    // next allocation; file <prefix>.<pid>.<n>.heap, n counting this process's dumps.
    for (const p of procs) try { process.kill(p.pid, 'SIGUSR2'); } catch {}
    await sleep(400);
  }
  const cats = {};
  let phys = 0;
  for (const p of procs) {
    const f = footprintOf(p.pid, `${String(states.length).padStart(2, '0')}-${label.replace(/[^a-z0-9]+/gi, '_')}`);
    p.physKb = f ? Math.round(f.phys / 1024) : null;
    p.cats = f?.cats;
    if (f) { phys += f.phys; for (const [k, v] of Object.entries(f.cats)) cats[k] = (cats[k] ?? 0) + v; }
  }
  const rss = procs.reduce((a, p) => a + p.rssKb, 0) * 1024;
  const top = Object.entries(cats).sort((a, b) => b[1] - a[1]).slice(0, 6).filter(([, v]) => v > 262144).map(([k, v]) => `${k} ${MB(v)}`).join(', ');
  states.push({ label, rssMB: +MB(rss), physMB: +MB(phys), procs: procs.map(({ pid, rssKb, physKb, cats }) => ({ pid, rssKb, physKb, cats })), cats });
  console.log(`${label.padEnd(34)} procs ${String(procs.length).padStart(2)}  rss ${MB(rss).padStart(7)} MB  phys ${MB(phys).padStart(7)} MB  [${procs.map((p) => `${MB(p.rssKb * 1024)}/${p.physKb == null ? '?' : MB(p.physKb * 1024)}`).join(' ')}]`);
  console.log(`${''.padEnd(34)} ${top}`);
}

if (RAW) fs.mkdirSync(RAW, { recursive: true });
const port = await freePort();
const child = spawn(BIN, ['serve', '--host', '127.0.0.1', '--port', String(port), ...FLAGS, ...(FORK ? ['--fork'] : []), ...extra], { stdio: 'ignore', detached: true, env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true', ...(HEAP ? { LIGHTNINGPANDA_HEAP_PROFILE: HEAP } : {}) } });
serverPid = child.pid;
const kill = () => { try { process.kill(-child.pid, 'SIGKILL'); } catch {} };
process.on('exit', kill);
for (;;) { try { if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {} await sleep(2); }
console.log(`# ${BIN}${FORK ? ' --fork' : ''} ${extra.join(' ')}  (per process: rss/phys MB)`);
try {
  await snap('idle');
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 15000 });
  await snap('connected');
  if (FLOW === 'speed') {
    const upstream = /lightpanda$/.test(BIN) && !/lightningpanda$/.test(BIN);
    for (let i = 0; i < 10; i++) { const c = await browser.newContext(); await c.newPage(); await c.close(); }
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    await snap('10 contexts opened+closed, 1 page');
    for (let i = 0; i < 10; i++) await page.goto(site + '/index.html');
    for (let i = 0; i < 5; i++) await page.goto(site + '/big.html');
    for (let i = 0; i < 50; i++) await page.evaluate(() => 1 + 1);
    await snap('goto index x10, big x5');
    for (let i = 0; i < 10; i++) {
      await page.goto(site + '/form.html');
      await page.getByLabel('Name').fill('Ada');
      await page.getByLabel('I agree').check();
      await page.getByLabel('Color').selectOption('g');
      await page.getByRole('button', { name: 'Save' }).click();
      await page.getByRole('status').filter({ hasText: 'Saved' }).waitFor({ timeout: 5000 });
    }
    await snap('form flow x10');
    try { for (let i = 0; i < 5; i++) await page.screenshot(); } catch (e) { console.log('screenshot failed'); }
    await snap('screenshot x5');
    try { for (let i = 0; i < 5; i++) await page.pdf(); } catch (e) { console.log('pdf failed'); }
    await snap('pdf x5');
    if (spa) {
      if (upstream) await ctx.close();
      for (let i = 0; i < 5; i++) {
        const c = await browser.newContext(); const p = await c.newPage();
        await p.goto(spa + '/', { waitUntil: 'load', timeout: 30000 });
        await p.waitForFunction(() => document.querySelectorAll('input,button').length > 0, null, { timeout: 30000 });
        if (i === 0) await snap('SPA page 1 open');
        await c.close();
        if (i === 0) await snap('SPA page 1 closed');
      }
      await snap('SPA x5 done (speed.mjs "after")');
    }
  } else {
    const ctxs = [];
    { const c = await browser.newContext(); await c.newPage(); ctxs.push(c); }
    await snap('one blank page');
    const url = spa ? spa + '/' : site + '/big.html';
    const open = async () => {
      const c = await browser.newContext(); const p = await c.newPage();
      await p.goto(url, { waitUntil: 'load', timeout: 30000 });
      if (spa) await p.waitForFunction(() => document.querySelectorAll('input,button').length > 0, null, { timeout: 30000 });
      ctxs.push(c);
    };
    await open();
    await snap('1 page loaded');
    for (let i = 1; i < N; i++) await open();
    await snap(`${N} pages loaded`);
    for (const c of ctxs) await c.close();
    await snap('all pages closed');
  }
  await browser.close().catch(() => {});
  await snap('disconnected');
} catch (e) { console.log('FAILED', String(e.message).split('\n')[0]); }
if (OUT) fs.writeFileSync(OUT, JSON.stringify({ bin: BIN, fork: FORK, extra, flow: FLOW, states }, null, 1));
kill();
process.exit(0);
