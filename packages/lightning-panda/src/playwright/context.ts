// The running test, for `test.info()`, `test.step()` and `expect.soft()`.
import { AsyncLocalStorage } from 'node:async_hooks';
import { mkdirSync, readFileSync } from 'node:fs';
import { dirname, join, basename } from 'node:path';

export type Annotation = { type: string; description?: string };
export type Attachment = { name: string; contentType: string; path?: string; body?: Buffer };

/** Playwright's TestInfo. */
export class TestInfo {
  annotations: Annotation[] = [];
  attachments: Attachment[] = [];
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
  readonly snapshotSuffix = '';
  _softErrors: Error[] = [];
  /** Set by `test.skip()` / `test.fixme()` in the body: the run stops there. */
  _skipped?: string;
  _onTimeout?: (ms: number) => void;
  private startedAt = Date.now();

  constructor(
    readonly title: string,
    readonly titlePath: string[],
    readonly file: string,
    readonly project: { name: string; use: Record<string, unknown>; outputDir: string; testDir: string; timeout: number; retries: number; metadata: Record<string, unknown> },
    readonly config: Record<string, any>,
    public timeout: number,
    readonly retry: number,
  ) {}
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
  get snapshotDir(): string {
    return join(dirname(this.file), '__screenshots__');
  }
  snapshotPath(...parts: string[]): string {
    return join(this.snapshotDir, ...parts);
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
