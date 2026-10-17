// Baseline orchestrator: startup latency, smoke (direct + via CDP recording proxy), RSS sampling.
// Usage: node run-baseline.mjs --bin /path/lightpanda --results /path/results [--startup-runs 10] [--smoke-runs 5] [--no-proxy]
import { spawn, execFile } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const arg = (k, d) => { const i = args.indexOf(k); return i >= 0 ? args[i + 1] : d; };
const BIN = arg('--bin');
const RES = arg('--results');
const STARTUP_RUNS = Number(arg('--startup-runs', 10));
const SMOKE_RUNS = Number(arg('--smoke-runs', 5));
const WITH_PROXY = !args.includes('--no-proxy');
// --warm-cpu: keep one busy-spinning child process alive for the whole run so the CPU clusters stay out of
// their idle low-frequency state (Apple Silicon DVFS). Measured on this host: idle => ~107ms startup, warm => ~58ms.
const WARM_CPU = args.includes('--warm-cpu');
if (!BIN || !RES) { console.error('usage: --bin <lightpanda> --results <dir>'); process.exit(2); }
fs.mkdirSync(RES, { recursive: true });

// Exact flags used by TOS.
const LP_FLAGS = ['--log-level', 'error', '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'];
// Same env as TOS fixture (apps/fe/e2e/fixtures/lightpanda.ts).
const LP_ENV = { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const freePort = () => new Promise((res, rej) => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => res(p)); }); s.on('error', rej); });
const getJson = (url) => new Promise((res, rej) => {
  const req = http.get(url, { agent: false }, (r) => { const c = []; r.on('data', (d) => c.push(d)); r.on('end', () => res({ status: r.statusCode, body: Buffer.concat(c).toString() })); });
  req.on('error', rej); req.setTimeout(1000, () => req.destroy(new Error('timeout')));
});
const pct = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.ceil((p / 100) * s.length) - 1)]; };
const median = (xs) => { const s = [...xs].sort((a, b) => a - b); const n = s.length; return n % 2 ? s[(n - 1) / 2] : (s[n / 2 - 1] + s[n / 2]) / 2; };
const stats = (xs) => ({ n: xs.length, min: Math.min(...xs), median: +median(xs).toFixed(2), p95: +pct(xs, 95).toFixed(2), max: Math.max(...xs), samples: xs.map((x) => +x.toFixed(2)) });
const exited = (cp) => new Promise((r) => (cp.exitCode !== null || cp.signalCode !== null) ? r() : cp.once('exit', r));

async function waitReady(port, timeoutMs = 15000) {
  const start = performance.now();
  for (;;) {
    try { const r = await getJson(`http://127.0.0.1:${port}/json/version`); if (r.status === 200) return r; } catch {}
    if (performance.now() - start > timeoutMs) throw new Error(`not ready on ${port} after ${timeoutMs}ms`);
    await sleep(2);
  }
}

function waitLine(cp, re) {
  return new Promise((res, rej) => {
    let buf = '';
    cp.stdout.on('data', (d) => { buf += d; const m = buf.match(re); if (m) res(m); });
    cp.once('exit', (c) => rej(new Error(`exited ${c} before ${re}: ${buf}`)));
  });
}

// ---------- 1. startup latency ----------
async function startupRuns() {
  const times = []; let versionBody = null;
  for (let i = 0; i < STARTUP_RUNS; i++) {
    const port = await freePort();
    const t = performance.now();
    const cp = spawn(BIN, ['serve', '--host', '127.0.0.1', '--port', String(port), ...LP_FLAGS], { stdio: ['ignore', 'ignore', 'pipe'], env: LP_ENV });
    let err = ''; cp.stderr.on('data', (d) => { err += d; });
    try {
      const r = await waitReady(port);
      times.push(performance.now() - t);
      versionBody ??= JSON.parse(r.body);
    } catch (e) { console.error(`startup run ${i}: ${e.message} stderr=${err}`); }
    cp.kill('SIGTERM'); await exited(cp);
    await sleep(200);
  }
  return { method: 'spawn() -> first HTTP 200 on GET /json/version, polled every ~2ms with fresh TCP connections; process SIGTERMed between runs; 200ms gap', ...stats(times), versionBody };
}

// ---------- 2. one smoke run (optionally via proxy) ----------
async function smokeRun(tag, siteUrl, useProxy) {
  const port = await freePort();
  const lpLog = path.join(RES, `${tag}.lightpanda.stderr.log`);
  const timeOut = path.join(RES, `${tag}.time-l.txt`);
  // /usr/bin/time -l reports max RSS of the child at exit.
  const tp = spawn('/usr/bin/time', ['-l', '-o', timeOut, BIN, 'serve', '--host', '127.0.0.1', '--port', String(port), ...LP_FLAGS], { stdio: ['ignore', 'ignore', fs.openSync(lpLog, 'w')], env: LP_ENV });
  await waitReady(port);
  // find lightpanda pid (child of /usr/bin/time)
  const lpPid = await new Promise((res) => { const p = spawn('pgrep', ['-P', String(tp.pid)]); let o = ''; p.stdout.on('data', (d) => (o += d)); p.on('exit', () => res(Number(o.trim().split('\n')[0]))); });
  const rssFile = path.join(RES, `${tag}.rss-samples.txt`);
  // ps sampler: node setInterval(20ms) (node timers are precise on this host; blocking sleeps are coalesced),
  // one `ps -o rss= -p PID` in flight at a time (~7ms per exec).
  const psLines = []; let psBusy = false;
  const psTimer = setInterval(() => {
    if (psBusy) return; psBusy = true;
    execFile('/bin/ps', ['-o', 'rss=', '-p', String(lpPid)], (err, stdout) => { psBusy = false; const v = Number(String(stdout).trim()); if (!err && v > 0) psLines.push(`${Date.now()} ${v}`); });
  }, 20);
  const sampler = { kill: () => { clearInterval(psTimer); fs.writeFileSync(rssFile, psLines.join('\n') + '\n'); } };
  // native sampler: proc_pid_rusage resident + phys_footprint, 20ms nominal
  const nativeFile = path.join(RES, `${tag}.rss-native.txt`);
  const nsampler = spawn(process.env.RSS_SAMPLER ?? path.join(HERE, '..', 'rss-sampler', 'target', 'release', 'rss-sampler'), [String(lpPid), nativeFile, '20'], { stdio: 'ignore' });

  let proxy, cdpPort = port, jsonl = null;
  if (useProxy) {
    jsonl = path.join(RES, `${tag}.cdp.jsonl`);
    fs.rmSync(jsonl, { force: true });
    proxy = spawn(process.execPath, [path.join(HERE, 'cdp-record-proxy.mjs'), '--upstream', `http://127.0.0.1:${port}`, '--out', jsonl, '--listen', '0'], { stdio: ['ignore', 'pipe', 'inherit'] });
    cdpPort = Number((await waitLine(proxy, /PROXY_LISTENING (\d+)/))[1]);
    // exercise HTTP forwarding (/json/version through the proxy)
    const v = await getJson(`http://127.0.0.1:${cdpPort}/json/version`);
    fs.writeFileSync(path.join(RES, `${tag}.json-version-via-proxy.json`), v.body);
  }

  const smokeOut = path.join(RES, `${tag}.smoke.json`);
  const t = performance.now();
  const sp = spawn(process.execPath, [path.join(HERE, 'smoke.mjs'), `ws://127.0.0.1:${cdpPort}`, siteUrl, smokeOut], { stdio: ['ignore', 'ignore', 'pipe'] });
  let serr = ''; sp.stderr.on('data', (d) => (serr += d));
  await exited(sp);
  const smokeProcMs = performance.now() - t;
  await sleep(100);

  if (proxy) { proxy.kill('SIGTERM'); await exited(proxy); }
  sampler.kill('SIGTERM'); nsampler.kill('SIGTERM');
  const lpAlive = (() => { try { process.kill(lpPid, 0); return true; } catch { return false; } })();
  try { process.kill(lpPid, 'SIGTERM'); } catch {}
  await exited(tp);
  await sleep(50);

  const samples = fs.readFileSync(rssFile, 'utf8').split('\n').map((l) => l.trim()).filter(Boolean);
  const kb = samples.map((l) => Number(l.split(' ')[1])).filter((x) => x > 0);
  const tms = samples.map((l) => Number(l.split(' ')[0]));
  const nat = fs.readFileSync(nativeFile, 'utf8').split('\n').filter(Boolean).map((l) => l.split(' ').map(Number));
  const timeTxt = fs.readFileSync(timeOut, 'utf8');
  const maxrss = Number(timeTxt.match(/(\d+)\s+maximum resident set size/)?.[1]);
  const footprint = Number(timeTxt.match(/(\d+)\s+peak memory footprint/)?.[1]);
  const smoke = fs.existsSync(smokeOut) ? JSON.parse(fs.readFileSync(smokeOut, 'utf8')) : { error: serr };
  return {
    tag, viaProxy: useProxy, lpPid, lpAliveAfterSmoke: lpAlive,
    smokeProcessWallMs: +smokeProcMs.toFixed(1), smokeInnerWallMs: smoke.wallMs,
    passed: smoke.passed, total: smoke.total, steps: smoke.steps, smokeStderr: serr.slice(0, 2000),
    rss: {
      psSamples: kb.length, psSpanMs: tms.length ? tms[tms.length - 1] - tms[0] : 0,
      psEffectiveIntervalMs: kb.length > 1 ? +((tms[tms.length - 1] - tms[0]) / (kb.length - 1)).toFixed(1) : null,
      psPeakKiB: kb.length ? Math.max(...kb) : null, psIdleFirstKiB: kb[0] ?? null,
      nativeSamples: nat.length, nativeSpanMs: nat.length ? nat[nat.length - 1][0] - nat[0][0] : 0,
      nativeEffectiveIntervalMs: nat.length > 1 ? +((nat[nat.length - 1][0] - nat[0][0]) / (nat.length - 1)).toFixed(1) : null,
      nativePeakResidentKiB: nat.length ? Math.max(...nat.map((r) => r[1])) : null,
      nativePeakFootprintKiB: nat.length ? Math.max(...nat.map((r) => r[2])) : null,
      nativeIdleResidentKiB: nat[0]?.[1] ?? null,
      timeL_maxRssBytes: maxrss, timeL_peakFootprintBytes: footprint,
    },
    cdpJsonl: jsonl,
  };
}

// ---------- main ----------
const spinner = WARM_CPU ? spawn(process.execPath, ['-e', 'for(;;){}'], { stdio: 'ignore' }) : null;
const site = spawn(process.execPath, [path.join(HERE, 'testsite.mjs'), '0'], { stdio: ['ignore', 'pipe', 'inherit'] });
const sitePort = Number((await waitLine(site, /LISTENING (\d+)/))[1]);
const siteUrl = `http://127.0.0.1:${sitePort}`;

const result = { startedAt: new Date().toISOString(), bin: BIN, flags: LP_FLAGS, siteUrl };
result.startup = await startupRuns();
console.error('startup', result.startup.median, result.startup.p95);
result.smoke = [];
for (let i = 0; i < SMOKE_RUNS; i++) {
  const r = await smokeRun(`smoke-direct-${i + 1}`, siteUrl, false);
  console.error(r.tag, `${r.passed}/${r.total}`, r.smokeProcessWallMs, r.rss.psPeakKiB);
  result.smoke.push(r);
}
if (WITH_PROXY) {
  result.proxySmoke = await smokeRun('smoke-proxy-1', siteUrl, true);
  console.error('proxy', `${result.proxySmoke.passed}/${result.proxySmoke.total}`);
}
const direct = result.smoke;
result.smokeAggregate = {
  smokeProcessWallMs: stats(direct.map((r) => r.smokeProcessWallMs)),
  smokeInnerWallMs: stats(direct.map((r) => r.smokeInnerWallMs ?? NaN).filter((x) => !Number.isNaN(x))),
  psPeakRssKiB: stats(direct.map((r) => r.rss.psPeakKiB).filter(Boolean)),
  nativePeakResidentKiB: stats(direct.map((r) => r.rss.nativePeakResidentKiB).filter(Boolean)),
  nativePeakFootprintKiB: stats(direct.map((r) => r.rss.nativePeakFootprintKiB).filter(Boolean)),
  timeL_maxRssBytes: stats(direct.map((r) => r.rss.timeL_maxRssBytes).filter(Boolean)),
};
site.kill('SIGTERM');
spinner?.kill('SIGKILL');
result.warmCpu = WARM_CPU;
result.finishedAt = new Date().toISOString();
fs.writeFileSync(path.join(RES, 'baseline.json'), JSON.stringify(result, null, 2));
console.log(JSON.stringify({ startup: result.startup, smokeAggregate: result.smokeAggregate, firstSmokeSteps: direct[0]?.steps }, null, 2));
process.exit(0);
