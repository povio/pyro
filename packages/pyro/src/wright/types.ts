// Public types of the test runner, shaped like `@playwright/test`'s so code
// written for Playwright type-checks unchanged, and limited to what this
// package implements: an API it does not have is a type error, not a run-time
// surprise.
import type { Browser, BrowserContext, BrowserContextOptions, BootSpec, chromium } from './browser.js';
import type { Expect } from './expect.js';
import type { TestInfo } from './context.js';
import type { Page } from './page.js';
import type { APIRequestContext, request } from './request.js';
import type { devices } from './config.js';
import type { TimeoutError } from './page.js';

export type KeyValue = { [key: string]: any };


export type TestStatus = 'passed' | 'failed' | 'timedOut' | 'skipped' | 'interrupted';
export type TestAnnotation = { type: string; description?: string };
export type TestDetails = { tag?: string | string[]; annotation?: TestAnnotation | TestAnnotation[] };

export interface WorkerInfo {
  workerIndex: number;
  parallelIndex: number;
  project: TestInfo['project'];
  config: TestInfo['config'];
}

export type TestFixture<R, Args extends {}> = (args: Args, use: (r: R) => Promise<void>, testInfo: TestInfo) => any;
export type WorkerFixture<R, Args extends {}> = (args: Args, use: (r: R) => Promise<void>, workerInfo: WorkerInfo) => any;
type TestFixtureValue<R, Args extends {}> = Exclude<R, Function> | TestFixture<R, Args>;
type WorkerFixtureValue<R, Args extends {}> = Exclude<R, Function> | WorkerFixture<R, Args>;
type FixtureDetails = { timeout?: number | undefined; title?: string; box?: boolean | 'self' };

/**
 * `test.extend()`'s argument: new test fixtures `T`, new worker fixtures `W`,
 * and overrides of the fixtures and options already there (`PT`, `PW`).
 */
export type Fixtures<T extends {} = {}, W extends {} = {}, PT extends {} = {}, PW extends {} = {}> = {
  [K in keyof PW]?: WorkerFixtureValue<PW[K], W & PW> | [WorkerFixtureValue<PW[K], W & PW>, { scope: 'worker' } & FixtureDetails];
} & {
  [K in keyof PT]?: TestFixtureValue<PT[K], T & W & PT & PW> | [TestFixtureValue<PT[K], T & W & PT & PW>, { scope: 'test' } & FixtureDetails];
} & {
  [K in Exclude<keyof W, keyof PW | keyof PT>]?: [WorkerFixtureValue<W[K], W & PW>, { scope: 'worker'; auto?: boolean; option?: boolean } & FixtureDetails];
} & {
  [K in Exclude<keyof T, keyof PW | keyof PT>]?: TestFixtureValue<T[K], T & W & PT & PW> | [TestFixtureValue<T[K], T & W & PT & PW>, { scope?: 'test'; auto?: boolean; option?: boolean } & FixtureDetails];
};

export type TestBody<Args> = (args: Args, testInfo: TestInfo) => Promise<void> | void;
export type ConditionBody<Args> = (args: Args) => boolean;
type Hook<Args> = (args: Args, testInfo: TestInfo) => Promise<any> | any;

export interface TestStepInfo {
  skip(condition?: boolean, description?: string): void;
}
type StepOptions = { box?: boolean; timeout?: number };

interface SuiteFunction {
  (title: string, callback: () => void): void;
  (callback: () => void): void;
  (title: string, details: TestDetails, callback: () => void): void;
  only(title: string, callback: () => void): void;
  only(callback: () => void): void;
  only(title: string, details: TestDetails, callback: () => void): void;
}

export interface TestType<TestArgs extends {}, WorkerArgs extends {}> {
  (title: string, body: TestBody<TestArgs & WorkerArgs>): void;
  (title: string, details: TestDetails, body: TestBody<TestArgs & WorkerArgs>): void;
  only(title: string, body: TestBody<TestArgs & WorkerArgs>): void;
  only(title: string, details: TestDetails, body: TestBody<TestArgs & WorkerArgs>): void;
  describe: {
    (title: string, callback: () => void): void;
    (callback: () => void): void;
    (title: string, details: TestDetails, callback: () => void): void;
    only(title: string, callback: () => void): void;
    only(callback: () => void): void;
    only(title: string, details: TestDetails, callback: () => void): void;
    skip(title: string, callback: () => void): void;
    skip(callback: () => void): void;
    skip(title: string, details: TestDetails, callback: () => void): void;
    fixme(title: string, callback: () => void): void;
    fixme(callback: () => void): void;
    fixme(title: string, details: TestDetails, callback: () => void): void;
    serial: SuiteFunction;
    parallel: SuiteFunction;
    configure: (options: { mode?: 'default' | 'parallel' | 'serial'; retries?: number; timeout?: number }) => void;
  };
  skip(title: string, body: TestBody<TestArgs & WorkerArgs>): void;
  skip(title: string, details: TestDetails, body: TestBody<TestArgs & WorkerArgs>): void;
  skip(): void;
  skip(condition: boolean, description?: string): void;
  skip(callback: ConditionBody<TestArgs & WorkerArgs>, description?: string): void;
  fixme(title: string, body: TestBody<TestArgs & WorkerArgs>): void;
  fixme(title: string, details: TestDetails, body: TestBody<TestArgs & WorkerArgs>): void;
  fixme(): void;
  fixme(condition: boolean, description?: string): void;
  fixme(callback: ConditionBody<TestArgs & WorkerArgs>, description?: string): void;
  fail: {
    (title: string, body: TestBody<TestArgs & WorkerArgs>): void;
    (title: string, details: TestDetails, body: TestBody<TestArgs & WorkerArgs>): void;
    (condition: boolean, description?: string): void;
    (callback: ConditionBody<TestArgs & WorkerArgs>, description?: string): void;
    (): void;
    only(title: string, body: TestBody<TestArgs & WorkerArgs>): void;
    only(title: string, details: TestDetails, body: TestBody<TestArgs & WorkerArgs>): void;
  };
  slow(): void;
  slow(condition: boolean, description?: string): void;
  slow(callback: ConditionBody<TestArgs & WorkerArgs>, description?: string): void;
  setTimeout(timeout: number): void;
  beforeEach(inner: Hook<TestArgs & WorkerArgs>): void;
  beforeEach(title: string, inner: Hook<TestArgs & WorkerArgs>): void;
  afterEach(inner: Hook<TestArgs & WorkerArgs>): void;
  afterEach(title: string, inner: Hook<TestArgs & WorkerArgs>): void;
  /** Worker fixtures only at run time (a test fixture is an error, as in Playwright). */
  beforeAll(inner: Hook<TestArgs & WorkerArgs>): void;
  beforeAll(title: string, inner: Hook<TestArgs & WorkerArgs>): void;
  afterAll(inner: Hook<TestArgs & WorkerArgs>): void;
  afterAll(title: string, inner: Hook<TestArgs & WorkerArgs>): void;
  use(fixtures: Fixtures<{}, {}, TestArgs, WorkerArgs>): void;
  step: {
    <T>(title: string, body: (step: TestStepInfo) => T | Promise<T>, options?: StepOptions): Promise<T>;
    skip(title: string, body: (step: TestStepInfo) => any | Promise<any>, options?: StepOptions): Promise<void>;
  };
  expect: Expect;
  extend<T extends {}, W extends {} = {}>(fixtures: Fixtures<T, W, TestArgs, WorkerArgs>): TestType<TestArgs & T, WorkerArgs & W>;
  info(): TestInfo;
}

export type ViewportSize = { width: number; height: number };
export type StorageState = {
  cookies: { name: string; value: string; domain: string; path: string; expires: number; httpOnly: boolean; secure: boolean; sameSite: 'Strict' | 'Lax' | 'None' }[];
  origins: { origin: string; localStorage: { name: string; value: string }[] }[];
};

/** The `use` options of Playwright that apply to a test's context. */
export interface PlaywrightTestOptions {
  acceptDownloads: boolean;
  bypassCSP: boolean;
  colorScheme: 'light' | 'dark' | 'no-preference' | null;
  reducedMotion: 'reduce' | 'no-preference' | null;
  deviceScaleFactor: number | undefined;
  extraHTTPHeaders: { [key: string]: string } | undefined;
  geolocation: { latitude: number; longitude: number; accuracy?: number } | undefined;
  hasTouch: boolean;
  httpCredentials: { username: string; password: string; origin?: string; send?: 'unauthorized' | 'always' } | undefined;
  ignoreHTTPSErrors: boolean;
  isMobile: boolean;
  javaScriptEnabled: boolean;
  locale: string | undefined;
  offline: boolean;
  permissions: string[] | undefined;
  proxy: { server: string; bypass?: string; username?: string; password?: string } | undefined;
  storageState: string | StorageState | undefined;
  timezoneId: string | undefined;
  userAgent: string | undefined;
  viewport: ViewportSize | null;
  baseURL: string | undefined;
  contextOptions: BrowserContextOptions;
  actionTimeout: number;
  navigationTimeout: number;
  testIdAttribute: string;
  serviceWorkers: 'allow' | 'block';
}

export type ScreenshotMode = 'off' | 'on' | 'only-on-failure' | 'on-first-failure';
export type VideoMode = 'off' | 'on' | 'retain-on-failure' | 'on-first-retry';
export type TraceMode = 'off' | 'on' | 'retain-on-failure' | 'on-first-retry' | 'on-all-retries' | 'retain-on-first-failure';

/** How tests reach pyro: `use: { pyro: { … } }`. */
export type PyroOptions = {
  /** A running `pyro serve` (default: `$PYRO_WS`, else one is launched). */
  wsEndpoint?: string;
  /** The binary to launch (default: `$PYRO_BIN`, else the bundled one). */
  bin?: string;
  /** Extra `pyro serve` arguments. */
  args?: string[];
  /** Pages kept booted ahead (see the README, "Pages booted ahead"). */
  boot?: BootSpec[];
};

export interface PlaywrightWorkerOptions {
  /** Accepted for configs written for Playwright; every project runs on pyro. */
  browserName: 'chromium' | 'firefox' | 'webkit';
  defaultBrowserType: 'chromium' | 'firefox' | 'webkit';
  headless: boolean;
  channel: string | undefined;
  launchOptions: { [key: string]: unknown };
  connectOptions: { wsEndpoint: string; headers?: { [key: string]: string }; timeout?: number } | undefined;
  screenshot: ScreenshotMode | { mode: ScreenshotMode; fullPage?: boolean; omitBackground?: boolean };
  video: VideoMode | { mode: VideoMode; size?: ViewportSize };
  trace: TraceMode | { mode: TraceMode; snapshots?: boolean; screenshots?: boolean; sources?: boolean; attachments?: boolean };
  pyro: PyroOptions;
}

export interface PlaywrightTestArgs {
  context: BrowserContext;
  page: Page;
  request: APIRequestContext;
}

export interface PlaywrightWorkerArgs {
  browser: Browser;
  playwright: {
    chromium: typeof chromium;
    request: typeof request;
    devices: typeof devices;
    errors: { TimeoutError: typeof TimeoutError };
    selectors: { setTestIdAttribute(attributeName: string): void };
  };
}

export type ReporterDescription = Readonly<[string] | [string, any]>;

export type UseOptions<TestArgs = {}, WorkerArgs = {}> = Partial<PlaywrightTestOptions & PlaywrightWorkerOptions> & Partial<TestArgs> & Partial<WorkerArgs>;

export type WebServerConfig = {
  command: string;
  url?: string;
  port?: number;
  reuseExistingServer?: boolean;
  timeout?: number;
  cwd?: string;
  env?: { [key: string]: string };
  stdout?: 'pipe' | 'ignore';
  stderr?: 'pipe' | 'ignore';
  ignoreHTTPSErrors?: boolean;
  gracefulShutdown?: { signal: 'SIGINT' | 'SIGTERM'; timeout: number };
  name?: string;
};

type Matcher = string | RegExp | Array<string | RegExp>;

export interface TestProject<TestArgs = {}, WorkerArgs = {}> {
  name?: string;
  use?: UseOptions<TestArgs, WorkerArgs>;
  testDir?: string;
  testMatch?: Matcher;
  testIgnore?: Matcher;
  outputDir?: string;
  timeout?: number;
  retries?: number;
  fullyParallel?: boolean;
  grep?: RegExp | RegExp[];
  grepInvert?: RegExp | RegExp[];
  dependencies?: string[];
  metadata?: { [key: string]: unknown };
  repeatEach?: number;
  snapshotDir?: string;
  snapshotPathTemplate?: string;
  teardown?: string;
  expect?: {
    timeout?: number;
    /** Playwright's `expect.toMatchAriaSnapshot`: the default `/children` mode and the snapshot path template. */
    toMatchAriaSnapshot?: { children?: 'contain' | 'equal' | 'deep-equal'; pathTemplate?: string };
  };
}

export interface PlaywrightTestConfig<TestArgs = {}, WorkerArgs = {}> extends Omit<TestProject<TestArgs, WorkerArgs>, 'dependencies' | 'teardown'> {
  projects?: TestProject<TestArgs, WorkerArgs>[];
  webServer?: WebServerConfig | WebServerConfig[];
  globalSetup?: string | string[];
  globalTeardown?: string | string[];
  globalTimeout?: number;
  workers?: number | string;
  maxFailures?: number;
  forbidOnly?: boolean;
  reporter?: 'list' | 'dot' | 'line' | 'github' | 'json' | 'junit' | 'null' | 'html' | ReporterDescription[];
  reportSlowTests?: { max: number; threshold: number } | null;
  quiet?: boolean;
  preserveOutput?: 'always' | 'never' | 'failures-only';
  updateSnapshots?: 'all' | 'changed' | 'missing' | 'none';
  captureGitInfo?: { commit?: boolean; diff?: boolean };
  tsconfig?: string;
  shard?: { total: number; current: number } | null;
}

/** `PlaywrightTestConfig` by its other name in `@playwright/test`. */
export type Config<TestArgs = {}, WorkerArgs = {}> = PlaywrightTestConfig<TestArgs, WorkerArgs>;
export type Project<TestArgs = {}, WorkerArgs = {}> = TestProject<TestArgs, WorkerArgs>;
