// Playwright's config file, read by the runner: `defineConfig`, `devices`,
// `use` (config, then the selected project), `timeout`, `retries`,
// `expect.timeout`, `outputDir`, `webServer`, `globalSetup` and
// `globalTeardown`. The file is `$LP_CONFIG` (or `$PLAYWRIGHT_CONFIG`), else
// `lightningpanda.config.*` or `playwright.config.*` in the working
// directory; the project is `$LP_PROJECT`, else the first.
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { existsSync, readdirSync, statSync } from 'node:fs';
import { connect } from 'node:net';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import type { KeyValue, PlaywrightTestConfig as Typed } from './types.js';

export type WebServer = {
  command: string;
  url?: string;
  port?: number;
  reuseExistingServer?: boolean;
  timeout?: number;
  cwd?: string;
  env?: Record<string, string>;
  stdout?: 'pipe' | 'ignore';
  stderr?: 'pipe' | 'ignore';
  ignoreHTTPSErrors?: boolean;
};
export type Project = { name?: string; use?: Record<string, unknown>; timeout?: number; retries?: number; testDir?: string; outputDir?: string; metadata?: Record<string, unknown>; [k: string]: unknown };
export type PlaywrightTestConfig = Project & {
  projects?: Project[];
  expect?: {
    timeout?: number;
    /** Playwright's `expect.toMatchAriaSnapshot`: the default `/children` mode and the snapshot path template. */
    toMatchAriaSnapshot?: { children?: 'contain' | 'equal' | 'deep-equal'; pathTemplate?: string };
    [k: string]: unknown;
  };
  webServer?: WebServer | WebServer[];
  globalSetup?: string | string[];
  globalTeardown?: string | string[];
  fullyParallel?: boolean;
  workers?: number | string;
  reporter?: unknown;
  [k: string]: unknown;
};

/** Playwright's `defineConfig`: later configs override earlier ones (`use` and `expect` merged). */
export function defineConfig<T extends KeyValue = {}, W extends KeyValue = {}>(config: Typed<T, W>, ...overrides: Typed<T, W>[]): Typed<T, W> {
  return [config, ...overrides].reduce<Typed<T, W>>((a, b) => ({ ...a, ...b, use: { ...(a.use ?? {}), ...(b.use ?? {}) } as Typed<T, W>['use'], expect: { ...(a.expect ?? {}), ...(b.expect ?? {}) } }), {} as Typed<T, W>);
}

const chromeUA = (v: string) => `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/${v} Safari/537.36`;
/** A few of Playwright's device descriptors (viewport, user agent, scale, touch). */
export const devices: Record<string, Record<string, unknown>> = {
  'Desktop Chrome': { userAgent: chromeUA('147.0.0.0'), viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1, isMobile: false, hasTouch: false, defaultBrowserType: 'chromium' },
  'Desktop Chrome HiDPI': { userAgent: chromeUA('147.0.0.0'), viewport: { width: 1280, height: 720 }, deviceScaleFactor: 2, isMobile: false, hasTouch: false, defaultBrowserType: 'chromium' },
  'Desktop Edge': { userAgent: `${chromeUA('147.0.0.0')} Edg/147.0.0.0`, viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1, isMobile: false, hasTouch: false, defaultBrowserType: 'chromium' },
  'Pixel 7': { userAgent: 'Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Mobile Safari/537.36', viewport: { width: 412, height: 839 }, deviceScaleFactor: 2.625, isMobile: true, hasTouch: true, defaultBrowserType: 'chromium' },
  'iPhone 15': { userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1', viewport: { width: 393, height: 659 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true, defaultBrowserType: 'webkit' },
};

export type ResolvedProject = {
  name: string;
  /** Its index in `config.projects` (0 without projects). */
  index: number;
  use: Record<string, unknown>;
  outputDir: string;
  testDir: string;
  testMatch: Matcher;
  testIgnore: Matcher;
  timeout: number;
  retries: number;
  metadata: Record<string, unknown>;
  /** Where snapshots live (`snapshotDir`, else testDir) and their path template. */
  snapshotDir: string;
  snapshotPathTemplate?: string;
  /** The config file's directory: snapshot paths resolve against it, as in Playwright. */
  configDir: string;
  expect: Record<string, any>;
};
type Matcher = string | RegExp | (string | RegExp)[];
export type Loaded = {
  config: PlaywrightTestConfig;
  /** The default project: `$LP_PROJECT`, else the first. */
  project: ResolvedProject;
  projects: ResolvedProject[];
  dir: string;
};

const DEFAULT_MATCH = '**/*.@(spec|test).?(c|m)[jt]s?(x)';

/** A Playwright glob (`**`, `*`, `?`, `{a,b}`, `@(a|b)`, `?(a)`, `[..]`) as a RegExp. */
export function glob(pattern: string): RegExp {
  let out = '';
  for (let i = 0; i < pattern.length; i++) {
    const c = pattern[i];
    const next = pattern[i + 1];
    if (c === '*' && next === '*') {
      i++;
      if (pattern[i + 1] === '/') {
        i++;
        out += '(?:.*/)?';
      } else out += '.*';
    } else if (c === '*') out += '[^/]*';
    else if ((c === '@' || c === '?' || c === '+' || c === '!') && next === '(') {
      const close = pattern.indexOf(')', i);
      const alts = pattern.slice(i + 2, close).split('|').map((a) => glob(a).source.slice(1, -1)).join('|');
      out += c === '@' ? `(?:${alts})` : c === '?' ? `(?:${alts})?` : c === '+' ? `(?:${alts})+` : `(?!(?:${alts}))[^/]*`;
      i = close;
    } else if (c === '?') out += '[^/]';
    else if (c === '{') {
      const close = pattern.indexOf('}', i);
      out += `(?:${pattern.slice(i + 1, close).split(',').map((a) => glob(a).source.slice(1, -1)).join('|')})`;
      i = close;
    } else if (c === '[') {
      const close = pattern.indexOf(']', i);
      out += pattern.slice(i, close + 1);
      i = close;
    } else out += c.replace(/[.+^$()|\\]/g, '\\$&');
  }
  return new RegExp(`^${out}$`);
}

/** Whether `file` (absolute) matches a project's `testMatch`/`testIgnore`: a pattern without `/` tests the file name. */
function matches(m: Matcher, file: string, testDir: string): boolean {
  const rel = relative(testDir, file).split(sep).join('/');
  return ([] as (string | RegExp)[]).concat(m).some((p) => {
    if (p instanceof RegExp) return p.test(file);
    const re = glob(p);
    return p.includes('/') ? re.test(rel) || re.test(file) : re.test(basename(file));
  });
}

/** Test files of `project` under its testDir. */
export function projectFiles(p: ResolvedProject): string[] {
  const out: string[] = [];
  const walkDir = (dir: string) => {
    for (const name of readdirSync(dir).sort()) {
      if (name === 'node_modules' || name.startsWith('.')) continue;
      const f = join(dir, name);
      const st = statSync(f, { throwIfNoEntry: false });
      if (st?.isDirectory()) {
        if (resolve(f) !== resolve(p.outputDir)) walkDir(f);
      } else if (st?.isFile() && matches(p.testMatch, f, p.testDir) && !(p.testIgnore && matches(p.testIgnore, f, p.testDir))) out.push(f);
    }
  };
  if (existsSync(p.testDir)) walkDir(p.testDir);
  return out;
}

/** The project a test file belongs to: `$LP_FILE_PROJECTS` (set by `lightningpanda test`), else the first whose files include it. */
export function projectFor(l: Loaded, file: string): ResolvedProject {
  const map = process.env.LP_FILE_PROJECTS;
  if (map) {
    const i = (JSON.parse(map) as Record<string, number>)[file];
    if (i !== undefined && l.projects[i]) return l.projects[i];
  }
  if (process.env.LP_PROJECT === undefined && l.projects.length > 1) {
    const found = l.projects.find((p) => file.startsWith(p.testDir + sep) && matches(p.testMatch, file, p.testDir));
    if (found) return found;
  }
  return l.project;
}

function configFile(): string | undefined {
  const named = process.env.LP_CONFIG ?? process.env.PLAYWRIGHT_CONFIG;
  if (named) return isAbsolute(named) ? named : resolve(process.cwd(), named);
  for (const base of ['lightningpanda.config', 'playwright.config']) {
    for (const ext of ['ts', 'mts', 'mjs', 'js']) {
      const f = join(process.cwd(), `${base}.${ext}`);
      if (existsSync(f)) return f;
    }
  }
  return undefined;
}

export async function loadConfig(): Promise<Loaded> {
  const file = configFile();
  const config: PlaywrightTestConfig = file ? ((await import(pathToFileURL(file).href)).default ?? {}) : {};
  const dir = file ? dirname(file) : process.cwd();
  const resolveProject = (p: Project, index: number): ResolvedProject => ({
    name: p.name ?? '',
    index,
    use: { ...(config.use ?? {}), ...(p.use ?? {}) },
    outputDir: resolve(dir, (p.outputDir ?? config.outputDir ?? 'test-results') as string),
    testDir: resolve(dir, (p.testDir ?? config.testDir ?? '.') as string),
    testMatch: (p.testMatch ?? config.testMatch ?? DEFAULT_MATCH) as Matcher,
    testIgnore: (p.testIgnore ?? config.testIgnore ?? []) as Matcher,
    timeout: (p.timeout ?? config.timeout ?? 30_000) as number,
    retries: Number(process.env.LP_RETRIES ?? p.retries ?? config.retries ?? 0),
    metadata: { ...((config.metadata as object) ?? {}), ...(p.metadata ?? {}) },
    snapshotDir: resolve(dir, (p.snapshotDir ?? config.snapshotDir ?? p.testDir ?? config.testDir ?? '.') as string),
    snapshotPathTemplate: (p.snapshotPathTemplate ?? config.snapshotPathTemplate) as string | undefined,
    configDir: dir,
    // Playwright's takeFirst: a project's `expect` replaces the top-level one.
    expect: { ...(((p.expect ?? config.expect) as object) ?? {}) },
  });
  const projects = config.projects?.length ? config.projects.map(resolveProject) : [resolveProject({}, 0)];
  const want = process.env.LP_PROJECT;
  const project = want === undefined ? projects[0] : projects.find((p) => p.name === want);
  if (!project) throw new Error(`lightning-panda: no project named ${JSON.stringify(want)} in ${file}`);
  if (process.env.LP_TIMEOUT) for (const p of projects) p.timeout = Number(process.env.LP_TIMEOUT);
  return { config, project, projects, dir };
}

const children: ChildProcess[] = [];
/** Stops the web servers (their process groups). */
export function stopWebServers(): void {
  for (const c of children.splice(0)) {
    if (!c.pid || c.exitCode !== null) continue;
    try {
      process.kill(-c.pid, 'SIGTERM');
    } catch {
      c.kill();
    }
  }
}

/**
 * Stops process group `pid` once this process is gone. `bun test` runs no
 * `exit` handlers, so a run that ends early (or crashes) would leave it behind.
 */
export function stopWithThisProcess(pid: number | undefined): void {
  if (!pid || process.platform === 'win32') return;
  const w = spawn('/bin/sh', ['-c', `while kill -0 ${process.pid} 2>/dev/null; do sleep 1; done; kill -TERM -${pid} 2>/dev/null || kill -TERM ${pid} 2>/dev/null`], {
    detached: true,
    stdio: 'ignore',
  });
  w.unref();
}

const TEST_FILE = /(\.|_)(test|spec)\.(c|m)?[jt]sx?$/;
function walk(dir: string, out: string[]): void {
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules' || name.startsWith('.')) continue;
    const p = join(dir, name);
    const st = statSync(p, { throwIfNoEntry: false });
    if (st?.isDirectory()) walk(p, out);
    else if (st?.isFile() && TEST_FILE.test(name)) out.push(p);
  }
}

/**
 * The files this `bun test` run loads, from its command line: named files,
 * test files under named directories, and default discovery filtered by the
 * other arguments (as Bun does). `undefined` when it cannot tell.
 */
export function runFiles(): Set<string> | undefined {
  let args: string[];
  try {
    args = execFileSync('ps', ['-o', 'args=', '-p', String(process.pid)], { encoding: 'utf8' }).trim().split(/\s+/);
  } catch {
    return undefined;
  }
  const at = args.indexOf('test');
  if (at < 0) return undefined;
  const valued = new Set(['-t', '--test-name-pattern', '--timeout', '--preload', '-r', '--rerun-each', '--reporter', '--reporter-outfile', '--coverage-reporter', '--coverage-dir', '--bail', '--seed', '--max-concurrency', '--path-ignore-patterns']);
  const paths: string[] = [];
  for (let i = at + 1; i < args.length; i++) {
    const a = args[i];
    if (a.startsWith('-')) {
      if (valued.has(a)) i++;
      continue;
    }
    paths.push(a);
  }
  const files = new Set<string>();
  const filters: string[] = [];
  for (const a of paths) {
    const p = resolve(process.cwd(), a);
    const st = statSync(p, { throwIfNoEntry: false });
    if (st?.isFile()) files.add(p);
    else if (st?.isDirectory()) {
      const found: string[] = [];
      walk(p, found);
      for (const f of found) files.add(f);
    } else filters.push(a);
  }
  if (filters.length || !paths.length) {
    const found: string[] = [];
    walk(process.cwd(), found);
    for (const f of found) if (!filters.length || filters.some((x) => f.includes(x))) files.add(f);
  }
  return files;
}

async function reachable(s: WebServer): Promise<boolean> {
  if (s.url) {
    try {
      const r = await fetch(s.url, { redirect: 'manual', signal: AbortSignal.timeout(2_000) });
      return r.status < 404 || r.status === 405;
    } catch {
      return false;
    }
  }
  return new Promise((ok) => {
    const sock = connect(s.port!, '127.0.0.1');
    sock.once('connect', () => (sock.destroy(), ok(true)));
    sock.once('error', () => ok(false));
  });
}

/** Starts the config's web servers (or reuses running ones) and waits for each. */
export async function startWebServers(l: Loaded): Promise<void> {
  const list = l.config.webServer === undefined ? [] : Array.isArray(l.config.webServer) ? l.config.webServer : [l.config.webServer];
  for (const s of list) {
    if (!s.url && !s.port) throw new Error('lightning-panda: webServer needs url or port');
    if (await reachable(s)) {
      if (s.reuseExistingServer !== false) continue;
      throw new Error(`lightning-panda: ${s.url ?? `port ${s.port}`} is already used; set webServer.reuseExistingServer`);
    }
    const child = spawn(s.command, {
      shell: true,
      cwd: s.cwd ? resolve(l.dir, s.cwd) : l.dir,
      env: { ...process.env, ...(s.env ?? {}) },
      stdio: ['ignore', s.stdout === 'pipe' ? 'inherit' : 'ignore', s.stderr === 'ignore' ? 'ignore' : 'inherit'],
      detached: process.platform !== 'win32',
    });
    children.push(child);
    stopWithThisProcess(child.pid);
    const deadline = Date.now() + (s.timeout ?? 60_000);
    while (!(await reachable(s))) {
      if (child.exitCode !== null) throw new Error(`lightning-panda: webServer "${s.command}" exited with ${child.exitCode}`);
      if (Date.now() > deadline) throw new Error(`lightning-panda: webServer "${s.command}" not ready after ${s.timeout ?? 60_000} ms`);
      await new Promise((r) => setTimeout(r, 100));
    }
  }
}

/** Runs `globalSetup` and returns the teardowns (`globalTeardown`, functions it returned). */
export async function globalSetup(l: Loaded): Promise<(() => Promise<unknown>)[]> {
  const teardowns: (() => Promise<unknown>)[] = [];
  const load = async (f: string) => {
    const mod = await import(pathToFileURL(resolve(l.dir, f)).href);
    return (mod.default ?? mod) as (config: unknown) => unknown;
  };
  for (const f of ([] as string[]).concat(l.config.globalSetup ?? [])) {
    const r = await (await load(f))(l.config);
    if (typeof r === 'function') teardowns.push(async () => r(l.config));
  }
  for (const f of ([] as string[]).concat(l.config.globalTeardown ?? [])) teardowns.push(async () => (await load(f))(l.config));
  return teardowns;
}
