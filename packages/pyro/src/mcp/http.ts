// MCP's Streamable HTTP transport, the JSON-response form: POST /mcp takes
// one JSON-RPC message (or a batch) and answers with JSON (202 for
// notifications); GET /mcp is 405 (this server sends nothing unasked);
// DELETE /mcp ends the session. `initialize` starts a session whose id comes
// back in `Mcp-Session-Id` and is required on every later request. Each
// session has its own browser contexts and tabs; the sessions share one
// browser.
//
// Bound to 127.0.0.1 by default. A request with an `Origin` header must come
// from an allowed origin (loopback ones by default) and the `Host` must be a
// loopback name when bound to loopback, against DNS rebinding.
import { randomUUID } from 'node:crypto';
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { BrowserAgentSession, withTimeout } from '../agent/session.js';
import type { Browser } from '../wright/browser.js';
import { McpServer, PROTOCOL_VERSIONS, type McpServerOptions } from './server.js';

export type HttpOptions = McpServerOptions & {
  port: number;
  /** Bind address (default 127.0.0.1). */
  host?: string;
  /** Origins allowed besides loopback ones (`http://localhost:*` style entries match any port). */
  allowedHttpOrigins?: string[];
  /** Largest request body, bytes (default 4 MiB). */
  maxBody?: number;
};

const LOOPBACK = /^(localhost|127(?:\.\d+){3}|\[::1\])(:\d+)?$/i;

export async function serveHttp(options: HttpOptions): Promise<{ server: Server; url: string; close: () => Promise<void> }> {
  const host = options.host ?? '127.0.0.1';
  const loopbackOnly = LOOPBACK.test(host) || host === '::1';
  const sessions = new Map<string, McpServer>();
  // One browser for every MCP session (each gets contexts of its own).
  let shared: Promise<Browser> | undefined;
  const browser = () => (shared ??= new BrowserAgentSession(options).browser());

  const originAllowed = (origin: string | undefined) => {
    if (!origin) return true;
    let u: URL;
    try {
      u = new URL(origin);
    } catch {
      return false;
    }
    if (LOOPBACK.test(u.host)) return true;
    return (options.allowedHttpOrigins ?? []).some((a) => (a.endsWith(':*') ? origin.startsWith(a.slice(0, -1)) : a === origin));
  };

  const server = createServer(async (req, res) => {
    try {
      await handle(req, res);
    } catch (e) {
      if (!res.headersSent) send(res, 500, { jsonrpc: '2.0', id: null, error: { code: -32603, message: `Internal error: ${e instanceof Error ? e.message : e}` } });
    }
  });

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const url = new URL(req.url ?? '/', 'http://localhost');
    if (url.pathname !== '/mcp') return send(res, 404, { error: 'not found: the MCP endpoint is /mcp' });
    if (!originAllowed(req.headers.origin)) return send(res, 403, { error: `origin not allowed: ${req.headers.origin}` });
    if (loopbackOnly && !LOOPBACK.test(req.headers.host ?? '')) return send(res, 403, { error: `host not allowed: ${req.headers.host}` });
    const version = req.headers['mcp-protocol-version'];
    if (typeof version === 'string' && !(PROTOCOL_VERSIONS as readonly string[]).includes(version)) {
      return send(res, 400, { jsonrpc: '2.0', id: null, error: { code: -32600, message: `Unsupported MCP-Protocol-Version: ${version}` } });
    }
    const sid = req.headers['mcp-session-id'];
    if (req.method === 'GET') {
      res.writeHead(405, { allow: 'POST, DELETE' }).end();
      return;
    }
    if (req.method === 'DELETE') {
      const mcp = typeof sid === 'string' ? sessions.get(sid) : undefined;
      if (!mcp) return send(res, 404, { error: 'unknown session' });
      sessions.delete(sid as string);
      await mcp.close();
      res.writeHead(204).end();
      return;
    }
    if (req.method !== 'POST') {
      res.writeHead(405, { allow: 'POST, DELETE' }).end();
      return;
    }
    let body: unknown;
    try {
      body = JSON.parse(await readBody(req, options.maxBody ?? 4 * 1024 * 1024));
    } catch (e) {
      return send(res, 400, { jsonrpc: '2.0', id: null, error: { code: -32700, message: `Parse error: ${e instanceof Error ? e.message : e}` } });
    }
    const messages = Array.isArray(body) ? body : [body];
    const initializing = messages.some((m) => m && typeof m === 'object' && (m as { method?: unknown }).method === 'initialize');
    let mcp: McpServer | undefined;
    let newId: string | undefined;
    if (initializing) {
      newId = randomUUID();
      const session = new BrowserAgentSession({ ...options, browser: await browser() });
      mcp = new McpServer({ ...options, session });
      sessions.set(newId, mcp);
    } else {
      if (typeof sid !== 'string') return send(res, 400, { jsonrpc: '2.0', id: null, error: { code: -32600, message: 'Bad Request: Mcp-Session-Id header is required' } });
      mcp = sessions.get(sid);
      if (!mcp) return send(res, 404, { jsonrpc: '2.0', id: null, error: { code: -32001, message: 'Session not found: initialize a new one' } });
    }
    const reply = await mcp.receive(body);
    const headers: Record<string, string> = newId ? { 'mcp-session-id': newId } : {};
    if (!reply) {
      res.writeHead(202, headers).end();
      return;
    }
    send(res, 200, reply, headers);
  }

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(options.port, host, () => resolve());
  });
  const address = server.address();
  const port = typeof address === 'object' && address ? address.port : options.port;
  const close = async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    for (const mcp of sessions.values()) await mcp.close();
    sessions.clear();
    if (shared) await withTimeout(shared.then((b) => b.close()), 10_000).catch(() => undefined);
  };
  return { server, url: `http://${host.includes(':') ? `[${host}]` : host}:${port}/mcp`, close };
}

function send(res: ServerResponse, status: number, body: unknown, headers: Record<string, string> = {}): void {
  res.writeHead(status, { 'content-type': 'application/json', ...headers }).end(JSON.stringify(body));
}

function readBody(req: IncomingMessage, max: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    req.on('data', (c: Buffer) => {
      size += c.length;
      if (size > max) {
        reject(new Error(`body larger than ${max} bytes`));
        req.destroy();
        return;
      }
      chunks.push(c);
    });
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', reject);
  });
}
