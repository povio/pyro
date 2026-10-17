#!/usr/bin/env node
// CDP recording proxy: sits between a CDP client (Playwright/Puppeteer/...) and a real CDP server
// (e.g. `lightpanda serve`), forwards HTTP /json/* and the WebSocket transparently, and appends
// one JSONL record per interesting CDP message.
//
// Usage:
//   node cdp-record-proxy.mjs --upstream http://127.0.0.1:9222 --out /path/cdp.jsonl [--listen 9333] [--host 127.0.0.1] [--params]
//
//   --upstream  base URL (http://host:port) of the real CDP server. Required.
//   --out       JSONL output file (appended). Required.
//   --listen    port to listen on (default 0 = ephemeral). Prints "PROXY_LISTENING <port>" on stdout.
//   --host      listen address (default 127.0.0.1).
//   --params    also record message params/error payloads (truncated to 2 KB). Off by default.
//
// Records (one JSON object per line):
//   {"t":ms, "conn":n, "dir":"c2s", "kind":"command", "id":..., "method":"Page.navigate", "session":true|false}
//   {"t":ms, "conn":n, "dir":"s2c", "kind":"event",   "method":"Page.loadEventFired", "session":true|false}
//   {"t":ms, "conn":n, "dir":"s2c", "kind":"error",   "id":..., "method":"<method of the command>", "session":bool, "error":{code,message}}
//   {"t":ms, "conn":n, "dir":"s2c", "kind":"response", "id":..., "method":..., "session":bool, "ms":latency}   (successful responses)
//   {"t":ms, "conn":n, "kind":"ws-open"|"ws-close"|"http", ...}
//
// WebSocket implementation: the 'ws' package, loaded by absolute path from the tos-monorepo bun store
// (read-only use; nothing is installed there).
import http from 'node:http';
import fs from 'node:fs';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const WS_PATH = process.env.CDP_PROXY_WS_PATH ?? 'ws';
const { WebSocket, WebSocketServer } = require(WS_PATH);

function parseArgs(argv) {
  const o = { listen: 0, host: '127.0.0.1', params: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--upstream') o.upstream = argv[++i];
    else if (a === '--out') o.out = argv[++i];
    else if (a === '--listen') o.listen = Number(argv[++i]);
    else if (a === '--host') o.host = argv[++i];
    else if (a === '--params') o.params = true;
    else { console.error(`unknown arg ${a}`); process.exit(2); }
  }
  if (!o.upstream || !o.out) {
    console.error('usage: node cdp-record-proxy.mjs --upstream http://127.0.0.1:9222 --out cdp.jsonl [--listen 9333] [--host 127.0.0.1] [--params]');
    process.exit(2);
  }
  return o;
}

const opts = parseArgs(process.argv.slice(2));
const upstream = new URL(opts.upstream.replace(/^ws/, 'http'));
const out = fs.createWriteStream(opts.out, { flags: 'a' });
const T0 = Date.now();
const rec = (o) => out.write(JSON.stringify({ t: Date.now() - T0, ...o }) + '\n');
const trunc = (v) => { const s = JSON.stringify(v); return s && s.length > 2048 ? s.slice(0, 2048) + '...' : v; };
let connSeq = 0;
let proxyHostPort = '';

// ---------- HTTP (/json/*, anything else) ----------
const server = http.createServer((req, res) => {
  const body = [];
  req.on('data', (c) => body.push(c));
  req.on('end', () => {
    const up = http.request({
      host: upstream.hostname, port: upstream.port, method: req.method, path: req.url,
      headers: { ...req.headers, host: `${upstream.hostname}:${upstream.port}` },
    }, (ur) => {
      const chunks = [];
      ur.on('data', (c) => chunks.push(c));
      ur.on('end', () => {
        let buf = Buffer.concat(chunks);
        // Rewrite ws URLs advertised by /json/* so clients come back through the proxy.
        if (req.url.startsWith('/json')) {
          const upHP = `${upstream.hostname}:${upstream.port}`;
          const s = buf.toString('utf8');
          const r = s.split(upHP).join(proxyHostPort);
          if (r !== s) buf = Buffer.from(r, 'utf8');
        }
        const headers = { ...ur.headers };
        delete headers['content-length'];
        delete headers['transfer-encoding'];
        headers['content-length'] = String(buf.length);
        res.writeHead(ur.statusCode, headers);
        res.end(buf);
        rec({ kind: 'http', method: req.method, path: req.url, status: ur.statusCode });
      });
    });
    up.on('error', (e) => {
      rec({ kind: 'http', method: req.method, path: req.url, status: 502, error: e.message });
      res.writeHead(502); res.end(`upstream error: ${e.message}`);
    });
    up.end(Buffer.concat(body));
  });
});

// ---------- WebSocket ----------
const wss = new WebSocketServer({ noServer: true, perMessageDeflate: false, maxPayload: 512 * 1024 * 1024 });

server.on('upgrade', (req, socket, head) => {
  wss.handleUpgrade(req, socket, head, (client) => {
    const conn = ++connSeq;
    const upUrl = `ws://${upstream.hostname}:${upstream.port}${req.url}`;
    const upWs = new WebSocket(upUrl, { perMessageDeflate: false, maxPayload: 512 * 1024 * 1024 });
    const pending = new Map(); // key `${sessionId}|${id}` -> {method, t}
    const queue = [];
    rec({ conn, kind: 'ws-open', path: req.url, upstream: upUrl });

    client.on('message', (data, isBinary) => {
      try {
        const m = JSON.parse(data.toString('utf8'));
        const r = { conn, dir: 'c2s', kind: 'command', id: m.id, method: m.method, session: !!m.sessionId };
        if (opts.params && m.params) r.params = trunc(m.params);
        rec(r);
        pending.set(`${m.sessionId ?? ''}|${m.id}`, { method: m.method, t: Date.now() });
      } catch { rec({ conn, dir: 'c2s', kind: 'unparsable', bytes: data.length }); }
      if (upWs.readyState === WebSocket.OPEN) upWs.send(data, { binary: isBinary });
      else queue.push([data, isBinary]);
    });

    upWs.on('open', () => { for (const [d, b] of queue.splice(0)) upWs.send(d, { binary: b }); });
    upWs.on('message', (data, isBinary) => {
      try {
        const m = JSON.parse(data.toString('utf8'));
        if (m.method) {
          const r = { conn, dir: 's2c', kind: 'event', method: m.method, session: !!m.sessionId };
          if (opts.params && m.params) r.params = trunc(m.params);
          rec(r);
        } else if (m.id !== undefined) {
          const key = `${m.sessionId ?? ''}|${m.id}`;
          const p = pending.get(key); pending.delete(key);
          const base = { conn, dir: 's2c', id: m.id, method: p?.method ?? null, session: !!m.sessionId, ms: p ? Date.now() - p.t : null };
          if (m.error) rec({ ...base, kind: 'error', error: opts.params ? trunc(m.error) : { code: m.error.code, message: String(m.error.message).slice(0, 300) } });
          else rec({ ...base, kind: 'response' });
        }
      } catch { rec({ conn, dir: 's2c', kind: 'unparsable', bytes: data.length }); }
      if (client.readyState === WebSocket.OPEN) client.send(data, { binary: isBinary });
    });

    let closed = false;
    const closeBoth = (who, code, reason) => {
      if (closed) return;
      closed = true;
      rec({ conn, kind: 'ws-close', by: who, code, reason: reason?.toString?.() ?? '', unanswered: [...pending.values()].map((p) => p.method) });
      const c = (typeof code === 'number' && code >= 1000 && code < 5000 && code !== 1005 && code !== 1006) ? code : 1000;
      try { if (client.readyState === WebSocket.OPEN) client.close(c); } catch {}
      try { if (upWs.readyState === WebSocket.OPEN) upWs.close(c); } catch {}
      if (upWs.readyState === WebSocket.CONNECTING) upWs.terminate();
    };
    client.on('close', (code, reason) => closeBoth('client', code, reason));
    upWs.on('close', (code, reason) => closeBoth('upstream', code, reason));
    upWs.on('error', (e) => { rec({ conn, kind: 'ws-error', side: 'upstream', error: e.message }); client.terminate(); });
    client.on('error', (e) => { rec({ conn, kind: 'ws-error', side: 'client', error: e.message }); upWs.terminate(); });
  });
});

server.listen(opts.listen, opts.host, () => {
  const { port } = server.address();
  proxyHostPort = `${opts.host}:${port}`;
  console.log(`PROXY_LISTENING ${port}`);
});
const shutdown = () => { server.close(); out.end(() => process.exit(0)); setTimeout(() => process.exit(0), 500).unref(); };
process.on('SIGTERM', shutdown);
process.on('SIGINT', shutdown);
