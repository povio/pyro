// MCP's stdio transport: one JSON-RPC message per line on stdin and stdout.
// stdout carries the protocol only (console.log goes to stderr meanwhile);
// the browser's own output goes to stderr. When stdin ends the server closes
// its browser and the process exits.
import { McpServer, type McpServerOptions, type Outgoing } from './server.js';

export async function serveStdio(options: McpServerOptions = {}): Promise<void> {
  console.log = console.error;
  console.info = console.error;
  const write = (message: Outgoing | Outgoing[]) => {
    process.stdout.write(`${JSON.stringify(message)}\n`);
  };
  const server = new McpServer({ ...options, notify: write });
  const inflight = new Set<Promise<void>>();
  let buffer = '';
  const onLine = (line: string) => {
    if (!line.trim()) return;
    const p: Promise<void> = server
      .receive(line)
      .then((reply) => {
        if (reply) write(reply);
      })
      .catch((e) => console.error('pyro mcp:', e))
      .finally(() => inflight.delete(p));
    inflight.add(p);
  };
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (chunk: string) => {
    buffer += chunk;
    for (let i = buffer.indexOf('\n'); i >= 0; i = buffer.indexOf('\n')) {
      onLine(buffer.slice(0, i));
      buffer = buffer.slice(i + 1);
    }
  });
  const ended = new Promise<void>((resolve) => {
    process.stdin.once('end', resolve);
    process.stdin.once('close', resolve);
  });
  const stop = () => process.stdin.destroy();
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);
  process.once('SIGHUP', stop);
  await ended;
  if (buffer.trim()) onLine(buffer);
  await server.close();
}
