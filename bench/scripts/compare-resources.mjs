// macOS, same-host release comparison of an intentionally limited HTTP workload.
// Never substitute these results for TOS compatibility or full browser parity.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', 'package.json'));
const { chromium } = require('playwright-core');
const args = process.argv.slice(2);
const arg = (key, fallback) => { const i = args.indexOf(key); return i < 0 ? fallback : args[i + 1]; };
const config = {
  samples: Number(arg('--samples', '3')), navigations: Number(arg('--navigations', '50')),
  contexts: Number(arg('--contexts', '20')), idleMs: Number(arg('--idle-ms', '10000')),
  retainedMs: Number(arg('--retained-ms', '2000')), samplerIntervalMs: 20,
};
const binaries = [
  { name: 'upstream', path: arg('--upstream'), sourceSha: arg('--upstream-sha') },
  { name: 'rewrite', path: arg('--rewrite'), sourceSha: arg('--rewrite-sha') },
];
const sampler = arg('--sampler');
const output = arg('--out');
if (process.platform !== 'darwin' || !sampler || !output || binaries.some(b => !b.path || !/^[a-f0-9]{40}$/.test(b.sourceSha ?? '')) ||
    Object.values(config).some(n => !Number.isSafeInteger(n) || n <= 0)) {
  throw new Error('macOS usage: --upstream BIN --upstream-sha SHA --rewrite BIN --rewrite-sha SHA --sampler BIN --out NEW_DIR [--samples 3 --navigations 50 --contexts 20 --idle-ms 10000 --retained-ms 2000]');
}
fs.mkdirSync(output); // Refuse to overwrite retained evidence.
const sha256 = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
for (const bin of binaries) { bin.path = fs.realpathSync(bin.path); bin.sha256 = sha256(bin.path); bin.bytes = fs.statSync(bin.path).size; }
const flags = ['--log-level', 'error', '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'];
const children = new Set();
function start(bin, args, options = {}) {
  const cp = spawn(bin, args, { stdio: ['ignore', 'ignore', 'pipe'], detached: true, ...options });
  children.add(cp);
  cp.once('exit', () => children.delete(cp));
  cp.once('error', error => { cp.spawnError = error; });
  return cp;
}
function kill(cp, signal = 'SIGKILL') { if (cp?.pid) { try { process.kill(-cp.pid, signal); } catch {} } }
const exited = cp => cp.exitCode !== null || cp.signalCode !== null || cp.spawnError
  ? Promise.resolve() : new Promise(resolve => cp.once('exit', resolve));
async function stop(cp) {
  if (!cp) return;
  kill(cp, 'SIGTERM');
  const timeout = setTimeout(() => kill(cp), 2000);
  try { await exited(cp); } finally { clearTimeout(timeout); }
}
process.on('exit', () => { for (const cp of children) kill(cp); });
process.on('SIGINT', () => process.exit(130));
process.on('SIGTERM', () => process.exit(143));
const deadline = setTimeout(() => { console.error('overall resource benchmark deadline'); process.exit(1); }, 600000);
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer(); s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const port = s.address().port; s.close(() => resolve(port)); });
});
function snapshot(pid) {
  const fields = execFileSync(sampler, [String(pid), '-', '--once'], { encoding: 'utf8', timeout: 5000 }).trim().split(/\s+/).map(Number);
  assert.equal(fields.length, 6); assert.ok(fields.every(Number.isFinite));
  return { epochMs: fields[0], residentKiB: fields[1], footprintKiB: fields[2], userNs: fields[3], systemNs: fields[4], monotonicNs: fields[5] };
}
async function phase(pid, work) {
  const before = snapshot(pid), cpu = process.cpuUsage(), begin = performance.now();
  await work();
  const workWallMs = performance.now() - begin, harness = process.cpuUsage(cpu), after = snapshot(pid);
  const userMs = (after.userNs - before.userNs) / 1e6, systemMs = (after.systemNs - before.systemNs) / 1e6;
  const sampledWallMs = (after.monotonicNs - before.monotonicNs) / 1e6;
  assert.ok(userMs >= 0 && systemMs >= 0 && sampledWallMs > 0);
  return { workWallMs, sampledWallMs, browserUserMs: userMs, browserSystemMs: systemMs,
    browserCpuMs: userMs + systemMs, browserOneCorePercent: 100 * (userMs + systemMs) / sampledWallMs,
    harnessNodeUserMs: harness.user / 1000, harnessNodeSystemMs: harness.system / 1000, before, after };
}
const body = '<!doctype html><script>globalThis.parityValue=Array.from({length:100},(_,i)=>i*i).reduce((a,b)=>a+b,0)</script>';
const server = http.createServer((req, res) => { res.writeHead(200, { 'Content-Type': 'text/html', 'Cache-Control': 'no-store' }); res.end(body); });
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const site = `http://127.0.0.1:${server.address().port}`;
async function navigate(page, index) {
  const url = `${site}/document?n=${index}`;
  const response = await page.goto(url, { waitUntil: 'load', timeout: 10000 });
  assert.equal(response.status(), 200); assert.equal(response.url(), url);
  assert.equal(await response.text(), body);
  assert.equal(await page.evaluate(() => globalThis.parityValue), 328350);
}
const results = {
  startedAt: new Date().toISOString(), config, binaries, sampler: { path: sampler, sha256: sha256(sampler) },
  harnessSha256: sha256(fileURLToPath(import.meta.url)),
  host: { platform: os.platform(), release: os.release(), arch: os.arch(), cpus: os.cpus().length,
    cpuModel: os.cpus()[0].model, totalMemoryBytes: os.totalmem(), node: process.version },
  playwright: require('playwright-core/package.json').version, flags,
  conditions: 'Fresh process per sample; OS/file caches uncontrolled and likely warm. One warmup HTTP navigation before measurement. Alternating AB/BA order; no busy-spin warmup. Host is shared; DVFS/background load uncontrolled. Release binaries supplied by caller; source SHA declarations must match build provenance.',
  scope: 'Local no-store HTTP + inline JS checksum + response body; 50 repeated navigations and 20 create/navigate/dispose contexts by default. No DOM interaction, subresource/HTTPS/auth/Fetch workload or TOS harness. Results apply only to successful equivalent assertions.',
  accounting: 'Browser PID only; CPU converted from Mach ticks using mach_timebase_info, all threads included, children excluded. Native 20ms memory trace; peak is sampled, not lifetime maximum. Phase CPU windows bracket snapshot helper launches. Harness Node CPU (Playwright and local server) separate, excludes sampler/helper children. No forced GC. Retained memory measured after context disposal and a fixed quiet interval.',
  fixtureSha256: createHash('sha256').update(body).digest('hex'), runs: [],
};
const persist = () => fs.writeFileSync(path.join(output, 'resources.json'), JSON.stringify(results, null, 2) + '\n');
async function run(bin, sample, order) {
  const port = await freePort(), tag = `${sample}-${order}-${bin.name}`;
  const run = { tag, name: bin.name, sample, order, sourceSha: bin.sourceSha, phases: {}, ok: false };
  let browser, samplerChild, stderr = '';
  const cp = start(bin.path, ['serve', '--host', '127.0.0.1', '--port', String(port), ...flags],
    { env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' } });
  cp.stderr.on('data', data => { stderr = (stderr + data).slice(-8192); });
  try {
    const startup = performance.now();
    for (;;) {
      if (cp.spawnError) throw cp.spawnError;
      if (cp.exitCode !== null || cp.signalCode !== null) throw new Error(`browser exited: ${cp.exitCode}/${cp.signalCode}`);
      try {
        const response = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) });
        if (response.ok) { run.version = await response.json(); break; }
      } catch {}
      if (performance.now() - startup > 15000) throw new Error('startup timeout');
      await sleep(20);
    }
    run.readinessMs = performance.now() - startup;
    samplerChild = start(sampler, [String(cp.pid), path.join(output, `${tag}.samples.txt`), String(config.samplerIntervalMs)]);
    browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`, { timeout: 15000 });
    const context = await browser.newContext({ locale: undefined });
    const page = await context.newPage();
    await navigate(page, 'warmup');
    run.phases.idle = await phase(cp.pid, () => sleep(config.idleMs));
    run.phases.navigations = await phase(cp.pid, async () => {
      for (let i = 0; i < config.navigations; i++) await navigate(page, i);
    });
    await context.close();
    run.phases.contexts = await phase(cp.pid, async () => {
      for (let i = 0; i < config.contexts; i++) {
        const c = await browser.newContext({ locale: undefined });
        try { await navigate(await c.newPage(), `context-${i}`); } finally { await c.close(); }
      }
    });
    run.phases.retained = await phase(cp.pid, () => sleep(config.retainedMs));
    run.browserCumulativeBeforeClose = snapshot(cp.pid);
    run.samplerCumulativeBeforeStop = snapshot(samplerChild.pid);
    await stop(samplerChild); samplerChild = null;
    const trace = fs.readFileSync(path.join(output, `${tag}.samples.txt`), 'utf8').trim().split('\n').map(line => line.split(/\s+/).map(Number));
    assert.ok(trace.length > 1 && trace.every(row => row.length === 6 && row.every(Number.isFinite)), 'native trace missing or malformed');
    run.memory = { sampleCount: trace.length, peakResidentKiB: Math.max(...trace.map(r => r[1])), peakFootprintKiB: Math.max(...trace.map(r => r[2])),
      retainedResidentKiB: run.phases.retained.after.residentKiB, retainedFootprintKiB: run.phases.retained.after.footprintKiB };
    run.ok = true;
  } catch (error) { run.error = String(error.stack ?? error); }
  finally {
    // Do not allow a disconnected browser or helper to survive a failed assertion.
    if (browser) await Promise.race([browser.close().catch(() => {}), sleep(2000)]);
    await stop(samplerChild); await stop(cp);
    run.exitCode = cp.exitCode; run.signal = cp.signalCode;
    fs.writeFileSync(path.join(output, `${tag}.stderr.log`), stderr);
  }
  results.runs.push(run); persist();
  console.error(`${tag}: ${run.ok ? 'PASS' : run.error}`);
}
try {
  for (let sample = 1; sample <= config.samples; sample++) {
    const order = sample % 2 ? binaries : [...binaries].reverse();
    for (let index = 0; index < order.length; index++) await run(order[index], sample, index);
  }
} finally {
  server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); clearTimeout(deadline);
  results.finishedAt = new Date().toISOString(); persist();
}
if (results.runs.some(run => !run.ok)) process.exitCode = 1;
console.log(path.join(output, 'resources.json'));
