// WPT's own server (`wpt serve`, Python) for run.mjs --server wptserve, and the forward proxy that lets the browser
// reach its hosts without /etc/hosts: README "wptserve".
//
// wptserve serves every origin a test names (web-platform.test, www/www1/www2 and the IDN subdomains, the
// not-web-platform.test alternate host; http, https with WPT's test certificates, ws, wss) and runs the Python
// handlers, substitutions and pipes. It binds 127.0.0.1 (`server_host`), so it needs no hosts file, and the browser
// reaches the names through proxy(): every context gets it as its proxyServer, and it connects whatever host a request
// names to 127.0.0.1 on the same port (wptrunner gives Chrome the same mapping as --host-resolver-rules).
// testharnessreport.js is ours (window.report), as wptrunner serves its own, by a file alias.
//
// Needs Python 3.9+ with tools/serve/requirements.txt (WPT_PYTHON=/path/to/python; default: the venv
// fetch-wpt.sh --venv made in the checkout, else python3).
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const BROWSER_HOST = 'web-platform.test';

const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.once('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

export function pythonFor(wpt) {
  if (process.env.WPT_PYTHON) return process.env.WPT_PYTHON;
  const venv = path.join(wpt, '.git', 'pyro-venv', 'bin', 'python');
  return fs.existsSync(venv) ? venv : 'python3';
}

// Starts wptserve on free ports. Resolves { ports, base, stop } once its main http server answers.
export async function startWptserve(wpt, { log = process.stderr } = {}) {
  const ports = {};
  for (const k of ['http', 'http2', 'https', 'https2', 'httpLocal', 'httpPublic', 'httpsLocal', 'httpsPublic', 'ws', 'wss']) ports[k] = await freePort();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pyro-wptserve-'));
  const config = {
    browser_host: BROWSER_HOST,
    server_host: '127.0.0.1',
    check_subdomains: false,
    ports: {
      http: [ports.http, ports.http2], https: [ports.https, ports.https2],
      'http-local': [ports.httpLocal], 'http-public': [ports.httpPublic],
      'https-local': [ports.httpsLocal], 'https-public': [ports.httpsPublic],
      ws: [ports.ws], wss: [ports.wss], h2: [], 'webtransport-h3': [], dns: [],
    },
    aliases: [
      { 'url-path': '/resources/testharnessreport.js', 'local-dir': HERE },
      // testdriver's vendor half (testdriver-vendor.js, testdriver.mjs), as wptrunner serves Chrome's.
      { 'url-path': '/resources/testdriver-vendor.js', 'local-dir': HERE },
    ],
  };
  const cfg = path.join(dir, 'config.json');
  fs.writeFileSync(cfg, JSON.stringify(config));
  const py = pythonFor(wpt);
  // `wpt serve` would make its own venv; the server needs only its requirements, so run serve.py's entry directly.
  const child = spawn(py, [path.join(wpt, 'wpt'), '--venv', path.dirname(path.dirname(py)), '--skip-venv-setup',
    'serve', '--no-h2', '--config', cfg], { cwd: wpt, stdio: ['ignore', 'pipe', 'pipe'], detached: true });
  const out = fs.createWriteStream(path.join(dir, 'serve.log'));
  child.stdout.pipe(out); child.stderr.pipe(out);
  let exited = null;
  child.once('exit', (code) => { exited = code ?? 'signal'; });
  const base = `http://${BROWSER_HOST}:${ports.http}`;
  const deadline = Date.now() + 60000;
  for (;;) {
    if (exited !== null) throw new Error(`wptserve exited (${exited}); log: ${path.join(dir, 'serve.log')}`);
    const ok = await new Promise((resolve) => {
      const req = http.get({ host: '127.0.0.1', port: ports.http, path: '/resources/testharness.js', headers: { host: `${BROWSER_HOST}:${ports.http}` } },
        (res) => { res.resume(); resolve(res.statusCode === 200); });
      req.on('error', () => resolve(false));
      req.setTimeout(2000, () => { req.destroy(); resolve(false); });
    });
    // Each server is a process of its own and https ones start later: wait until every port accepts.
    const listening = ok && (await Promise.all(Object.values(ports).map((port) => new Promise((resolve) => {
      const c = net.connect(port, '127.0.0.1', () => { c.destroy(); resolve(true); });
      c.on('error', () => resolve(false));
    })))).every(Boolean);
    if (listening) break;
    if (Date.now() > deadline) { stop(); throw new Error(`wptserve did not answer in 60 s; log: ${path.join(dir, 'serve.log')}`); }
    await new Promise((r) => setTimeout(r, 250));
  }
  function stop() {
    // serve.py starts a process per server: kill the group.
    try { process.kill(-child.pid, 'SIGTERM'); } catch {}
  }
  log.write?.(`wptserve on ${base} (log ${path.join(dir, 'serve.log')})\n`);
  return { ports, base, stop, logFile: path.join(dir, 'serve.log') };
}

// A forward proxy (absolute-URI requests and CONNECT tunnels) that sends every request to 127.0.0.1 on the port it
// names: the browser's host mapping. Resolves { port, close }.
export function startProxy() {
  const server = http.createServer((req, res) => {
    let u;
    try { u = new URL(req.url); } catch { res.writeHead(400); res.end(); return; }
    const up = http.request({ host: '127.0.0.1', port: u.port || 80, method: req.method, path: u.pathname + u.search, headers: req.headers },
      (r) => { res.writeHead(r.statusCode, r.rawHeaders); r.pipe(res); });
    up.on('error', () => { if (!res.headersSent) res.writeHead(502); res.end(); });
    res.on('close', () => up.destroy());
    req.pipe(up);
  });
  server.on('connect', (req, sock, head) => {
    const port = Number(req.url.split(':').pop()) || 443;
    let open = false;
    const up = net.connect(port, '127.0.0.1', () => {
      open = true;
      sock.write('HTTP/1.1 200 Connection Established\r\n\r\n');
      if (head.length) up.write(head);
      up.pipe(sock); sock.pipe(up);
    });
    const end = () => { up.destroy(); sock.destroy(); };
    // A refused tunnel is answered (the browser reports a tunnel failure rather than waiting).
    up.on('error', () => { if (!open && sock.writable) sock.end('HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n'); else end(); });
    sock.on('error', end);
  });
  // WebSocket upgrades over plain http (ws://) arrive as absolute-URI requests with Upgrade.
  server.on('upgrade', (req, sock, head) => {
    let u;
    try { u = new URL(req.url); } catch { sock.destroy(); return; }
    const up = net.connect(Number(u.port) || 80, '127.0.0.1', () => {
      const lines = [`${req.method} ${u.pathname + u.search} HTTP/1.1`];
      for (let i = 0; i < req.rawHeaders.length; i += 2) lines.push(`${req.rawHeaders[i]}: ${req.rawHeaders[i + 1]}`);
      up.write(lines.join('\r\n') + '\r\n\r\n');
      if (head.length) up.write(head);
      up.pipe(sock); sock.pipe(up);
    });
    const end = () => { up.destroy(); sock.destroy(); };
    up.on('error', end); sock.on('error', end);
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ port: server.address().port, close: () => server.close() })));
}
