// `pyro e2e`: runs spec files in this process. Tests from every file
// go into one queue that runs up to `concurrency` of them at a time, so cases
// of different files interleave; they share one fork-mode browser, which this
// executor launches on first use and closes when the run ends.
import { spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join, relative, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import { chain, newScope, registry, titlePath, type Fn, type Scope, type TestCase, type TestRun } from './registry.js';
import { consoleReporter, junitReport, type Failure, type Summary } from './report.js';
import { resolveBinary } from '../binary.js';
import type { BrowserOptions, Setup } from './index.js';

export type Options = {
  cwd: string;
  /** Files, directories, globs, or substrings of spec paths (as `bun test` takes them). */
  paths: string[];
  concurrency: number;
  /** Tests of one file at a time (`bun test --max-concurrency`); default unlimited. */
  perFile?: number;
  /** Default timeout of tests and hooks, ms. */
  timeout: number;
  testNamePattern?: RegExp;
  only: boolean;
  shard?: { index: number; count: number };
  /** `{"version": 1, "files": {"<path>": ms}}`, paths relative to `cwd`. */
  timings?: string;
  updateTimings: boolean;
  junit?: string;
  bail?: number;
  setup?: string;
  preload: string[];
  browserArgs: string[];
  updateSnapshots: boolean;
};

const SPEC = /[._](test|spec)\.[cm]?[jt]sx?$/;

/** Spec files for `paths` (all of `cwd` when none name a path), relative to `cwd`. */
export function discover(cwd: string, paths: string[]): string[] {
  const files = new Set<string>();
  const filters: string[] = [];
  let named = false;
  const walk = (dir: string) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.name === 'node_modules' || entry.name.startsWith('.')) continue;
      const path = join(dir, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile() && SPEC.test(entry.name)) files.add(path);
    }
  };
  for (const arg of paths) {
    const path = resolve(cwd, arg);
    const stat = statSync(path, { throwIfNoEntry: false });
    if (stat?.isDirectory()) {
      named = true;
      walk(path);
    } else if (stat?.isFile()) {
      named = true;
      files.add(path);
    } else if (/[*?[{]/.test(arg)) {
      named = true;
      const glob = new (globalThis as any).Bun.Glob(arg);
      for (const match of glob.scanSync({ cwd, absolute: true, onlyFiles: true })) {
        if (!match.split(sep).includes('node_modules')) files.add(match);
      }
    } else filters.push(arg);
  }
  if (!named) walk(cwd);
  return [...files]
    .map((file) => relative(cwd, file))
    .filter((file) => !filters.length || filters.some((f) => file.includes(f)))
    .sort();
}

export function readTimings(file?: string): Map<string, number> {
  if (!file) return new Map();
  try {
    const json = JSON.parse(readFileSync(file, 'utf8')) as { files?: Record<string, number> };
    return new Map(Object.entries(json.files ?? {}).filter(([, ms]) => typeof ms === 'number'));
  } catch {
    return new Map();
  }
}

/**
 * The files of shard `index` (1-based) of `count`, balanced by their recorded
 * durations (longest first onto the least loaded shard; files without one
 * count as the average). Every shard computes the same split. Longest first.
 */
export function shardFiles(files: string[], timings: Map<string, number>, index: number, count: number): string[] {
  const known = files.map((f) => timings.get(f)).filter((ms): ms is number => ms !== undefined);
  const fallback = known.length ? known.reduce((a, b) => a + b, 0) / known.length : 1;
  const weighted = files
    .map((file) => ({ file, ms: timings.get(file) ?? fallback }))
    .sort((a, b) => b.ms - a.ms || (a.file < b.file ? -1 : a.file > b.file ? 1 : 0));
  const loads = new Array<number>(count).fill(0);
  const shards: string[][] = Array.from({ length: count }, () => []);
  for (const { file, ms } of weighted) {
    let least = 0;
    for (let i = 1; i < count; i++) if (loads[i] < loads[least]) least = i;
    loads[least] += ms;
    shards[least].push(file);
  }
  return shards[index - 1] ?? [];
}

class TimeoutError extends Error {
  override name = 'TimeoutError';
}

class HookError extends Error {
  override name = 'HookError';
  constructor(message: string, readonly cause: unknown) {
    super(`${message}: ${cause instanceof Error ? cause.message : String(cause)}`);
    if (cause instanceof Error && cause.stack) this.stack = `${this.name}: ${this.message}\n${cause.stack.split('\n').slice(1).join('\n')}`;
  }
}

/** Calls `fn` (a promise, a `done` callback, or sync) and fails it after `ms`. */
function call(fn: Fn, ms: number, what: string): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const timer = ms > 0 && Number.isFinite(ms) ? setTimeout(() => reject(new TimeoutError(`${what} timed out after ${ms} ms`)), ms) : undefined;
    const settle = (error?: unknown, failed = error !== undefined) => {
      clearTimeout(timer);
      if (failed) reject(error ?? new Error(`${what} failed`));
      else resolve();
    };
    try {
      if (fn.length > 0) {
        fn((error?: unknown) => settle(error ?? undefined));
        return;
      }
      const result = fn() as Promise<unknown> | undefined;
      if (result && typeof result.then === 'function') result.then(() => settle(), (error) => settle(error, true));
      else settle();
    } catch (error) {
      settle(error, true);
    }
  });
}

const scopeName = (s: Scope) => (s.kind === 'run' ? 'the run' : s.kind === 'file' ? s.file : `describe '${s.name}'`);

/**
 * After a test timed out: the runs its pages still wait on are cancelled
 * (`Pyro.cancelRun`; a run's script that never returns is stopped), so its
 * afterEach hooks find the pages as they were at the timeout and not busy.
 * At most `ms`; a browser that does not answer is left to the page closing.
 */
async function cancelRuns(pages: TestRun['pages'], ms: number): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const cancelled = Promise.all([...pages].map((p) => p.send?.('Pyro.cancelRun').catch(() => {})));
  await Promise.race([cancelled, new Promise((r) => (timer = setTimeout(r, ms)))]);
  clearTimeout(timer);
}

export class Executor {
  private readonly r = registry();
  private readonly root = newScope('run', '', '');
  private queue: TestCase[] = [];
  private tests: TestCase[] = [];
  private filtered = 0;
  private failures: Failure[] = [];
  private loadFailures = 0;
  private hookFailures = 0;
  private failed = 0;
  private stopped = false;
  private aborted = false;
  private readonly started = performance.now();
  /** Busy time (tests and hooks) per file, ms. */
  private busy = new Map<string, number>();
  private setupModule?: Setup<any>;
  private launched?: Promise<{ close(): void; page: Fn }>;
  private browserInstance?: { close(): void };
  private torn = false;
  private files: string[] = [];
  private readonly report = consoleReporter();
  private readonly chains = new Map<TestCase, Scope[]>();
  /** Tests running now, per file. */
  private readonly fileRunning = new Map<string, number>();
  /** Starts what can start now (set while the queue runs). */
  private pump?: () => void;

  constructor(private readonly o: Options) {}

  private now() {
    return performance.now() - this.started;
  }

  private charge(file: string, ms: number) {
    if (file) this.busy.set(file, (this.busy.get(file) ?? 0) + ms);
  }

  /** Runs everything; the exit code. */
  async run(): Promise<number> {
    const { o, r } = this;
    process.env.NODE_ENV ??= 'test';
    if (o.updateSnapshots) process.env.PYRO_UPDATE_SNAPSHOTS = '1';
    r.active = true;
    r.defaultTimeout = o.timeout;
    r.browser = () => (this.launched ??= this.launch());
    process.once('exit', () => this.browserInstance?.close());
    // Outside `bun test` these would end the process: they fail the run instead.
    const stray = (kind: string) => (error: unknown) => {
      const test = r.context.getStore()?.test;
      this.hookFailures++;
      this.failures.push({ title: `(${kind}${test ? ` during ${titlePath(test).join(' > ')}` : ''})`, file: test?.scope.file ?? '', error });
      this.report.hookFailure(test?.scope.file ?? '', `(${kind})`, error);
    };
    process.on('unhandledRejection', stray('unhandled rejection'));
    process.on('uncaughtException', stray('uncaught exception'));

    const all = discover(o.cwd, o.paths);
    const timings = readTimings(o.timings && resolve(o.cwd, o.timings));
    let files = o.shard ? shardFiles(all, timings, o.shard.index, o.shard.count) : all;
    if (!o.shard && timings.size) files = shardFiles(all, timings, 1, 1); // longest first
    this.files = files;
    if (!all.length) {
      console.error('pyro e2e: no spec files found');
      return 1;
    }
    if (!files.length) {
      console.log(`No spec files in shard ${o.shard!.index}/${o.shard!.count}.`);
      return 0;
    }

    try {
      if (o.setup) await this.globalSetup(resolve(o.cwd, o.setup));
      await this.collect(files);
      this.select(this.root, false, false, false);
      if (!this.queue.length && !this.loadFailures) {
        console.error(`pyro e2e: no tests matched in ${files.length} file${files.length === 1 ? '' : 's'}`);
        return 1;
      }
      await this.schedule();
      await this.drain();
    } catch (error) {
      this.failures.push({ title: 'pyro e2e', file: '', error });
      this.failed++;
    } finally {
      await this.finish();
    }
    const summary = this.summary();
    this.report.summary(summary);
    if (o.junit) writeFileSync(resolve(o.cwd, o.junit), junitReport(summary));
    if (o.updateTimings && o.timings && !o.testNamePattern && !o.only && !this.stopped) this.writeTimings(resolve(o.cwd, o.timings), timings);
    return summary.failed || summary.hookFailures || summary.loadFailures || summary.notRun ? 1 : 0;
  }

  /** Stops on a signal: no new tests, the browser closes, teardown runs. */
  async abort(): Promise<void> {
    this.stopped = this.aborted = true;
    await this.finish();
  }

  private async globalSetup(path: string) {
    const mod = (await import(pathToFileURL(path).href)) as Setup<unknown> & { default?: Setup<unknown> };
    const setup: Setup<unknown> = mod.default && typeof mod.default === 'object' ? mod.default : mod;
    this.setupModule = setup;
    this.r.hasSetup = true;
    this.r.setupResult = setup.setup ? await setup.setup() : undefined;
  }

  private async launch() {
    // The SDK by its package name: the copy the spec files import.
    const { Pyro } = (await import('@povio/pyro')) as unknown as typeof import('../index.js');
    const options: BrowserOptions = this.setupModule?.browser ? await this.setupModule.browser(this.r.setupResult) : {};
    const extra = [...(options.args ?? []), ...this.o.browserArgs];
    const connections = extra.includes('--cdp-max-connections') ? [] : ['--cdp-max-connections', String(Math.max(64, this.o.concurrency * 4 + 8))];
    // A browser without fork mode (the Intel macOS build): pages boot their profile.
    const bin = options.bin ?? resolveBinary();
    const version = spawnSync(bin, ['version'], { encoding: 'utf8' });
    const fork = /\bfork: true\b/.test(version.stdout ?? '');
    if (!fork) console.warn(`pyro e2e: ${bin} has no fork mode; every page boots its profile`);
    const lp = await Pyro.launch({ bin, args: [...(fork ? ['--fork'] : []), ...connections, ...extra] });
    if (this.aborted) {
      lp.close();
      throw new Error('the run was interrupted');
    }
    this.browserInstance = lp;
    // Pages a test opens are closed when it ends, so a test that timed out
    // or forgot one does not keep its forked process.
    const page = lp.page.bind(lp);
    lp.page = async (opts?: Parameters<typeof page>[0]) => {
      const p = await page(opts);
      this.r.context.getStore()?.test?.run?.pages.add(p);
      return p;
    };
    return lp;
  }

  private async collect(files: string[]) {
    const { r } = this;
    for (const preload of this.o.preload) {
      r.collecting = this.root;
      try {
        await import(pathToFileURL(resolve(this.o.cwd, preload)).href);
      } catch (error) {
        this.loadFailures++;
        this.failures.push({ title: '(load)', file: preload, error });
        this.report.loadFailure(preload, error);
      } finally {
        r.collecting = undefined;
      }
    }
    for (const file of files) {
      const scope = newScope('file', file, file, this.root);
      this.root.children.push(scope);
      r.collecting = scope;
      try {
        await import(pathToFileURL(resolve(this.o.cwd, file)).href);
      } catch (error) {
        this.loadFailures++;
        this.failures.push({ title: '(load)', file, error });
        this.report.loadFailure(file, error);
      } finally {
        r.collecting = undefined;
      }
    }
  }

  /** Marks what runs (in declaration order), what is skipped and what is filtered out. */
  private select(scope: Scope, skip: boolean, todo: boolean, only: boolean) {
    for (const child of scope.children) {
      if (child.kind !== 'test') {
        this.select(child, skip || child.skip, todo || child.todo, only || child.only);
        continue;
      }
      this.tests.push(child);
      const name = titlePath(child).join(' ');
      if (todo || child.todo) child.run = { status: 'todo', start: 0, ms: 0, defers: [], pages: new Set() };
      else if (skip || child.skip) child.run = { status: 'skip', start: 0, ms: 0, defers: [], pages: new Set() };
      else if ((this.o.testNamePattern && !this.o.testNamePattern.test(name)) || (this.o.only && !(only || child.only))) this.filtered++;
      else {
        this.queue.push(child);
        for (const s of chain(child.scope)) s.state.remaining++;
        child.scope.state.unfinished.add(child.index);
        if (this.serial(child)) child.scope.state.unfinishedSerial.add(child.index);
      }
    }
  }

  private serial(test: TestCase) {
    return test.serial || test.scope.serial;
  }

  private scopes(test: TestCase): Scope[] {
    let scopes = this.chains.get(test);
    if (!scopes) this.chains.set(test, (scopes = chain(test.scope)));
    return scopes;
  }

  private startable(test: TestCase): boolean {
    if (this.o.perFile && (this.fileRunning.get(test.scope.file) ?? 0) >= this.o.perFile) return false;
    for (const s of this.scopes(test)) if (s.state.claimed && !s.state.setupDone) return false; // its beforeAll runs
    const st = test.scope.state;
    let first = Infinity;
    for (const i of st.unfinished) if (i < first) first = i;
    if (this.serial(test)) return st.running === 0 && first === test.index;
    if (st.runningSerial) return false;
    for (const i of st.unfinishedSerial) if (i < test.index) return false;
    return true;
  }

  private schedule(): Promise<void> {
    return new Promise<void>((resolveAll) => {
      let running = 0;
      const pump = () => {
        while (!this.stopped && running < this.o.concurrency) {
          const at = this.queue.findIndex((t) => this.startable(t));
          if (at < 0) break;
          const [test] = this.queue.splice(at, 1);
          // This test runs the beforeAll hooks still to run: the others of those scopes wait, without a slot.
          for (const s of this.scopes(test)) s.state.claimed = true;
          running++;
          const st = test.scope.state;
          st.running++;
          const file = test.scope.file;
          this.fileRunning.set(file, (this.fileRunning.get(file) ?? 0) + 1);
          if (this.serial(test)) st.runningSerial = true;
          void this.execute(test).finally(() => {
            running--;
            st.running--;
            this.fileRunning.set(file, this.fileRunning.get(file)! - 1);
            if (this.serial(test)) st.runningSerial = false;
            pump();
          });
        }
        if (running === 0) resolveAll();
      };
      this.pump = pump;
      pump();
    });
  }

  private setup(scope: Scope): Promise<unknown> {
    const st = scope.state;
    st.setup ??= (async () => {
      const t0 = performance.now();
      for (const hook of scope.beforeAll) {
        try {
          await this.r.context.run({ scope }, () => call(hook.fn, hook.timeout ?? this.r.defaultTimeout, `beforeAll of ${scopeName(scope)}`));
        } catch (error) {
          st.setupError = error;
          break;
        }
      }
      st.setupDone = true;
      this.charge(scope.file, performance.now() - t0);
      // The tests that waited for it can start now, not when another test ends.
      queueMicrotask(() => this.pump?.());
    })();
    return st.setup;
  }

  private async execute(test: TestCase) {
    const run: TestRun = { status: 'pass', start: this.now(), ms: 0, defers: [], pages: new Set() };
    test.run = run;
    const scopes = this.scopes(test);
    let error: unknown;
    for (const s of scopes) {
      await this.setup(s);
      if (s.state.setupError !== undefined) {
        error = new HookError(`beforeAll of ${scopeName(s)} failed`, s.state.setupError);
        break;
      }
    }
    const t0 = performance.now();
    if (error === undefined) {
      const timeout = test.timeout ?? this.r.defaultTimeout;
      error = await this.r.context.run({ scope: test.scope, test }, async () => {
        let failure: unknown;
        try {
          for (const s of scopes) for (const h of s.beforeEach) await call(h.fn, h.timeout ?? timeout, 'beforeEach hook');
          await call(test.fn, timeout, 'test');
        } catch (e) {
          failure = e;
          if (e instanceof TimeoutError) await cancelRuns(run.pages, 1_000);
        }
        for (const s of [...scopes].reverse()) {
          for (const h of s.afterEach) {
            try {
              await call(h.fn, h.timeout ?? timeout, 'afterEach hook');
            } catch (e) {
              failure ??= e;
            }
          }
        }
        for (const d of run.defers.reverse()) {
          try {
            await call(d, timeout, 'deferred cleanup');
          } catch (e) {
            failure ??= e;
          }
        }
        return failure;
      });
    }
    for (const page of run.pages) {
      try {
        page.close();
      } catch {}
    }
    run.ms = performance.now() - t0;
    this.charge(test.scope.file, run.ms);
    if (error !== undefined) {
      run.status = 'fail';
      run.error = error;
      this.failed++;
      this.failures.push({ title: titlePath(test).join(' > '), file: test.scope.file, error });
      if (this.o.bail && this.failed >= this.o.bail) this.stopped = true;
    }
    this.report.test(test);
    test.scope.state.unfinished.delete(test.index);
    test.scope.state.unfinishedSerial.delete(test.index);
    // A describe (then its file, then the run) whose last test this was.
    for (const s of [...scopes].reverse()) {
      s.state.remaining--;
      if (s.state.remaining === 0) await this.close(s);
    }
  }

  /** A scope's afterAll hooks and deferred work, once. */
  private async close(scope: Scope) {
    const st = scope.state;
    if (st.finished) return;
    st.finished = true;
    const t0 = performance.now();
    if (st.setup) {
      for (const hook of scope.afterAll) {
        try {
          await this.r.context.run({ scope }, () => call(hook.fn, hook.timeout ?? this.r.defaultTimeout, `afterAll of ${scopeName(scope)}`));
        } catch (error) {
          this.hookFailure(scope, 'afterAll', error);
        }
      }
    }
    for (const d of st.defers.reverse()) {
      try {
        await this.r.context.run({ scope }, () => call(d, this.r.defaultTimeout, 'deferred cleanup'));
      } catch (error) {
        this.hookFailure(scope, 'deferred cleanup', error);
      }
    }
    this.charge(scope.file, performance.now() - t0);
  }

  private hookFailure(scope: Scope, what: string, error: unknown) {
    this.hookFailures++;
    const title = `(${what} of ${scopeName(scope)})`;
    this.failures.push({ title, file: scope.file, error });
    this.report.hookFailure(scope.file, title, error);
  }

  /** After a stop (bail, signal): closes the scopes that were set up, innermost first. */
  private async drain() {
    const open: Scope[] = [];
    const visit = (s: Scope) => {
      for (const c of s.children) if (c.kind !== 'test') visit(c);
      if (!s.state.finished && (s.state.setup || s.state.defers.length)) open.push(s);
    };
    visit(this.root);
    for (const s of open) await this.close(s);
  }

  /** Closes the browser, then the setup module's teardown; once. */
  private async finish() {
    if (this.torn) return;
    this.torn = true;
    try {
      this.browserInstance?.close();
    } catch {}
    if (this.setupModule?.teardown) {
      try {
        await this.setupModule.teardown(this.r.setupResult);
      } catch (error) {
        this.hookFailures++;
        this.failures.push({ title: '(teardown)', file: this.o.setup ?? '', error });
        this.report.hookFailure(this.o.setup ?? '', '(teardown)', error);
      }
    }
    this.r.active = false;
  }

  private summary(): Summary {
    const counts = { pass: 0, fail: 0, skip: 0, todo: 0 };
    for (const t of this.tests) if (t.run) counts[t.run.status]++;
    return {
      tests: this.tests,
      files: this.files,
      passed: counts.pass,
      failed: counts.fail,
      skipped: counts.skip,
      todo: counts.todo,
      filtered: this.filtered,
      notRun: this.queue.length,
      loadFailures: this.loadFailures,
      hookFailures: this.hookFailures,
      failures: this.failures,
      ms: this.now(),
      interrupted: this.aborted,
    };
  }

  private writeTimings(path: string, previous: Map<string, number>) {
    const files = new Map(previous);
    for (const [file, ms] of this.busy) files.set(file, Math.round(ms));
    const sorted = Object.fromEntries([...files].sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)));
    writeFileSync(path, `${JSON.stringify({ version: 1, files: sorted }, null, 2)}\n`);
  }
}
