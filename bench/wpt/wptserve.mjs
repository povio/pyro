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
import { spawn, spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import https from 'node:https';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { pipeline } from 'node:stream';
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

// What `lsof` lists for listening TCP sockets with -F pn (`p<pid>` lines, then `n<addr>:<port>` lines): the ports.
export function listeningPorts(lsofOutput) {
  const ports = new Set();
  for (const line of lsofOutput.split('\n')) {
    const m = line.match(/^n.*:(\d+)$/);
    if (m) ports.add(Number(m[1]));
  }
  return ports;
}
// The TCP ports processes of the process group `pgid` listen on (wptserve starts one process per server).
function ownedPorts(pgid) {
  const r = spawnSync('lsof', ['-nP', '-a', '-g', String(pgid), '-iTCP', '-sTCP:LISTEN', '-F', 'pn'], { encoding: 'utf8' });
  // No lsof: unknown (null); the token check still proves the http and https servers are this run's.
  return r.error ? null : listeningPorts(r.stdout ?? '');
}

// GET `pathname` from 127.0.0.1:`port` as `host` (https: WPT's test certificate, not checked). Resolves the body, or
// null on any failure or a status other than 200.
function fetchLocal(port, host, pathname, { tls = false, timeout = 5000 } = {}) {
  return new Promise((resolve) => {
    const req = (tls ? https : http).get({ host: '127.0.0.1', port, path: pathname, headers: { host: `${host}:${port}` }, servername: tls ? host : undefined, rejectUnauthorized: false, agent: false }, (res) => {
      let body = '';
      res.setEncoding('utf8');
      res.on('data', (d) => { body += d; });
      res.on('end', () => resolve(res.statusCode === 200 ? body : null));
      res.on('error', () => resolve(null));
    });
    req.on('error', () => resolve(null));
    req.setTimeout(timeout, () => { req.destroy(); resolve(null); });
  });
}

const PORT_NAMES = ['http', 'http2', 'https', 'https2', 'httpLocal', 'httpPublic', 'httpsLocal', 'httpsPublic', 'ws', 'wss'];

// Starts wptserve on free ports. Resolves { ports, base, stop, alive, onExit, logFile } once it is serving: its
// http and https servers answer this run's token (a file only this run's config aliases, so a listener of another
// run, which may have taken a port between the probe and Python's bind, cannot pass), and every port is listened on
// by its own process group. A clash (a port taken, the server exiting) starts again on new ports, up to 3 times.
// The server's process group dies with the runner: stop() on any exit of the runner, and a watchdog process that
// ends the group once the runner's pid is gone (a runner killed with SIGKILL, a cancelled CI job).
// `options.ports` (tests): the ports of the first attempt.
export async function startWptserve(wpt, { log = process.stderr, ports: first = null } = {}) {
  let lastError;
  for (let attempt = 0; attempt < 3; attempt++) {
    const ports = attempt === 0 && first ? { ...first } : {};
    for (const k of PORT_NAMES) ports[k] ??= await freePort();
    try { return await startOnce(wpt, ports, log); } catch (e) {
      lastError = e;
      log.write?.(`wptserve: ${e.message}; starting again on new ports\n`);
    }
  }
  throw lastError;
}

async function startOnce(wpt, ports, log) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pyro-wptserve-'));
  const token = crypto.randomBytes(16).toString('hex');
  const tokenPath = `/pyro-run-${token}/token.txt`;
  fs.mkdirSync(path.join(dir, 'token'));
  fs.writeFileSync(path.join(dir, 'token', 'token.txt'), token);
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
      // This run's token: readiness and liveness ask for it (see startWptserve).
      { 'url-path': `/pyro-run-${token}/`, 'local-dir': path.join(dir, 'token') },
    ],
  };
  const cfg = path.join(dir, 'config.json');
  fs.writeFileSync(cfg, JSON.stringify(config));
  const py = pythonFor(wpt);
  // Without the venv wptserve still starts on python3, but a handler that imports its requirements fails.
  if (py === 'python3') log.write?.(`wptserve: no venv in ${wpt} and no WPT_PYTHON; using python3 (handlers that need tools/serve/requirements.txt fail: bench/wpt/fetch-wpt.sh --venv ${wpt})\n`);
  // `wpt serve` would make its own venv; the server needs only its requirements, so run serve.py's entry directly.
  const child = spawn(py, [path.join(wpt, 'wpt'), '--venv', path.dirname(path.dirname(py)), '--skip-venv-setup',
    'serve', '--no-h2', '--config', cfg], { cwd: wpt, stdio: ['ignore', 'pipe', 'pipe'], detached: true });
  const out = fs.createWriteStream(path.join(dir, 'serve.log'));
  child.stdout.pipe(out); child.stderr.pipe(out);
  let exited = null, stopped = false;
  const exitListeners = [];
  child.once('exit', (code) => { exited = code ?? 'signal'; if (!stopped) for (const f of exitListeners) f(exited); });
  function stop() {
    stopped = true;
    // serve.py starts a process per server: kill the group.
    try { process.kill(-child.pid, 'SIGTERM'); } catch {}
  }
  process.once('exit', stop);
  // The watchdog: a shell that ends the server's group once this runner is gone, or exits with the group.
  spawn('/bin/sh', ['-c', `while kill -0 ${process.pid} 2>/dev/null && kill -0 -${child.pid} 2>/dev/null; do sleep 2; done; kill -TERM -${child.pid} 2>/dev/null`],
    { stdio: 'ignore', detached: true }).unref();
  const logFile = path.join(dir, 'serve.log');
  const base = `http://${BROWSER_HOST}:${ports.http}`;
  const deadline = Date.now() + 60000;
  for (;;) {
    if (exited !== null) throw new Error(`wptserve exited (${exited}); log: ${logFile}`);
    const ok = (await fetchLocal(ports.http, BROWSER_HOST, tokenPath, { timeout: 2000 })) === token
      && (await fetchLocal(ports.https, BROWSER_HOST, tokenPath, { tls: true, timeout: 2000 })) === token;
    if (ok) {
      const owned = ownedPorts(child.pid);
      const foreign = owned ? PORT_NAMES.filter((k) => !owned.has(ports[k])) : [];
      if (!foreign.length) break;
      // Each server is a process of its own and some start later; a port another process holds stays foreign.
      if (Date.now() > deadline - 50000) { stop(); throw new Error(`ports not held by this wptserve: ${foreign.map((k) => `${k} ${ports[k]}`).join(', ')}; log: ${logFile}`); }
    }
    if (Date.now() > deadline) { stop(); throw new Error(`wptserve did not answer this run's token in 60 s; log: ${logFile}`); }
    await new Promise((r) => setTimeout(r, 250));
  }
  log.write?.(`wptserve on ${base} (log ${logFile})\n`);
  return {
    ports, base, stop, logFile, pid: child.pid,
    // Whether this run's server still answers (its token, over http).
    alive: async () => exited === null && (await fetchLocal(ports.http, BROWSER_HOST, tokenPath)) === token,
    // `f(code)` when the server exits without stop().
    onExit: (f) => { if (exited !== null && !stopped) f(exited); else exitListeners.push(f); },
  };
}

// A forward proxy (absolute-URI requests and CONNECT tunnels) that sends every request to 127.0.0.1 on the port it
// names: the browser's host mapping. Resolves { port, close }.
// Hop-by-hop headers (RFC 9110 7.6.1) stay on their own connection: wptserve's `Connection: close` is not the
// browser's business, and the proxy keeps its own connections to wptserve alive.
const HOP = new Set(['connection', 'keep-alive', 'proxy-connection', 'proxy-authenticate', 'proxy-authorization', 'te', 'trailer', 'transfer-encoding', 'upgrade']);
const endToEnd = (raw) => {
  const out = [];
  for (let i = 0; i < raw.length; i += 2) if (!HOP.has(raw[i].toLowerCase())) out.push(raw[i], raw[i + 1]);
  return out;
};

// The header pairs Node accepts (a name that is a token, a value without control characters).
const writable = (raw) => {
  const out = [];
  for (let i = 0; i < raw.length; i += 2) {
    try { http.validateHeaderName(raw[i]); http.validateHeaderValue(raw[i], raw[i + 1]); out.push(raw[i], raw[i + 1]); } catch { /* dropped */ }
  }
  return out;
};

export function startProxy() {
  const agent = new http.Agent({ keepAlive: true, maxSockets: 256 });
  const server = http.createServer((req, res) => {
    let u;
    try { u = new URL(req.url); } catch { res.writeHead(400); res.end(); return; }
    const headers = Object.fromEntries(Object.entries(req.headers).filter(([k]) => !HOP.has(k)));
    const up = http.request({ host: '127.0.0.1', port: u.port || 80, method: req.method, path: u.pathname + u.search, headers, agent },
      (r) => {
        // WPT pages send status texts and header values Node refuses to write (control characters): such a response
        // goes on with the standard status text, and without the offending headers, never as a throw (an exception
        // here ends the whole run).
        const raw = endToEnd(r.rawHeaders);
        const text = /^[\t\x20-\x7e\x80-\xff]*$/.test(r.statusMessage) ? r.statusMessage : (http.STATUS_CODES[r.statusCode] ?? '');
        try { res.writeHead(r.statusCode, text, raw); } catch {
          try { res.writeHead(r.statusCode, text, writable(raw)); } catch { res.statusMessage = ''; res.writeHead(502); res.end(); r.destroy(); return; }
        }
        pipeline(r, res, () => {});
      });
    // After the head went out the response cannot be ended cleanly while the upstream may still be piped into it.
    up.on('error', () => { if (res.headersSent) res.destroy(); else { res.writeHead(502); res.end(); } });
    // A response that ended (the upstream failed, the browser left) while the upstream still sends: without a
    // listener the "write after end" is an uncaught exception.
    res.on('error', () => up.destroy());
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
