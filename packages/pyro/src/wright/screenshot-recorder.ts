// The `screenshot` option of the test runner (`use: { screenshot }`), as in
// Playwright 1.59: screenshots of a test's pages attached to its TestInfo as
// `screenshot`, saved as `test-failed-N.png` or `test-finished-N.png` in
// `testInfo.outputDir`.
//
// Right after the test function, before its afterEach hooks, every page
// still open is captured when the outcome so far asks for it: 'on' always,
// 'only-on-failure' when the test failed or timed out, 'on-first-failure'
// when it did on its first try. A context that closes while the outcome asks
// for it (in fixture teardown after an afterEach hook failed, say) has its
// pages captured as it closes, attached after fixture teardown. Each page
// once per test; a page that cannot be captured within 5 s is left out.
//
// Playwright also captures a context that closes while the test still
// passes, in case it fails later, and drops the capture when it does not:
// a screenshot per page of every passing test (7 ms for a page of a 300-row
// table here, up to hundreds when it has to lay out first). Here such a
// context is not captured.
import { randomUUID } from 'node:crypto';
import { mkdtempSync, renameSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { BrowserContext } from './browser.js';
import type { TestInfo } from './context.js';
import type { Page } from './page.js';
import type { ScreenshotMode } from './types.js';

export type ScreenshotOption = ScreenshotMode | { mode: ScreenshotMode; fullPage?: boolean; omitBackground?: boolean };

/** How long one capture may take (Playwright's `timeout: 5000`). */
const CAPTURE_TIMEOUT = 5_000;

export class ScreenshotRecorder {
  private readonly mode: ScreenshotMode;
  private readonly options: { fullPage?: boolean; omitBackground?: boolean };
  /** Screenshots attached so far (an earlier attempt's count on in the numbering, as Playwright's). */
  private ordinal: number;
  private readonly taken = new WeakSet<Page>();
  /** Captures of closing contexts, until fixture teardown is done. */
  private readonly kept: string[] = [];
  private keptDir?: string;

  constructor(
    private readonly info: TestInfo,
    option: ScreenshotOption | undefined,
  ) {
    this.mode = (typeof option === 'object' && option ? option.mode : option) ?? 'off';
    this.options = typeof option === 'object' && option ? { fullPage: option.fullPage, omitBackground: option.omitBackground } : {};
    this.ordinal = info.attachments.filter((a) => a.name === 'screenshot').length;
  }

  private failing(): boolean {
    return this.info.status !== 'skipped' && this.info.status !== this.info.expectedStatus;
  }

  /** Whether the outcome so far asks for screenshots. */
  private wanted(): boolean {
    return this.mode === 'on' || (this.mode === 'only-on-failure' && this.failing()) || (this.mode === 'on-first-failure' && this.failing() && this.info.retry === 0);
  }

  /** Right after the test function: `pages`, when the outcome asks for them. */
  async finish(pages: Page[]): Promise<void> {
    if (this.wanted()) await Promise.all(pages.map((p) => this.capture(p, false)));
  }

  /** A context of the test closing: its pages when the outcome so far asks for them, kept until fixture teardown is done. */
  async closing(context: BrowserContext): Promise<void> {
    if (this.wanted()) await Promise.all(context.pages().map((p) => this.capture(p, true)));
  }

  /** After fixture teardown: the kept captures attached when the outcome asks for them, else removed. */
  persist(): void {
    const keep = this.wanted();
    for (const file of this.kept.splice(0)) {
      try {
        if (!keep) continue;
        const path = this.nextPath();
        renameSync(file, path);
        this.attach(path);
      } catch {
        // As a failed capture: left out.
      }
    }
    if (this.keptDir) rmSync(this.keptDir, { recursive: true, force: true });
    this.keptDir = undefined;
  }

  private nextPath(): string {
    return this.info.outputPath(`test-${this.failing() ? 'failed' : 'finished'}-${++this.ordinal}.png`);
  }

  private attach(path: string): void {
    this.info.attachments.push({ name: 'screenshot', path, contentType: 'image/png' });
  }

  private async capture(page: Page, keep: boolean): Promise<void> {
    if (this.taken.has(page) || page.isClosed()) return;
    this.taken.add(page);
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const path = keep ? join((this.keptDir ??= mkdtempSync(join(tmpdir(), 'pyro-screenshots-'))), `${randomUUID()}.png`) : this.nextPath();
      const late = new Promise<never>((_, reject) => (timer = setTimeout(() => reject(new Error('screenshot timed out')), CAPTURE_TIMEOUT)));
      await Promise.race([page.screenshot({ ...this.options, path }), late]);
      if (keep) this.kept.push(path);
      else this.attach(path);
    } catch {
      // As in Playwright: a page that cannot be captured is left out.
    } finally {
      clearTimeout(timer);
    }
  }
}
