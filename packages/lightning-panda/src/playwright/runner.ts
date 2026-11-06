// Playwright's test runner API on `bun test`: `test`, `test.describe`,
// hooks, fixtures (`test.extend`, `test.use`, options, worker scope, auto),
// `test.step`, `test.info()`, skip/fixme/fail/slow, retries and timeouts,
// read from the Playwright config file (see config.ts).
//
// Bun registers and runs the tests; this module resolves their fixtures,
// runs their hooks, applies the timeout and retries, and reports through
// Bun. Tests of a file run one after another (a describe's `mode:
// 'parallel'` too, as its tests may share module state set in hooks);
// `LP_CONCURRENT=1` runs those in parallel describes concurrently.
import { appendFileSync } from 'node:fs';
import { relative } from 'node:path';
import { Lightningpanda } from '../index.js';
import { Browser, chromium, setTestDefaults, type BootSpec, type ContextOptions } from './browser.js';
import { als, currentTest, SkipError, TestInfo } from './context.js';
import { devices, globalSetup, loadConfig, projectFor, runFiles, startWebServers, stopWebServers, stopWithThisProcess, type Loaded, type ResolvedProject } from './config.js';
import { expect, ready, setExpectDefaults } from './expect.js';
import { TimeoutError } from './page.js';
import { APIRequestContext, request } from './request.js';

type BunTest = typeof import('bun:test');
// Synchronous: no top-level await, so CommonJS loaders can import this module too.
const bun: BunTest | undefined = (globalThis as any).Bun ? require('bun:test') : undefined;

// ---- config, web servers, global setup: once per process ----

let loaded: Loaded | undefined;
let loading: Promise<Loaded> | undefined;
let teardowns: (() => Promise<unknown>)[] = [];
/** The run's files, and those done: the last one's afterAll shuts the run down. */
let expected: Set<string> | undefined | null = null;
const finished = new Set<string>();
/** Config, web servers and global setup, once per process, before the first file's tests. */
function ensureLoaded(): Promise<Loaded> {
  return (loading ??= (async () => {
    const l = await loadConfig();
    setExpectDefaults({ timeout: l.config.expect?.timeout });
    // A worker of `lightningpanda test`: the parent started the servers and ran global setup.
    if (!WORKER) {
      await startWebServers(l);
      teardowns = await globalSetup(l);
    }
    await ready;
    loaded = l;
    return l;
  })());
}

const BIG = 2_147_483_647;

/** `lightningpanda test` runs this process as worker LP_WORKER_INDEX of LP_WORKERS. */
const WORKER = process.env.LP_WORKER === '1';
const WORKERS = Math.max(1, Number(process.env.LP_WORKERS ?? 1));
const WORKER_INDEX = Number(process.env.LP_WORKER_INDEX ?? 0);
const FULLY_PARALLEL = process.env.LP_FULLY_PARALLEL === '1';
const GREP = process.env.LP_GREP ? new RegExp(process.env.LP_GREP) : undefined;
const GREP_INVERT = process.env.LP_GREP_INVERT ? new RegExp(process.env.LP_GREP_INVERT) : undefined;
const LIST = process.env.LP_LIST === '1';
const OWNERS: Record<string, number> = process.env.LP_FILE_OWNERS ? JSON.parse(process.env.LP_FILE_OWNERS) : {};

/** One line per finished test for `lightningpanda test`'s reporter (`$LP_RESULTS`). */
type Outcome = { file: string; titlePath: string[]; status: 'passed' | 'failed' | 'timedOut' | 'skipped'; flaky?: boolean; expectedFailure?: boolean; duration: number; retries: number; error?: { message: string; stack?: string }; skipReason?: string; worker: number };
function record(o: Omit<Outcome, 'worker'>): void {
  const out = process.env.LP_RESULTS;
  if (out) appendFileSync(out, JSON.stringify({ ...o, worker: WORKER_INDEX }) + '\n');
}
const cfg = (): Loaded => {
  if (!loaded) throw new Error('lightning-panda: the test runner needs `bun test`');
  return loaded;
};

// ---- fixtures ----

type FixtureFn = (args: any, use: (value: any) => Promise<void>, info: any) => any;
type Def = { fn?: FixtureFn; value?: unknown; scope: 'test' | 'worker'; auto: boolean; option: boolean; timeout?: number };
type Defs = Map<string, Def[]>;

/** Names a fixture function destructures from its first parameter (as Playwright reads them). */
function paramNames(fn: Function): string[] {
  const src = fn.toString();
  const open = src.indexOf('(');
  const arrow = src.indexOf('=>');
  let i = open < 0 || (arrow >= 0 && arrow < open) ? -1 : open + 1;
  if (i < 0) return [];
  while (/\s/.test(src[i])) i++;
  if (src[i] !== '{') return [];
  let depth = 0;
  let j = i;
  for (; j < src.length; j++) {
    if (src[j] === '{' || src[j] === '[' || src[j] === '(') depth++;
    else if (src[j] === '}' || src[j] === ']' || src[j] === ')') {
      depth--;
      if (depth === 0) break;
    }
  }
  const body = src.slice(i + 1, j);
  const names: string[] = [];
  let d = 0;
  let cur = '';
  for (const c of body + ',') {
    if ('{[('.includes(c)) d++;
    if ('}])'.includes(c)) d--;
    if (c === ',' && d === 0) {
      const name = cur.trim().replace(/^\.\.\./, '').split(/[:=]/)[0].trim();
      if (name && !cur.trim().startsWith('...')) names.push(name);
      cur = '';
    } else cur += c;
  }
  return names;
}

function toDef(v: unknown, inherit?: Def): Def {
  if (Array.isArray(v) && v.length === 2 && v[1] && typeof v[1] === 'object' && !Array.isArray(v[1])) {
    const [inner, o] = v as [unknown, { scope?: 'test' | 'worker'; auto?: boolean; option?: boolean; timeout?: number }];
    return { ...(typeof inner === 'function' ? { fn: inner as FixtureFn } : { value: inner }), scope: o.scope ?? inherit?.scope ?? 'test', auto: !!o.auto, option: !!o.option, timeout: o.timeout };
  }
  return { ...(typeof v === 'function' ? { fn: v as FixtureFn } : { value: v }), scope: inherit?.scope ?? 'test', auto: inherit?.auto ?? false, option: inherit?.option ?? false };
}

/** Worker-scope fixture values, kept for the file (built-in browsers for the process). */
const workerValues = new Map<string, { value: unknown; teardown: () => Promise<void> }>();

class Run {
  private values = new Map<string, Promise<unknown>>();
  private teardowns: (() => Promise<void>)[] = [];
  constructor(
    private defs: Defs,
    private use: Record<string, unknown>,
    private info: TestInfo | undefined,
    /** beforeAll/afterAll: worker fixtures only. */
    private workerOnly = false,
  ) {}

  private def(name: string, level?: number): { def: Def; level: number } {
    const stack = this.defs.get(name);
    if (!stack?.length) throw new Error(`Fixture "${name}" is not defined (test.extend() declares fixtures)`);
    const l = level ?? stack.length - 1;
    if (l < 0) throw new Error(`Fixture "${name}" has no definition to extend`);
    return { def: stack[l], level: l };
  }

  /** The value of fixture `name` (at definition `level`; a same-name dependency is the one below). */
  get(name: string, level?: number): Promise<unknown> {
    const { def, level: l } = this.def(name, level);
    if (def.scope === 'test' && this.workerOnly) throw new Error(`"${name}" is a test fixture: beforeAll and afterAll get worker fixtures only`);
    // An option takes the value `use` gives it (config, project, test.use).
    if (l === this.defs.get(name)!.length - 1 && name in this.use) return Promise.resolve(this.use[name]);
    if (def.scope === 'worker') return this.worker(name, def, l);
    const key = `${name}@${l}`;
    let v = this.values.get(key);
    if (!v) this.values.set(key, (v = this.setUp(name, def, l, (t) => this.teardowns.push(t))));
    return v;
  }

  private worker(name: string, def: Def, level: number): Promise<unknown> {
    const options = [...this.defs].filter(([, s]) => s[s.length - 1].option && s[s.length - 1].scope === 'worker').map(([n]) => [n, this.use[n]]);
    const key = `${name}@${level}:${JSON.stringify(options)}`;
    const have = workerValues.get(key);
    if (have) return Promise.resolve(have.value);
    let v = this.values.get(key);
    if (!v) {
      this.values.set(
        key,
        (v = this.setUp(name, def, level, (teardown) => {}, (value, teardown) => workerValues.set(key, { value, teardown }))),
      );
    }
    return v;
  }

  private async setUp(name: string, def: Def, level: number, onTeardown: (t: () => Promise<void>) => void, worker?: (v: unknown, t: () => Promise<void>) => void): Promise<unknown> {
    if (!def.fn) {
      worker?.(def.value, async () => {});
      return def.value;
    }
    const args: Record<string, unknown> = {};
    for (const dep of paramNames(def.fn)) args[dep] = await this.get(dep, dep === name ? level - 1 : undefined);
    let provide!: (v: unknown) => void;
    const provided = new Promise<unknown>((r) => (provide = r));
    let release!: () => void;
    const released = new Promise<void>((r) => (release = r));
    const second = def.scope === 'worker' ? { workerIndex: WORKER_INDEX, parallelIndex: WORKER_INDEX, project: this.info?.project ?? cfg().project, config: cfg().config } : this.info;
    let used = false;
    const done = Promise.resolve()
      .then(() =>
        def.fn!(args, async (value: unknown) => {
          used = true;
          provide(value);
          await released;
        }, second),
      )
      .then(() => {
        if (!used) provide(undefined);
      });
    const value = await Promise.race([provided, done.then(() => provided)]);
    const teardown = async () => {
      release();
      await done;
    };
    if (worker) worker(value, teardown);
    else onTeardown(teardown);
    return value;
  }

  async autos(): Promise<void> {
    for (const [name, stack] of this.defs) {
      const top = stack[stack.length - 1];
      if (top.auto && (!this.workerOnly || top.scope === 'worker')) await this.get(name);
    }
  }

  /** `fn(fixtures, testInfo)` with the fixtures it destructures. */
  async call(fn: Function): Promise<unknown> {
    const args: Record<string, unknown> = {};
    for (const name of paramNames(fn)) args[name] = await this.get(name);
    return fn(args, this.info);
  }

  async teardown(): Promise<void> {
    const errors: unknown[] = [];
    for (const t of this.teardowns.splice(0).reverse()) {
      try {
        await t();
      } catch (e) {
        errors.push(e);
      }
    }
    if (errors.length) throw errors[0];
  }
}

// ---- built-in fixtures ----

/** After the run's last file: global teardown, browsers closed, web servers stopped. */
async function shutdown(): Promise<void> {
  for (const t of teardowns.splice(0)) await t();
  for (const lp of launched) lp.close();
  launched.clear();
  browsers.clear();
  stopWebServers();
}

/** One lightningpanda per process (per `lightningpanda` option), and one Browser per boot config on it. */
const browsers = new Map<string, Promise<{ lp: Lightningpanda; browser: Browser }>>();
const launched = new Set<Lightningpanda>();
// Synchronous: `exit` runs no promise callbacks. `close()` kills the launched process group.
process.on('exit', () => {
  for (const lp of launched) lp.close();
});
async function sharedBrowser(o: { wsEndpoint?: string; bin?: string; args?: string[]; boot?: BootSpec[] }): Promise<Browser> {
  const key = JSON.stringify(o);
  let b = browsers.get(key);
  if (!b) {
    b = (async () => {
      const ws = o.wsEndpoint ?? process.env.LIGHTNINGPANDA_WS;
      const lp = ws ? await Lightningpanda.connect(ws) : await Lightningpanda.launch({ bin: o.bin, args: o.args });
      launched.add(lp);
      stopWithThisProcess((lp as any).child?.pid);
      return { lp, browser: new Browser(lp, false, o.boot ?? []) };
    })();
    browsers.set(key, b);
  }
  return (await b).browser;
}

const CONTEXT_OPTIONS = [
  'acceptDownloads', 'bypassCSP', 'colorScheme', 'deviceScaleFactor', 'extraHTTPHeaders', 'geolocation', 'hasTouch', 'httpCredentials',
  'ignoreHTTPSErrors', 'isMobile', 'javaScriptEnabled', 'locale', 'offline', 'permissions', 'proxy', 'storageState', 'timezoneId',
  'userAgent', 'viewport', 'baseURL', 'serviceWorkers', 'reducedMotion',
] as const;

const builtins: Record<string, unknown> = {
  lightningpanda: [{}, { scope: 'worker', option: true }],
  browserName: ['chromium', { scope: 'worker', option: true }],
  defaultBrowserType: ['chromium', { scope: 'worker', option: true }],
  headless: [true, { scope: 'worker', option: true }],
  channel: [undefined, { scope: 'worker', option: true }],
  launchOptions: [{}, { scope: 'worker', option: true }],
  connectOptions: [undefined, { scope: 'worker', option: true }],
  screenshot: ['off', { scope: 'worker', option: true }],
  video: ['off', { scope: 'worker', option: true }],
  trace: ['off', { scope: 'worker', option: true }],
  playwright: [{ chromium, request, devices, errors: { TimeoutError }, selectors: { setTestIdAttribute: () => {}, register: async () => {} } }, { scope: 'worker' }],
  browser: [async ({ lightningpanda }: any, use: any) => use(await sharedBrowser(lightningpanda ?? {})), { scope: 'worker' }],
  ...Object.fromEntries(CONTEXT_OPTIONS.map((n) => [n, [n === 'viewport' ? { width: 1280, height: 720 } : n === 'acceptDownloads' ? true : undefined, { option: true }]])),
  contextOptions: [{}, { option: true }],
  actionTimeout: [0, { option: true }],
  navigationTimeout: [0, { option: true }],
  testIdAttribute: ['data-testid', { option: true }],
  // The use options `browser.newContext()` starts from, as in Playwright (set below).
  _contextDefaults: [undefined, { auto: true }],
  context: async ({ browser, _contextDefaults }: any, use: any) => {
    void _contextDefaults;
    const context = await (browser as Browser).newContext();
    await use(context);
    await context.close();
  },
  page: async ({ context }: any, use: any) => use(await context.newPage()),
  request: async ({ baseURL, extraHTTPHeaders, httpCredentials, ignoreHTTPSErrors, userAgent, storageState }: any, use: any) => {
    const r = new APIRequestContext({ baseURL, extraHTTPHeaders, httpCredentials, ignoreHTTPSErrors, userAgent, storageState });
    await use(r);
    await r.dispose();
  },
};
// The `_contextDefaults` fixture reads every option: its parameter list is spelled out.
(builtins._contextDefaults as [FixtureFn, object])[0] = new Function(
  'setTestDefaults',
  'CONTEXT_OPTIONS',
  `return async ({ ${[...CONTEXT_OPTIONS, 'contextOptions', 'actionTimeout', 'navigationTimeout', 'testIdAttribute'].join(', ')} }, use) => {
    const o = { ${[...CONTEXT_OPTIONS, 'contextOptions', 'actionTimeout', 'navigationTimeout', 'testIdAttribute'].join(', ')} };
    const options = { ...(o.contextOptions ?? {}) };
    for (const n of CONTEXT_OPTIONS) if (o[n] !== undefined) options[n] = o[n];
    setTestDefaults({ options, timeout: o.actionTimeout || undefined, navigationTimeout: o.navigationTimeout || undefined, testIdAttribute: o.testIdAttribute || 'data-testid' });
    await use();
    setTestDefaults(undefined);
  }`,
)(setTestDefaults, CONTEXT_OPTIONS) as FixtureFn;

// ---- describe scopes ----

type Hook = { kind: 'beforeEach' | 'afterEach' | 'beforeAll' | 'afterAll'; fn: Function; title?: string };
type Condition = { cond: boolean | Function | undefined; reason?: string; kind: 'skip' | 'fixme' | 'fail' | 'slow' };
type Scope = {
  title: string;
  parent?: Scope;
  file: string;
  use: Record<string, unknown>;
  hooks: Hook[];
  conditions: Condition[];
  mode?: 'default' | 'serial' | 'parallel';
  retries?: number;
  timeout?: number;
  failed?: boolean;
  defs?: Defs;
  workerRun?: Run;
  /** Tests this process runs in the scope and its describes (its beforeAll/afterAll run only then). */
  registered?: number;
};

const roots = new Map<string, Scope>();
const stack: Scope[] = [];

/** The test file calling into the runner (the first stack frame outside this package). */
function callerFile(): string {
  const here = import.meta.url.replace(/^file:\/\//, '').replace(/[^/]*$/, '');
  for (const line of (new Error().stack ?? '').split('\n').slice(1)) {
    const m = /(?:\(|at )(?:file:\/\/)?(\/[^():]+):\d+(?::\d+)?\)?\s*$/.exec(line);
    if (m && !m[1].startsWith(here)) return m[1];
  }
  return process.cwd();
}

function scope(): Scope {
  if (stack.length) return stack[stack.length - 1];
  const file = callerFile();
  let root = roots.get(file);
  if (!root) {
    roots.set(file, (root = { title: '', file, use: {}, hooks: [], conditions: [] }));
    bun?.beforeAll(() => ensureLoaded(), BIG);
    // Worker fixtures a file set up are torn down after it (built-in browsers stay).
    bun?.afterAll(async () => {
      for (const [key, w] of [...workerValues]) {
        workerValues.delete(key);
        await w.teardown();
      }
      finished.add(file);
      if (expected === null) expected = runFiles();
      if (expected && [...expected].every((f) => finished.has(f))) await shutdown();
    }, BIG);
  }
  return root;
}

const chain = (s: Scope): Scope[] => {
  const out: Scope[] = [];
  for (let c: Scope | undefined = s; c; c = c.parent) out.unshift(c);
  return out;
};
const projectOf = (file: string): ResolvedProject => projectFor(cfg(), file);
const useOf = (s: Scope) => Object.assign({}, projectOf(s.file).use, ...chain(s).map((c) => c.use));

// ---- running a test ----

type Entry = { title: string; fn: Function; scope: Scope; defs: Defs; details?: { tag?: string | string[]; annotation?: unknown }; expectFail?: boolean };

async function withTimeout<T>(info: TestInfo, run: () => Promise<T>, onTimeout: () => void): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let fail!: (e: Error) => void;
  const timedOut = new Promise<never>((_, reject) => (fail = reject));
  const arm = (ms: number) => {
    if (timer) clearTimeout(timer);
    if (ms > 0) {
      timer = setTimeout(() => {
        onTimeout();
        fail(new TimeoutError(`Test timeout of ${ms}ms exceeded.`));
      }, Math.max(0, ms - info.elapsed));
    }
  };
  info._onTimeout = arm;
  arm(info.timeout);
  try {
    return await Promise.race([run(), timedOut]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

async function conditions(entry: Entry, run: Run, info: TestInfo): Promise<void> {
  for (const s of chain(entry.scope)) {
    for (const c of s.conditions) {
      const hit = typeof c.cond === 'function' ? !!(await run.call(c.cond)) : c.cond === undefined ? true : c.cond;
      if (!hit) continue;
      if (c.kind === 'slow') info.slow(true, c.reason);
      else if (c.kind === 'fail') info.fail(true, c.reason);
      else info[c.kind](true, c.reason);
    }
  }
}

async function runOnce(entry: Entry, info: TestInfo): Promise<void> {
  const run = new Run(entry.defs, useOf(entry.scope), info);
  const scopes = chain(entry.scope);
  let failure: unknown;
  try {
    await withTimeout(info, async () => {
      await run.autos();
      await conditions(entry, run, info);
      for (const s of scopes) for (const h of s.hooks) if (h.kind === 'beforeEach') await run.call(h.fn);
      await run.call(entry.fn);
    }, () => void run.teardown().catch(() => {}));
  } catch (e) {
    failure = e;
  }
  for (const s of [...scopes].reverse()) {
    for (const h of s.hooks) {
      if (h.kind !== 'afterEach') continue;
      try {
        await run.call(h.fn);
      } catch (e) {
        failure ??= e;
      }
    }
  }
  try {
    await run.teardown();
  } catch (e) {
    failure ??= e;
  }
  if (failure === undefined && info._softErrors.length) {
    const [first, ...rest] = info._softErrors;
    failure = rest.length ? new Error(`${info._softErrors.length} soft assertions failed:\n\n${info._softErrors.map((e) => e.message).join('\n\n')}`) : first;
  }
  if (failure !== undefined) throw failure;
}

const titlePathOf = (s: Scope, title: string) => [relative(process.cwd(), s.file), ...chain(s).map((c) => c.title).filter(Boolean), title];

async function runTest(entry: Entry): Promise<void> {
  const l = cfg();
  const project = projectOf(entry.scope.file);
  const scopes = chain(entry.scope);
  const titlePath = titlePathOf(entry.scope, entry.title);
  const started = Date.now();
  if (scopes.some((s) => s.mode === 'serial' && s.failed)) {
    if (!process.env.LP_RESULTS) console.log(`  ↳ skipped: ${entry.title} (an earlier test of its serial group failed)`);
    record({ file: entry.scope.file, titlePath, status: 'skipped', duration: 0, retries: 0, skipReason: 'an earlier test of its serial group failed' });
    return;
  }
  const retries = [...scopes].reverse().find((s) => s.retries !== undefined)?.retries ?? project.retries;
  const timeout = [...scopes].reverse().find((s) => s.timeout !== undefined)?.timeout ?? project.timeout;
  for (let attempt = 0; ; attempt++) {
    const info = new TestInfo(entry.title, titlePath, entry.scope.file, project, l.config, timeout, attempt);
    (info as { workerIndex: number }).workerIndex = WORKER_INDEX;
    (info as { parallelIndex: number }).parallelIndex = WORKER_INDEX;
    if (entry.expectFail) info.fail();
    try {
      await als.run(info, () => runOnce(entry, info));
      if (info.expectedStatus === 'failed') throw new Error('Expected to fail, but passed.');
      record({ file: entry.scope.file, titlePath, status: 'passed', flaky: attempt > 0, duration: Date.now() - started, retries: attempt });
      return;
    } catch (e) {
      if (e instanceof SkipError || info._skipped !== undefined) {
        if (!process.env.LP_RESULTS) console.log(`  ↳ skipped: ${entry.title}${info._skipped ? ` (${info._skipped})` : ''}`);
        record({ file: entry.scope.file, titlePath, status: 'skipped', duration: Date.now() - started, retries: attempt, skipReason: info._skipped || undefined });
        return;
      }
      if (info.expectedStatus === 'failed') {
        record({ file: entry.scope.file, titlePath, status: 'passed', expectedFailure: true, duration: Date.now() - started, retries: attempt });
        return;
      }
      if (attempt < retries) {
        if (!process.env.LP_RESULTS) console.log(`  ↳ retry ${attempt + 1}/${retries}: ${entry.title}`);
        continue;
      }
      for (const s of scopes) if (s.mode === 'serial') s.failed = true;
      const err = e as Error;
      record({ file: entry.scope.file, titlePath, status: err instanceof TimeoutError && /Test timeout/.test(err.message) ? 'timedOut' : 'failed', duration: Date.now() - started, retries: attempt, error: { message: String(err?.message ?? e), stack: err?.stack } });
      throw e;
    }
  }
}

/** beforeAll/afterAll: worker fixtures, run once for their describe. */
function allHook(kind: 'beforeAll' | 'afterAll', fn: Function, defs: Defs, s: Scope): void {
  if (!bun) return;
  const register = kind === 'beforeAll' ? bun.beforeAll : bun.afterAll;
  register(async () => {
    // As Playwright: a worker that runs none of the describe's tests runs none of its hooks.
    if (!s.registered) return;
    const l = cfg();
    const project = projectOf(s.file);
    const info = new TestInfo(`${kind} hook`, [relative(process.cwd(), s.file), s.title, kind].filter(Boolean), s.file, project, l.config, project.timeout, 0);
    const run = new Run(defs, useOf(s), info, true);
    await als.run(info, async () => {
      try {
        await withTimeout(info, async () => {
          await run.autos();
          await run.call(fn);
        }, () => {});
      } finally {
        await run.teardown();
      }
    });
  }, BIG);
}

// ---- the `test` object ----

type Fixtures = Record<string, unknown>;
type TestFn<A> = (args: A, info: TestInfo) => unknown;
type Details = { tag?: string | string[]; annotation?: { type: string; description?: string } | { type: string; description?: string }[] };

export type TestType<A = any, W = any> = {
  (title: string, fn: TestFn<A & W>): void;
  (title: string, details: Details, fn: TestFn<A & W>): void;
  only: (title: string, a: any, b?: any) => void;
  skip: (...args: any[]) => void;
  fixme: (...args: any[]) => void;
  fail: ((...args: any[]) => void) & { only: (...args: any[]) => void };
  slow: (...args: any[]) => void;
  describe: Describe;
  beforeEach: (a: string | TestFn<A & W>, b?: TestFn<A & W>) => void;
  afterEach: (a: string | TestFn<A & W>, b?: TestFn<A & W>) => void;
  beforeAll: (a: string | TestFn<W>, b?: TestFn<W>) => void;
  afterAll: (a: string | TestFn<W>, b?: TestFn<W>) => void;
  use: (options: Partial<A & W> & Record<string, unknown>) => void;
  step: <T>(title: string, body: (step: { skip: (c?: boolean, d?: string) => void }) => T | Promise<T>, options?: { box?: boolean; timeout?: number; location?: unknown }) => Promise<T>;
  extend: <T extends Fixtures = {}, U extends Fixtures = {}>(fixtures: Record<string, unknown>) => TestType<A & T, W & U>;
  info: () => TestInfo;
  setTimeout: (ms: number) => void;
  expect: typeof expect;
};
type Describe = {
  (title: string, fn: () => void): void;
  (fn: () => void): void;
  (title: string, details: Details, fn: () => void): void;
  only: Describe;
  skip: Describe;
  fixme: Describe;
  serial: Describe & { only: Describe; skip: Describe };
  parallel: Describe & { only: Describe };
  configure: (o: { mode?: 'default' | 'serial' | 'parallel'; retries?: number; timeout?: number }) => void;
};

/** Tests declared per file: a parallel test goes to worker (its index % LP_WORKERS). */
const declared = new Map<string, number>();
/**
 * Whether this worker runs a test declared in `s`: a parallel one (`fullyParallel`,
 * or a describe with `mode: 'parallel'`) by its index in the file; the others
 * with their whole file, on the worker that owns it.
 */
function owned(s: Scope): boolean {
  if (WORKERS === 1) return true;
  const scopes = chain(s);
  const serial = scopes.some((c) => c.mode === 'serial' || c.mode === 'default');
  const parallel = !serial && (FULLY_PARALLEL || scopes.some((c) => c.mode === 'parallel'));
  if (parallel) {
    const i = declared.get(s.file) ?? 0;
    declared.set(s.file, i + 1);
    return i % WORKERS === WORKER_INDEX;
  }
  const owner = OWNERS[s.file] ?? [...s.file].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7) % WORKERS;
  return owner === WORKER_INDEX;
}

function make(defs: Defs): TestType {
  const declare = (variant: 'normal' | 'only' | 'skip' | 'fixme' | 'fail', title: string, a: unknown, b?: unknown) => {
    if (!bun) throw new Error('lightning-panda: run tests with `bun test`');
    const fn = (typeof a === 'function' ? a : b) as Function;
    const details = (typeof a === 'object' ? a : undefined) as Details | undefined;
    const s = scope();
    const entry: Entry = { title, fn, scope: s, defs, details, expectFail: variant === 'fail' };
    const tags = ([] as string[]).concat(details?.tag ?? []);
    const name = tags.length ? `${title} ${tags.join(' ')}` : title;
    const titlePath = titlePathOf(s, title);
    const full = [...titlePath, ...tags].join(' ');
    if ((GREP && !GREP.test(full)) || (GREP_INVERT && GREP_INVERT.test(full))) return;
    if (!owned(s)) return;
    if (LIST) {
      record({ file: s.file, titlePath, status: 'skipped', duration: 0, retries: 0, skipReason: 'list' });
      return;
    }
    if (variant === 'skip' || variant === 'fixme') {
      record({ file: s.file, titlePath, status: 'skipped', duration: 0, retries: 0, skipReason: variant });
      return bun.test.skip(name, () => {});
    }
    for (const c of chain(s)) c.registered = (c.registered ?? 0) + 1;
    const body = () => runTest(entry);
    const concurrent = process.env.LP_CONCURRENT === '1' && chain(s).some((c) => c.mode === 'parallel');
    if (variant === 'only') bun.test.only(name, body, BIG);
    else if (concurrent) bun.test.concurrent(name, body, BIG);
    else bun.test(name, body, BIG);
  };

  // test.skip(): a test, a describe-level condition, or (in a test) skip it now.
  const conditional = (kind: 'skip' | 'fixme' | 'fail' | 'slow') => (...args: any[]) => {
    if (typeof args[0] === 'string' && (typeof args[1] === 'function' || typeof args[2] === 'function')) {
      if (kind === 'slow') throw new Error('test.slow(title, fn) is not a thing: call test.slow() in the test');
      return declare(kind === 'fail' ? 'fail' : kind, args[0], args[1], args[2]);
    }
    const info = currentTest();
    const [cond, reason] = args as [boolean | Function | string | undefined, string | undefined];
    if (info && typeof cond !== 'function') {
      const c = typeof cond === 'string' ? true : (cond ?? true);
      const r = typeof cond === 'string' ? cond : reason;
      if (kind === 'slow') return info.slow(c, r);
      if (kind === 'fail') return info.fail(c, r);
      return info[kind](c, r);
    }
    scope().conditions.push({ cond: typeof cond === 'string' ? true : cond, reason: typeof cond === 'string' ? cond : reason, kind });
  };

  const t = ((title: string, a: unknown, b?: unknown) => declare('normal', title, a, b)) as unknown as TestType;
  t.only = ((title: string, a: unknown, b?: unknown) => declare('only', title, a, b)) as TestType['only'];
  t.skip = conditional('skip');
  t.fixme = conditional('fixme');
  t.fail = Object.assign(conditional('fail'), { only: (title: string, a: unknown, b?: unknown) => declare('only', title, a, b) });
  t.slow = conditional('slow');

  const describe = (variant: 'normal' | 'only' | 'skip' | 'serial' | 'parallel') => (a: string | (() => void), b?: unknown, c?: unknown) => {
    if (!bun) throw new Error('lightning-panda: run tests with `bun test`');
    const title = typeof a === 'string' ? a : '';
    const fn = (typeof a === 'function' ? a : typeof b === 'function' ? b : c) as () => void;
    const parent = scope();
    const s: Scope = { title, parent, file: parent.file, use: {}, hooks: [], conditions: [], ...(variant === 'serial' ? { mode: 'serial' } : variant === 'parallel' ? { mode: 'parallel' } : {}) };
    const body = () => {
      stack.push(s);
      try {
        fn();
      } finally {
        stack.pop();
      }
    };
    if (variant === 'skip') return bun.describe.skip(title || 'describe', body);
    if (variant === 'only') return bun.describe.only(title || 'describe', body);
    if (title) bun.describe(title, body);
    else body();
  };
  const d = describe('normal') as unknown as Describe;
  d.only = describe('only') as unknown as Describe;
  d.skip = describe('skip') as unknown as Describe;
  d.fixme = describe('skip') as unknown as Describe;
  d.serial = Object.assign(describe('serial'), { only: describe('only'), skip: describe('skip') }) as unknown as Describe['serial'];
  d.parallel = Object.assign(describe('parallel'), { only: describe('only') }) as unknown as Describe['parallel'];
  d.configure = (o) => {
    const s = scope();
    if (o.mode) s.mode = o.mode;
    if (o.retries !== undefined) s.retries = o.retries;
    if (o.timeout !== undefined) s.timeout = o.timeout;
  };
  t.describe = d;

  const hook = (kind: Hook['kind']) => (a: string | Function, b?: Function) => {
    const fn = (typeof a === 'function' ? a : b) as Function;
    const s = scope();
    if (kind === 'beforeAll' || kind === 'afterAll') allHook(kind, fn, defs, s);
    else s.hooks.push({ kind, fn, title: typeof a === 'string' ? a : undefined });
  };
  t.beforeEach = hook('beforeEach') as TestType['beforeEach'];
  t.afterEach = hook('afterEach') as TestType['afterEach'];
  t.beforeAll = hook('beforeAll') as TestType['beforeAll'];
  t.afterAll = hook('afterAll') as TestType['afterAll'];
  t.use = (options) => Object.assign(scope().use, options);
  t.step = async (title, body, o = {}) => {
    const info = currentTest();
    const run = () => Promise.resolve(body({ skip: (c = true, r) => info?.skip(c, r) }));
    if (!o.timeout) return run();
    return Promise.race([run(), new Promise<never>((_, reject) => setTimeout(() => reject(new TimeoutError(`Step timeout of ${o.timeout}ms exceeded: ${title}`)), o.timeout))]);
  };
  t.extend = (fixtures) => {
    const next: Defs = new Map([...defs].map(([k, v]) => [k, [...v]]));
    for (const [name, v] of Object.entries(fixtures)) {
      const stackOf = next.get(name) ?? [];
      const inherit = stackOf[stackOf.length - 1];
      // An option given a plain value in extend keeps being an option with that default.
      const def = inherit?.option && !(Array.isArray(v) && typeof v[1] === 'object') ? { ...inherit, fn: undefined, value: v } : toDef(v, inherit);
      next.set(name, [...stackOf, def]);
    }
    return make(next);
  };
  t.info = () => {
    const info = currentTest();
    if (!info) throw new Error('test.info() can only be called while a test is running');
    return info;
  };
  t.setTimeout = (ms) => {
    const info = currentTest();
    if (info) info.setTimeout(ms);
    else scope().timeout = ms;
  };
  t.expect = expect;
  return t;
}

const base: Defs = new Map(Object.entries(builtins).map(([k, v]) => [k, [toDef(v)]]));
export const test: TestType = make(base);
export { expect };
