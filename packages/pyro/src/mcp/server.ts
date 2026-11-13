// The MCP server: JSON-RPC 2.0 methods of the Model Context Protocol over the
// browser tools, independent of the transport (stdio.ts, http.ts).
//
// Implements initialize (version negotiation), ping, tools/list, tools/call,
// notifications/cancelled (stops waiting for a call, which gets no response),
// and logging/setLevel. A tool's failure is a result with `isError`, not a
// JSON-RPC error; JSON-RPC errors are for malformed requests and unknown
// methods or tools.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { executeTool, resolveCaps, selectTools, toolDefinition, type BrowserToolsOptions } from '../agent/index.js';
import { INSTRUCTIONS } from '../agent/instructions.js';
import { BrowserAgentSession, withTimeout } from '../agent/session.js';
import type { ToolResult } from '../agent/content.js';
import type { BrowserTool } from '../agent/tools.js';

/** Protocol revisions this server speaks, newest first. */
export const PROTOCOL_VERSIONS = ['2025-11-25', '2025-06-18', '2025-03-26', '2024-11-05'] as const;

type Id = string | number;
type Incoming = { jsonrpc?: string; id?: Id | null; method?: unknown; params?: any; result?: unknown; error?: unknown };
export type Outgoing = { jsonrpc: '2.0'; id: Id | null; result?: unknown; error?: { code: number; message: string; data?: unknown } } | { jsonrpc: '2.0'; method: string; params?: unknown };

const LEVELS = ['debug', 'info', 'notice', 'warning', 'error', 'critical', 'alert', 'emergency'];

export type McpServerOptions = BrowserToolsOptions & {
  /** Server→client notifications (logging); without it they are dropped. */
  notify?: (message: Outgoing) => void;
};

class RpcError extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
  }
}

export class McpServer {
  readonly session: BrowserAgentSession;
  private readonly tools: Map<string, BrowserTool>;
  private readonly running = new Map<Id, AbortController>();
  /** The negotiated revision (the newest until a client says otherwise). */
  protocolVersion: string = PROTOCOL_VERSIONS[0];
  private logLevel = 'info';
  private closed = false;

  constructor(private readonly options: McpServerOptions = {}) {
    this.session = options.session ?? new BrowserAgentSession(options);
    const selected = selectTools(resolveCaps(options.caps), { isolated: this.session.profiles.readOnly });
    this.tools = new Map(selected.map((t) => [t.name, t]));
  }

  /**
   * Handles one message as read from the transport (a JSON text or a parsed
   * value; an array is a batch). Returns what to send back, or undefined
   * (notifications, responses, cancelled calls).
   */
  async receive(message: unknown): Promise<Outgoing | Outgoing[] | undefined> {
    let value = message;
    if (typeof message === 'string') {
      try {
        value = JSON.parse(message);
      } catch {
        return error(null, -32700, 'Parse error: not JSON');
      }
    }
    if (Array.isArray(value)) {
      if (!value.length) return error(null, -32600, 'Invalid Request: empty batch');
      const replies = (await Promise.all(value.map((m) => this.one(m)))).filter((r): r is Outgoing => !!r);
      return replies.length ? replies : undefined;
    }
    return this.one(value);
  }

  private async one(raw: unknown): Promise<Outgoing | undefined> {
    if (!raw || typeof raw !== 'object') return error(null, -32600, 'Invalid Request');
    const m = raw as Incoming;
    const id = m.id === undefined || m.id === null ? undefined : m.id;
    if (typeof m.method !== 'string') {
      // A response to a request of ours (we send none) or junk.
      return 'result' in m || 'error' in m ? undefined : error(id ?? null, -32600, 'Invalid Request: no method');
    }
    if (id === undefined) {
      this.notification(m.method, m.params);
      return undefined;
    }
    if (typeof id !== 'string' && typeof id !== 'number') return error(null, -32600, 'Invalid Request: bad id');
    try {
      const result = await this.request(id, m.method, m.params ?? {});
      return result === CANCELLED ? undefined : { jsonrpc: '2.0', id, result };
    } catch (e) {
      if (e instanceof RpcError) return error(id, e.code, e.message);
      return error(id, -32603, `Internal error: ${e instanceof Error ? e.message : e}`);
    }
  }

  private notification(method: string, params: any): void {
    if (method === 'notifications/cancelled') {
      const id = params?.requestId;
      this.running.get(id)?.abort(params?.reason ?? 'cancelled');
      if (this.running.has(id)) this.log('info', `cancelled request ${JSON.stringify(id)}${params?.reason ? `: ${params.reason}` : ''}`);
    }
    // notifications/initialized and others need nothing.
  }

  private async request(id: Id, method: string, params: any): Promise<unknown> {
    switch (method) {
      case 'initialize':
        return this.initialize(params);
      case 'ping':
        return {};
      case 'tools/list':
        return { tools: [...this.tools.values()].map((t) => this.listed(t)) };
      case 'tools/call':
        return this.call(id, params);
      case 'logging/setLevel': {
        if (!LEVELS.includes(params?.level)) throw new RpcError(-32602, `Invalid params: level is one of ${LEVELS.join(', ')}`);
        this.logLevel = params.level;
        return {};
      }
      default:
        throw new RpcError(-32601, `Method not found: ${method}`);
    }
  }

  private initialize(params: any): unknown {
    const asked = params?.protocolVersion;
    this.protocolVersion = (PROTOCOL_VERSIONS as readonly string[]).includes(asked) ? asked : PROTOCOL_VERSIONS[0];
    return {
      protocolVersion: this.protocolVersion,
      capabilities: { tools: { listChanged: false }, logging: {} },
      serverInfo: { name: 'pyro', title: 'Pyro browser', version: packageVersion() },
      instructions: INSTRUCTIONS,
    };
  }

  /** A tool as `tools/list` gives it (`outputSchema` from revision 2025-06-18 on). */
  private listed(t: BrowserTool): Record<string, unknown> {
    const d = toolDefinition(t);
    if (this.protocolVersion < '2025-06-18') delete d.outputSchema;
    return d;
  }

  private async call(id: Id, params: any): Promise<unknown> {
    const name = params?.name;
    if (typeof name !== 'string') throw new RpcError(-32602, 'Invalid params: name is required');
    const tool = this.tools.get(name);
    if (!tool) throw new RpcError(-32602, `Unknown tool: ${name}`);
    const controller = new AbortController();
    this.running.set(id, controller);
    try {
      const result = await executeTool(tool, params.arguments ?? {}, this.session, controller.signal);
      if (controller.signal.aborted) return CANCELLED;
      return this.compatible(result);
    } finally {
      this.running.delete(id);
    }
  }

  /** A result for the negotiated revision: before 2025-06-18 no resource links or structured content. */
  private compatible(result: ToolResult): ToolResult {
    if (this.protocolVersion >= '2025-06-18') return result;
    const { structuredContent: _structured, ...rest } = result;
    return {
      ...rest,
      content: result.content.map((c) => (c.type === 'resource_link' ? { type: 'text', text: `File: ${fileURLToPath(c.uri)}${c.mimeType ? ` (${c.mimeType})` : ''}` } : c)),
    };
  }

  /** A `notifications/message` to the client, at or above the level it asked for. */
  log(level: string, data: string): void {
    if (LEVELS.indexOf(level) < LEVELS.indexOf(this.logLevel)) return;
    this.options.notify?.({ jsonrpc: '2.0', method: 'notifications/message', params: { level, logger: 'pyro', data } });
  }

  /** Cancels running calls and closes the browser session. */
  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    for (const c of this.running.values()) c.abort('server closing');
    await withTimeout(this.session.close(), 10_000).catch(() => undefined);
  }
}

const CANCELLED = Symbol('cancelled');

function error(id: Id | null, code: number, message: string): Outgoing {
  return { jsonrpc: '2.0', id, error: { code, message } };
}

let version: string | undefined;
/** This package's version (from its package.json, wherever the bundler put this module). */
export function packageVersion(): string {
  if (version) return version;
  let dir = dirname(fileURLToPath(import.meta.url));
  for (let i = 0; i < 5; i++) {
    try {
      const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'));
      if (pkg.name === '@povio/pyro') return (version = String(pkg.version));
    } catch {
      // keep looking
    }
    dir = dirname(dir);
  }
  return (version = '0.0.0');
}
