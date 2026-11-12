// Page.printToPDF benchmark: ToS pdfgen visual cases through the production
// flow (apps/be pdf-renderer.ts): postMessage generate-pdf, wait for
// #document-generated[data-generation-token], document.fonts.ready, page.pdf().
//
//   bun run.mjs --cases <dir with pdf-visual-cases.json> --base <build dir> --fixed <build dir>
//               --lp <pyro binary> --out <dir> [--reps 3] [--only a,b] [--concurrency 6]
//
// Engines: chrome-base (Chrome, base build), chrome-fixed (Chrome, page box
// shadow removed), pyro-fixed (Pyro, fixed build). Runs are
// interleaved per case and repetition. Peak RSS is sampled (ps, 25 ms) over
// each browser's process tree while its case runs.

import { execFileSync, spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { chromium } from 'playwright-core';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith('--') ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const CHROME = args.chrome ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const reps = Number(args.reps ?? 3);
const out = path.resolve(args.out ?? 'out');
const VIEWPORT = { deviceScaleFactor: 1, hasTouch: false, height: 1080, isLandscape: false, isMobile: false, width: 794 };

const manifestDir = path.resolve(args.cases);
const manifest = JSON.parse(await readFile(path.join(manifestDir, 'pdf-visual-cases.json'), 'utf8'));
let cases = manifest.cases.filter((c) => c.enabled !== false);
if (args.only) cases = cases.filter((c) => args.only.split(',').includes(c.name));

const readPayload = async (c) => {
  const source = JSON.parse(await readFile(path.resolve(manifestDir, c.payloadFile ?? c.dataFile), 'utf8'));
  const wrapped = source && typeof source === 'object' && ('template' in source || 'data' in source);
  const base = c.dataFile || !wrapped ? { data: source } : source;
  const payload = {
    data: base.data ?? {},
    language: c.language ?? base.language,
    showPageBreaks: c.showPageBreaks ?? base.showPageBreaks,
    template: c.template ?? base.template,
  };
  const landscape = c.landscape === true || payload.template === 'BmdReport';
  return { payload: landscape ? { ...payload, landscape: true } : payload, landscape };
};

// ---- process-tree RSS ----
// One long-lived `ps` loop in a child shell (50 ms), parsed as it streams:
// sampling must not block this process's event loop, which times the runs.
const psLoop = spawn('sh', ['-c', 'while :; do ps -A -o pid=,ppid=,rss=; echo ---; sleep 0.05; done'], {
  stdio: ['ignore', 'pipe', 'ignore'],
});
let psBuf = '';
let psRows = [];
const watchers = new Set();
psLoop.stdout.on('data', (d) => {
  psBuf += d;
  let i;
  while ((i = psBuf.indexOf('---\n')) >= 0) {
    const block = psBuf.slice(0, i);
    psBuf = psBuf.slice(i + 4);
    psRows = block
      .trim()
      .split('\n')
      .map((l) => l.trim().split(/\s+/).map(Number));
    for (const w of watchers) w();
  }
});
const treeRss = (root) => {
  const kids = new Map();
  for (const [pid, ppid] of psRows) kids.set(ppid, [...(kids.get(ppid) ?? []), pid]);
  const rss = new Map(psRows.map(([pid, , r]) => [pid, r]));
  let total = 0;
  const stack = [root];
  while (stack.length) {
    const p = stack.pop();
    total += rss.get(p) ?? 0;
    stack.push(...(kids.get(p) ?? []));
  }
  return total * 1024;
};
const sampler = (pid) => {
  let peak = treeRss(pid);
  const w = () => (peak = Math.max(peak, treeRss(pid)));
  watchers.add(w);
  return { stop: () => (watchers.delete(w), w(), peak) };
};

// ---- engines ----
// Both engines are driven over CDP by Playwright (connectOverCDP): Chrome is
// started by this script, so its process tree is known. page.pdf() is
// Playwright's (Page.printToPDF, transferMode ReturnAsStream, IO.read).
const waitCdp = async (port) => {
  for (let i = 0; i < 400; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`no CDP endpoint on ${port}`);
};

// Fixed ports let a run silently measure another run's server, so ports are free ones by default
// and a port someone else is listening on is an error.
const freePort = () =>
  new Promise((resolve, reject) => {
    const server = net.createServer();
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
const assertPortFree = (port) =>
  new Promise((resolve, reject) => {
    const server = net.createServer();
    server.on('error', () => reject(new Error(`port ${port} is in use: another browser would answer this run`)));
    server.listen(port, '127.0.0.1', () => server.close(resolve));
  });

const startEngine = async (name, build, cmd, cmdArgs, port, env = {}) => {
  await assertPortFree(port);
  const proc = spawn(cmd, cmdArgs, { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, ...env } });
  let err = '';
  let partial = '';
  proc.stderr.on('data', (d) => {
    err = (err + d).slice(-4000);
    // PYRO_GEOMETRY_PROFILE=1: pass the engine's timing lines through.
    partial += String(d);
    const lines = partial.split('\n');
    partial = lines.pop();
    for (const line of lines) if (line.includes('-profile')) console.log(name, line);
  });
  let exit;
  proc.on('exit', (code, signal) => (exit = { code, signal }));
  await waitCdp(port);
  if (exit) throw new Error(`${name} exited (${exit.code ?? exit.signal}) yet port ${port} answers: ${err}`);
  const connect = () => chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const browser = await connect();
  return { name, build, browser, pid: proc.pid, proc, connect, stderr: () => err, exit: () => exit };
};

const launchChrome = (build, port) =>
  startEngine(`chrome-${build.name}`, build, CHROME, [
    '--headless=new',
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${path.join(out, `.chrome-profile-${build.name}`)}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-gpu',
    '--allow-file-access-from-files',
    'about:blank',
  ], port);

const launchLp = (build, port, geometry) =>
  startEngine(
    geometry ? `lp-${geometry}` : `lp-${build.name}`,
    { ...build, url: 'http://pdfgen.localhost/index.html' },
    path.resolve(args.lp),
    // Pyro has no file: scheme: the build is served in place of an origin.
    ['serve', '--port', String(port), '--static', `http://pdfgen.localhost=${build.dir}`],
    port,
    geometry ? { PYRO_GEOMETRY: geometry } : {},
  );

const newPage = async (browser) => {
  const ctx = browser.contexts()[0] ?? (await browser.newContext());
  return ctx.newPage();
};

const preparePage = async (page, build) => {
  await page.emulateMedia({ media: 'screen' });
  await page.setViewportSize({ height: 1123, width: 794 });
  await page.goto(build.url ?? pathToFileURL(path.join(build.dir, 'index.html')).href, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('html[data-pdfgen-ready="true"]', { state: 'attached', timeout: 60000 });
};

// Pyro.geometryStats: Blitz-backed layouts since the last call (pyro-blitz only).
const geometryStats = async (page) => {
  const session = await page.context().newCDPSession(page);
  const r = await session.send('Pyro.geometryStats').catch(() => null);
  await session.detach().catch(() => {});
  return r;
};

let seq = 0;
const renderOnce = async (page, payload, landscape, geometry = false) => {
  const token = `pdf-${++seq}`;
  if (geometry) await geometryStats(page);
  const t0 = performance.now();
  await page.evaluate(
    ({ data, token }) => globalThis.postMessage({ type: 'generate-pdf', data, generationToken: token }),
    { data: payload, token },
  );
  await page.waitForSelector(`#document-generated[data-generation-token="${token}"]`, {
    state: 'attached',
    timeout: 120000,
  });
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
  // --probe <expression>: print what it evaluates to in the paginated page.
  if (args.probe) console.log('probe', JSON.stringify(await page.evaluate(args.probe)));
  const t1 = performance.now();
  // Outside the timed window: the layouts pagination forced.
  const geom = geometry ? await geometryStats(page) : undefined;
  const t1b = performance.now();
  const pdf = await page.pdf({
    displayHeaderFooter: false,
    format: 'A4',
    landscape,
    margin: { bottom: '0px', left: '0px', right: '0px', top: '0px' },
    preferCSSPageSize: false,
    printBackground: true,
    scale: 1,
    tagged: false,
  });
  const t2 = performance.now();
  return { pdf: Buffer.from(pdf), paginateMs: t1 - t0, pdfMs: t2 - t1b, totalMs: t1 - t0 + (t2 - t1b), geom };
};

const pageCount = (file) => {
  try {
    return Number(/Pages:\s+(\d+)/.exec(execFileSync('pdfinfo', [file], { encoding: 'utf8' }))?.[1] ?? 0);
  } catch {
    return -1;
  }
};

const builds = [
  { name: 'base', dir: path.resolve(args.base) },
  { name: 'fixed', dir: path.resolve(args.fixed) },
];
await mkdir(out, { recursive: true });
const engines = [];
if (!args.skipChrome) {
  engines.push(await launchChrome(builds[0], await freePort()));
  engines.push(await launchChrome(builds[1], await freePort()));
}
if (args.lp && !args.skipFlow) engines.push(await launchLp(builds[1], args.port ? Number(args.port) : await freePort()));
// --blitz: the same binary with Blitz-backed layout queries.
if (args.lp && args.blitz) engines.push(await launchLp(builds[1], await freePort(), 'blitz'));

for (const e of engines) {
  e.page = await newPage(e.browser);
  await preparePage(e.page, e.build);
}

// Pyro's own report (page boxes, overflow, skipped features, stage
// timings) rides on the printToPDF reply, which page.pdf() drops: fetched with
// one extra, untimed raw call per case.
const lpReport = async (page, landscape) => {
  const session = await page.context().newCDPSession(page);
  const r = await session.send('Page.printToPDF', {
    landscape,
    paperWidth: 8.27,
    paperHeight: 11.7,
    printBackground: true,
  });
  await session.detach().catch(() => {});
  return r.lp;
};

// Warm-up: one untimed render per engine (process-wide font scans, JIT).
{
  const { payload, landscape } = await readPayload(cases[0]);
  for (const e of engines) {
    await renderOnce(e.page, payload, landscape).catch((x) => console.log('warm-up', e.name, String(x).slice(0, 200)));
    await preparePage(e.page, e.build);
  }
}

const load = () => execFileSync('uptime', { encoding: 'utf8' }).trim();
const meta = { started: new Date().toISOString(), loadBefore: load() };
// Rows are written after every case; a rerun with the same --out resumes.
const runsFile = path.join(out, 'runs.json');
const rows = await readFile(runsFile, 'utf8').then(JSON.parse).catch(() => []);
for (const c of cases) {
  if (rows.some((r) => r.case === c.name)) continue;
  const { payload, landscape } = await readPayload(c);
  for (let rep = 0; rep < reps; rep++) {
    for (const e of engines) {
      const s = sampler(e.pid);
      let r;
      try {
        r = await renderOnce(e.page, payload, landscape, e.name === 'pyro-blitz');
      } catch (err) {
        s.stop();
        rows.push({ case: c.name, engine: e.name, rep, error: String(err).slice(0, 300) });
        console.log(`${c.name} ${e.name} rep${rep} ERROR ${String(err).slice(0, 200)}`);
        if (String(err).includes('has been closed')) {
          console.log(`${e.name} server exit ${JSON.stringify(e.exit() ?? 'none yet')}, stderr tail:\n${e.stderr().slice(-1000)}`);
        }
        try {
          await e.page.close();
        } catch {}
        e.page = await newPage(e.browser);
        await preparePage(e.page, e.build).catch(() => {});
        continue;
      }
      const peakRss = s.stop();
      const file = path.join(out, e.name, `${c.name}.pdf`);
      if (rep === 0) {
        await mkdir(path.dirname(file), { recursive: true });
        await writeFile(file, r.pdf);
      }
      const row = {
        case: c.name,
        engine: e.name,
        rep,
        paginateMs: r.paginateMs,
        pdfMs: r.pdfMs,
        totalMs: r.totalMs,
        bytes: r.pdf.length,
        pages: rep === 0 ? pageCount(file) : undefined,
        peakRss,
        geometry: r.geom,
        lp: rep === 0 && e.name.startsWith('lp') ? await lpReport(e.page, landscape).catch((x) => String(x)) : undefined,
      };
      rows.push(row);
      console.log(
        `${c.name} ${e.name} rep${rep} paginate ${r.paginateMs.toFixed(0)} pdf ${r.pdfMs.toFixed(0)} bytes ${r.pdf.length} pages ${row.pages ?? ''} rss ${(peakRss / 1048576).toFixed(0)}MB`,
      );
      // Production re-prepares the page after each render (releasePage).
      await preparePage(e.page, e.build);
    }
  }
  await writeFile(runsFile, JSON.stringify(rows, null, 1));
}
meta.loadAfterSequential = load();
await writeFile(runsFile, JSON.stringify(rows, null, 1));

// ---- throughput: N concurrent requests over all cases ----
const concurrency = Number(args.concurrency ?? 6);
const throughput = {};
if (concurrency > 0) {
  const payloads = await Promise.all(cases.map(readPayload));
  for (const e of engines) {
    if (e.name === 'chrome-base' && args.skipBaseThroughput) continue;
    // Chrome: one browser, N pages. Pyro: one server, N CDP connections (a render thread each).
    const pages = [];
    for (let i = 0; i < concurrency; i++) {
      const b = e.name.startsWith('lp') ? await e.connect() : e.browser;
      const p = await newPage(b);
      await preparePage(p, e.build);
      pages.push({ b, p });
    }
    const s = sampler(e.pid);
    const queue = payloads.map((p, i) => ({ ...p, i }));
    const lat = [];
    let failed = 0;
    const t0 = performance.now();
    await Promise.all(
      pages.map(async ({ p }) => {
        for (let job = queue.shift(); job; job = queue.shift()) {
          try {
            const r = await renderOnce(p, job.payload, job.landscape);
            lat.push(r.totalMs);
          } catch {
            failed++;
          }
          await preparePage(p, e.build).catch(() => {});
        }
      }),
    );
    const wall = performance.now() - t0;
    const peak = s.stop();
    lat.sort((a, b) => a - b);
    throughput[e.name] = {
      concurrency,
      docs: cases.length,
      failed,
      wallMs: wall,
      docsPerSec: (cases.length - failed) / (wall / 1000),
      p50Ms: lat[Math.floor(lat.length * 0.5)],
      p95Ms: lat[Math.floor(lat.length * 0.95)],
      peakRss: peak,
    };
    console.log(e.name, JSON.stringify(throughput[e.name]));
    for (const { b, p } of pages) {
      await p.close().catch(() => {});
      if (b !== e.browser) await b.close().catch(() => {});
    }
  }
}
meta.loadAfterThroughput = load();
meta.finished = new Date().toISOString();
await writeFile(path.join(out, 'throughput.json'), JSON.stringify(throughput, null, 1));
await writeFile(path.join(out, 'meta.json'), JSON.stringify(meta, null, 1));

for (const e of engines) {
  await e.browser.close().catch(() => {});
  e.proc.kill();
}
psLoop.kill();
