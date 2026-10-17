// Spawn-to-first-page timing for any CDP browser binary, with Playwright 1.59.1.
// Phases (ms from spawn): ready (/json/version 200), connected (connectOverCDP),
// page (context.newPage() resolved), evaluated (first page.evaluate(1+1) on about:blank,
// recorded as null if the engine has no executable about:blank context).
// Usage: node first-page.mjs --bin A [--bin B ...] [--runs 10] [--out file.json]
// Binaries are run alternately (A,B,A,B,...) to spread drift. Exit 1 if any run fails before `page`.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import fs from 'node:fs';
import net from 'node:net';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_CORE_PATH ?? 'playwright-core');
const args = process.argv.slice(2);
const bins = args.flatMap((a, i) => (a === '--bin' ? [args[i + 1]] : []));
const runs = Number(args[args.indexOf('--runs') + 1] || 10);
const out = args.includes('--out') ? args[args.indexOf('--out') + 1] : null;
if (!bins.length) { console.error('usage: --bin <path> [--bin <path>] [--runs N] [--out file]'); process.exit(2); }
const FLAGS = ['--log-level', 'error', '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'];
const free = () => new Promise((r, j) => { const s = net.createServer(); s.on('error', j); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });

async function once(bin) {
  const port = await free();
  const t0 = performance.now();
  const child = spawn(bin, ['serve', '--host', '127.0.0.1', '--port', String(port), ...FLAGS], { stdio: 'ignore', env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' } });
  const rec = { ready: null, connected: null, page: null, evaluated: null, error: null };
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
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    rec.page = performance.now() - t0;
    try {
      const v = await Promise.race([page.evaluate(() => 1 + 1), new Promise((_, j) => setTimeout(() => j(new Error('evaluate timeout')), 3000))]);
      if (v === 2) rec.evaluated = performance.now() - t0;
    } catch (e) { rec.evaluateError = String(e.message).split('\n')[0]; }
    await browser.close().catch(() => {});
  } catch (e) {
    rec.error = String(e.message).split('\n')[0];
  } finally {
    child.kill('SIGKILL');
    await new Promise((r) => (child.exitCode !== null || child.signalCode !== null ? r() : child.once('exit', r)));
  }
  for (const k of ['ready', 'connected', 'page', 'evaluated']) if (rec[k] !== null) rec[k] = +rec[k].toFixed(1);
  return rec;
}

const median = (xs) => { const s = xs.filter((x) => x !== null).sort((a, b) => a - b); return s.length ? s[Math.floor(s.length / 2)] : null; };
const results = Object.fromEntries(bins.map((b) => [b, []]));
for (let i = 0; i < runs; i++) for (const b of bins) results[b].push(await once(b));
const summary = Object.fromEntries(bins.map((b) => [b, Object.fromEntries(['ready', 'connected', 'page', 'evaluated'].map((k) => [k, median(results[b].map((r) => r[k]))]))]));
const report = { runs, flags: FLAGS, summary, results };
if (out) fs.writeFileSync(out, JSON.stringify(report, null, 2));
console.log(JSON.stringify(summary, null, 2));
process.exit(Object.values(results).flat().some((r) => r.page === null) ? 1 : 0);
