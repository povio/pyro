// Playwright's test runner API on `bun test`: `test`, `test.describe`,
// hooks, fixtures (`test.extend`, `test.use`, options, worker scope, auto),
// `test.step`, `test.info()`, skip/fixme/fail/slow, retries and timeouts,
// read from the Playwright config file (see config.ts).
//
// Bun registers and runs the tests; this module resolves their fixtures,
// runs their hooks, applies the timeout and retries, and reports through
// Bun. As in Playwright's worker, a describe's beforeAll hooks run with the
// first of its tests this process runs (an error fails that test, and the
// describe's other tests do not run) and its afterAll hooks with the last
// (an error fails that test); worker fixtures live as long as the process.
// Tests of a file run one after another (a describe's `mode: 'parallel'`
// too, as its tests may share module state set in hooks);
// `PYRO_CONCURRENT=1` runs those in parallel describes concurrently.
import { appendFileSync } from 'node:fs';
import { relative } from 'node:path';
import { Pyro } from '../index.js';
import { Browser, chromium, setTestDefaults, type BootSpec } from './browser.js';
import { als, currentTest, SkipError, TestInfo } from './context.js';
import { devices, globalSetup, loadConfig, projectFor, runFiles, runPlan, startWebServers, stopWebServers, stopWithThisProcess, type FilePlan, type Loaded, type ResolvedProject } from './config.js';
import { expect, ready, setExpectDefaults } from './expect.js';
import { TimeoutError } from './page.js';
import { APIRequestContext, request } from './request.js';
import type { PlaywrightTestArgs, PlaywrightTestOptions, PlaywrightWorkerArgs, PlaywrightWorkerOptions, TestDetails, TestType, WorkerInfo } from './types.js';

/** The implementation's view of `test` (the public one is `TestType`). */
type AnyTest = any;

type BunTest = typeof import('bun:test');
// Synchronous: no top-level await, so CommonJS loaders can import this module too.
const bun: BunTest | undefined = (globalThis as any).Bun ? require('bun:test') : undefined;

// ---- config, web servers, global setup: once per process ----

let loaded: Loaded | undefined;
let loading: Promise<Loaded> | undefined;
let teardowns: (() => Promise<unknown>)[] = [];
/** The run's files, and those done: after the last one the process ends (see `finish`). */
let expected: Set<string> | undefined | null = null;
const finished = new Set<string>();
/** Config, web servers and global setup, once per process, before the first file's tests. */
function ensureLoaded(): Promise<Loaded> {
  return (loading ??= (async () => {
    const l = await loadConfig();
    setExpectDefaults({ timeout: l.config.expect?.timeout });
    // A worker of `pyro run`: the parent started the servers and ran global setup.
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

/** `pyro run` runs this process as one of its workers, with a plan of the tests it runs (config.ts, `RunPlan`). */
const WORKER = process.env.PYRO_WORKER === '1';
const PLAN = runPlan();
const SLOTS = Math.max(1, PLAN?.slots ?? 1);
/** This worker's slot (`parallelIndex`) and its number in the run (`workerIndex`). */
const SLOT = Number(process.env.PYRO_WORKER_INDEX ?? 0);
const WORKER_ID = Number(process.env.PYRO_WORKER_ID ?? SLOT);
const GREP = process.env.PYRO_GREP ? new RegExp(process.env.PYRO_GREP) : undefined;
const GREP_INVERT = process.env.PYRO_GREP_INVERT ? new RegExp(process.env.PYRO_GREP_INVERT) : undefined;
const LIST = process.env.PYRO_LIST === '1';

/**
 * `pyro run` preloads a module whose afterAll, after the last file, calls
 * `finish` (worker fixtures torn down, browsers closed). Without it, the
 * last file's afterAll does.
 */
const preloaded = () => !!(globalThis as any)[Symbol.for('pyro.wright.preload')];
((globalThis as any)[Symbol.for('pyro.wright.finish')] ??= new Set<() => Promise<void>>()).add(() => finish());

/** One line per finished test for `pyro run`'s reporter (`$PYRO_RESULTS`). */
type Outcome = {
  /** An error outside any test: a worker fixture's teardown, an afterAll hook no test ran. */
  kind?: 'error';
  file: string;
  project: number;
  titlePath: string[];
  status: 'passed' | 'failed' | 'timedOut' | 'skipped';
  flaky?: boolean;
  expectedFailure?: boolean;
  /** Skipped because it could not run: a beforeAll hook of its describe failed, or an earlier test of its serial group. */
  didNotRun?: boolean;
  duration: number;
  retries: number;
  errors?: { message: string; stack?: string }[];
  skipReason?: string;
  /** `--list`: the test's index among its file's parallel tests (the others run as one group). */
  parallel?: number;
  worker: number;
};
function record(o: Omit<Outcome, 'worker'>): void {
  const out = process.env.PYRO_RESULTS;
  if (out) appendFileSync(out, JSON.stringify({ ...o, worker: WORKER_ID }) + '\n');
}
const serialize = (e: unknown) => ({ message: String((e as Error)?.message ?? e), stack: (e as Error)?.stack });
const asError = (e: unknown): Error => (e instanceof Error ? e : new Error(String(e)));
/** An error outside any test: reported by `pyro run` (it fails the run), thrown by plain `bun test`. */
function recordError(file: string, where: string, e: unknown): void {
  record({ kind: 'error', file, project: PLAN?.files[file]?.project ?? 0, titlePath: [file ? relative(process.cwd(), file) : '', where].filter(Boolean), status: 'failed', duration: 0, retries: 0, errors: [serialize(e)] });
}
const cfg = (): Loaded => {
  if (!loaded) throw new Error('pyro: the test runner needs `bun test`');
  return loaded;
};

// ---- fixtures ----

type FixtureFn = (args: any, use: (value: any) => Promise<void>, info: any) => any;
type Def = { fn?: FixtureFn; value?: unknown; scope: 'test' | 'worker'; auto: boolean; option: boolean; timeout?: number; allHooks?: boolean };
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

/**
 * Worker fixture values, kept until the process ends (Playwright keeps them
 * for the worker's life), then torn down last-first. Keyed by the fixture,
 * the `test` object's worker fixtures (Playwright's worker hash) and the
 * worker options.
 */
const workerValues = new Map<string, { value: unknown; teardown: () => Promise<void> }>();
const workerPending = new Map<string, Promise<unknown>>();
const defIds = new WeakMap<Def, number>();
let defCount = 0;
const poolIds = new WeakMap<Defs, string>();
function workerPool(defs: Defs): string {
  let id = poolIds.get(defs);
  if (id === undefined) {
    const parts: string[] = [];
    for (const [name, stack] of defs) {
      for (const d of stack) {
        if (d.scope !== 'worker') continue;
        if (!defIds.has(d)) defIds.set(d, defCount++);
        parts.push(`${name}:${defIds.get(d)}`);
      }
    }
    poolIds.set(defs, (id = parts.join(',')));
  }
  return id;
}

const workerInfo = (info: TestInfo): WorkerInfo => ({ workerIndex: WORKER_ID, parallelIndex: SLOT, project: info.project, config: info.config });

class Run {
  private values = new Map<string, Promise<unknown>>();
  private teardowns: (() => Promise<void>)[] = [];
  constructor(
    private defs: Defs,
    private use: Record<string, unknown>,
    private info: TestInfo,
    /** A beforeAll/afterAll hook: its auto fixtures are the worker ones (and `_contextDefaults`); test fixtures are its own. */
    private hook = false,
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
    const key = `${name}@${level}:${workerPool(this.defs)}:${JSON.stringify(options)}`;
    const have = workerValues.get(key);
    if (have) return Promise.resolve(have.value);
    let v = workerPending.get(key);
    if (!v) {
      v = this.setUp(name, def, level, () => {}, (value, teardown) => workerValues.set(key, { value, teardown })).finally(() => workerPending.delete(key));
      workerPending.set(key, v);
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
    const second = def.scope === 'worker' ? workerInfo(this.info) : this.info;
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
      if (top.auto && (!this.hook || top.scope === 'worker' || top.allHooks)) await this.get(name);
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

/** Whether a modifier's condition reads worker fixtures only (Playwright then runs it before the describe's beforeAll hooks). */
const workerOnly = (fn: Function, defs: Defs) => paramNames(fn).every((n) => defs.get(n)?.at(-1)?.scope === 'worker');

// ---- built-in fixtures ----

/** After the run's last file: global teardown, browsers closed, web servers stopped. */
async function shutdown(): Promise<void> {
  for (const t of teardowns.splice(0)) await t();
  for (const lp of launched) lp.close();
  launched.clear();
  browsers.clear();
  stopWebServers();
}

/** One pyro per process (per `pyro` option), and one Browser per boot config on it. */
const browsers = new Map<string, Promise<{ lp: Pyro; browser: Browser }>>();
const launched = new Set<Pyro>();
// Synchronous: `exit` runs no promise callbacks. `close()` kills the launched process group.
process.on('exit', () => {
  for (const lp of launched) lp.close();
});
async function sharedBrowser(o: { wsEndpoint?: string; bin?: string; args?: string[]; boot?: BootSpec[] }): Promise<Browser> {
  const key = JSON.stringify(o);
  let b = browsers.get(key);
  if (!b) {
    b = (async () => {
      const ws = o.wsEndpoint ?? process.env.PYRO_WS;
      const lp = ws ? await Pyro.connect(ws) : await Pyro.launch({ bin: o.bin, args: o.args });
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

/**
 * Playwright's defaults of the context options that have one, as fixture
 * values (`contextOptions` first, as in Playwright). `browser.newContext()`
 * gets these only when they differ from the default or `contextOptions` sets
 * them: pyro's contexts already behave so. `acceptDownloads` and `viewport`
 * are passed on as before.
 */
const OPTION_DEFAULTS: Record<string, unknown> = {
  acceptDownloads: true,
  bypassCSP: false,
  colorScheme: 'light',
  hasTouch: false,
  ignoreHTTPSErrors: false,
  isMobile: false,
  javaScriptEnabled: true,
  locale: 'en-US',
  offline: false,
  serviceWorkers: 'allow',
  viewport: { width: 1280, height: 720 },
};
const UNFORWARDED_DEFAULTS = Object.fromEntries(Object.entries(OPTION_DEFAULTS).filter(([n]) => n !== 'acceptDownloads' && n !== 'viewport'));
const optionDefault = (name: string): FixtureFn => async ({ contextOptions }: any, use: any) => use(contextOptions?.[name] === undefined ? OPTION_DEFAULTS[name] : contextOptions[name]);

const builtins: Record<string, unknown> = {
  pyro: [{}, { scope: 'worker', option: true }],
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
  browser: [async ({ pyro }: any, use: any) => use(await sharedBrowser(pyro ?? {})), { scope: 'worker' }],
  ...Object.fromEntries(CONTEXT_OPTIONS.map((n) => [n, [n in OPTION_DEFAULTS ? optionDefault(n) : undefined, { option: true }]])),
  // As Playwright's: `$PLAYWRIGHT_TEST_BASE_URL` when nothing sets it.
  baseURL: [async ({}, use: any) => use(process.env.PLAYWRIGHT_TEST_BASE_URL), { option: true }],
  contextOptions: [{}, { option: true }],
  actionTimeout: [0, { option: true }],
  navigationTimeout: [0, { option: true }],
  testIdAttribute: ['data-testid', { option: true }],
  // The use options `browser.newContext()` starts from, as in Playwright (set below); in beforeAll/afterAll hooks too.
  _contextDefaults: [undefined, { auto: true }],
  context: async ({ browser, _contextDefaults }: any, use: any, info: TestInfo) => {
    void _contextDefaults;
    if (info?._hook) {
      throw new Error(
        `"context" and "page" fixtures are not supported in "${info._hook}" since they are created on a per-test basis. ` +
          'If you would like to reuse a single page between tests, create context manually with browser.newContext(). ' +
          'If you would like to configure your page before each test, do that in beforeEach hook instead.',
      );
    }
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
  'UNFORWARDED_DEFAULTS',
  `return async ({ ${[...CONTEXT_OPTIONS, 'contextOptions', 'actionTimeout', 'navigationTimeout', 'testIdAttribute'].join(', ')} }, use) => {
    const o = { ${[...CONTEXT_OPTIONS, 'contextOptions', 'actionTimeout', 'navigationTimeout', 'testIdAttribute'].join(', ')} };
    const options = { ...(o.contextOptions ?? {}) };
    for (const n of CONTEXT_OPTIONS) {
      if (o[n] === undefined || (!(n in options) && n in UNFORWARDED_DEFAULTS && o[n] === UNFORWARDED_DEFAULTS[n])) continue;
      options[n] = o[n];
    }
    setTestDefaults({ options, timeout: o.actionTimeout || undefined, navigationTimeout: o.navigationTimeout || undefined, testIdAttribute: o.testIdAttribute || 'data-testid' });
    await use();
    setTestDefaults(undefined);
  }`,
)(setTestDefaults, CONTEXT_OPTIONS, UNFORWARDED_DEFAULTS) as FixtureFn;

// ---- describe scopes ----

type Hook = { kind: 'beforeEach' | 'afterEach' | 'beforeAll' | 'afterAll'; fn: Function; defs: Defs; title?: string };
type Condition = { cond: boolean | Function | undefined; reason?: string; kind: 'skip' | 'fixme' | 'fail' | 'slow'; defs: Defs };
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
  /** `describe.skip` / `describe.fixme`: its tests are skipped where they are declared. */
  skip?: 'skip' | 'fixme';
  /** Tests this process runs in the scope and its describes, and how many of them finished. */
  registered?: number;
  done?: number;
  /** Its beforeAll stage, once a test started it: a skip or the error it ended with, and whether its afterAll hooks ran. */
  all?: { ready: Promise<void>; skip?: string; error?: unknown; reported?: boolean; closed?: boolean };
};

const roots = new Map<string, Scope>();
/** Every scope of a file, in declaration order. */
const fileScopes = new Map<string, Scope[]>();
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
    const r: Scope = { title: '', file, use: {}, hooks: [], conditions: [] };
    roots.set(file, (root = r));
    fileScopes.set(file, [r]);
    bun?.beforeAll(() => ensureLoaded(), BIG);
    // After the file: afterAll hooks no test ran (Bun skipped the describe's last test: `.only`). Without
    // `pyro run`'s preload, the process ends after the last file.
    bun?.afterAll(async () => {
      const errors = await closeFile(file);
      finished.add(file);
      if (!preloaded()) {
        if (expected === null) expected = runFiles();
        // When it cannot tell which file is the last, worker fixtures are torn down after each.
        if (expected === undefined) errors.push(...(await teardownWorkerFixtures()));
        else if ([...expected].every((f) => finished.has(f))) errors.push(...(await finish()));
      }
      if (errors.length && !process.env.PYRO_RESULTS) throw errors[0];
    }, BIG);
  }
  return root;
}

/** Runs the afterAll hooks of the file's describes whose beforeAll stage ran and whose afterAll did not, innermost first. */
async function closeFile(file: string): Promise<unknown[]> {
  const errors: unknown[] = [];
  for (const s of [...(fileScopes.get(file) ?? [])].reverse()) {
    for (const e of await closeScope(s)) {
      recordError(file, `${[...chain(s).map((c) => c.title).filter(Boolean), 'afterAll hook'].join(' › ')}`, e);
      errors.push(e);
    }
  }
  return errors;
}

/** Tears down the worker fixtures, last set up first. */
async function teardownWorkerFixtures(): Promise<unknown[]> {
  const errors: unknown[] = [];
  for (const [key, w] of [...workerValues].reverse()) {
    workerValues.delete(key);
    try {
      await w.teardown();
    } catch (e) {
      recordError('', `worker fixture "${key.slice(0, key.indexOf('@'))}" teardown`, e);
      errors.push(e);
    }
  }
  return errors;
}

let finishing: Promise<unknown[]> | undefined;
/** The end of the process: afterAll hooks still due, worker fixtures, then global teardown, browsers and web servers. */
function finish(): Promise<unknown[]> {
  return (finishing ??= (async () => {
    const errors: unknown[] = [];
    for (const file of fileScopes.keys()) errors.push(...(await closeFile(file)));
    errors.push(...(await teardownWorkerFixtures()));
    await shutdown();
    return errors;
  })());
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

/** Test and hook timeouts (not an action's TimeoutError): the test is 'timedOut'. */
const timeouts = new WeakSet<object>();
const isTimeout = (e: unknown) => typeof e === 'object' && e !== null && timeouts.has(e);

async function withTimeout<T>(info: TestInfo, run: () => Promise<T>, onTimeout: () => void): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let fail!: (e: Error) => void;
  const timedOut = new Promise<never>((_, reject) => (fail = reject));
  const arm = (ms: number) => {
    if (timer) clearTimeout(timer);
    if (ms > 0) {
      timer = setTimeout(() => {
        const e = new TimeoutError(info._hook ? `"${info._hook}" hook timeout of ${ms}ms exceeded.` : `Test timeout of ${ms}ms exceeded.`);
        timeouts.add(e);
        // Fixture teardown (started at once, to unblock the test) sees the status.
        info._failWithError(e, true);
        onTimeout();
        fail(e);
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

/** A describe's `test.skip(condition)` that is not a function: its tests are skipped before any hook runs. */
function staticSkip(scopes: Scope[]): string | undefined {
  for (const s of scopes) {
    for (const c of s.conditions) {
      if (typeof c.cond === 'function' || (c.kind !== 'skip' && c.kind !== 'fixme')) continue;
      if (c.cond === undefined || c.cond) return c.reason ?? '';
    }
  }
  return undefined;
}

/** Sets the indexes Playwright's TestInfo carries. */
function placed(info: TestInfo): TestInfo {
  (info as { workerIndex: number }).workerIndex = WORKER_ID;
  (info as { parallelIndex: number }).parallelIndex = SLOT;
  return info;
}

/** Runs a beforeAll/afterAll hook (or a describe's modifier) with its own TestInfo, timeout and test fixtures. */
async function allHook(s: Scope, defs: Defs, kind: 'beforeAll' | 'afterAll', fn: Function): Promise<unknown> {
  const l = cfg();
  const project = projectOf(s.file);
  const info = placed(new TestInfo(`${kind} hook`, titlePathOf(s, `${kind} hook`), s.file, project, l.full, project.timeout, 0));
  info._hook = kind;
  const run = new Run(defs, useOf(s), info, true);
  return als.run(info, async () => {
    let failure: unknown;
    let failed = false;
    let result: unknown;
    try {
      result = await withTimeout(info, async () => {
        await run.autos();
        return run.call(fn);
      }, () => void run.teardown().catch(() => {}));
    } catch (e) {
      [failure, failed] = [e, true];
    }
    try {
      await run.teardown();
    } catch (e) {
      if (!failed) [failure, failed] = [e, true];
    }
    if (failed) throw failure;
    return result;
  });
}

/**
 * A describe's beforeAll stage, started by the first of its tests this
 * process runs: modifiers that read worker fixtures only, then the beforeAll
 * hooks (all of them; the first error is kept). A skip there skips the
 * describe's tests.
 */
function openScope(s: Scope): NonNullable<Scope['all']> {
  if (s.all) return s.all;
  const st: NonNullable<Scope['all']> = { ready: Promise.resolve() };
  s.all = st;
  st.ready = (async () => {
    for (const c of s.conditions) {
      if (typeof c.cond !== 'function' || (c.kind !== 'skip' && c.kind !== 'fixme') || !workerOnly(c.cond, c.defs)) continue;
      try {
        if (await allHook(s, c.defs, 'beforeAll', c.cond)) {
          st.skip = c.reason ?? '';
          return;
        }
      } catch (e) {
        st.error = e;
        return;
      }
    }
    for (const h of s.hooks) {
      if (h.kind !== 'beforeAll') continue;
      try {
        await allHook(s, h.defs, 'beforeAll', h.fn);
      } catch (e) {
        if (e instanceof SkipError) {
          st.skip = e.message;
          return;
        }
        st.error ??= e;
      }
    }
  })();
  return st;
}

/** A describe's afterAll hooks, once its beforeAll stage ran (failed or not), in order; all run, their errors are returned. */
async function closeScope(s: Scope): Promise<unknown[]> {
  const st = s.all;
  if (!st || st.closed) return [];
  st.closed = true;
  await st.ready;
  const errors: unknown[] = [];
  for (const h of s.hooks) {
    if (h.kind !== 'afterAll') continue;
    try {
      await allHook(s, h.defs, 'afterAll', h.fn);
    } catch (e) {
      if (!(e instanceof SkipError)) errors.push(e);
    }
  }
  return errors;
}

/** Runs one attempt: fixtures, modifiers, beforeEach, the body; then afterEach and fixture teardown, which see the test's status. */
async function runOnce(entry: Entry, info: TestInfo): Promise<void> {
  const run = new Run(entry.defs, useOf(entry.scope), info);
  const scopes = chain(entry.scope);
  let failure: unknown;
  const fail = (e: unknown) => {
    failure ??= e;
    if (e instanceof SkipError) {
      if (info.status === 'passed') info.status = 'skipped';
    } else info._failWithError(asError(e), isTimeout(e));
  };
  try {
    await withTimeout(info, async () => {
      await run.autos();
      await conditions(entry, run, info);
      for (const s of scopes) for (const h of s.hooks) if (h.kind === 'beforeEach') await run.call(h.fn);
      await run.call(entry.fn);
    }, () => void run.teardown().catch(() => {}));
  } catch (e) {
    fail(e);
  }
  info.duration = info.elapsed;
  for (const s of [...scopes].reverse()) {
    for (const h of s.hooks) {
      if (h.kind !== 'afterEach') continue;
      try {
        await run.call(h.fn);
      } catch (e) {
        fail(e);
      }
    }
  }
  try {
    await run.teardown();
  } catch (e) {
    fail(e);
  }
  if (failure === undefined && info._softErrors.length) {
    const [first, ...rest] = info._softErrors;
    failure = rest.length ? new Error(`${info._softErrors.length} soft assertions failed:\n\n${info._softErrors.map((e) => e.message).join('\n\n')}`) : first;
  }
  if (failure !== undefined) throw failure;
}

const titlePathOf = (s: Scope, title: string) => [relative(process.cwd(), s.file), ...chain(s).map((c) => c.title).filter(Boolean), title];

/** A test's result before it is recorded; `thrown` is what plain `bun test` reports. */
type Result = { status: Outcome['status']; retries: number; errors: unknown[]; thrown?: unknown; flaky?: boolean; expectedFailure?: boolean; didNotRun?: boolean; skipReason?: string };
const RETRY = Symbol('retry');

async function attempts(entry: Entry, project: ResolvedProject, scopes: Scope[], titlePath: string[]): Promise<Result> {
  if (scopes.some((s) => s.mode === 'serial' && s.failed)) return { status: 'skipped', retries: 0, errors: [], didNotRun: true, skipReason: 'an earlier test of its serial group failed' };
  const skip = staticSkip(scopes);
  if (skip !== undefined) return { status: 'skipped', retries: 0, errors: [], skipReason: skip || undefined };
  const failSerial = () => {
    for (const s of scopes) if (s.mode === 'serial') s.failed = true;
  };
  const retries = [...scopes].reverse().find((s) => s.retries !== undefined)?.retries ?? project.retries;
  const timeout = [...scopes].reverse().find((s) => s.timeout !== undefined)?.timeout ?? project.timeout;
  for (let attempt = 0; ; attempt++) {
    // The beforeAll stage of each describe, outermost first.
    let blocked: Result | typeof RETRY | undefined;
    for (const s of scopes) {
      const st = openScope(s);
      await st.ready;
      if (st.skip !== undefined) {
        blocked = { status: 'skipped', retries: attempt, errors: [], skipReason: st.skip || undefined };
        break;
      }
      if (st.error === undefined) continue;
      if (st.reported) {
        blocked = { status: 'skipped', retries: 0, errors: [], didNotRun: true, skipReason: 'a beforeAll hook of its describe failed' };
      } else if (attempt < retries) {
        // A retry runs the describe's hooks again (Playwright retries in a new worker).
        await closeScope(s);
        s.all = undefined;
        blocked = RETRY;
      } else {
        // The first test of the describe fails with the hook's error; the others do not run.
        st.reported = true;
        failSerial();
        blocked = { status: isTimeout(st.error) ? 'timedOut' : 'failed', retries: attempt, errors: [st.error], thrown: st.error };
      }
      break;
    }
    if (blocked === RETRY) {
      if (!process.env.PYRO_RESULTS) console.log(`  ↳ retry ${attempt + 1}/${retries}: ${entry.title}`);
      continue;
    }
    if (blocked) return blocked;

    const info = placed(new TestInfo(entry.title, titlePath, entry.scope.file, project, cfg().full, timeout, attempt));
    if (entry.expectFail) info.fail();
    try {
      await als.run(info, () => runOnce(entry, info));
    } catch (e) {
      if (info.status === 'skipped') return { status: 'skipped', retries: attempt, errors: [], skipReason: info._skipped || undefined };
      if (info.expectedStatus === 'failed') return { status: 'passed', expectedFailure: true, retries: attempt, errors: [] };
      // A missing snapshot written in "missing" mode is not retried, as in Playwright.
      if (attempt < retries && !info._hasNonRetriableError) {
        if (!process.env.PYRO_RESULTS) console.log(`  ↳ retry ${attempt + 1}/${retries}: ${entry.title}`);
        continue;
      }
      failSerial();
      return { status: info.status === 'timedOut' ? 'timedOut' : 'failed', retries: attempt, errors: info.errors.length ? info.errors : [e], thrown: e instanceof SkipError ? info.errors[0] : e };
    }
    if (info.expectedStatus === 'failed') {
      const e = new Error('Expected to fail, but passed.');
      failSerial();
      return { status: 'failed', retries: attempt, errors: [e], thrown: e };
    }
    return { status: 'passed', flaky: attempt > 0, retries: attempt, errors: [] };
  }
}

async function runTest(entry: Entry): Promise<void> {
  const project = projectOf(entry.scope.file);
  const scopes = chain(entry.scope);
  const titlePath = titlePathOf(entry.scope, entry.title);
  const started = Date.now();
  let r = await attempts(entry, project, scopes, titlePath);
  // The describes whose last test (in this process) this is: their afterAll hooks, innermost first. An error fails this test, as in Playwright.
  for (const s of [...scopes].reverse()) {
    s.done = (s.done ?? 0) + 1;
    if (s.done < (s.registered ?? 0)) continue;
    const errors = await closeScope(s);
    if (!errors.length) continue;
    const failing = r.status === 'failed' || r.status === 'timedOut';
    r = { status: r.status === 'timedOut' ? 'timedOut' : 'failed', retries: r.retries, errors: [...(failing ? r.errors : []), ...errors], thrown: failing ? r.thrown : errors[0] };
  }
  const { status, retries, flaky, expectedFailure, didNotRun, skipReason } = r;
  record({ file: entry.scope.file, project: project.index, titlePath, status, flaky, expectedFailure, didNotRun, skipReason, duration: Date.now() - started, retries, errors: r.errors.length ? r.errors.map(serialize) : undefined });
  if (status === 'skipped' && !process.env.PYRO_RESULTS) console.log(`  ↳ ${didNotRun ? 'did not run' : 'skipped'}: ${entry.title}${skipReason ? ` (${skipReason})` : ''}`);
  if (status === 'failed' || status === 'timedOut') throw r.thrown ?? r.errors[0];
}

// ---- the `test` object ----

/** Parallel tests declared per file, in order. */
const declared = new Map<string, number>();
/**
 * Whether this worker runs a test declared in `s`, and the test's index among
 * its file's parallel tests. A parallel test (`fullyParallel`, or a describe
 * with `mode: 'parallel'`) goes to a worker by that index (in its shard,
 * from the file's owner on); the others go with their whole file to the
 * worker that owns it.
 */
function place(s: Scope, f: FilePlan | undefined): { mine: boolean; parallel?: number } {
  const scopes = chain(s);
  const serial = scopes.some((c) => c.mode === 'serial' || c.mode === 'default');
  const parallel = !serial && (!!f?.fullyParallel || scopes.some((c) => c.mode === 'parallel'));
  // A file the plan does not name (plain `bun test`: one worker; else a hash of its path).
  const owner = f?.owner ?? [...s.file].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7) % SLOTS;
  if (parallel) {
    const i = declared.get(s.file) ?? 0;
    declared.set(s.file, i + 1);
    const rank = f?.shard ? f.shard.parallel.indexOf(i) : i;
    return { mine: rank >= 0 && (owner + rank) % SLOTS === SLOT, parallel: i };
  }
  if (f?.shard && !f.shard.general) return { mine: false };
  return { mine: owner === SLOT };
}

const regexes = (list: [string, string][] | undefined) => list?.map(([source, flags]) => new RegExp(source, flags.replace('g', '')));
const projectGreps = new Map<string, { grep?: RegExp[]; grepInvert?: RegExp[] }>();
/** `--grep`/`--grep-invert` (not for a project that runs as a dependency), then the project's `grep`/`grepInvert`. */
function grepped(file: string, f: FilePlan | undefined, title: string): boolean {
  if (f?.grep !== false && ((GREP && !GREP.test(title)) || (GREP_INVERT && GREP_INVERT.test(title)))) return false;
  if (!f) return true;
  let g = projectGreps.get(file);
  if (!g) projectGreps.set(file, (g = { grep: regexes(f.projectGrep), grepInvert: regexes(f.projectGrepInvert) }));
  if (g.grep && !g.grep.some((re) => re.test(title))) return false;
  return !g.grepInvert?.some((re) => re.test(title));
}

class StepSkip extends Error {
  override name = 'StepSkip';
}

function make(defs: Defs): AnyTest {
  const declare = (variant: 'normal' | 'only' | 'skip' | 'fixme' | 'fail', title: string, a: unknown, b?: unknown) => {
    if (!bun) throw new Error('pyro: run tests with `bun test`');
    const fn = (typeof a === 'function' ? a : b) as Function;
    const details = (typeof a === 'object' ? a : undefined) as TestDetails | undefined;
    const s = scope();
    const entry: Entry = { title, fn, scope: s, defs, details, expectFail: variant === 'fail' };
    const tags = ([] as string[]).concat(details?.tag ?? []);
    const name = tags.length ? `${title} ${tags.join(' ')}` : title;
    const titlePath = titlePathOf(s, title);
    const f = PLAN?.files[s.file];
    if (!grepped(s.file, f, [...titlePath, ...tags].join(' '))) return;
    const at = place(s, f);
    if (!at.mine) return;
    const project = f?.project ?? 0;
    if (LIST) {
      record({ file: s.file, project, titlePath, status: 'skipped', duration: 0, retries: 0, skipReason: 'list', parallel: at.parallel });
      return;
    }
    const skipped = variant === 'skip' || variant === 'fixme' ? variant : chain(s).find((c) => c.skip)?.skip;
    if (skipped) {
      record({ file: s.file, project, titlePath, status: 'skipped', duration: 0, retries: 0, skipReason: skipped });
      return bun.test.skip(name, () => {});
    }
    for (const c of chain(s)) c.registered = (c.registered ?? 0) + 1;
    const body = () => runTest(entry);
    const concurrent = process.env.PYRO_CONCURRENT === '1' && chain(s).some((c) => c.mode === 'parallel');
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
    scope().conditions.push({ cond: typeof cond === 'string' ? true : cond, reason: typeof cond === 'string' ? cond : reason, kind, defs });
  };

  const t = ((title: string, a: unknown, b?: unknown) => declare('normal', title, a, b)) as AnyTest;
  t.only = ((title: string, a: unknown, b?: unknown) => declare('only', title, a, b));
  t.skip = conditional('skip');
  t.fixme = conditional('fixme');
  t.fail = Object.assign(conditional('fail'), { only: (title: string, a: unknown, b?: unknown) => declare('only', title, a, b) });
  t.slow = conditional('slow');

  const describe = (variant: 'normal' | 'only' | 'skip' | 'fixme' | 'serial' | 'parallel') => (a: string | (() => void), b?: unknown, c?: unknown) => {
    if (!bun) throw new Error('pyro: run tests with `bun test`');
    const title = typeof a === 'string' ? a : '';
    const fn = (typeof a === 'function' ? a : typeof b === 'function' ? b : c) as () => void;
    const parent = scope();
    const s: Scope = {
      title,
      parent,
      file: parent.file,
      use: {},
      hooks: [],
      conditions: [],
      ...(variant === 'serial' ? { mode: 'serial' } : variant === 'parallel' ? { mode: 'parallel' } : {}),
      ...(variant === 'skip' || variant === 'fixme' ? { skip: variant } : {}),
    };
    fileScopes.get(parent.file)?.push(s);
    const body = () => {
      stack.push(s);
      try {
        fn();
      } finally {
        stack.pop();
      }
    };
    if (variant === 'skip' || variant === 'fixme') return bun.describe.skip(title || 'describe', body);
    if (variant === 'only') return bun.describe.only(title || 'describe', body);
    if (title) bun.describe(title, body);
    else body();
  };
  const d = describe('normal') as AnyTest;
  d.only = describe('only') as AnyTest;
  d.skip = describe('skip') as AnyTest;
  d.fixme = describe('fixme') as AnyTest;
  d.serial = Object.assign(describe('serial'), { only: describe('only'), skip: describe('skip') }) as AnyTest;
  d.parallel = Object.assign(describe('parallel'), { only: describe('only') }) as AnyTest;
  d.configure = (o: { mode?: 'default' | 'serial' | 'parallel'; retries?: number; timeout?: number }) => {
    const s = scope();
    if (o.mode) s.mode = o.mode;
    if (o.retries !== undefined) s.retries = o.retries;
    if (o.timeout !== undefined) s.timeout = o.timeout;
  };
  t.describe = d;

  const hook = (kind: Hook['kind']) => (a: string | Function, b?: Function) => {
    const fn = (typeof a === 'function' ? a : b) as Function;
    scope().hooks.push({ kind, fn, defs, title: typeof a === 'string' ? a : undefined });
  };
  t.beforeEach = hook('beforeEach');
  t.afterEach = hook('afterEach');
  t.beforeAll = hook('beforeAll');
  t.afterAll = hook('afterAll');
  t.use = (options: Record<string, unknown>) => Object.assign(scope().use, options);
  // A step's `skip()` ends that step (it returns undefined); the test goes on.
  t.step = async <T>(title: string, body: (step: { skip(c?: boolean, d?: string): void }) => T | Promise<T>, o: { timeout?: number; box?: boolean } = {}) => {
    const step = {
      skip: (c = true, description?: string) => {
        if (c) throw new StepSkip(description);
      },
    };
    const run = async () => {
      try {
        return await body(step);
      } catch (e) {
        if (e instanceof StepSkip) return undefined;
        throw e;
      }
    };
    if (!o.timeout) return run();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timedOut = new Promise<never>((_, reject) => (timer = setTimeout(() => reject(new TimeoutError(`Step timeout of ${o.timeout}ms exceeded: ${title}`)), o.timeout)));
    return Promise.race([run(), timedOut]).finally(() => clearTimeout(timer));
  };
  t.step.skip = async () => undefined;
  t.extend = (fixtures: Record<string, unknown>) => {
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
  t.setTimeout = (ms: number) => {
    const info = currentTest();
    if (info) info.setTimeout(ms);
    else scope().timeout = ms;
  };
  t.expect = expect;
  return t;
}

const base: Defs = new Map(Object.entries(builtins).map(([k, v]) => [k, [toDef(v)]]));
base.get('_contextDefaults')![0].allHooks = true;
export const test: TestType<PlaywrightTestArgs & PlaywrightTestOptions, PlaywrightWorkerArgs & PlaywrightWorkerOptions> = make(base);
export { expect };
