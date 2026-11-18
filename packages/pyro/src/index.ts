// pyro: E2E tests for `bun test` on Pyro.
//
//   const lp = await Pyro.connect(wsUrl);          // or Pyro.launch()
//   await lp.profile('employee', { url: `${app}/platform`, localStorage: tokens });
//   await using page = await lp.page({ as: 'employee', route: `/positions/${id}` });
//   const r = await page.run([
//     button('Settings').click(),
//     dialog().hasText('will be closed').button('Close Position').click(),
//     check.request('POST', `/positions/${id}/close`).status(200),
//     check(button('Reopen Position')).count(1),
//   ]);
//
// A profile keeps signed-in pages booted ahead (Pyro.registerPreparedPage);
// `pyro.page()` takes one and attaches in one call, and its `route` becomes the
// first step of the first run. `page.run()` is one Pyro.run command: the browser
// runs every step, waiting in place, and replies once.
//
// Against `pyro serve --fork` every page gets a connection of its own:
// the server forks it from a process that booted each profile once, so the
// page starts booted and its process exits when the page closes. Profiles
// there are shared by name with every client of the server: one launcher
// starts the server and registers profiles, and workers in other processes
// `connect()` to `lp.wsEndpoint` and `adopt()` them (or `page({ as })` them
// directly) instead of booting their own.
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { open, rm, type FileHandle } from 'node:fs/promises';
import { createServer } from 'node:net';
import { resolveBinary } from './binary.js';
import { localeScript } from './locale.js';
import { Cdp, CdpError } from './cdp.js';
import { loaded, route as routeStep, goto, matchScreenshot, ScreenshotStep, type ScreenshotOptions, type Step, type StepLike } from './steps.js';
import { mismatch, prepare, settle, ScreenshotMismatchError, type ScreenshotResult, type SentBaselines } from './screenshots.js';

export * from './steps.js';
export { Cdp, CdpError } from './cdp.js';
export { ScreenshotMismatchError, updateMode, type ScreenshotResult } from './screenshots.js';

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
   * only): every page of this profile then starts from their result. When a
   * step fails, `profile()` rejects with a `ProfileSetupError` naming it, and
   * no page of the profile is handed out.
   */
  setup?: StepLike[];
  /**
   * Starts from another profile's snapshot (`serve --fork` only): its page is
   * forked from that profile's, and only this profile's `setup` runs on it.
   * The base may be a profile another client of the server registered. It
   * has the base's `userAgent` and proxy, and fails when the base does.
   */
  base?: string;
  /**
   * The `User-Agent` of every request of the profile's pages (the document,
   * its subresources, fetch and XHR, what layout, screenshots and PDFs load,
   * the `setup` steps' and every page's since) and their
   * `navigator.userAgent`, instead of Pyro's own. Part of the definition:
   * under a shared name a different one is refused.
   */
  userAgent?: string;
  /**
   * HTTP proxy of every request of the profile's pages, as
   * `Target.createBrowserContext` takes it: `http://[user:pass@]host:port`
   * (CONNECT for https), `direct://` for none. In place of the server's
   * `--proxy`. Part of the definition.
   */
  proxyServer?: string;
  /** Hosts that skip `proxyServer`, in Chrome's `--proxy-bypass-list` syntax (loopback always does unless it lists `<-loopback>`). */
  proxyBypassList?: string;
  /**
   * The pages' locale, as Playwright's context option: `navigator.language`,
   * `Accept-Language`, and the default of `Intl` and the `toLocale*String`
   * methods wherever a page leaves the locales argument out (e.g. `'en-GB'`).
   * Part of the definition; a profile with a `base` has its base's.
   */
  locale?: string;
  /**
   * The pages' time zone, as Playwright's context option (an IANA name, e.g.
   * `'Europe/Zurich'`). It is the server's JavaScript VM's: every profile of
   * one server (fork mode) or connection must name the same zone or none.
   * Part of the definition; a profile with a `base` has its base's.
   */
  timezoneId?: string;
};

/**
 * A profile's `setup` failed (`serve --fork`), or that of the profile it
 * starts from (`base`): the message names the step and why, as a run's
 * errors do (`profile app: setup step 2 (Click …): timed out after 5000 ms:
 * …`). No page of the profile is handed out; declaring it again tries again.
 */
export class ProfileSetupError extends Error {
  constructor(
    message: string,
    /** The profile's name. */
    readonly profile: string,
    /** Index of the failed step in the profile's own `setup` (none when its base failed, or no step did). */
    readonly step: number | undefined,
  ) {
    super(message);
  }
}

/** `p`, its rejection handled where nothing awaits it (it still rejects where something does). */
const handled = <T>(p: Promise<T>): Promise<T> => {
  p.catch(() => {});
  return p;
};

const setupError = (profile: string, error: string) => {
  const step = /^setup step (\d+)/.exec(error);
  return new ProfileSetupError(`profile ${profile}: ${error}`, profile, step ? Number(step[1]) : undefined);
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
    // Runners print `RunError: step …` instead of `Error: step …`.
    this.name = 'RunError';
  }
}

const storageScript = (entries: Record<string, string>) =>
  `(() => { try { const s = window.localStorage; for (const [k, v] of Object.entries(${JSON.stringify(entries)})) s.setItem(k, v); } catch {} })();`;

type ProfileEntry = {
  ids: number[] | null;
  profile: Profile;
  scripts: string[];
  /** Shared profiles: the registered definition, to tell a changed one. */
  definition?: string;
  /** Fork mode: settles once the server has set the profile up; rejects with its `ProfileSetupError`. */
  ready?: Promise<void>;
  /** Its setup failed (fork mode): no page of it is handed out. */
  failed?: ProfileSetupError;
};

/** A registration's or adoption's reply (`state` and `error` from servers that report them). */
type ProfileReply = { profileId: number; state?: string; error?: string; setup?: unknown[]; base?: number | null };

/** A profile's `locale` and `timezoneId` as `registerPreparedPage` takes them. */
function emulationParams(p?: Profile): { locale?: string; timezoneId?: string } {
  return {
    ...(p?.locale !== undefined ? { locale: p.locale } : {}),
    ...(p?.timezoneId !== undefined ? { timezoneId: p.timezoneId } : {}),
  };
}

/** One CDP connection: one browser thread, with its own page pools and screenshot baselines. */
type Conn = {
  cdp: Cdp;
  open: number;
  baselines: SentBaselines;
  /** Opened for one page (fork mode): closed with it. */
  own?: boolean;
};

/** Raw pixel layouts of `page.captureFrame()`, by ffmpeg's `-pix_fmt` names. */
export type RawFormat = 'rgba' | 'rgb24' | 'yuv420p';

/** A frame `page.captureFrame()` captured. */
export type Frame = {
  /** Device px. */
  width: number;
  height: number;
  /** The frame's size in bytes (also when it went to `path`). */
  bytes: number;
  /** The pixels; empty when they went to `path`. */
  data: Buffer;
  /**
   * The browser's timings (ms): `snapshot`, `layout`, `scene`, `raster`,
   * `encode` (the pixel conversion), `write` (to `path`), `total` (wall), and
   * `cpu`: the browser process's CPU time for the frame, all threads.
   */
  ms: Record<string, number>;
};

/**
 * A FIFO to stream `page.captureFrame({ path })` frames through into a
 * reader such as ffmpeg (`-f rawvideo -pix_fmt rgba -s 3840x2160 -r 30 -i
 * <path>`). The browser opens the path, appends a frame and closes it again
 * for each capture; this holds the FIFO open for writing in between, so the
 * reader sees one stream until `close()`. Start the reader first: `open()`
 * waits for it. macOS and Linux.
 */
export class FrameFifo {
  private handle?: FileHandle;

  /** Makes the FIFO at `path` (absolute; it must not exist). */
  constructor(readonly path: string) {
    const r = spawnSync('mkfifo', ['-m', '600', path], { encoding: 'utf8' });
    if (r.status !== 0) throw new Error(`mkfifo ${path}: ${r.error?.message ?? r.stderr.trim()}`);
  }

  /** Holds the FIFO open for writing; resolves once its reader opened it. */
  async open(): Promise<void> {
    this.handle ??= await open(this.path, 'w');
  }

  /** Ends the stream (the reader sees its end once the last frame is read) and removes the FIFO. */
  async close(): Promise<void> {
    await this.handle?.close();
    this.handle = undefined;
    await rm(this.path, { force: true });
  }
}

export type ConnectOptions = {
  /**
   * CDP connections to open (default 1). Each is its own browser thread with
   * its own pools: a test takes a page from the connection with the fewest
   * open pages, and a connection refills its pools as soon as it has no open
   * page, in parallel with tests on the others.
   */
  connections?: number;
};

export class Pyro {
  private profiles = new Map<string, ProfileEntry>();
  private profileMutation: Promise<void> = Promise.resolve();
  private turn = 0;
  /** `serve --fork`: one connection (a forked process) per page. */
  private fork = false;
  /**
   * The server keeps profiles by name for all its clients (`serve --fork`):
   * this client holds the ones it registers or adopts until it releases
   * them or disconnects.
   */
  private shared = false;
  /** Pages taken from a pool, and pages booted on demand because none was waiting. */
  readonly stats = { prepared: 0, preparedBooting: 0, fresh: 0 };

  private constructor(
    private wsUrl: string,
    private conns: Conn[],
    private child?: ChildProcess,
  ) {}

  /** Asks the server whether it forks connections (`serve --fork`) and shares profiles. */
  private async detect(): Promise<this> {
    try {
      const { fork, sharedProfiles } = await this.cdp.send('Pyro.capabilities');
      this.fork = fork === true;
      this.shared = sharedProfiles === true;
    } catch (e) {
      if (!(e instanceof CdpError)) throw e; // an older server: no fork mode
    }
    return this;
  }

  /** The first connection, for browser-level commands. */
  get cdp(): Cdp {
    return this.conns[0].cdp;
  }

  /**
   * The server's CDP endpoint: other processes `Pyro.connect()` to
   * it and share its profiles (`serve --fork`).
   */
  get wsEndpoint(): string {
    return this.wsUrl;
  }

  static async connect(wsUrl: string, opts: ConnectOptions = {}): Promise<Pyro> {
    return new Pyro(wsUrl, await openConns(wsUrl, opts)).detect();
  }

  /**
   * Starts `pyro serve` on a free port and connects to it. The binary
   * is `bin`, else `$PYRO_BIN`, else the bundled binary for this
   * platform (or the sibling binary in a legacy release archive). The server
   * exits with this process, however it ends (`close()` ends it sooner).
   *
   * `args` are extra `pyro serve` flags (`pyro serve --help`), e.g.
   * `['--hyphen-digit-breaks', 'on']` to wrap text between a hyphen and a digit as Chrome
   * does (`4b87-|916e`; off by default). `stderr: 'ignore'` keeps the server's
   * own output (its address, engine warnings) off this process's stderr.
   */
  static async launch(opts: ConnectOptions & { bin?: string; args?: string[]; stderr?: 'inherit' | 'ignore' } = {}): Promise<Pyro> {
    const port = await freePort();
    const child = spawn(opts.bin ?? resolveBinary(), ['serve', '--host', '127.0.0.1', '--port', String(port), ...(opts.args ?? [])], {
      stdio: ['ignore', 'ignore', opts.stderr ?? 'inherit'],
      detached: process.platform !== 'win32',
      // `serve --parent-pid`: a runner may stop this process without running
      // its exit handlers (bun test --parallel workers), and the browser's
      // process group would outlive it.
      env: { ...process.env, PYRO_PARENT_PID: String(process.pid) },
    });
    let spawnError: Error | undefined;
    child.once('error', (error) => { spawnError = error; });
    const deadline = Date.now() + 15_000;
    for (;;) {
      try {
        const r = await fetch(`http://127.0.0.1:${port}/json/version`);
        const v = (await r.json()) as { webSocketDebuggerUrl?: string };
        const url = v.webSocketDebuggerUrl ?? `ws://127.0.0.1:${port}/`;
        return await new Pyro(url, await openConns(url, opts), child).detect();
      } catch (e) {
        if (spawnError || child.exitCode !== null || child.signalCode !== null || Date.now() > deadline) {
          stopLaunchedBrowser(child);
          throw new Error(`pyro serve did not start: ${spawnError ?? e}`);
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

  /**
   * Declares a kind of page tests ask for by name (`lp.page({ as: name })`).
   *
   * Against `serve --fork` the name is shared by every client of the server:
   * when another client registered the same definition under it, this one
   * adopts that profile (nothing boots again); a different definition is
   * refused while another client holds the name. Declaring a name again
   * with a different definition replaces this client's own.
   *
   * A profile with `setup` (or a `base`) resolves once the server has run
   * its steps, and rejects with a `ProfileSetupError` when one fails (or
   * its base's did); declaring it again tries again.
   */
  async profile(name: string, profile: Profile): Promise<void> {
    const entry = await this.updateProfiles(async (): Promise<ProfileEntry> => {
      const base = profile.base === undefined ? undefined : (await this.resolveLocked(profile.base))?.ids?.[0];
      if (profile.base !== undefined && base === undefined) throw new Error(`no prepared profile ${profile.base} to start from`);
      // The locale's script first, so that it is the first script of every document.
      const scripts = [
        ...(profile.locale !== undefined ? [localeScript(profile.locale)] : []),
        ...(profile.localStorage ? [storageScript(profile.localStorage)] : []),
        ...(profile.initScripts ?? []),
      ];
      const params = {
        url: profile.url,
        initScripts: scripts,
        ...(profile.viewport ? { viewport: profile.viewport } : {}),
        pool: profile.pool ?? 2,
        ...(profile.setup?.length ? { setup: profile.setup.map((s) => s.toStep()) } : {}),
        ...(base !== undefined ? { base } : {}),
        ...contextParams(profile),
        ...emulationParams(profile),
      };
      const local = this.profiles.get(name);
      if (this.shared && profile.prepare !== false) {
        // The template keeps one page per profile: the pool does not matter there.
        const definition = JSON.stringify({ ...params, pool: undefined });
        if (local?.definition === definition && !local.failed) return local;
        if (local?.ids) await this.cdp.send('Pyro.unregisterPreparedPage', { name });
        this.profiles.delete(name);
        const r: ProfileReply = await this.cdp.send('Pyro.registerPreparedPage', { name, ...params });
        const entry: ProfileEntry = { ids: [r.profileId], profile, scripts, definition };
        entry.ready = handled(this.settle(name, entry, r, false));
        this.profiles.set(name, entry);
        return entry;
      }
      if (local?.definition !== undefined) {
        await this.cdp.send('Pyro.unregisterPreparedPage', { name });
        this.profiles.delete(name);
      }
      if (profile.prepare === false) {
        const entry: ProfileEntry = { ids: null, profile, scripts };
        this.profiles.set(name, entry);
        return entry;
      }
      // Fork mode: the server's template boots the profile once for every connection.
      const conns = this.fork ? this.conns.slice(0, 1) : this.conns;
      const replies = await Promise.all(conns.map((c): Promise<ProfileReply> => c.cdp.send('Pyro.registerPreparedPage', params)));
      const entry: ProfileEntry = { ids: replies.map((r) => r.profileId), profile, scripts };
      if (this.fork) entry.ready = handled(this.settle(name, entry, replies[0], false));
      this.profiles.set(name, entry);
      return entry;
    });
    // Outside the queue: profiles declared meanwhile boot alongside this one.
    await entry.ready;
  }

  /**
   * Holds the profile another client of this server registered as `name`
   * (`serve --fork`), so that `page({ as: name })` and `base: name` work
   * here too: no boot, the pages fork from the server's snapshot. `page()`
   * and `profile()` adopt a name they do not know by themselves; the server
   * keeps a profile while some client holds it (until it releases it with
   * `releaseProfile` or disconnects) or another profile derives from it.
   * Rejects with the `ProfileSetupError` of a profile whose setup failed
   * (not holding it).
   */
  async adopt(name: string): Promise<void> {
    const entry = await this.updateProfiles(async () => {
      const entry = await this.resolveLocked(name);
      if (!entry) throw new Error(`no profile ${name}: adopting a profile needs pyro serve --fork`);
      return entry;
    });
    await entry.ready;
    if (entry.failed) throw entry.failed;
  }

  /** This client's profile `name`, adopting the server's when it does not have one. */
  private async resolveLocked(name: string): Promise<ProfileEntry | undefined> {
    const local = this.profiles.get(name);
    if (local || !this.shared) return local;
    let r: any;
    try {
      r = await this.cdp.send('Pyro.adoptPreparedPage', { name });
    } catch (e) {
      if (e instanceof CdpError && e.message.endsWith(`: no profile ${name}`)) {
        throw new Error(`no profile ${name} on this server: declare it with pyro.profile()`);
      }
      // Its setup failed: the server did not let this client hold it.
      const failed = e instanceof CdpError ? (e.data as { setupError?: unknown } | undefined)?.setupError : undefined;
      if (typeof failed === 'string') throw setupError(name, failed);
      throw e;
    }
    const profile: Profile = {
      url: r.url,
      initScripts: r.initScripts ?? [],
      ...(r.viewport ? { viewport: r.viewport } : {}),
      ...(r.userAgent != null ? { userAgent: r.userAgent } : {}),
      ...(r.proxyServer != null ? { proxyServer: r.proxyServer } : {}),
      ...(r.proxyBypassList != null ? { proxyBypassList: r.proxyBypassList } : {}),
      ...(r.locale != null ? { locale: r.locale } : {}),
      ...(r.timezoneId != null ? { timezoneId: r.timezoneId } : {}),
    };
    const entry: ProfileEntry = { ids: [r.profileId], profile, scripts: r.initScripts ?? [], definition: '' };
    entry.ready = handled(this.settle(name, entry, r, true));
    this.profiles.set(name, entry);
    return entry;
  }

  /**
   * Fork mode: resolves once the server has set profile `name` up (at once
   * when it was, or has no setup and no base, or the server does not say),
   * and rejects with its `ProfileSetupError` when its setup failed. An
   * adopted profile that failed is released and forgotten: asking for it
   * again asks the server again. Released or declared anew meanwhile, it is
   * no longer waited for.
   */
  private async settle(name: string, entry: ProfileEntry, reply: ProfileReply, adopted: boolean): Promise<void> {
    let { state, error } = reply;
    // Only a setup (its own or its base's) can fail: a profile without one is not waited for.
    const fallible = adopted
      ? !!reply.setup?.length || reply.base != null
      : !!entry.profile.setup?.length || entry.profile.base !== undefined;
    if (state !== 'failed' && !(state === 'booting' && fallible)) return;
    const current = () => this.profiles.get(name) === entry;
    for (let pause = 5; state === 'booting'; pause = Math.min(pause * 2, 50)) {
      await new Promise((r) => setTimeout(r, pause));
      if (!current()) return;
      try {
        ({ state, error } = await this.cdp.send('Pyro.preparedProfileStatus', { profileId: entry.ids![0] }));
      } catch (e) {
        if (!current()) return; // retired with its release
        throw e;
      }
    }
    if (state !== 'failed') return;
    entry.failed = setupError(name, error ?? 'setup failed');
    if (adopted) {
      await this.updateProfiles(async () => {
        if (this.profiles.get(name) !== entry) return;
        this.profiles.delete(name);
        await this.cdp.send('Pyro.unregisterPreparedPage', { name }).catch(() => {});
      });
    }
    throw entry.failed;
  }

  /**
   * Releases one name. Against `serve --fork` this drops this client's hold:
   * the server retires the profile once no client holds it and no other
   * profile derives from it. Otherwise shared native snapshots are retired
   * after their last alias. Acquired pages remain independent.
   */
  async releaseProfile(name: string): Promise<void> {
    return this.updateProfiles(async () => {
      const entry = this.profiles.get(name);
      if (!entry) return;
      if (entry.definition !== undefined) {
        await this.cdp.send('Pyro.unregisterPreparedPage', { name });
        this.profiles.delete(name);
        return;
      }
      if (entry.ids) {
        const conns = this.fork ? this.conns.slice(0, 1) : this.conns;
        await Promise.all(conns.map(async (conn, i) => {
          const profileId = entry.ids![i];
          const shared = [...this.profiles].some(([alias, other]) => alias !== name && other.ids?.[i] === profileId);
          if (!shared) await conn.cdp.send('Pyro.unregisterPreparedPage', { profileId });
        }));
      }
      this.profiles.delete(name);
    });
  }

  /**
   * Waits until every connection's pool of profile `name` is fully booted
   * (fork mode: once its setup ran). Rejects with the `ProfileSetupError` of
   * a profile whose setup failed.
   */
  async warm(name: string, timeoutMs = 5_000): Promise<void> {
    const entry = this.profiles.get(name) ?? (this.shared ? await this.updateProfiles(() => this.resolveLocked(name)) : undefined);
    if (!entry?.ids) return;
    await entry.ready;
    if (entry.failed) throw entry.failed;
    const deadline = Date.now() + timeoutMs;
    if (this.fork) {
      // A new connection is a fork of the template as it is now.
      for (;;) {
        const cdp = await Cdp.open(this.forkUrl(entry.ids[0]));
        const { pages } = await cdp.send('Pyro.preparedPages').finally(() => cdp.close());
        if (pages.some((p: { profileId: number; ready: boolean }) => p.profileId === entry.ids![0] && p.ready)) return;
        if (Date.now() > deadline) throw new Error(`no booted page for ${name} after ${timeoutMs} ms`);
        await new Promise((r) => setTimeout(r, 10));
      }
    }
    await Promise.all(
      this.conns.map(async (c, i) => {
        for (;;) {
          const { pages } = await c.cdp.send('Pyro.preparedPages');
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
   * page then boots fresh instead of coming from the pool. A profile whose
   * setup is still running is waited for; one whose setup failed throws its
   * `ProfileSetupError`.
   */
  async page(opts: { as?: string; route?: string; url?: string; initScripts?: string[] } = {}): Promise<Page> {
    const as = opts.as;
    let entry = as === undefined ? undefined : this.profiles.get(as);
    // Another client's profile on this server: adopt it.
    if (as !== undefined && !entry && this.shared) entry = await this.updateProfiles(() => this.resolveLocked(as));
    if (as !== undefined && !entry) throw new Error(`no profile ${as}: declare it with pyro.profile()`);
    await entry?.ready?.catch(() => {});
    if (entry?.failed) throw entry.failed;
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
        const r = await conn.cdp.send('Pyro.acquirePreparedPage', { profileId: entry.ids[index], attach: true });
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

/** A profile's `userAgent` and proxy as `registerPreparedPage` and `createBrowserContext` take them. */
function contextParams(p?: Profile): { userAgent?: string; proxyServer?: string; proxyBypassList?: string } {
  return {
    ...(p?.userAgent !== undefined ? { userAgent: p.userAgent } : {}),
    ...(p?.proxyServer !== undefined ? { proxyServer: p.proxyServer } : {}),
    ...(p?.proxyBypassList !== undefined ? { proxyBypassList: p.proxyBypassList } : {}),
  };
}

async function freshPage(conn: Conn, entry?: ProfileEntry, extra: string[] = []): Promise<Page> {
  const cdp = conn.cdp;
  // The profile's agent and proxy are its context's, as for its booted pages.
  const { browserContextId } = await cdp.send('Target.createBrowserContext', contextParams(entry?.profile));
  const { targetId } = await cdp.send('Target.createTarget', { url: 'about:blank', browserContextId });
  const { sessionId } = await cdp.send('Target.attachToTarget', { targetId, flatten: true });
  for (const source of [...(entry?.scripts ?? []), ...extra]) {
    await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source }, sessionId);
  }
  const v = entry?.profile.viewport;
  if (v) await cdp.send('Emulation.setDeviceMetricsOverride', { ...v, deviceScaleFactor: 1, mobile: false }, sessionId);
  const { locale, timezoneId } = emulationParams(entry?.profile);
  if (locale !== undefined) await cdp.send('Emulation.setLocaleOverride', { locale }, sessionId);
  if (timezoneId !== undefined) await cdp.send('Emulation.setTimezoneOverride', { timezoneId }, sessionId);
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
      r = await this.cdp.send('Pyro.run', { steps: wire, ...opts }, this.sessionId);
    } catch (e) {
      const m = e instanceof CdpError ? /step (\d+)/.exec(e.message) : null;
      if (!m) throw e;
      const index = Number(m[1]) - prefix.length;
      const where = index < 0 ? `opening the page (${prefix[Number(m[1])] && JSON.stringify(prefix[Number(m[1])].toStep())})` : `step ${index}`;
      const message = (e as Error).message.replace(/^Pyro\.run: step \d+/, where);
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
   * What the page shows now as raw pixels, nothing encoded (pyro's
   * `Page.captureScreenshot` formats `rgba`, `rgb24` and `yuv420p`): frames for
   * ffmpeg's `-f rawvideo -pix_fmt <format> -s <width>x<height>`. With `path`
   * (absolute) the browser appends the frame to that file, or to a FIFO ffmpeg
   * reads, and `data` is empty; a FIFO needs its reader first, and stays open
   * only while someone holds it open for writing (`frameFifo()`). `clip` is
   * in document CSS px; the frame is the viewport by default, at the device
   * scale factor. A page's pending route or navigation runs first.
   */
  async captureFrame(opts: { format?: RawFormat; path?: string; clip?: { x: number; y: number; width: number; height: number; scale?: number } } = {}): Promise<Frame> {
    // The page's route or navigation first, as a run would.
    if (this.prefix.length) await this.run([]);
    const r = await this.send('Page.captureScreenshot', {
      format: opts.format ?? 'rgba',
      ...(opts.path ? { path: opts.path } : {}),
      ...(opts.clip ? { clip: { scale: 1, ...opts.clip } } : {}),
    });
    return { width: r.pyro.width, height: r.pyro.height, bytes: r.pyro.bytes, data: Buffer.from(r.data, 'base64'), ms: r.pyro.ms };
  }

  /**
   * Runs `source` in every document this page loads from now on, before its
   * scripts (Playwright's `page.addInitScript`): follow it with `goto` or
   * `reload` for the current page to see it.
   */
  async addInitScript(source: string): Promise<void> {
    await this.cdp.send('Page.addScriptToEvaluateOnNewDocument', { source }, this.sessionId);
  }

  /**
   * Any other CDP command on this page's session. Browser-level commands
   * (`Browser.grantPermissions`, ...) are not page-session commands: use
   * `grantPermissions` / `resetPermissions`.
   */
  send<T = any>(method: string, params: object = {}): Promise<T> {
    return this.cdp.send(method, params, this.sessionId);
  }

  /**
   * Grants `permissions` (CDP `PermissionType`s such as `clipboardSanitizedWrite`,
   * `clipboardReadWrite`, `geolocation`) to this page's browser context, for
   * `origin` or every origin. What the context does not list is denied for
   * the origin, so an empty list denies everything there. Only this page's
   * context changes: it is sent on the page's own connection with its
   * `browserContextId`, never another's.
   */
  async grantPermissions(permissions: readonly string[], opts: { origin?: string } = {}): Promise<void> {
    await this.cdp.send('Browser.grantPermissions', {
      browserContextId: this.browserContextId,
      permissions,
      ...(opts.origin === undefined ? {} : { origin: opts.origin }),
    });
  }

  /** Clears every permission this page's browser context was granted or set. */
  async resetPermissions(): Promise<void> {
    await this.cdp.send('Browser.resetPermissions', { browserContextId: this.browserContextId });
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
