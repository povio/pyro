// A line-delimited JSON-RPC client over the stdio of a `pyro mcp` child process.
import { spawn, type ChildProcess } from 'node:child_process';
import { join } from 'node:path';
import { bin } from './site';

export const pkg = join(import.meta.dir, '../..');

export type Reply = { id?: number | string; result?: any; error?: { code: number; message: string }; method?: string; params?: any };

export class Client {
  private next = 1;
  private waiting = new Map<number | string, (r: Reply) => void>();
  readonly replies: Reply[] = [];
  readonly exited: Promise<number | null>;
  /** Everything the server wrote to stdout, as it came (for checks that a secret never crossed). */
  raw = '';
  constructor(readonly child: ChildProcess) {
    let buffer = '';
    child.stdout!.setEncoding('utf8');
    child.stdout!.on('data', (chunk: string) => {
      this.raw += chunk;
      buffer += chunk;
      for (let i = buffer.indexOf('\n'); i >= 0; i = buffer.indexOf('\n')) {
        const line = buffer.slice(0, i);
        buffer = buffer.slice(i + 1);
        if (!line.trim()) continue;
        const msg = JSON.parse(line) as Reply;
        this.replies.push(msg);
        if (msg.id !== undefined) this.waiting.get(msg.id)?.(msg);
      }
    });
    this.exited = new Promise((resolve) => child.once('exit', (code) => resolve(code)));
  }
  send(msg: object): void {
    this.child.stdin!.write(`${JSON.stringify({ jsonrpc: '2.0', ...msg })}\n`);
  }
  request(method: string, params: object = {}, id: number | string = this.next++): Promise<Reply> {
    return new Promise((resolve) => {
      this.waiting.set(id, resolve);
      this.send({ id, method, params });
    });
  }
  call(name: string, args: object = {}): Promise<Reply> {
    return this.request('tools/call', { name, arguments: args });
  }
  /** Ends the server (stdin closed) and waits for it to exit. */
  async end(): Promise<number | null> {
    if (this.child.exitCode === null) this.child.stdin!.end();
    return this.exited;
  }
}

/** `pyro mcp` started as `command` (e.g. `['bun', 'src/cli.ts']`) with this checkout's browser. */
export function start(command: string[], extraArgs: string[] = []): Client {
  const child = spawn(command[0], [...command.slice(1), 'mcp', ...extraArgs], {
    cwd: pkg,
    env: { ...process.env, PYRO_BIN: bin },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  child.stderr!.on('data', () => {});
  return new Client(child);
}

/** The text content of a tool call's result. */
export const text = (r: Reply): string => (r.result?.content ?? []).filter((c: any) => c.type === 'text').map((c: any) => c.text).join('\n');
