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
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { Cdp, CdpError } from './cdp';
import { loaded, route as routeStep, goto, type Step, type StepLike } from './steps';

export * from './steps';
export { Cdp, CdpError } from './cdp';

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
};

export type RunResult = {
  /** Values of `read(as)` and `evaluate(expr, as)` steps. */
  values: Record<string, unknown>;
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

type ProfileEntry = { id: number | null; profile: Profile; scripts: string[] };

export class Lightningpanda {
  private profiles = new Map<string, ProfileEntry>();
  /** Pages taken from a pool, and pages booted on demand because none was waiting. */
  readonly stats = { prepared: 0, preparedBooting: 0, fresh: 0 };

  private constructor(
    readonly cdp: Cdp,
    private child?: ChildProcess,
  ) {}

  static async connect(wsUrl: string): Promise<Lightningpanda> {
    return new Lightningpanda(await Cdp.open(wsUrl));
  }

  /** Starts `lightningpanda serve` on a free port and connects to it. */
  static async launch(opts: { bin?: string; args?: string[] } = {}): Promise<Lightningpanda> {
    const port = await freePort();
    const child = spawn(opts.bin ?? 'lightningpanda', ['serve', '--host', '127.0.0.1', '--port', String(port), ...(opts.args ?? [])], {
      stdio: ['ignore', 'ignore', 'inherit'],
    });
    const deadline = Date.now() + 15_000;
    for (;;) {
      try {
        const r = await fetch(`http://127.0.0.1:${port}/json/version`);
        const v = (await r.json()) as { webSocketDebuggerUrl?: string };
        return new Lightningpanda(await Cdp.open(v.webSocketDebuggerUrl ?? `ws://127.0.0.1:${port}/`), child);
      } catch (e) {
        if (child.exitCode !== null || Date.now() > deadline) {
          child.kill();
          throw new Error(`lightningpanda serve did not start: ${e}`);
        }
        await new Promise((r) => setTimeout(r, 5));
      }
    }
  }

  /** Declares a kind of page tests ask for by name (`lp.page({ as: name })`). */
  async profile(name: string, profile: Profile): Promise<void> {
    const scripts = [...(profile.localStorage ? [storageScript(profile.localStorage)] : []), ...(profile.initScripts ?? [])];
    if (profile.prepare === false) {
      this.profiles.set(name, { id: null, profile, scripts });
      return;
    }
    const { profileId } = await this.cdp.send('Lightningpanda.registerPreparedPage', {
      url: profile.url,
      initScripts: scripts,
      ...(profile.viewport ? { viewport: profile.viewport } : {}),
      pool: profile.pool ?? 2,
    });
    this.profiles.set(name, { id: profileId, profile, scripts });
  }

  /** Waits until a page of profile `name` has booted (the pool refills while the connection is quiet). */
  async warm(name: string, timeoutMs = 5_000): Promise<void> {
    const entry = this.profiles.get(name);
    if (!entry || entry.id === null) return;
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const { pages } = await this.cdp.send('Lightningpanda.preparedPages');
      if (pages.some((p: { profileId: number; ready: boolean }) => p.profileId === entry.id && p.ready)) return;
      if (Date.now() > deadline) throw new Error(`no booted page for ${name} after ${timeoutMs} ms`);
      await new Promise((r) => setTimeout(r, 2));
    }
  }

  /**
   * A page of profile `as` (signed in, booted ahead when one is waiting),
   * or a blank page in its own context. `route` (same-document) or `url`
   * (a full navigation) runs as the first step of the first `run`.
   */
  async page(opts: { as?: string; route?: string; url?: string } = {}): Promise<Page> {
    const entry = opts.as === undefined ? undefined : this.profiles.get(opts.as);
    if (opts.as !== undefined && !entry) throw new Error(`no profile ${opts.as}: declare it with lp.profile()`);
    const first: StepLike[] = [];
    let page: Page | undefined;
    if (entry && entry.id !== null) {
      try {
        const r = await this.cdp.send('Lightningpanda.acquirePreparedPage', { profileId: entry.id, attach: true });
        page = new Page(this.cdp, r.sessionId, r.targetId, r.browserContextId);
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
      page = await this.freshPage(entry);
      this.stats.fresh++;
      if (entry && !opts.url) first.push(goto(entry.profile.url));
    }
    if (opts.url) first.push(goto(opts.url));
    if (opts.route) first.push(routeStep(opts.route));
    page.prefix = first;
    return page;
  }

  private async freshPage(entry?: ProfileEntry): Promise<Page> {
    const { browserContextId } = await this.cdp.send('Target.createBrowserContext');
    const { targetId } = await this.cdp.send('Target.createTarget', { url: 'about:blank', browserContextId });
    const { sessionId } = await this.cdp.send('Target.attachToTarget', { targetId, flatten: true });
    for (const source of entry?.scripts ?? []) {
      await this.cdp.send('Page.addScriptToEvaluateOnNewDocument', { source }, sessionId);
    }
    const v = entry?.profile.viewport;
    if (v) await this.cdp.send('Emulation.setDeviceMetricsOverride', { ...v, deviceScaleFactor: 1, mobile: false }, sessionId);
    return new Page(this.cdp, sessionId, targetId, browserContextId);
  }

  close(): void {
    this.cdp.close();
    this.child?.kill();
  }
}

export class Page {
  /** Steps the next run starts with (the page's route or navigation). */
  prefix: StepLike[] = [];

  constructor(
    private cdp: Cdp,
    readonly sessionId: string,
    readonly targetId: string,
    readonly browserContextId: string,
  ) {}

  /** Runs the steps in the browser as one command; throws on the first failing step. */
  async run(steps: StepLike[], opts: { timeout?: number } = {}): Promise<RunResult> {
    const prefix = this.prefix;
    this.prefix = [];
    const wire: Step[] = [...prefix, ...steps].map((s) => s.toStep());
    try {
      const r = await this.cdp.send('LP.run', { steps: wire, ...opts }, this.sessionId);
      return { values: r.reads, results: r.results.slice(prefix.length), requests: r.requests, ms: r.ms };
    } catch (e) {
      const m = e instanceof CdpError ? /step (\d+)/.exec(e.message) : null;
      if (!m) throw e;
      const index = Number(m[1]) - prefix.length;
      const where = index < 0 ? `opening the page (${prefix[Number(m[1])] && JSON.stringify(prefix[Number(m[1])].toStep())})` : `step ${index}`;
      throw new RunError((e as Error).message.replace(/^LP\.run: step \d+/, where), index);
    }
  }

  /** Any other CDP command on this page. */
  send<T = any>(method: string, params: object = {}): Promise<T> {
    return this.cdp.send(method, params, this.sessionId);
  }

  /** Closes the page and its context without waiting. */
  close(): void {
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
