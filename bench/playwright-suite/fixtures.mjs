// Worker-scoped browser fixture: runs TARGET as a CDP server and connects over CDP, plus a local HTTP
// server with the fixture pages. TARGET is a label: BIN (any `serve --host --port` binary), or chrome
// (the chrome-serve shim), or chrome-launch (Playwright's own launch, used for the screenshot baseline).
import { test as base, chromium } from '@playwright/test';
import { spawn } from 'node:child_process';
import http from 'node:http';
import net from 'node:net';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const TARGET = process.env.TARGET ?? 'bin';
const BIN = TARGET === 'chrome' ? path.join(here, 'chrome-serve') : process.env.BIN;
const FLAGS = ['--log-level', 'error', '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'];

async function freePort() {
  return new Promise((res, rej) => {
    const s = net.createServer();
    s.on('error', rej);
    s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => res(port)); });
  });
}

async function waitReady(port, child, ms = 20000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (child.exitCode != null) throw new Error(`browser exited ${child.exitCode}`);
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) return r.json();
    } catch {}
    await new Promise((r) => setTimeout(r, 20));
  }
  throw new Error('browser not ready');
}

export const test = base.extend({
  server: [async ({}, use) => {
    const pages = path.join(here, 'pages');
    const srv = http.createServer((req, res) => {
      const u = new URL(req.url, 'http://x');
      if (u.pathname === '/api/echo') {
        let body = '';
        req.on('data', (d) => (body += d));
        req.on('end', () => {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(JSON.stringify({ method: req.method, body, cookie: req.headers.cookie ?? '', ua: req.headers['user-agent'], lang: req.headers['accept-language'] ?? '' }));
        });
        return;
      }
      if (u.pathname === '/api/slow') { setTimeout(() => { res.writeHead(200, { 'content-type': 'application/json' }); res.end('{"slow":true}'); }, 200); return; }
      if (u.pathname === '/set-cookie') { res.writeHead(200, { 'set-cookie': 'srv=1; Path=/', 'content-type': 'text/html' }); res.end('<p>cookie set</p>'); return; }
      if (u.pathname === '/redirect') { res.writeHead(302, { location: '/form.html' }); res.end(); return; }
      if (u.pathname === '/download') { res.writeHead(200, { 'content-type': 'text/plain', 'content-disposition': 'attachment; filename="hello.txt"' }); res.end('hello download'); return; }
      if (u.pathname === '/upload' && req.method === 'POST') {
        let n = 0; req.on('data', (d) => (n += d.length));
        req.on('end', () => { res.writeHead(200, { 'content-type': 'text/html' }); res.end(`<p id="r">received ${n}</p>`); });
        return;
      }
      const file = path.join(pages, u.pathname === '/' ? 'index.html' : u.pathname);
      if (!file.startsWith(pages) || !fs.existsSync(file)) { res.writeHead(404); res.end('nf'); return; }
      const ext = path.extname(file);
      const type = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.png': 'image/png' }[ext] ?? 'application/octet-stream';
      res.writeHead(200, { 'content-type': type });
      fs.createReadStream(file).pipe(res);
    });
    await new Promise((r) => srv.listen(0, '127.0.0.1', r));
    await use(`http://127.0.0.1:${srv.address().port}`);
    srv.close();
  }, { scope: 'worker' }],

  browser: [async ({}, use) => {
    if (TARGET === 'chrome-launch') {
      const b = await chromium.launch();
      await use(b);
      await b.close();
      return;
    }
    if (process.env.PYRO_PORT) {
      // A `serve` someone else started (scripts/pgo/train.sh, profiling): connect, do not spawn.
      const b = await chromium.connectOverCDP(`http://127.0.0.1:${process.env.PYRO_PORT}`, { timeout: 15000 });
      await use(b);
      await b.close().catch(() => {});
      return;
    }
    if (!BIN) throw new Error('set BIN=/abs/path/to/binary, or TARGET=chrome, or PYRO_PORT=<running serve>');
    const port = await freePort();
    const child = spawn(BIN, ['serve', '--host', '127.0.0.1', '--port', String(port), ...FLAGS, ...(process.env.SERVE_ARGS?.split(' ') ?? [])],
      { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' }, detached: true });
    child.stderr.on('data', () => {});
    try {
      await waitReady(port, child);
      const b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 15000 });
      await use(b);
      await b.close().catch(() => {});
    } finally {
      try { process.kill(-child.pid, 'SIGKILL'); } catch {}
    }
  }, { scope: 'worker', timeout: 60000 }],
});
export { expect } from '@playwright/test';
