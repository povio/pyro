// bun test bench/wpt/wptserve.test.mjs
// The runner never passes a page it could not load, and a run whose server is not its own or died fails.
// The parts that start wptserve need WPT_DIR (a checkout with fetch-wpt.sh --venv); the parts that run the browser
// also need PYRO_BIN. Without them those tests are skipped.
import { expect, test } from 'bun:test';
import { execFile, spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { documentError } from './lib.mjs';
import { refKey } from './reftest.mjs';
import { BROWSER_HOST, listeningPorts, startProxy, startWptserve } from './wptserve.mjs';

const WPT = process.env.WPT_DIR;
const BIN = process.env.PYRO_BIN;
const HAS_WPT = !!WPT && fs.existsSync(path.join(WPT, 'wpt'));
const res = (status) => ({ status: () => status });

test.skipIf(!HAS_WPT || !/Features:.*\bHTTP2\b/i.test(spawnSync('curl', ['--version'], { encoding: 'utf8' }).stdout ?? ''))(
  'the optional WPT HTTP/2 listener negotiates h2 through the browser proxy', async () => {
    const server = await startWptserve(WPT, { http2: true });
    try {
      const proxy = await startProxy();
      try {
        const { stdout } = await promisify(execFile)('curl', ['--disable', '--silent', '--fail', '--http2', '--max-time', '5',
          '--cacert', path.join(WPT, 'tools/certs/cacert.pem'), '--noproxy', '', '--proxy', `http://127.0.0.1:${proxy.port}`,
          '--output', '/dev/null', '--write-out', '%{http_version}', `https://${BROWSER_HOST}:${server.ports.h2}/resources/testharness.js`]);
        expect(stdout).toBe('2');
      } finally { proxy.close(); }
    } finally { server.stop(); }
  }, 120000,
);

test('a main document that is not a 2xx response is an error, never a page to judge', () => {
  expect(documentError(res(200), 'a.html')).toBeNull();
  expect(documentError(res(204), 'a.html')).toBeNull();
  // The proxy's answer for a server that stopped answering, a missing file, a redirect left unfollowed.
  expect(documentError(res(502), 'a.html')).toBe('HTTP 502 for a.html');
  expect(documentError(res(404), 'a.html')).toBe('HTTP 404 for a.html');
  expect(documentError(res(301), 'a.html')).toBe('HTTP 301 for a.html');
  expect(documentError(null, 'a.html')).toBe('no response for a.html');
  expect(documentError(null, 'about:blank')).toBeNull();
});

test('a reference render is cached per origin it loads from', () => {
  const v = { width: 800, height: 600 };
  expect(refKey(v, 'http://web-platform.test:8000', 'r.html')).not.toBe(refKey(v, 'https://web-platform.test:8443', 'r.html'));
  expect(refKey(v, 'http://web-platform.test:8000', 'r.html')).not.toBe(refKey(v, 'http://www.web-platform.test:8000', 'r.html'));
  expect(refKey(v, 'http://a:1', 'r.html')).toBe(refKey(v, 'http://a:1', 'r.html'));
  expect(refKey({ width: 600, height: 600 }, 'http://a:1', 'r.html')).not.toBe(refKey(v, 'http://a:1', 'r.html'));
  expect(refKey(v, 'http://a:1', 'about:blank')).toBe(refKey(v, 'https://b:2', 'about:blank'));
});

test('lsof -F pn output gives the listening ports', () => {
  expect([...listeningPorts('p123\nn127.0.0.1:8000\nn*:8443\np124\nn[::1]:9000\n')].sort()).toEqual([8000, 8443, 9000]);
  expect(listeningPorts('').size).toBe(0);
});

const freePort = () => new Promise((r) => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });
const quiet = { write() {} };

test.skipIf(!HAS_WPT)('wptserve: a port another listener holds is not taken for this run\'s; stop and death are seen', async () => {
  // Another gate's server on the port this run would use: answers everything with 200.
  const foreign = http.createServer((q, s) => { s.writeHead(200); s.end('not yours'); });
  const taken = await new Promise((r) => foreign.listen(0, '127.0.0.1', () => r(foreign.address().port)));
  try {
    const ws = await startWptserve(WPT, { log: quiet, ports: { http: taken } });
    try {
      expect(ws.ports.http).not.toBe(taken);
      expect(await ws.alive()).toBe(true);
      let exited = null;
      ws.onExit((code) => { exited = code; });
      process.kill(-ws.pid, 'SIGKILL');
      for (let i = 0; i < 50 && exited === null; i++) await new Promise((r) => setTimeout(r, 100));
      expect(exited).not.toBeNull();
      expect(await ws.alive()).toBe(false);
    } finally { ws.stop(); }
  } finally { foreign.close(); }
}, 120000);

// A page discovery keeps, served by a server that answers every request with an empty 502 (what the proxy answers
// for a wptserve that stopped answering): the crashtest and the reftest (whose test and reference would render the
// same empty document) are errors.
test.skipIf(!HAS_WPT || !BIN)('pages served as HTTP errors are errors, not passes', async () => {
  const bad = http.createServer((q, s) => { s.writeHead(502, { 'content-length': 0 }); s.end(); });
  const port = await new Promise((r) => bad.listen(0, '127.0.0.1', () => r(bad.address().port)));
  try {
    for (const [kind, match] of [['crashtest', '^css/css-multicol/crashtests/(balance-|change-)'], ['reftest', '^css/css-backgrounds/background-color-body-propagation-00']]) {
      const out = path.join(os.tmpdir(), `wptserve-test-${process.pid}-${kind}.json`);
      // Not spawnSync: the 502 server answers from this process's event loop.
      const run = spawn(process.execPath, [path.join(import.meta.dir, 'run.mjs'), '--kind', kind, '--wpt', WPT, '--bin', `t=${BIN}`, '--base', `http://127.0.0.1:${port}`,
        '--match', match, '--jobs', '2', '--out', out], { stdio: 'ignore' });
      expect(await new Promise((r) => run.once('exit', r))).toBe(0);
      const results = JSON.parse(fs.readFileSync(out, 'utf8')).results.slice(0, 6);
      expect(results.length).toBeGreaterThan(0);
      for (const t of results) {
        expect(t.status).toBe('error');
        expect(t.error).toMatch(/HTTP 502/);
        expect((t.subtests ?? []).some((s) => s.status === 'Pass')).toBe(false);
      }
      fs.rmSync(out, { force: true });
    }
  } finally { bad.close(); }
}, 300000);

// The run's wptserve dies while pages run: the runner exits 3 and writes no results.
test.skipIf(!HAS_WPT || !BIN)('a run whose wptserve dies fails as a whole', async () => {
  const out = path.join(os.tmpdir(), `wptserve-test-${process.pid}-dies.json`);
  const run = spawn(process.execPath, [path.join(import.meta.dir, 'run.mjs'), '--kind', 'crashtest', '--wpt', WPT, '--bin', `t=${BIN}`, '--server', 'wptserve',
    '--match', '^css/', '--jobs', '1', '--out', out], { stdio: ['ignore', 'ignore', 'pipe'] });
  let err = '';
  const logDir = await new Promise((resolve) => run.stderr.on('data', (d) => { err += d; const m = err.match(/\(log (\S+)\/serve\.log\)/); if (m) resolve(m[1]); }));
  const pid = spawnSync('pgrep', ['-f', `${logDir}/config.json`], { encoding: 'utf8' }).stdout.trim().split('\n')[0];
  expect(pid).toMatch(/^\d+$/);
  await new Promise((r) => setTimeout(r, 3000));
  process.kill(-Number(pid), 'SIGKILL');
  const code = await new Promise((r) => run.once('exit', r));
  expect(code).toBe(3);
  expect(err).toMatch(/the WPT server \(wptserve\) exited/);
  expect(fs.existsSync(out)).toBe(false);
  fs.rmSync(out + '.partial', { force: true });
}, 120000);

// Exercise upstream testharness.js in an actual window, including its duplicate-name
// validation. Serve two local fixtures at an external base to keep the WPT checkout intact.
test.skipIf(!HAS_WPT || !BIN)('inventory mode retains both duplicate window tests and an empty harness error', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'wpt-window-outcomes-'));
  const fixture = path.join(dir, 'cases');
  fs.mkdirSync(fixture);
  fs.symlinkSync(path.join(WPT, 'fonts'), path.join(dir, 'fonts'));
  const scripts = '<script src="/resources/testharness.js"></script><script src="/resources/testharnessreport.js"></script>';
  const duplicateName = 'duplicate|name\nsecond line';
  const name = JSON.stringify(duplicateName);
  fs.writeFileSync(path.join(fixture, 'duplicates.html'), scripts + '<script>test(() => {}, ' + name
    + '); test(() => assert_unreached("second occurrence"), ' + name + ');</script>');
  fs.writeFileSync(path.join(fixture, 'empty-error.html'), scripts + '<script>setup(() => { throw Error("early setup error") });</script>');
  const manifest = path.join(dir, 'MANIFEST.json');
  fs.writeFileSync(manifest, JSON.stringify({ url_base: '/', items: { testharness: {} } }));
  const files = new Map([
    ['/cases/duplicates.html', path.join(fixture, 'duplicates.html')],
    ['/cases/empty-error.html', path.join(fixture, 'empty-error.html')],
    ['/resources/testharness.js', path.join(WPT, 'resources/testharness.js')],
    ['/resources/testharnessreport.js', path.join(import.meta.dir, 'testharnessreport.js')],
  ]);
  const server = http.createServer((req, res) => {
    const file = files.get(req.url);
    if (!file) { res.writeHead(404); res.end(); return; }
    res.setHeader('Content-Type', file.endsWith('.html') ? 'text/html; charset=utf-8' : 'text/javascript');
    res.end(fs.readFileSync(file));
  });
  try {
    const port = await new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve(server.address().port)));
    const out = path.join(dir, 'results.json');
    const run = spawn(process.execPath, [path.join(import.meta.dir, 'run.mjs'), '--wpt', dir,
      '--dirs', 'cases', '--base', 'http://127.0.0.1:' + port, '--server', 'wptserve',
      '--dedicated-workers', manifest, '--jobs', '1', '--bin', 'browser=' + BIN, '--out', out], { stdio: 'ignore' });
    expect(await new Promise((resolve) => run.once('exit', resolve))).toBe(0);
    const results = new Map(JSON.parse(fs.readFileSync(out)).results.map((r) => [r.url, r]));
    expect(results.size).toBe(2);
    const duplicates = results.get('cases/duplicates.html');
    const empty = results.get('cases/empty-error.html');
    expect(duplicates.harnessStatus).toBe('Error');
    expect(duplicates.status).toBe('error');
    expect(duplicates.error).toContain('duplicate test name');
    expect(duplicates.subtests.map((t) => [t.name, t.status])).toEqual([[duplicateName, 'Pass'], [duplicateName, 'Fail']]);
    expect(empty.harnessStatus).toBe('Error');
    expect(empty.status).toBe('error');
    expect(empty.error).toContain('early setup error');
    expect(empty.subtests).toEqual([]);
  } finally {
    server.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}, 60000);
