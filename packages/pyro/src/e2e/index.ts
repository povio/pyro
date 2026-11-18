// `@povio/pyro/e2e`: spec files for `pyro e2e`, which runs
// every file in one process with one case queue and one fork-mode browser.
//
//   import { browser, describe, expect, test, beforeAll, defer } from '@povio/pyro/e2e';
//
//   describe('positions', () => {
//     beforeAll(async () => {
//       const lp = await browser();
//       await lp.profile('positions', { url, setup });
//       defer(() => lp.releaseProfile('positions')); // once the describe is done
//     });
//     test('closes a position', async () => {
//       await using page = await (await browser()).page({ as: 'positions' });
//       await page.run([...]);
//     }, 30_000);
//   });
//
// The functions have `bun:test`'s signatures; `expect` is Bun's. Tests from all
// files share the run's queue: any test may run alongside any other unless it
// is `test.serial` (or in a `describe.serial`).
import type { Pyro } from '../index.js';
import { registry, newScope, titlePath, SKIP, type Fn, type Scope, type SkipSignal, type TestCase } from './registry.js';

export { expect } from 'bun:test';

type TimeoutArg = number | { timeout?: number; retries?: number };
type TestFn = (name: string, fn: Fn, timeout?: TimeoutArg) => void;
type Flags = { serial?: boolean; skip?: boolean; todo?: boolean; only?: boolean };

function collector(what: string): Scope {
  const r = registry();
  if (!r.collecting) {
    throw new Error(
      r.active
        ? `${what}() registers while spec files load, not from inside a test or hook`
        : `${what}() is for spec files run by \`pyro e2e\``,
    );
  }
  return r.collecting;
}

const timeoutOf = (arg?: TimeoutArg) => (typeof arg === 'number' ? arg : arg?.timeout);
const retriesOf = (arg?: TimeoutArg) => (typeof arg === 'number' ? undefined : arg?.retries);

function define(flags: Flags): TestFn {
  return (name, fn, timeout) => {
    const scope = collector('test');
    const test: TestCase = {
      kind: 'test',
      name: String(name),
      fn: fn ?? (() => {}),
      scope,
      index: scope.children.length,
      timeout: timeoutOf(timeout),
      retries: retriesOf(timeout),
      serial: flags.serial ?? false,
      skip: flags.skip ?? false,
      todo: (flags.todo ?? false) || !fn,
      only: flags.only ?? false,
    };
    scope.children.push(test);
  };
}

/**
 * Declared: a test that does not run (`test.skip(name, fn)`). Inside a
 * running test or its `beforeEach`, as Playwright's: `test.skip()` ends the
 * test as skipped, `test.skip(condition, reason?)` when `condition` holds
 * (`test.skip(reason)` too). Its `afterEach` hooks and deferred work still run.
 */
type Skip = TestFn & ((condition?: unknown, reason?: string) => void);

function skip(...args: unknown[]): void {
  const running = registry().context.getStore();
  if (typeof args[1] === 'function' || !running) {
    return define({ skip: true })(args[0] as string, args[1] as Fn, args[2] as TimeoutArg);
  }
  if (!running.test) throw new Error('test.skip() skips the running test: call it inside a test or beforeEach, not in a beforeAll');
  const [condition, reason] = args.length === 0 ? [true, undefined] : typeof args[0] === 'string' && args.length === 1 ? [true, args[0]] : [args[0], args[1]];
  if (!condition) return;
  const signal = new Error(reason === undefined ? 'skipped' : `skipped: ${String(reason)}`) as SkipSignal & { reason?: string };
  signal[SKIP] = true;
  if (reason !== undefined) signal.reason = String(reason);
  throw signal;
}

type Test = TestFn & {
  /** Runs alone in its describe: after every test declared before it, before any declared after it. */
  serial: TestFn;
  /** Same as `test`: every test runs concurrently unless it is serial. */
  concurrent: TestFn;
  skip: Skip;
  todo: (name: string, fn?: Fn) => void;
  /** Runs only with `pyro e2e --only`. */
  only: TestFn;
  if: (condition: unknown) => TestFn;
  skipIf: (condition: unknown) => TestFn;
};

export const test: Test = Object.assign(define({}), {
  serial: define({ serial: true }),
  concurrent: define({}),
  skip: skip as Skip,
  todo: (name: string, fn?: Fn) => define({ todo: true })(name, fn as Fn),
  only: define({ only: true }),
  if: (condition: unknown) => define({ skip: !condition }),
  skipIf: (condition: unknown) => define({ skip: !!condition }),
});
export const it = test;

type DescribeFn = (name: unknown, fn: () => void) => void;

function group(flags: Flags & { serialAll?: boolean }): DescribeFn {
  return (name, fn) => {
    const parent = collector('describe');
    const scope = newScope('describe', typeof name === 'function' ? name.name : String(name), parent.file, parent);
    scope.serial = flags.serialAll ?? false;
    scope.skip = flags.skip ?? false;
    scope.todo = flags.todo ?? false;
    scope.only = flags.only ?? false;
    parent.children.push(scope);
    const r = registry();
    r.collecting = scope;
    try {
      const result = fn() as unknown;
      if (result && typeof (result as Promise<unknown>).then === 'function') {
        throw new Error(`describe('${scope.name}') must register its tests synchronously`);
      }
    } finally {
      r.collecting = parent;
    }
  };
}

type Describe = DescribeFn & {
  /** Its tests run one at a time, in order. */
  serial: DescribeFn;
  skip: DescribeFn;
  todo: DescribeFn;
  only: DescribeFn;
  if: (condition: unknown) => DescribeFn;
  skipIf: (condition: unknown) => DescribeFn;
};

export const describe: Describe = Object.assign(group({}), {
  serial: group({ serialAll: true }),
  skip: group({ skip: true }),
  todo: group({ todo: true }),
  only: group({ only: true }),
  if: (condition: unknown) => group({ skip: !condition }),
  skipIf: (condition: unknown) => group({ skip: !!condition }),
});

const hook = (kind: 'beforeAll' | 'afterAll' | 'beforeEach' | 'afterEach') => (fn: Fn, timeout?: TimeoutArg) => {
  collector(kind)[kind].push({ fn, timeout: timeoutOf(timeout) });
};

/** Once, before the first test of this describe (or file) runs. */
export const beforeAll = hook('beforeAll');
/** Once, after the last test of this describe (or file) finished. */
export const afterAll = hook('afterAll');
/** Before each test of this describe (or file), outer hooks first. */
export const beforeEach = hook('beforeEach');
/** After each test of this describe (or file), inner hooks first. */
export const afterEach = hook('afterEach');

/** The timeout of tests and hooks without one of their own (default 5 s, or `--timeout`). */
export function setDefaultTimeout(ms: number): void {
  registry().defaultTimeout = ms;
}

/**
 * Runs `fn` once what is running now is done: called from a test (or its
 * `beforeEach`/`afterEach`), after that test; from a describe's (or file's)
 * `beforeAll`/`afterAll` or while it registers, after that describe's last
 * test and its `afterAll` hooks. Later calls run first.
 *
 *   beforeAll(async () => { await lp.profile(name, def); defer(() => lp.releaseProfile(name)); });
 */
export function defer(fn: Fn): void {
  const r = registry();
  const running = r.context.getStore();
  if (running?.test?.run) running.test.run.defers.push(fn);
  else if (running) running.scope.state.defers.push(fn);
  else if (r.collecting) r.collecting.state.defers.push(fn);
  else throw new Error('defer() runs inside a test, a hook or a describe of `pyro e2e`');
}

/**
 * The run's browser: one `pyro serve --fork` the executor launches
 * on first use (with the setup module's `browser()` options) and closes when
 * the run ends, however it ends. Pages a test opens through it are closed
 * when the test ends (a test that timed out included).
 */
export async function browser(): Promise<Pyro> {
  const r = registry();
  if (!r.browser) throw new Error('browser() is for spec files run by `pyro e2e`');
  return (await r.browser()) as Pyro;
}

/** What the setup module's `setup()` returned (`--setup`). */
export function setupResult<T = unknown>(): T {
  const r = registry();
  if (!r.hasSetup) throw new Error('no setup module: pass one with `pyro e2e --setup <module>`');
  return r.setupResult as T;
}

/**
 * The spec file running now, relative to the working directory: in a test, its
 * hooks, and a file's or describe's `beforeAll`/`afterAll`. Undefined outside a
 * spec file (the setup module, the run's own hooks).
 */
export function currentFile(): string | undefined {
  const running = registry().context.getStore();
  return running?.test?.scope.file ?? (running?.scope.file || undefined);
}

/** The test running now (in a test or its `beforeEach`/`afterEach`). */
export function testInfo(): { file: string; name: string; titlePath: string[] } | undefined {
  const test = registry().context.getStore()?.test;
  return test && { file: test.scope.file, name: test.name, titlePath: titlePath(test) };
}

/**
 * A setup module (`pyro e2e --setup <module>`): its default export,
 * or the same names exported one by one. `setup()` runs before any spec file
 * loads; specs read its result with `setupResult()`. `browser()` gives the
 * run's browser its options. `teardown()` runs after the run, also when it is
 * interrupted.
 */
export type Setup<T = unknown> = {
  setup?: () => T | Promise<T>;
  browser?: (result: T) => BrowserOptions | Promise<BrowserOptions>;
  teardown?: (result: T) => unknown;
};

export type BrowserOptions = {
  /** Extra `pyro serve` arguments (`--static`, `--proxy`, ...); `--fork` is added. */
  args?: string[];
  /** The browser binary (default: `$PYRO_BIN`, else the bundled one). */
  bin?: string;
};
