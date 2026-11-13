// Playwright's config file, read by the runner: `defineConfig`, `devices`,
// `use` (config, then the selected project), `timeout`, `retries`,
// `expect.timeout`, `outputDir`, `webServer`, `globalSetup` and
// `globalTeardown`, project `dependencies` and `teardown`. The file is
// `$PYRO_CONFIG` (or `$PLAYWRIGHT_CONFIG`), else `pyro.config.*` or
// `playwright.config.*` in the working directory; the project is
// `$PYRO_PROJECT`, else the first.
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { connect } from 'node:net';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import type { FullConfig, FullProject, KeyValue, PlaywrightTestConfig as Typed } from './types.js';

/** The Playwright release whose API this runner follows (`config.version`). */
const PLAYWRIGHT_VERSION = '1.59.1';

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

/** Playwright's device descriptors, as in Playwright. */
export { devices } from './devices.js';

/** Playwright's FullProject (`testInfo.project`), plus what the runner needs. */
export type ResolvedProject = FullProject & {
  /** Its index in `config.projects` (0 without projects). */
  index: number;
  /** The project's `fullyParallel`, else the config's. */
  fullyParallel: boolean;
  /** Where snapshots live (`snapshotDir`, else testDir) and their path template. */
  snapshotPathTemplate?: string;
  /** The config file's directory: snapshot paths resolve against it, as in Playwright. */
  configDir: string;
  expect: Record<string, any>;
};
type Matcher = string | RegExp | (string | RegExp)[];
export type Loaded = {
  config: PlaywrightTestConfig;
  /** The config file (absolute), if there is one. */
  file?: string;
  /** The default project: `$PYRO_PROJECT`, else the first. */
  project: ResolvedProject;
  projects: ResolvedProject[];
  dir: string;
  /** Playwright's FullConfig of this run: `testInfo.config`, and (in a run without `pyro run`) global setup's argument. */
  full: FullConfig;
};

/** What `pyro run` resolved for this run (FullConfig's run-time fields). */
export type RunOptions = {
  workers?: number;
  shard?: { total: number; current: number } | null;
  grep?: RegExp;
  grepInvert?: RegExp;
  maxFailures?: number;
  quiet?: boolean;
  fullyParallel?: boolean;
  updateSnapshots?: FullConfig['updateSnapshots'];
};

const asList = <T>(v: T | T[] | undefined | null): T[] => (v === undefined || v === null ? [] : Array.isArray(v) ? v : [v]);

/** Playwright's FullConfig for `l`, with what the run resolved (`pyro run`'s flags). */
export function fullConfig(l: Omit<Loaded, 'full'>, o: RunOptions = {}): FullConfig {
  const c = l.config;
  const firstOf = (v: string | string[] | undefined) => (asList(v).length ? resolve(l.dir, asList(v)[0]) : null);
  const reporter = (typeof c.reporter === 'string' ? [[c.reporter, undefined]] : Array.isArray(c.reporter) ? c.reporter.map((r) => (typeof r === 'string' ? [r, undefined] : r)) : [['list', undefined]]) as FullConfig['reporter'];
  return {
    configFile: l.file,
    rootDir: c.testDir === undefined ? l.dir : resolve(l.dir, c.testDir as string),
    forbidOnly: !!c.forbidOnly,
    fullyParallel: o.fullyParallel ?? !!c.fullyParallel,
    globalSetup: firstOf(c.globalSetup),
    globalTeardown: firstOf(c.globalTeardown),
    globalTimeout: (c.globalTimeout as number | undefined) ?? 0,
    grep: o.grep ?? (c.grep as RegExp | RegExp[] | undefined) ?? /.*/,
    grepInvert: o.grepInvert ?? (c.grepInvert as RegExp | RegExp[] | undefined) ?? null,
    maxFailures: o.maxFailures ?? (c.maxFailures as number | undefined) ?? 0,
    metadata: { ...((c.metadata as object | undefined) ?? {}) },
    preserveOutput: (c.preserveOutput as FullConfig['preserveOutput'] | undefined) ?? 'always',
    projects: l.projects,
    quiet: o.quiet ?? !!c.quiet,
    reporter,
    reportSlowTests: c.reportSlowTests === null ? null : ((c.reportSlowTests as FullConfig['reportSlowTests']) ?? { max: 5, threshold: 300_000 }),
    shard: o.shard ?? (c.shard as FullConfig['shard'] | undefined) ?? null,
    tags: [],
    updateSnapshots: o.updateSnapshots ?? (c.updateSnapshots as FullConfig['updateSnapshots'] | undefined) ?? 'missing',
    updateSourceMethod: 'patch',
    version: PLAYWRIGHT_VERSION,
    // Playwright gives a lone web server here, and null for a list of them.
    webServer: c.webServer && !Array.isArray(c.webServer) ? ({ ...c.webServer } as FullConfig['webServer']) : null,
    workers: o.workers ?? 1,
  };
}

/** `--shard` / `config.shard` as `{ current, total }` (1-based), or an error message. */
export function parseShard(v: string): { current: number; total: number } | string {
  const m = /^\s*(\d+)\s*\/\s*(\d+)\s*$/.exec(v);
  const current = Number(m?.[1]);
  const total = Number(m?.[2]);
  if (!m || total < 1 || current < 1 || current > total) return `--shard "${v}": expected "current/total", 1-based, for example "3/5"`;
  return { current, total };
}

/**
 * The FullConfig fields a `pyro run` worker gets from its parent
 * (`$PYRO_RUN_WORKERS`, `$PYRO_SHARD`, `$PYRO_GREP`, ...).
 */
function runOptionsFromEnv(): RunOptions {
  const e = process.env;
  const shard = e.PYRO_SHARD ? parseShard(e.PYRO_SHARD) : null;
  return {
    workers: e.PYRO_RUN_WORKERS ? Number(e.PYRO_RUN_WORKERS) : undefined,
    shard: typeof shard === 'string' ? null : shard,
    grep: e.PYRO_GREP ? new RegExp(e.PYRO_GREP) : undefined,
    grepInvert: e.PYRO_GREP_INVERT ? new RegExp(e.PYRO_GREP_INVERT) : undefined,
    updateSnapshots: e.PYRO_UPDATE_SNAPSHOTS ? ((['1', 'true'].includes(e.PYRO_UPDATE_SNAPSHOTS) ? 'changed' : e.PYRO_UPDATE_SNAPSHOTS) as RunOptions['updateSnapshots']) : undefined,
  };
}

/** A project by name, for `dependencies` and `teardown` (names must be unique, as in Playwright). */
function named(projects: ResolvedProject[], name: string, of: ResolvedProject, what: 'dependency' | 'teardown'): ResolvedProject {
  const found = projects.filter((p) => p.name === name);
  if (!found.length) throw new Error(`pyro: project '${of.name}' has unknown ${what === 'teardown' ? 'teardown project' : 'dependency'} '${name}'`);
  if (found.length > 1) throw new Error(`pyro: project ${what === 'teardown' ? 'teardowns' : 'dependencies'} need unique project names, there are ${found.length} named '${name}'`);
  return found[0];
}

/** Playwright's checks of `dependencies` and `teardown`. */
function checkDependencies(projects: ResolvedProject[]): void {
  const teardowns = new Set<ResolvedProject>();
  for (const p of projects) {
    for (const d of p.dependencies) named(projects, d, p, 'dependency');
    if (p.teardown) teardowns.add(named(projects, p.teardown, p, 'teardown'));
  }
  for (const t of teardowns) if (t.dependencies.length) throw new Error(`pyro: teardown project '${t.name}' must not have dependencies`);
  for (const p of projects) {
    for (const d of p.dependencies) {
      const dep = named(projects, d, p, 'dependency');
      if (teardowns.has(dep)) throw new Error(`pyro: project '${p.name}' must not depend on teardown project '${dep.name}'`);
    }
  }
}

export const dependenciesOf = (l: { projects: ResolvedProject[] }, p: ResolvedProject): ResolvedProject[] => p.dependencies.map((d) => named(l.projects, d, p, 'dependency'));
export const teardownOf = (l: { projects: ResolvedProject[] }, p: ResolvedProject): ResolvedProject | undefined => (p.teardown ? named(l.projects, p.teardown, p, 'teardown') : undefined);

/**
 * Playwright's project closure: `roots` are 'top-level', the projects they
 * depend on (and those projects' teardown projects) 'dependency'.
 */
export function projectClosure(l: { projects: ResolvedProject[] }, roots: ResolvedProject[]): Map<ResolvedProject, 'top-level' | 'dependency'> {
  const out = new Map<ResolvedProject, 'top-level' | 'dependency'>();
  const visit = (depth: number, p: ResolvedProject) => {
    if (depth > 100) throw new Error('pyro: circular dependency between projects');
    if (out.get(p) !== 'dependency') out.set(p, depth ? 'dependency' : 'top-level');
    for (const d of dependenciesOf(l, p)) visit(depth + 1, d);
    const t = teardownOf(l, p);
    if (t) visit(depth + 1, t);
  };
  for (const p of roots) visit(0, p);
  // In config order, as Playwright lists them.
  return new Map(l.projects.filter((p) => out.has(p)).map((p) => [p, out.get(p)!]));
}

/**
 * Playwright's phases: each project runs once the projects it depends on
 * finished; a teardown project once every project that depends on its setup
 * project finished. Projects of one phase run together.
 */
export function projectPhases(l: { projects: ResolvedProject[] }, projects: ResolvedProject[]): ResolvedProject[][] {
  const inRun = new Set(projects);
  const dependents = (setups: ResolvedProject[]): Set<ResolvedProject> => {
    const out = new Set<ResolvedProject>();
    const visit = (depth: number, p: ResolvedProject) => {
      if (depth > 100) throw new Error('pyro: circular dependency between projects');
      out.add(p);
      for (const q of projects) if (dependenciesOf(l, q).includes(p)) visit(depth + 1, q);
      const t = teardownOf(l, p);
      if (t) visit(depth + 1, t);
    };
    for (const s of setups) visit(0, s);
    return out;
  };
  const waitFor = new Map<ResolvedProject, ResolvedProject[]>();
  for (const p of projects) {
    const setups = projects.filter((s) => teardownOf(l, s) === p);
    const after = setups.length ? [...dependents(setups)].filter((q) => q !== p) : [];
    waitFor.set(p, [...dependenciesOf(l, p), ...after].filter((q) => inRun.has(q)));
  }
  const phases: ResolvedProject[][] = [];
  const done = new Set<ResolvedProject>();
  while (done.size < projects.length) {
    const phase = projects.filter((p) => !done.has(p) && waitFor.get(p)!.every((q) => done.has(q)));
    if (!phase.length) throw new Error('pyro: circular dependency between projects');
    for (const p of phase) done.add(p);
    phases.push(phase);
  }
  return phases;
}

/** How `pyro run` places a test file in a worker process (`$PYRO_PLAN`). */
export type FilePlan = {
  /** The project's index in `config.projects`. */
  project: number;
  /** The worker (slot) that runs the file's tests that are not parallel. */
  owner: number;
  fullyParallel: boolean;
  /** The run's `--grep`/`--grep-invert` apply (not to a project that runs as a dependency). */
  grep: boolean;
  /** The project's `grep`/`grepInvert` (RegExp sources and flags). */
  projectGrep?: [string, string][];
  projectGrepInvert?: [string, string][];
  /** With `--shard`: whether this shard runs the file's tests that are not parallel, and which parallel ones (by index in the file). */
  shard?: { general: boolean; parallel: number[] };
};
export type RunPlan = { slots: number; files: Record<string, FilePlan> };
let plan: RunPlan | null | undefined = null;
/** The plan of this `pyro run` worker process, or undefined (plain `bun test`). */
export function runPlan(): RunPlan | undefined {
  if (plan === null) plan = process.env.PYRO_PLAN ? (JSON.parse(readFileSync(process.env.PYRO_PLAN, 'utf8')) as RunPlan) : undefined;
  return plan;
}

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

/** The project a test file belongs to: the `pyro run` worker's plan, else the first whose files include it. */
export function projectFor(l: Loaded, file: string): ResolvedProject {
  const i = runPlan()?.files[file]?.project;
  if (i !== undefined && l.projects[i]) return l.projects[i];
  if (process.env.PYRO_PROJECT === undefined && l.projects.length > 1) {
    const found = l.projects.find((p) => file.startsWith(p.testDir + sep) && matches(p.testMatch, file, p.testDir));
    if (found) return found;
  }
  return l.project;
}

function configFile(): string | undefined {
  const named = process.env.PYRO_CONFIG ?? process.env.PLAYWRIGHT_CONFIG;
  if (named) return isAbsolute(named) ? named : resolve(process.cwd(), named);
  for (const base of ['pyro.config', 'playwright.config']) {
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
    dependencies: [...((p.dependencies as string[] | undefined) ?? [])],
    ...(p.teardown ? { teardown: p.teardown as string } : {}),
    fullyParallel: !!(p.fullyParallel ?? config.fullyParallel),
    grep: (p.grep ?? config.grep ?? /.*/) as RegExp | RegExp[],
    grepInvert: (p.grepInvert ?? config.grepInvert ?? null) as RegExp | RegExp[] | null,
    ignoreSnapshots: !!(p.ignoreSnapshots ?? config.ignoreSnapshots),
    repeatEach: (p.repeatEach ?? config.repeatEach ?? 1) as number,
    outputDir: resolve(dir, (p.outputDir ?? config.outputDir ?? 'test-results') as string),
    testDir: resolve(dir, (p.testDir ?? config.testDir ?? '.') as string),
    testMatch: (p.testMatch ?? config.testMatch ?? DEFAULT_MATCH) as Matcher,
    testIgnore: (p.testIgnore ?? config.testIgnore ?? []) as Matcher,
    timeout: (p.timeout ?? config.timeout ?? 30_000) as number,
    retries: Number(process.env.PYRO_RETRIES ?? p.retries ?? config.retries ?? 0),
    metadata: { ...((config.metadata as object) ?? {}), ...(p.metadata ?? {}) },
    snapshotDir: resolve(dir, (p.snapshotDir ?? config.snapshotDir ?? p.testDir ?? config.testDir ?? '.') as string),
    snapshotPathTemplate: (p.snapshotPathTemplate ?? config.snapshotPathTemplate) as string | undefined,
    configDir: dir,
    // Playwright's takeFirst: a project's `expect` replaces the top-level one.
    expect: { ...(((p.expect ?? config.expect) as object) ?? {}) },
  });
  const projects = config.projects?.length ? config.projects.map(resolveProject) : [resolveProject({}, 0)];
  checkDependencies(projects);
  const want = process.env.PYRO_PROJECT;
  const project = want === undefined ? projects[0] : projects.find((p) => p.name === want);
  if (!project) throw new Error(`pyro: no project named ${JSON.stringify(want)} in ${file}`);
  if (process.env.PYRO_TIMEOUT) for (const p of projects) p.timeout = Number(process.env.PYRO_TIMEOUT);
  const l = { config, file, project, projects, dir };
  return { ...l, full: fullConfig(l, runOptionsFromEnv()) };
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
    if (!s.url && !s.port) throw new Error('pyro: webServer needs url or port');
    if (await reachable(s)) {
      if (s.reuseExistingServer !== false) continue;
      throw new Error(`pyro: ${s.url ?? `port ${s.port}`} is already used; set webServer.reuseExistingServer`);
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
      if (child.exitCode !== null) throw new Error(`pyro: webServer "${s.command}" exited with ${child.exitCode}`);
      if (Date.now() > deadline) throw new Error(`pyro: webServer "${s.command}" not ready after ${s.timeout ?? 60_000} ms`);
      await new Promise((r) => setTimeout(r, 100));
    }
  }
}

/**
 * Runs `globalSetup` with Playwright's FullConfig (`full`, else the loaded
 * one) and returns the teardowns: functions it returned, then `globalTeardown`
 * (called with the same FullConfig).
 */
export async function globalSetup(l: Loaded, full: FullConfig = l.full): Promise<(() => Promise<unknown>)[]> {
  const teardowns: (() => Promise<unknown>)[] = [];
  const load = async (f: string) => {
    const mod = await import(pathToFileURL(resolve(l.dir, f)).href);
    return (mod.default ?? mod) as (config: FullConfig) => unknown;
  };
  for (const f of asList(l.config.globalSetup)) {
    const r = await (await load(f))(full);
    if (typeof r === 'function') teardowns.push(async () => r(full));
  }
  for (const f of asList(l.config.globalTeardown)) teardowns.push(async () => (await load(f))(full));
  return teardowns;
}
