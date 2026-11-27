// bun test bench/wpt/wptserve.test.mjs
// The runner never passes a page it could not load, and a run whose server is not its own or died fails.
// The parts that start wptserve need WPT_DIR (a checkout with fetch-wpt.sh --venv); the parts that run the browser
// also need PYRO_BIN. Without them those tests are skipped.
import { expect, test } from 'bun:test';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { documentError } from './lib.mjs';
import { refKey } from './reftest.mjs';
import { listeningPorts, startProxy, startWptserve } from './wptserve.mjs';

const WPT = process.env.WPT_DIR;
const BIN = process.env.PYRO_BIN;
const HAS_WPT = !!WPT && fs.existsSync(path.join(WPT, 'wpt'));
const res = (status) => ({ status: () => status });

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

// The whole-suite testharness run died on these (runs 44821108769, 44827586210): an exception in the proxy ends run.mjs.
test('the proxy forwards a response whose status text or header Node cannot write, and survives an upstream that dies mid-body', async () => {
  const raw = (head, body, die) => net.createServer((sock) => {
    sock.once('data', () => {
      sock.write(head + body);
      if (die) setTimeout(() => sock.destroy(), 20); else sock.end();
    });
    sock.on('error', () => {});
  });
  const listen = (server) => new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve(server.address().port)));
  const odd = raw('HTTP/1.1 200 \u00e9t\u00e9 \x01\r\nX-Ok: 1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n', 'ok');
  const dying = raw('HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n', 'partial', true);
  const [oddPort, dyingPort] = [await listen(odd), await listen(dying)];
  const proxy = await startProxy();
  const viaProxy = (port) => new Promise((resolve) => {
    const req = http.request({ host: '127.0.0.1', port: proxy.port, path: `http://web-platform.test:${port}/x`, agent: false }, (res) => {
      let body = '';
      res.on('data', (c) => { body += c; });
      const done = () => resolve({ status: res.statusCode, ok: res.headers['x-ok'], body });
      res.on('end', done); res.on('close', done); res.on('error', done);
    });
    req.on('error', () => resolve({ status: 0 }));
    req.end();
  });
  try {
    expect(await viaProxy(oddPort)).toEqual({ status: 200, ok: '1', body: 'ok' });
    expect((await viaProxy(dyingPort)).status).toBe(200);
    // Still serving after both.
    expect(await viaProxy(oddPort)).toEqual({ status: 200, ok: '1', body: 'ok' });
  } finally { proxy.close(); odd.close(); dying.close(); }
});

// wptserve's `.asis` heads, methods Node's parser does not know and what a browser spells in a request reach wptserve as sent
// (fetch/api/basic/header-value-combining, redirect-method.any.js, request-headers-case, header-values).
test('the proxy forwards .asis responses, unknown methods, header name case and control characters as bytes', async () => {
  const seen = [];
  const upstream = net.createServer((sock) => {
    let buf = '';
    sock.on('data', (c) => {
      buf += c.toString('latin1');
      if (!buf.includes('\r\n\r\n')) return;
      seen.push(buf);
      // An .asis head: bare LF, no blank line, then the close.
      if (buf.includes('.asis')) sock.end('HTTP/1.1 280 HELLO\nfoo-test: 1\nfoo-test: 2\n');
      else sock.end('HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok');
    });
    sock.on('error', () => {});
  });
  const port = await new Promise((resolve) => upstream.listen(0, '127.0.0.1', () => resolve(upstream.address().port)));
  const proxy = await startProxy();
  const exchange = (request) => new Promise((resolve) => {
    const sock = net.connect(proxy.port, '127.0.0.1', () => sock.write(request));
    let out = '';
    sock.on('data', (c) => { out += c.toString('latin1'); });
    sock.on('close', () => resolve(out));
    sock.on('error', () => resolve(out));
  });
  const via = (method, target, headers) => `${method} http://web-platform.test:${port}${target} HTTP/1.1\r\nHost: web-platform.test:${port}\r\n${headers}\r\n`;
  try {
    // The unterminated head gets its blank line, the bare LFs stay.
    expect(await exchange(via('GET', '/x.asis', ''))).toBe('HTTP/1.1 280 HELLO\nfoo-test: 1\nfoo-test: 2\n\n');
    expect(await exchange(via('TESTING', '/x?y=1', 'X-Case: 1\r\n'))).toContain('200 OK');
    expect(seen.at(-1)).toMatch(/^TESTING \/x\?y=1 HTTP\/1\.1\r\n/);
    expect(seen.at(-1)).toContain('X-Case: 1\r\n');
    expect(await exchange(via('GET', '/y', 'THIS-is-A-test: a\x01b\r\nX-Empty:\r\n'))).toContain('200 OK');
    expect(seen.at(-1)).toContain('THIS-is-A-test: a\x01b\r\n');
    expect(seen.at(-1)).toContain('X-Empty: \r\n');
  } finally { proxy.close(); upstream.close(); }
});
