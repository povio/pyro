// The running test, for `test.info()`, `test.step()` and `expect.soft()`.
import { AsyncLocalStorage } from 'node:async_hooks';
import { mkdirSync, readFileSync } from 'node:fs';
import { join, basename, relative } from 'node:path';
import { anonymousName, snapshotPath, type SnapshotKind } from './snapshot.js';
import type { FullConfig, FullProject } from './types.js';

export type Annotation = { type: string; description?: string };
export type Attachment = { name: string; contentType: string; path?: string; body?: Buffer };

/** Playwright's TestInfo. */
export class TestInfo {
  annotations: Annotation[] = [];
  attachments: Attachment[] = [];
  /** 'failed' or 'timedOut' from the first error on (afterEach hooks and fixture teardown see it). */
  status: 'passed' | 'failed' | 'timedOut' | 'skipped' | 'interrupted' = 'passed';
  expectedStatus: 'passed' | 'failed' | 'skipped' = 'passed';
  error?: Error;
  errors: Error[] = [];
  duration = 0;
  readonly workerIndex = 0;
  readonly parallelIndex = 0;
  readonly repeatEachIndex = 0;
  readonly column = 0;
  readonly line = 0;
  /** `{snapshotSuffix}` in the snapshot template: the platform, as Playwright's page fixture sets it; '' drops it. */
  snapshotSuffix: string = process.platform;
  /** Failed soft assertions: each also fails the test at once (`status`, `errors`), as in Playwright. */
  _softErrors: Error[] = [];
  /** A missing snapshot was written ("missing" mode): the test fails and is not retried. */
  _hasNonRetriableError = false;
  /** Set by `test.skip()` / `test.fixme()` in the body: the run stops there. */
  _skipped?: string;
  _onTimeout?: (ms: number) => void;
  /** Set for a beforeAll/afterAll hook's info: the `context` and `page` fixtures refuse it. */
  _hook?: 'beforeAll' | 'afterAll';
  private startedAt = Date.now();

  constructor(
    readonly title: string,
    readonly titlePath: string[],
    readonly file: string,
    readonly project: FullProject & { snapshotDir?: string; configDir?: string },
    readonly config: FullConfig,
    public timeout: number,
    readonly retry: number,
  ) {
    const push = this._softErrors.push.bind(this._softErrors);
    this._softErrors.push = (...errors: Error[]) => {
      for (const e of errors) this._failWithError(e);
      return push(...errors);
    };
  }
  /** Records an error of the test: the first one sets `status` (Playwright's `_failWithError`). */
  _failWithError(error: Error, timedOut = false): void {
    if (this.status === 'passed' || this.status === 'skipped') this.status = timedOut ? 'timedOut' : 'failed';
    if (!this.errors.includes(error)) this.errors.push(error);
    this.error ??= error;
  }
  get testId(): string {
    return `${basename(this.file)}:${this.titlePath.join(' > ')}`;
  }
  get outputDir(): string {
    const slug = this.titlePath.join('-').replace(/[^\w.-]+/g, '-').slice(0, 120);
    return join(this.project.outputDir, `${slug}${this.retry ? `-retry${this.retry}` : ''}`);
  }
  outputPath(...parts: string[]): string {
    mkdirSync(this.outputDir, { recursive: true });
    return join(this.outputDir, ...parts);
  }
  /** As Playwright's: `<project.snapshotDir>/<test file path>-snapshots`. */
  get snapshotDir(): string {
    return join(this.project.snapshotDir ?? this.project.testDir, relative(this.project.testDir, this.file) + '-snapshots');
  }
  /** Playwright's snapshot path for `name` (`snapshotPathTemplate`). */
  snapshotPath(...parts: string[]): string;
  snapshotPath(name: string, options: { kind: 'snapshot' | 'screenshot' | 'aria' }): string;
  snapshotPath(...args: any[]): string {
    let kind = 'snapshot';
    const last = args[args.length - 1];
    if (last && typeof last === 'object') {
      kind = last.kind ?? kind;
      args = args.slice(0, -1);
    }
    if (!['snapshot', 'screenshot', 'aria'].includes(kind)) throw new Error(`testInfo.snapshotPath: unknown kind "${kind}", must be one of "snapshot", "screenshot" or "aria"`);
    const k = kind as SnapshotKind;
    const name = args.length > 1 ? args : args[0] || anonymousName(this, { aria: '.aria.yml', screenshot: '.png', snapshot: '.txt' }[k], k, false);
    return snapshotPath(this, name, k);
  }
  setTimeout(ms: number): void {
    this.timeout = ms;
    this._onTimeout?.(ms);
  }
  slow(condition: boolean | string = true, description?: string): void {
    if (typeof condition === 'string' || condition) {
      this.annotations.push({ type: 'slow', description: typeof condition === 'string' ? condition : description });
      this.setTimeout(this.timeout * 3);
    }
  }
  skip(condition: boolean | string = true, description?: string): void {
    if (typeof condition === 'string' || condition) {
      const reason = typeof condition === 'string' ? condition : (description ?? '');
      this.annotations.push({ type: 'skip', description: reason });
      this.expectedStatus = 'skipped';
      this._skipped = reason;
      throw new SkipError(reason);
    }
  }
  fixme(condition: boolean | string = true, description?: string): void {
    if (typeof condition === 'string' || condition) {
      const reason = typeof condition === 'string' ? condition : (description ?? '');
      this.annotations.push({ type: 'fixme', description: reason });
      this.expectedStatus = 'skipped';
      this._skipped = reason;
      throw new SkipError(reason);
    }
  }
  fail(condition: boolean | string = true, description?: string): void {
    if (typeof condition === 'string' || condition) {
      this.annotations.push({ type: 'fail', description: typeof condition === 'string' ? condition : description });
      this.expectedStatus = 'failed';
    }
  }
  async attach(name: string, o: { body?: string | Buffer; path?: string; contentType?: string } = {}): Promise<void> {
    const body = o.body === undefined ? undefined : Buffer.isBuffer(o.body) ? o.body : Buffer.from(o.body);
    this.attachments.push({ name, contentType: o.contentType ?? (o.path ? 'application/octet-stream' : 'text/plain'), path: o.path, body: body ?? (o.path ? readFileSync(o.path) : undefined) });
  }
  get elapsed(): number {
    return Date.now() - this.startedAt;
  }
}

export class SkipError extends Error {
  override name = 'SkipError';
}

export const als = new AsyncLocalStorage<TestInfo>();
export const currentTest = (): TestInfo | undefined => als.getStore();
