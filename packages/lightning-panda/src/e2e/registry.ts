// What spec files register (describes, tests, hooks) and what the executor
// runs, shared through `globalThis`: the executor and the specs may load
// different copies of this module (a built `dist/` CLI, specs resolving
// `src/` through the `bun` export condition), so nothing here relies on
// module identity or `instanceof`.
import { AsyncLocalStorage } from 'node:async_hooks';

export type Fn = (...args: any[]) => unknown;

export type Hook = { fn: Fn; timeout?: number };

export type Scope = {
  kind: 'run' | 'file' | 'describe';
  name: string;
  /** Spec file path relative to the working directory ('' for the run). */
  file: string;
  parent?: Scope;
  children: (Scope | TestCase)[];
  beforeAll: Hook[];
  afterAll: Hook[];
  beforeEach: Hook[];
  afterEach: Hook[];
  /** `describe.serial`: its tests run one at a time. */
  serial: boolean;
  skip: boolean;
  todo: boolean;
  only: boolean;
  /** Set by the executor. */
  state: ScopeState;
};

export type ScopeState = {
  /** Tests below it that will run and have not finished. */
  remaining: number;
  /** A test that will run its beforeAll has started: the scope's other tests wait for it. */
  claimed: boolean;
  /** beforeAll: not started, running, or done with its error (if any). */
  setup?: Promise<unknown>;
  setupDone: boolean;
  setupError?: unknown;
  finished: boolean;
  /** `defer()` calls made while it was set up or its hooks ran. */
  defers: Fn[];
  /** Direct tests running now, and whether one of them is serial. */
  running: number;
  runningSerial: boolean;
  /** Its direct tests that will run and have not finished, by index. */
  unfinished: Set<number>;
  unfinishedSerial: Set<number>;
};

export type TestCase = {
  kind: 'test';
  name: string;
  fn: Fn;
  scope: Scope;
  /** Position among its scope's children. */
  index: number;
  timeout?: number;
  serial: boolean;
  skip: boolean;
  todo: boolean;
  only: boolean;
  /** Set by the executor. */
  run?: TestRun;
};

export type TestRun = {
  status: 'pass' | 'fail' | 'skip' | 'todo';
  error?: unknown;
  /** Start, in ms since the run began, and duration. */
  start: number;
  ms: number;
  defers: Fn[];
  /** Pages the test opened through the run's browser, closed when it ends. */
  pages: Set<{ close(): void }>;
};

/** What runs now: a test (with its scope) or a scope's own hooks. */
export type Running = { scope: Scope; test?: TestCase };

export type Registry = {
  /** Where describe/test/hooks register: set while a spec file (or preload) loads. */
  collecting?: Scope;
  context: AsyncLocalStorage<Running>;
  defaultTimeout: number;
  setupResult: unknown;
  hasSetup: boolean;
  browser?: () => Promise<unknown>;
  /** The executor is running (the API refuses to be used outside it). */
  active: boolean;
};

const KEY = Symbol.for('@povio/lightning-panda/e2e');

export function registry(): Registry {
  const g = globalThis as { [KEY]?: Registry };
  return (g[KEY] ??= {
    context: new AsyncLocalStorage<Running>(),
    defaultTimeout: 5_000,
    setupResult: undefined,
    hasSetup: false,
    active: false,
  });
}

export function newScope(kind: Scope['kind'], name: string, file: string, parent?: Scope): Scope {
  return {
    kind,
    name,
    file,
    parent,
    children: [],
    beforeAll: [],
    afterAll: [],
    beforeEach: [],
    afterEach: [],
    serial: false,
    skip: false,
    todo: false,
    only: false,
    state: {
      remaining: 0,
      claimed: false,
      setupDone: false,
      finished: false,
      defers: [],
      running: 0,
      runningSerial: false,
      unfinished: new Set(),
      unfinishedSerial: new Set(),
    },
  };
}

/** The scopes a test runs in, outermost first. */
export function chain(scope: Scope): Scope[] {
  const scopes: Scope[] = [];
  for (let s: Scope | undefined = scope; s; s = s.parent) scopes.unshift(s);
  return scopes;
}

/** The describes' and the test's names, outermost first (no file, no run). */
export function titlePath(test: TestCase): string[] {
  return [...chain(test.scope).filter((s) => s.kind === 'describe').map((s) => s.name), test.name];
}
