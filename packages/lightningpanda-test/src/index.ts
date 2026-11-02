// lightningpanda-test: E2E tests for `bun test` on Lightningpanda.
//
//   const lp = await Lightningpanda.connect(wsUrl);          // or Lightningpanda.launch()
//   await lp.profile('employee', { url: `${app}/platform`, localStorage: tokens });
//   await using page = await lp.page({ as: 'employee', route: `/positions/${id}` });
//   const r = await page.run([
//     button('Settings').click(),
//     dialog().hasText('will be closed').button('Close Position').click(),
//     check.request('POST', `/positions/${id}/close`).status(200),
//     check(button('Reopen Position')).count(1),
//   ]);
//
// A profile keeps signed-in pages booted ahead (Lightningpanda.registerPreparedPage);
// `lp.page()` takes one and attaches in one call, and its `route` becomes the
// first step of the first run. `page.run()` is one LP.run command: the browser
// runs every step, waiting in place, and replies once.
//
// Against `lightningpanda serve --fork` every page gets a connection of its own:
// the server forks it from a process that booted each profile once, so the
// page starts booted and its process exits when the page closes.
import { spawn, type ChildProcess } from 'node:child_process';
import { statSync } from 'node:fs';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import { Cdp, CdpError } from './cdp';
import { loaded, route as routeStep, goto, matchScreenshot, ScreenshotStep, type ScreenshotOptions, type Step, type StepLike } from './steps';
import { mismatch, prepare, settle, ScreenshotMismatchError, type ScreenshotResult, type SentBaselines } from './screenshots';

export * from './steps';
export { Cdp, CdpError } from './cdp';
export { ScreenshotMismatchError, updateMode, type ScreenshotResult } from './screenshots';

export type Profile = {
  /** Where its pages boot (and stay until a test takes one). */
  url: string;
  /** Written to localStorage before the app's scripts run (e.g. auth tokens). */
  localStorage?: Record<string, string>;
  /** Run in every document before its scripts. */
  initScripts?: string[];
  viewport?: { width: number; height: number };
  /** Pages kept booted (1-8). */
  pool?: number;
  /** Keep pages booted ahead (default); false boots each page when a test asks. */
  prepare?: boolean;
  /**
   * Steps run once on the booted page before tests get it (`serve --fork`
   * only): every page of this profile then starts from their result.
   */
  setup?: StepLike[];
  /**
   * Starts from another profile's snapshot (`serve --fork` only): its page is
   * forked from that profile's, and only this profile's `setup` runs on it.
   */
  base?: string;
};

export type RunResult = {
  /** Values of `read(as)` and `evaluate(expr, as)` steps. */
  values: Record<string, unknown>;
  /** `matchScreenshot` steps by name. */
  screenshots: Record<string, ScreenshotResult>;
  /** One entry per step: what a check saw, a request it matched, settle time. */
  results: unknown[];
  /** Requests the page made during the run. */
  requests: { method: string; url: string; status: number | null; error: string | null; ms: number | null }[];
  /** Time in the browser. */
  ms: number;
};

export class RunError extends Error {
  constructor(
    message: string,
    readonly step: number,
  ) {
    super(message);
  }
}

const storageScript = (entries: Record<string, string>) =>
  `(() => { try { const s = window.localStorage; for (const [k, v] of Object.entries(${JSON.stringify(entries)})) s.setItem(k, v); } catch {} })();`;

type ProfileEntry = { ids: number[] | null; profile: Profile; scripts: string[] };

/** One CDP connection: one browser thread, with its own page pools and screenshot baselines. */
type Conn = {
  cdp: Cdp;
  open: number;
  baselines: SentBaselines;
  /** Opened for one page (fork mode): closed with it. */
  own?: boolean;
};

export type ConnectOptions = {
  /**
   * CDP connections to open (default 1). Each is its own browser thread with
   * its own pools: a test takes a page from the connection with the fewest
   * open pages, and a connection refills its pools as soon as it has no open
   * page, in parallel with tests on the others.
   */
  connections?: number;
};

export class Lightningpanda {
  private profiles = new Map<string, ProfileEntry>();
  private profileMutation: Promise<void> = Promise.resolve();
  private turn = 0;
  /** `serve --fork`: one connection (a forked process) per page. */
  private fork = false;
  /** Pages taken from a pool, and pages booted on demand because none was waiting. */
  readonly stats = { prepared: 0, preparedBooting: 0, fresh: 0 };

  private constructor(
    private wsUrl: string,
    private conns: Conn[],
    private child?: ChildProcess,
  ) {}

  /** Asks the server whether it forks connections (`serve --fork`). */
  private async detect(): Promise<this> {
    try {
      const { fork } = await this.cdp.send('Lightningpanda.capabilities');
      this.fork = fork === true;
    } catch (e) {
      if (!(e instanceof CdpError)) throw e; // an older server: no fork mode
    }
    return this;
  }

  /** The first connection, for browser-level commands. */
  get cdp(): Cdp {
    return this.conns[0].cdp;
  }

  static async connect(wsUrl: string, opts: ConnectOptions = {}): Promise<Lightningpanda> {
    return new Lightningpanda(wsUrl, await openConns(wsUrl, opts)).detect();
  }

  /**
   * Starts `lightningpanda serve` on a free port and connects to it. The binary
   * is `bin`, else `$LIGHTNINGPANDA_BIN`, else the one next to this package in a
   * release archive, else `lightningpanda` on PATH.
   */
  static async launch(opts: ConnectOptions & { bin?: string; args?: string[] } = {}): Promise<Lightningpanda> {
    const port = await freePort();
    const child = spawn(opts.bin ?? defaultBin(), ['serve', '--host', '127.0.0.1', '--port', String(port), ...(opts.args ?? [])], {
      stdio: ['ignore', 'ignore', 'inherit'],
      detached: process.platform !== 'win32',
    });
    const deadline = Date.now() + 15_000;
    for (;;) {
      try {
        const r = await fetch(`http://127.0.0.1:${port}/json/version`);
        const v = (await r.json()) as { webSocketDebuggerUrl?: string };
        const url = v.webSocketDebuggerUrl ?? `ws://127.0.0.1:${port}/`;
        return await new Lightningpanda(url, await openConns(url, opts), child).detect();
      } catch (e) {
        if (child.exitCode !== null || Date.now() > deadline) {
          stopLaunchedBrowser(child);
          throw new Error(`lightningpanda serve did not start: ${e}`);
        }
        await new Promise((r) => setTimeout(r, 5));
      }
    }
  }

  private updateProfiles<T>(change: () => Promise<T>): Promise<T> {
    const result = this.profileMutation.then(change);
    this.profileMutation = result.then(() => undefined, () => undefined);
    return result;
  }

  /** Declares a kind of page tests ask for by name (`lp.page({ as: name })`). */
  async profile(name: string, profile: Profile): Promise<void> {
    return this.updateProfiles(async () => {
      const base = profile.base === undefined ? undefined : this.profiles.get(profile.base)?.ids?.[0];
      if (profile.base !== undefined && base === undefined) throw new Error(`no prepared profile ${profile.base} to start from`);
      const scripts = [...(profile.localStorage ? [storageScript(profile.localStorage)] : []), ...(profile.initScripts ?? [])];
      if (profile.prepare === false) {
        this.profiles.set(name, { ids: null, profile, scripts });
        return;
      }
      // Fork mode: the server's template boots the profile once for every connection.
      const conns = this.fork ? this.conns.slice(0, 1) : this.conns;
      const ids = await Promise.all(
        conns.map(async (c) => {
          const { profileId } = await c.cdp.send('Lightningpanda.registerPreparedPage', {
            url: profile.url,
            initScripts: scripts,
            ...(profile.viewport ? { viewport: profile.viewport } : {}),
            pool: profile.pool ?? 2,
            ...(profile.setup?.length ? { setup: profile.setup.map((s) => s.toStep()) } : {}),
            ...(base !== undefined ? { base } : {}),
          });
          return profileId as number;
        }),
      );
      this.profiles.set(name, { ids, profile, scripts });
    });
  }

  /** Releases one name, retiring shared native snapshots only after their last alias. Acquired pages remain independent. */
  async releaseProfile(name: string): Promise<void> {
    return this.updateProfiles(async () => {
      const entry = this.profiles.get(name);
      if (!entry) return;
      if (entry.ids) {
        const conns = this.fork ? this.conns.slice(0, 1) : this.conns;
        await Promise.all(conns.map(async (conn, i) => {
          const profileId = entry.ids![i];
          const shared = [...this.profiles].some(([alias, other]) => alias !== name && other.ids?.[i] === profileId);
          if (!shared) await conn.cdp.send('Lightningpanda.unregisterPreparedPage', { profileId });
        }));
      }
      this.profiles.delete(name);
    });
  }

  /** Waits until every connection's pool of profile `name` is fully booted. */
  async warm(name: string, timeoutMs = 5_000): Promise<void> {
    const entry = this.profiles.get(name);
    if (!entry?.ids) return;
    const deadline = Date.now() + timeoutMs;
    if (this.fork) {
      // A new connection is a fork of the template as it is now.
      for (;;) {
        const cdp = await Cdp.open(this.forkUrl(entry.ids[0]));
        const { pages } = await cdp.send('Lightningpanda.preparedPages').finally(() => cdp.close());
        if (pages.some((p: { profileId: number; ready: boolean }) => p.profileId === entry.ids![0] && p.ready)) return;
        if (Date.now() > deadline) throw new Error(`no booted page for ${name} after ${timeoutMs} ms`);
        await new Promise((r) => setTimeout(r, 10));
      }
    }
    await Promise.all(
      this.conns.map(async (c, i) => {
        for (;;) {
          const { pages } = await c.cdp.send('Lightningpanda.preparedPages');
          const ready = pages.filter((p: { profileId: number; ready: boolean }) => p.profileId === entry.ids![i] && p.ready);
          // A partly booted pool is still booting on this thread, in the way of the next test.
          if (ready.length >= (entry.profile.pool ?? 2)) return;
          if (Date.now() > deadline) throw new Error(`no booted page for ${name} after ${timeoutMs} ms`);
          await new Promise((r) => setTimeout(r, 2));
        }
      }),
    );
  }

  /** The connection with the fewest open pages (round-robin among equals). */
  private pick(): number {
    const n = this.conns.length;
    let best = this.turn % n;
    for (let k = 1; k < n; k++) {
      const i = (this.turn + k) % n;
      if (this.conns[i].open < this.conns[best].open) best = i;
    }
    this.turn = best + 1;
    return best;
  }

  /**
   * A page of profile `as` (signed in, booted ahead when one is waiting),
   * or a blank page in its own context. `route` (same-document) or `url`
   * (a full navigation) runs as the first step of the first `run`.
   * `initScripts` run in every document of this page before its scripts
   * (after the profile's); a booted page already ran its document, so the
   * page then boots fresh instead of coming from the pool.
   */
  async page(opts: { as?: string; route?: string; url?: string; initScripts?: string[] } = {}): Promise<Page> {
    const entry = opts.as === undefined ? undefined : this.profiles.get(opts.as);
    if (opts.as !== undefined && !entry) throw new Error(`no profile ${opts.as}: declare it with lp.profile()`);
    if (!this.fork) {
      const index = this.pick();
      return this.openPage(this.conns[index], index, entry, opts);
    }
    // The server forks the connection from this profile's snapshot.
    const conn: Conn = { cdp: await Cdp.open(this.forkUrl(entry?.ids?.[0])), open: 0, baselines: new Map(), own: true };
    try {
      return await this.openPage(conn, 0, entry, opts);
    } catch (e) {
      conn.cdp.close();
      throw e;
    }
  }

  private async openPage(
    conn: Conn,
    index: number,
    entry: ProfileEntry | undefined,
    opts: { route?: string; url?: string; initScripts?: string[] },
  ): Promise<Page> {
    const first: StepLike[] = [];
    let page: Page | undefined;
    const extra = opts.initScripts ?? [];
    if (entry?.ids && extra.length === 0) {
      try {
        const r = await conn.cdp.send('Lightningpanda.acquirePreparedPage', { profileId: entry.ids[index], attach: true });
        page = new Page(conn, r.sessionId, r.targetId, r.browserContextId);
        this.stats.prepared++;
        if (!r.ready) {
          this.stats.preparedBooting++;
          first.push(loaded());
        }
      } catch (e) {
        if (!(e instanceof CdpError) || e.code !== -32602) throw e;
      }
    }
    if (!page) {
      page = await freshPage(conn, entry, extra);
      this.stats.fresh++;
      if (entry && !opts.url) first.push(goto(entry.profile.url));
    }
    if (opts.url) first.push(goto(opts.url));
    if (opts.route) first.push(routeStep(opts.route));
    page.prefix = first;
    return page;
  }

  /** The connection URL that forks from `profileId`'s snapshot (fork mode). */
  private forkUrl(profileId?: number): string {
    if (profileId === undefined) return this.wsUrl;
    const url = new URL(this.wsUrl);
    url.searchParams.set('profile', String(profileId));
    return url.toString();
  }

  close(): void {
    for (const c of this.conns) c.cdp.close();
    if (this.child) {
      const child = this.child;
      this.child = undefined;
      stopLaunchedBrowser(child);
    }
  }
}

/** The launched server and its forked pages are one owned process group. */
function stopLaunchedBrowser(child: ChildProcess): void {
  if (process.platform === 'win32' || !child.pid) { child.kill(); return; }
  try { process.kill(-child.pid, 'SIGTERM'); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error; }
}

async function openConns(wsUrl: string, opts: ConnectOptions): Promise<Conn[]> {
  const n = Math.max(1, opts.connections ?? 1);
  const cdps = await Promise.all(Array.from({ length: n }, () => Cdp.open(wsUrl)));
  return cdps.map((cdp) => ({ cdp, open: 0, baselines: new Map() }));
}

async function freshPage(conn: Conn, entry?: ProfileEntry, extra: string[] = []): Promise<Page> {
  const cdp = conn.cdp;
  const { browserContextId } = await cdp.send('Target.createBrowserContext');
  const { targetId } = await cdp.send('Target.createTarget', { url: 'about:blank', browserContextId });
  const { sessionId } = await cdp.send('Target.attachToTarget', { targetId, flatten: true });
  for (const source of [...(entry?.scripts ?? []), ...extra]) {
    await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source }, sessionId);
  }
  const v = entry?.profile.viewport;
  if (v) await cdp.send('Emulation.setDeviceMetricsOverride', { ...v, deviceScaleFactor: 1, mobile: false }, sessionId);
  return new Page(conn, sessionId, targetId, browserContextId);
}

export class Page {
  /** Steps the next run starts with (the page's route or navigation). */
  prefix: StepLike[] = [];

  private closed = false;

  constructor(
    private conn: Conn,
    readonly sessionId: string,
    readonly targetId: string,
    readonly browserContextId: string,
  ) {
    conn.open++;
  }

  private get cdp(): Cdp {
    return this.conn.cdp;
  }

  /**
   * Runs the steps in the browser as one command; throws on the first failing
   * step (a `ScreenshotMismatchError` for a screenshot that does not match).
   */
  async run(steps: StepLike[], opts: { timeout?: number } = {}): Promise<RunResult> {
    const prefix = this.prefix;
    this.prefix = [];
    const all = [...prefix, ...steps];
    for (const s of all) if (s instanceof ScreenshotStep) prepare(s, this.conn.baselines);
    const wire: Step[] = all.map((s) => s.toStep());
    let r: any;
    try {
      r = await this.cdp.send('LP.run', { steps: wire, ...opts }, this.sessionId);
    } catch (e) {
      const m = e instanceof CdpError ? /step (\d+)/.exec(e.message) : null;
      if (!m) throw e;
      const index = Number(m[1]) - prefix.length;
      const where = index < 0 ? `opening the page (${prefix[Number(m[1])] && JSON.stringify(prefix[Number(m[1])].toStep())})` : `step ${index}`;
      const message = (e as Error).message.replace(/^LP\.run: step \d+/, where);
      const failed = all[Number(m[1])];
      const data = (e as CdpError).data as any;
      if (failed instanceof ScreenshotStep && data?.diff) {
        // The browser may have dropped the baseline with the connection's cache: resend next time.
        throw mismatch(failed, data, message);
      }
      // The connection's cache entry may be stale after a failure: resend next time.
      for (const s of all) if (s instanceof ScreenshotStep) this.conn.baselines.delete(String(s.wire.name ?? ''));
      throw new RunError(message, index);
    }
    const screenshots: Record<string, ScreenshotResult> = {};
    all.forEach((s, i) => {
      if (s instanceof ScreenshotStep) screenshots[s.name] = settle(s, r.results[i]);
    });
    return { values: r.reads, screenshots, results: r.results.slice(prefix.length), requests: r.requests, ms: r.ms };
  }

  /**
   * Expects the page (or `clip`, or `fullPage`) to match the baseline `name`:
   * one `matchScreenshot` step. Throws `ScreenshotMismatchError` on a mismatch.
   */
  async matchScreenshot(name: string, opts: ScreenshotOptions = {}): Promise<ScreenshotResult> {
    const step = matchScreenshot(name, opts);
    const r = await this.run([step]);
    return r.screenshots[step.name];
  }

  /**
   * Runs `source` in every document this page loads from now on, before its
   * scripts (Playwright's `page.addInitScript`): follow it with `goto` or
   * `reload` for the current page to see it.
   */
  async addInitScript(source: string): Promise<void> {
    await this.cdp.send('Page.addScriptToEvaluateOnNewDocument', { source }, this.sessionId);
  }

  /** Any other CDP command on this page. */
  send<T = any>(method: string, params: object = {}): Promise<T> {
    return this.cdp.send(method, params, this.sessionId);
  }

  /** Closes the page and its context without waiting. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.conn.open--;
    if (this.conn.own) {
      // Its own forked process: closing the connection ends it, page and all.
      this.cdp.close();
      return;
    }
    this.cdp.post('Target.closeTarget', { targetId: this.targetId });
    this.cdp.post('Target.disposeBrowserContext', { browserContextId: this.browserContextId });
  }

  async [Symbol.asyncDispose](): Promise<void> {
    this.close();
  }
}

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = createServer();
    s.listen(0, '127.0.0.1', () => {
      const port = (s.address() as { port: number }).port;
      s.close(() => resolve(port));
    });
    s.on('error', reject);
  });
}

/** A release archive unpacks as `lightningpanda` beside `lightningpanda-test/`. */
function defaultBin(): string {
  if (process.env.LIGHTNINGPANDA_BIN) return process.env.LIGHTNINGPANDA_BIN;
  const bin = fileURLToPath(new URL('../../lightningpanda', import.meta.url));
  return statSync(bin, { throwIfNoEntry: false })?.isFile() ? bin : 'lightningpanda';
}

/**
 * `expect.extend(screenshotMatchers)`, then
 * `await expect(page).toMatchScreenshot('home', { dir })`.
 */
export const screenshotMatchers = {
  async toMatchScreenshot(page: Page, name: string, opts: ScreenshotOptions = {}) {
    try {
      const r = await page.matchScreenshot(name, opts);
      return { pass: true, message: () => `expected screenshot '${name}' not to match (${r.diffPixels} pixels differ)` };
    } catch (e) {
      if (e instanceof ScreenshotMismatchError) return { pass: false, message: () => e.message };
      throw e;
    }
  },
};
