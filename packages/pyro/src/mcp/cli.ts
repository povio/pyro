// `pyro mcp`: the MCP server for Claude Code, Claude Desktop, Cursor, Codex,
// VS Code and any MCP client. stdio by default; `--port` serves Streamable
// HTTP instead. Runs under Node (>= 22) and Bun.
import { resolveCaps } from '../agent/index.js';
import { originList } from '../agent/origins.js';
import { sessionStatus, when } from '../agent/profiles.js';
import { BrowserAgentSession, type BrowserAgentOptions } from '../agent/session.js';
import { packageVersion, type McpServerOptions } from './server.js';

export const HELP = `usage: pyro mcp [options]

Serves the Pyro browser to AI agents over the Model Context Protocol: stdio by
default (an MCP client starts this command), Streamable HTTP with --port.

browser:
  --ws <url>                  use a running pyro serve (ws://… or http://host:port;
                              default $PYRO_WS, else one is launched)
  --fork                      launch pyro serve --fork
  --viewport <w>x<h>          viewport of new tabs, CSS px (default 1280x720)
  --device-scale-factor <n>   device pixels per CSS pixel of new tabs, 0.5 to 8 (default 1);
                              screenshots are taken at device pixels: 2056x1200 at 3
                              gives 6168x3600 images
  --user-agent <string>       user agent of new contexts
  --proxy <url>               proxy of new contexts: http://[user:pass@]host:port
  --timeout-action <ms>       how long an action waits for its element (default 5000)
  --timeout-navigation <ms>   how long a navigation may take (default 60000)

stored identities (browser_profiles):
  --profile-dir <dir>         where profiles live (default ~/.pyro/profiles)
  --profile <name>            start in this stored identity
  --isolated                  write nothing to the profile directory (no browser_profiles
                              tool; --profile still loads)
                              A person signs an identity in, never the agent:
                              pyro profile login <name> --url <app URL> (pyro profile --help)

safety:
  --allowed-origins <list>    origins the browser may request; others are blocked
  --blocked-origins <list>    origins it may not request (lists: comma-separated;
                              https://app.example.com, example.com, *.example.com)
  --output-dir <dir>          where screenshots, PDFs and downloads are written
                              (default <tmp>/pyro-mcp-output-<uid>); file names stay inside it
  --file-roots <list>         directories browser_file_upload may read from
                              (default: the output directory, and the working directory
                              unless it is / or your home directory)
  --caps <list>               tool groups: core, tabs, evaluate, files, profiles.
                              "core,tabs" keeps those only; "-evaluate,-files" switches
                              groups off (default: all)

output:
  --no-snapshot-after-action  actions return no snapshot (the agent calls browser_snapshot)

transport:
  --port <port>               serve Streamable HTTP at http://<host>:<port>/mcp
  --host <host>               HTTP bind address (default 127.0.0.1)
  --allowed-http-origins <list>  Origin headers accepted besides loopback ones

  -h, --help                  this help
  -V, --version               the version

Claude Code:  claude mcp add pyro -- npx -y @povio/pyro mcp
Any client:   { "command": "npx", "args": ["-y", "@povio/pyro", "mcp"] }`;

type Parsed = { options: McpServerOptions; port?: number; host?: string; allowedHttpOrigins?: string[] };

const VALUE_FLAGS = new Set([
  'ws', 'viewport', 'device-scale-factor', 'user-agent', 'proxy', 'timeout-action', 'timeout-navigation', 'profile-dir', 'profile',
  'allowed-origins', 'blocked-origins', 'output-dir', 'file-roots', 'caps', 'port', 'host', 'allowed-http-origins',
]);
const BOOL_FLAGS = new Set(['fork', 'isolated', 'no-snapshot-after-action', 'help', 'version']);

/** `pyro mcp` arguments as server options (throws on a bad one). */
export function parseArgs(argv: string[]): Parsed & { help?: boolean; version?: boolean } {
  const values = new Map<string, string>();
  const flags = new Set<string>();
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '-h') { flags.add('help'); continue; }
    if (arg === '-V') { flags.add('version'); continue; }
    const m = /^--([a-z-]+)(?:=(.*))?$/s.exec(arg);
    if (!m) throw new Error(`unexpected argument ${JSON.stringify(arg)} (pyro mcp --help)`);
    const [, name, inline] = m;
    if (BOOL_FLAGS.has(name)) {
      if (inline !== undefined) throw new Error(`--${name} takes no value`);
      flags.add(name);
    } else if (VALUE_FLAGS.has(name)) {
      const value = inline ?? argv[++i];
      if (value === undefined) throw new Error(`--${name} needs a value`);
      values.set(name, value);
    } else {
      throw new Error(`unknown option --${name} (pyro mcp --help)`);
    }
  }
  const int = (name: string) => {
    const v = values.get(name);
    if (v === undefined) return undefined;
    const n = Number(v);
    if (!Number.isInteger(n) || n < 0) throw new Error(`--${name} takes a whole number, got ${JSON.stringify(v)}`);
    return n;
  };
  const browser: BrowserAgentOptions = {};
  if (values.has('ws')) browser.wsEndpoint = values.get('ws');
  if (flags.has('fork')) browser.fork = true;
  const viewport = values.get('viewport');
  if (viewport !== undefined) {
    const vm = /^(\d+)[x×,](\d+)$/i.exec(viewport);
    if (!vm) throw new Error(`--viewport takes <width>x<height>, e.g. 1280x720, got ${JSON.stringify(viewport)}`);
    browser.viewport = { width: Number(vm[1]), height: Number(vm[2]) };
  }
  const dsf = values.get('device-scale-factor');
  if (dsf !== undefined) {
    const n = Number(dsf);
    if (!(n >= 0.5 && n <= 8)) throw new Error(`--device-scale-factor takes a number from 0.5 to 8, got ${JSON.stringify(dsf)}`);
    browser.deviceScaleFactor = n;
  }
  if (values.has('user-agent')) browser.userAgent = values.get('user-agent');
  if (values.has('proxy')) browser.proxy = values.get('proxy');
  if (values.has('timeout-action')) browser.actionTimeout = int('timeout-action');
  if (values.has('timeout-navigation')) browser.navigationTimeout = int('timeout-navigation');
  if (values.has('profile-dir')) browser.profileDir = values.get('profile-dir');
  if (values.has('profile')) browser.profile = values.get('profile');
  if (flags.has('isolated')) browser.isolated = true;
  if (values.has('allowed-origins')) browser.allowedOrigins = originList(values.get('allowed-origins'));
  if (values.has('blocked-origins')) browser.blockedOrigins = originList(values.get('blocked-origins'));
  if (values.has('output-dir')) browser.outputDir = values.get('output-dir');
  if (values.has('file-roots')) browser.fileRoots = values.get('file-roots')!.split(',').map((r) => r.trim()).filter(Boolean);
  if (flags.has('no-snapshot-after-action')) browser.snapshotAfterAction = false;
  const caps = values.get('caps');
  if (caps !== undefined) resolveCaps(caps); // fails early on an unknown group
  return {
    options: { ...browser, ...(caps !== undefined ? { caps } : {}) },
    port: int('port'),
    host: values.get('host'),
    allowedHttpOrigins: values.has('allowed-http-origins') ? originList(values.get('allowed-http-origins')) : undefined,
    help: flags.has('help'),
    version: flags.has('version'),
  };
}

export async function main(argv: string[]): Promise<number> {
  let parsed: ReturnType<typeof parseArgs>;
  try {
    parsed = parseArgs(argv);
  } catch (e) {
    console.error(`pyro mcp: ${e instanceof Error ? e.message : e}`);
    return 2;
  }
  if (parsed.help) {
    console.log(HELP);
    return 0;
  }
  if (parsed.version) {
    console.log(packageVersion());
    return 0;
  }
  if (parsed.options.profile) {
    // A missing startup identity fails here, where the user sees it, not at the agent's first tool call.
    const name = parsed.options.profile;
    try {
      const session = new BrowserAgentSession(parsed.options);
      const { state, login } = session.profiles.read(name);
      const status = state ? sessionStatus(state) : undefined;
      if (status?.expired) console.error(`pyro mcp: --profile ${name}: its session cookies expired ${when(status.expiresAt!)}; sign it in again with: ${session.loginCommand(name, login)}`);
    } catch (e) {
      console.error(`pyro mcp: --profile: ${e instanceof Error ? e.message : e}`);
      return 2;
    }
  }
  if (parsed.port !== undefined) {
    const { serveHttp } = await import('./http.js');
    const http = await serveHttp({ ...parsed.options, port: parsed.port, host: parsed.host, allowedHttpOrigins: parsed.allowedHttpOrigins });
    console.error(`pyro mcp: Streamable HTTP at ${http.url}`);
    await new Promise<void>((resolve) => {
      for (const s of ['SIGINT', 'SIGTERM', 'SIGHUP'] as const) process.once(s, () => resolve());
    });
    await http.close();
    return 0;
  }
  const { serveStdio } = await import('./stdio.js');
  await serveStdio(parsed.options);
  return 0;
}
