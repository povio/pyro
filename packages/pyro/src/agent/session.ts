// The browser an agent drives: one Pyro browser (launched or connected),
// its contexts and their pages (tabs), and what happened on them since the
// agent last looked (console errors, requests, dialogs, file choosers,
// downloads, new tabs, blocked requests). Built on `@povio/pyro/wright`.
import { copyFileSync, existsSync, lstatSync, mkdirSync, realpathSync, statSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { basename, dirname, extname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { chromium, type Browser, type BrowserContext, type BrowserContextOptions } from '../wright/browser.js';
import type { CDPSession } from '../wright/cdp-session.js';
import type { Dialog, FileChooser, Page, Request, Response } from '../wright/page.js';
import type { Download } from '../wright/handles.js';
import { OriginPolicy } from './origins.js';
import { identityKeys, identityOptions, proxyOption, ProfileStore, sessionStatus, when, type Identity, type LoginInfo, type SavedState, type SessionStatus } from './profiles.js';

export type BrowserAgentOptions = {
  /** A running `pyro serve` to use (`ws://…` or `http://host:port`); default `$PYRO_WS`, else one is launched. */
  wsEndpoint?: string;
  /** Launch `pyro serve --fork`. */
  fork?: boolean;
  /** The browser binary to launch (default `$PYRO_BIN`, else the bundled one). */
  bin?: string;
  /** Extra `pyro serve` arguments. */
  args?: string[];
  /** A wright browser to use instead (the session does not close it). */
  browser?: Browser;
  /** Viewport of the session's pages (default 1280x720). */
  viewport?: { width: number; height: number };
  /** Device pixels per CSS pixel of the session's pages (default 1): 3 makes a 2056x1200 viewport's screenshots 6168x3600. */
  deviceScaleFactor?: number;
  userAgent?: string;
  /** Proxy of the session's contexts: `http://user:pass@host:port` or Playwright's object. */
  proxy?: Identity['proxy'];
  /** Any other `browser.newContext()` options of the session's contexts. */
  contextOptions?: BrowserContextOptions;
  /** Where stored identities live (default `~/.pyro/profiles`). */
  profileDir?: string;
  /** Start in this stored identity. */
  profile?: string;
  /** Write nothing to the profile directory (`browser_profiles` is not offered; `profile` still loads). */
  isolated?: boolean;
  /** Origins the browser may request (default: all). */
  allowedOrigins?: string[];
  /** Origins the browser may not request. */
  blockedOrigins?: string[];
  /** Where screenshots, PDFs and downloads are written (default `<tmp>/pyro-mcp-output-<uid>`); file names stay inside it. */
  outputDir?: string;
  /** Directories `browser_file_upload` may read from (default: `outputDir`, and the working directory unless it is / or the home directory). */
  fileRoots?: string[];
  /** Action tools return the page's snapshot (default true). */
  snapshotAfterAction?: boolean;
  /** How long an action waits for its element to be actionable, ms (default 5000). */
  actionTimeout?: number;
  /** How long a navigation may take, ms (default 60000). */
  navigationTimeout?: number;
  /** Longest snapshot returned, in characters (default 100000). */
  maxSnapshotChars?: number;
};

type ConsoleEntry = { type: string; text: string; where?: string };
type RequestEntry = { method: string; url: string; type: string; status?: number; statusText?: string; failure?: string };

/** Where stored identities live unless `profileDir` says otherwise. */
export const DEFAULT_PROFILE_DIR = join(homedir(), '.pyro', 'profiles');

const CONSOLE_LIMIT = 1000;
const REQUEST_LIMIT = 500;

/** A browser context of the session: the default one, or a stored identity's. */
export class ContextEntry {
  /** Where a person signed the stored identity in (`pyro profile login`). */
  login?: LoginInfo;
  constructor(
    readonly context: BrowserContext,
    /** The stored identity it was loaded from. */
    readonly profile?: string,
  ) {}
}

/** One page of the session. */
export class Tab {
  readonly console: ConsoleEntry[] = [];
  /** Console entries already shown as new errors. */
  consoleSeen = 0;
  readonly requests: RequestEntry[] = [];
  private byRequest = new Map<Request, RequestEntry>();
  /** An open dialog: other tools wait until `browser_handle_dialog` answers it. */
  dialog?: Dialog;
  /** A file chooser a click opened: `browser_file_upload` answers it. */
  chooser?: FileChooser;
  /** The action a dialog interrupted (it resumes when the dialog is answered), and the URL before it. */
  pending?: { outcome: Promise<Settled>; before: string };
  /** The page's title when last read (read again whenever it is safe). */
  title = '';
  /** A page the agent did not open (a popup): the next result announces it. */
  announce = false;
  /** The sign-in page URL last pointed out (a stored identity's tab that landed on one). */
  signInNoted?: string;
  /** Device pixels per CSS pixel: the context's, or the last `browser_resize`'s. */
  deviceScaleFactor: number;
  private modalWaiters: (() => void)[] = [];
  private cdpSession?: Promise<CDPSession>;

  constructor(
    readonly page: Page,
    readonly entry: ContextEntry,
    session: BrowserAgentSession,
  ) {
    this.deviceScaleFactor = entry.context._options.deviceScaleFactor || 1;
    page.on('console', (m) => {
      const loc = m.location();
      this.log({ type: m.type(), text: m.text(), where: loc.url ? `${loc.url}:${loc.lineNumber + 1}` : undefined });
    });
    page.on('pageerror', (e) => this.log({ type: 'error', text: `Uncaught ${e.message}` }));
    page.on('request', (r) => this.onRequest(r));
    page.on('response', (r) => this.onResponse(r));
    page.on('requestfailed', (r) => {
      const e = this.byRequest.get(r);
      if (e) e.failure = r.failure()?.errorText ?? 'failed';
    });
    page.on('dialog', (d) => {
      this.dialog = d;
      this.fireModal();
    });
    page.on('filechooser', (c) => {
      this.chooser = c;
      this.fireModal();
    });
    // A new document: a chooser of the old one is gone with it.
    page.on('load', () => (this.chooser = undefined));
    page.on('download', (d) => session._download(this, d));
  }

  /** A raw CDP session on the page (screenshots with a clip scale, device metrics). */
  cdp(): Promise<CDPSession> {
    this.cdpSession ??= this.entry.context.newCDPSession(this.page).catch((e) => {
      this.cdpSession = undefined;
      throw e;
    });
    return this.cdpSession;
  }

  /** The viewport in CSS px, as the page was given it. */
  viewport(): { width: number; height: number } {
    return this.page.viewportSize() ?? { width: 1280, height: 720 };
  }

  /** `opened` resolves when a dialog or file chooser opens on this page; `dispose` stops waiting. */
  waitModal(): { opened: Promise<void>; dispose: () => void } {
    let waiter!: () => void;
    const opened = new Promise<void>((resolve) => (waiter = resolve));
    this.modalWaiters.push(waiter);
    return { opened, dispose: () => (this.modalWaiters = this.modalWaiters.filter((w) => w !== waiter)) };
  }

  private fireModal(): void {
    for (const w of this.modalWaiters.splice(0)) w();
  }

  private log(e: ConsoleEntry): void {
    this.console.push(e);
    if (this.console.length > CONSOLE_LIMIT) {
      const cut = this.console.length - CONSOLE_LIMIT;
      this.console.splice(0, cut);
      this.consoleSeen = Math.max(0, this.consoleSeen - cut);
    }
  }

  /** Console errors (and uncaught exceptions) since the last call. */
  takeNewErrors(): ConsoleEntry[] {
    const fresh = this.console.slice(this.consoleSeen).filter((e) => e.type === 'error');
    this.consoleSeen = this.console.length;
    return fresh;
  }

  private onRequest(r: Request): void {
    const e: RequestEntry = { method: r.method(), url: r.url(), type: r.resourceType() };
    this.requests.push(e);
    this.byRequest.set(r, e);
    if (this.requests.length > REQUEST_LIMIT) {
      const gone = this.requests.splice(0, this.requests.length - REQUEST_LIMIT);
      for (const [req, entry] of this.byRequest) if (gone.includes(entry)) this.byRequest.delete(req);
    }
  }

  private onResponse(r: Response): void {
    const e = this.byRequest.get(r.request());
    if (!e) return;
    e.status = r.status();
    e.statusText = r.statusText();
  }
}

export type Settled = { ok: true; value: unknown } | { ok: false; error: unknown };
/** `p` as a promise that never rejects. */
export const settle = (p: Promise<unknown>): Promise<Settled> =>
  p.then(
    (value) => ({ ok: true as const, value }),
    (error) => ({ ok: false as const, error }),
  );

export class BrowserAgentSession {
  readonly policy: OriginPolicy;
  readonly profiles: ProfileStore;
  readonly outputDir: string;
  /** The output directory is the default one in the temporary directory: it must be ours. */
  private readonly ownOutputDir: boolean;
  readonly fileRoots: string[];
  readonly snapshotAfterAction: boolean;
  readonly actionTimeout: number;
  readonly navigationTimeout: number;
  readonly maxSnapshotChars: number;
  private browserPromise?: Promise<Browser>;
  private entries: ContextEntry[] = [];
  private tabList: Tab[] = [];
  private currentTab?: Tab;
  private notes: string[] = [];
  private blocked: { url: string; why: string }[] = [];
  private queue: Promise<unknown> = Promise.resolve();
  private closed = false;
  /** Inside our own `newPage()`: the context's 'page' event is not a popup. */
  private opening = 0;
  /** The startup profile was loaded (or failed): the default context is a plain one from then on. */
  private startedProfile = false;

  constructor(readonly options: BrowserAgentOptions = {}) {
    this.policy = new OriginPolicy(options.allowedOrigins, options.blockedOrigins);
    this.profiles = new ProfileStore(options.profileDir ?? DEFAULT_PROFILE_DIR, options.isolated ?? false);
    this.ownOutputDir = options.outputDir === undefined;
    this.outputDir = resolve(options.outputDir ?? join(tmpdir(), `pyro-mcp-output${process.getuid ? `-${process.getuid()}` : ''}`));
    // The working directory, unless it is / or the home directory (where MCP clients often start servers).
    const cwd = resolve(process.cwd());
    this.fileRoots = (options.fileRoots ?? [...(cwd === resolve('/') || cwd === resolve(homedir()) ? [] : [cwd]), this.outputDir]).map((r) => resolve(r));
    this.snapshotAfterAction = options.snapshotAfterAction ?? true;
    this.actionTimeout = options.actionTimeout ?? 5_000;
    this.navigationTimeout = options.navigationTimeout ?? 60_000;
    this.maxSnapshotChars = options.maxSnapshotChars ?? 100_000;
  }

  get isClosed(): boolean {
    return this.closed;
  }

  /** Runs tool calls one at a time: a page does one thing at once. */
  run<T>(fn: () => Promise<T>): Promise<T> {
    const result = this.queue.then(fn, fn);
    this.queue = result.catch(() => undefined);
    return result;
  }

  /** The browser, launched or connected on first use. */
  browser(): Promise<Browser> {
    if (this.closed) return Promise.reject(new Error('the browser session is closed'));
    this.browserPromise ??= this.openBrowser().catch((e) => {
      this.browserPromise = undefined;
      throw e;
    });
    return this.browserPromise;
  }

  private async openBrowser(): Promise<Browser> {
    const o = this.options;
    const browser = o.browser
      ? o.browser
      : (o.wsEndpoint ?? process.env.PYRO_WS)
        ? await chromium.connectOverCDP((o.wsEndpoint ?? process.env.PYRO_WS)!)
        : await chromium.launch({ bin: o.bin, args: [...(o.fork ? ['--fork'] : []), ...(o.args ?? [])] });
    browser.on('disconnected', () => {
      if (this.browserPromise === undefined) return;
      this.browserPromise = undefined;
      this.entries = [];
      this.tabList = [];
      this.currentTab = undefined;
      if (!this.closed) this.note('The browser disconnected: its tabs are gone. The next browser tool call starts a new one.');
    });
    return browser;
  }

  /** Options of a new context: the session's, under the given ones. */
  baseContextOptions(): BrowserContextOptions {
    const o = this.options;
    return {
      ...o.contextOptions,
      ...(o.viewport ? { viewport: o.viewport } : {}),
      ...(o.deviceScaleFactor ? { deviceScaleFactor: o.deviceScaleFactor } : {}),
      ...(o.userAgent ? { userAgent: o.userAgent } : {}),
      ...(o.proxy ? { proxy: proxyOption(o.proxy) } : {}),
    };
  }

  /** A new context of the session, routed through the origin policy. */
  async newContext(options: BrowserContextOptions = {}, profile?: string): Promise<ContextEntry> {
    const browser = await this.browser();
    const context = await browser.newContext({ ...this.baseContextOptions(), ...options });
    if (this.policy.active) {
      await context.route('**/*', async (route, request) => {
        const why = this.policy.check(request.url());
        if (!why) return route.continue();
        this.blocked.push({ url: request.url(), why });
        await route.abort('blockedbyclient');
      });
    }
    const entry = new ContextEntry(context, profile);
    this.entries.push(entry);
    context.on('page', (page) => this.adopt(page, entry));
    context.on('close', () => {
      this.entries = this.entries.filter((e) => e !== entry);
    });
    return entry;
  }

  /** A page of one of our contexts as a tab (a popup when we did not open it). */
  private adopt(page: Page, entry: ContextEntry): Tab {
    const known = this.tabList.find((t) => t.page === page);
    if (known) return known;
    const tab = new Tab(page, entry, this);
    tab.announce = this.opening === 0;
    this.tabList.push(tab);
    page.on('close', () => this.forget(tab));
    return tab;
  }

  private forget(tab: Tab): void {
    const i = this.tabList.indexOf(tab);
    if (i < 0) return;
    this.tabList.splice(i, 1);
    if (this.currentTab === tab) this.currentTab = this.tabList[Math.min(i, this.tabList.length - 1)];
  }

  /** The tab tools act on: the current one, made (with its context) when there is none. */
  async tab(): Promise<Tab> {
    await this.sync();
    if (this.currentTab && !this.currentTab.page.isClosed()) return this.currentTab;
    const tab = await this.newTab();
    this.currentTab = tab;
    return tab;
  }

  /** The default context: the first one, made on first use (from the startup profile when there is one). */
  async defaultEntry(): Promise<ContextEntry> {
    if (this.entries.length) return this.entries[0];
    if (this.options.profile && !this.startedProfile) {
      this.startedProfile = true;
      const name = this.options.profile;
      const loaded = await this.profileEntry(name);
      if (loaded.session?.expired) this.note(expiredHint(name, `its session cookies expired ${when(loaded.session.expiresAt!)}`, this.loginCommand(name, loaded.login)));
      return loaded.entry;
    }
    return this.newContext();
  }

  /** A new tab in `entry` (default: the default context). */
  async newTab(entry?: ContextEntry): Promise<Tab> {
    const e = entry ?? (await this.defaultEntry());
    this.opening++;
    let page: Page;
    try {
      page = await e.context.newPage();
    } finally {
      this.opening--;
    }
    return this.adopt(page, e);
  }

  get tabs(): readonly Tab[] {
    return this.tabList;
  }

  get current(): Tab | undefined {
    return this.currentTab;
  }

  select(tab: Tab): void {
    this.currentTab = tab;
  }

  /** A tab with an open dialog (the current one first). */
  modalTab(): Tab | undefined {
    if (this.currentTab?.dialog) return this.currentTab;
    return this.tabList.find((t) => t.dialog);
  }

  /**
   * Takes in pages that appeared since the last call (popups: a popup's page
   * is in its context's pages as soon as the browser announces it) and drops
   * closed ones. Notes each new tab once it has loaded far enough to have a URL.
   */
  async sync(): Promise<void> {
    for (const tab of [...this.tabList]) if (tab.page.isClosed()) this.forget(tab);
    for (const entry of this.entries) {
      for (const page of entry.context.pages()) if (!page.isClosed()) this.adopt(page, entry);
    }
    for (const tab of this.tabList.filter((t) => t.announce)) {
      // The title waits for the popup's page to be attached, so its URL is known after it.
      if (!this.modalTab()) tab.title = await withTimeout(tab.page.title(), 5_000).catch(() => '');
      if (!tab.announce || tab.page.isClosed()) continue;
      tab.announce = false;
      this.note(`A new tab opened: [${this.tabList.indexOf(tab)}] ${tab.page.url()} (switch to it with browser_tabs select).`);
    }
  }

  /** An event for the next tool result. */
  note(text: string): void {
    this.notes.push(text);
  }

  /** Events since the last result, blocked requests summed up. */
  takeNotes(): string[] {
    const out = this.notes.splice(0);
    if (this.blocked.length) {
      const list = this.blocked.splice(0);
      const urls = [...new Set(list.map((b) => b.url))];
      const why = [...new Set(list.map((b) => b.why))].join(', ');
      out.push(`Blocked ${list.length} request${list.length > 1 ? 's' : ''} (${why}): ${urls.slice(0, 3).join(', ')}${urls.length > 3 ? `, and ${urls.length - 3} more` : ''}.`);
    }
    return out;
  }

  /** A download a page started: copied into the output directory when it ends. */
  _download(tab: Tab, download: Download): void {
    const name = safeName(download.suggestedFilename() || 'download');
    this.note(`Download started on tab [${this.tabList.indexOf(tab)}]: ${JSON.stringify(name)} from ${download.url()}.`);
    void download
      .path()
      .then((from) => {
        const to = uniquePath(this.outputPath(join('downloads', name), name));
        copyFileSync(from, to);
        this.note(`Download finished: ${to}`);
      })
      .catch((e) => this.note(`Download of ${JSON.stringify(name)} failed: ${e instanceof Error ? e.message : e}`));
  }

  /** A new context from stored identity `name`: its saved session, identity file and sign-in record. */
  private async profileEntry(name: string): Promise<LoadedProfile> {
    const { state, identity, login } = this.profiles.read(name);
    const options: BrowserContextOptions = { ...(identity ? identityOptions(identity) : {}), ...(state ? { storageState: state } : {}) };
    const entry = await this.newContext(options, name);
    entry.login = login;
    return { entry, state, identity, login, session: state ? sessionStatus(state) : undefined, applied: identity ? identityKeys(identity) : [] };
  }

  /** A new context from stored identity `name`, with a tab, made current. */
  async profileContext(name: string): Promise<LoadedProfile & { tab: Tab }> {
    const loaded = await this.profileEntry(name);
    const tab = await this.newTab(loaded.entry);
    this.currentTab = tab;
    return { ...loaded, tab };
  }

  /** The command a person runs to sign `name` in again (`--url` from the last sign-in, `--profile-dir` unless the default). */
  loginCommand(name: string, login?: LoginInfo): string {
    const words = ['pyro', 'profile', 'login', name, '--url', login?.url ?? '<the app URL>'];
    if (resolve(this.profiles.dir) !== resolve(DEFAULT_PROFILE_DIR)) words.push('--profile-dir', this.profiles.dir);
    return words.map((w) => (w.startsWith('<') ? w : shellWord(w))).join(' ');
  }

  /**
   * Sets `tab`'s viewport (CSS px) and, when given, its device scale factor
   * (`Emulation.setDeviceMetricsOverride`, as Playwright sets a context's).
   */
  async resize(tab: Tab, size: { width: number; height: number }, deviceScaleFactor?: number): Promise<void> {
    if (deviceScaleFactor !== undefined) {
      const o = tab.entry.context._options;
      const screen = o.screen ?? size;
      const landscape = screen.width > screen.height;
      await (await tab.cdp()).send('Emulation.setDeviceMetricsOverride', {
        width: size.width,
        height: size.height,
        deviceScaleFactor,
        mobile: !!o.isMobile,
        screenWidth: screen.width,
        screenHeight: screen.height,
        screenOrientation: o.isMobile ? (landscape ? { angle: 90, type: 'landscapePrimary' } : { angle: 0, type: 'portraitPrimary' }) : { angle: 0, type: 'landscapePrimary' },
      });
      tab.deviceScaleFactor = deviceScaleFactor;
    }
    // The size again through the driver, which keeps the scale and tracks the viewport.
    await tab.page.setViewportSize(size);
  }

  /**
   * A path for a file a tool writes: `filename` (default `fallback`) inside
   * the output directory. Absolute names, `..` and symlinks may not lead out of it.
   */
  outputPath(filename: string | undefined, fallback: string): string {
    const dir = this.outputDir;
    if (this.ownOutputDir) {
      // In a shared temporary directory another user could have made it first.
      mkdirSync(dir, { recursive: true, mode: 0o700 });
      const st = lstatSync(dir);
      if (st.isSymbolicLink() || (process.getuid && st.uid !== process.getuid())) {
        throw new Error(`the output directory ${dir} is not this user's: set another one (--output-dir)`);
      }
    }
    const target = resolve(dir, filename || fallback);
    if (!inside(target, dir)) throw new Error(`filename must stay inside the output directory ${dir}: ${filename}`);
    mkdirSync(dirname(target), { recursive: true });
    if (!inside(realpathSync(dirname(target)), realpathSync(dir))) throw new Error(`filename leads out of the output directory ${dir}: ${filename}`);
    if (existsSync(target) && lstatSync(target).isSymbolicLink()) throw new Error(`will not write through a symlink: ${target}`);
    return target;
  }

  /** `path` as a file `browser_file_upload` may read: an existing file under one of the file roots. */
  uploadPath(path: string): string {
    let real: string;
    try {
      real = realpathSync(resolve(path));
    } catch {
      throw new Error(`no such file: ${path}`);
    }
    if (!statSync(real).isFile()) throw new Error(`not a file: ${path}`);
    const roots = this.fileRoots.filter(existsSync).map((r) => realpathSync(r));
    if (!roots.some((r) => inside(real, r))) {
      throw new Error(`${path} is outside the directories uploads may read from (${roots.join(', ')}); the server's --file-roots option sets them`);
    }
    return real;
  }

  /** `browser_close`: closes every context and tab; the next tool call starts over. */
  async closeBrowser(): Promise<void> {
    const browserPromise = this.browserPromise;
    const entries = this.entries;
    this.entries = [];
    this.tabList = [];
    this.currentTab = undefined;
    this.browserPromise = undefined;
    this.startedProfile = false;
    if (!browserPromise) return;
    const browser = await browserPromise.catch(() => undefined);
    if (!browser) return;
    if (this.options.browser) {
      for (const e of entries) await e.context.close().catch(() => {});
    } else {
      await browser.close().catch(() => {});
    }
  }

  /** Ends the session: the browser it launched exits. */
  async close(): Promise<void> {
    if (this.closed) return;
    await this.closeBrowser();
    this.closed = true;
  }
}

/** A stored identity loaded into a context of its own. */
export type LoadedProfile = {
  entry: ContextEntry;
  state?: SavedState;
  identity?: Identity;
  /** Where the person signed in (`pyro profile login`), to check the session. */
  login?: LoginInfo;
  /** The saved session cookies' expiry. */
  session?: SessionStatus;
  /** Identity settings applied (names only). */
  applied: string[];
};

/** What the agent is told when a stored identity's session is gone: a person signs in again, never the agent. */
export function expiredHint(name: string, why: string, command: string): string {
  return `The saved session of stored identity ${JSON.stringify(name)} has expired: ${why}. Do not sign in yourself (no sign-in forms, links or codes): ask the person to run \`${command}\` in a terminal and to tell you when it is done, then load the identity again (browser_profiles load).`;
}

/** A shell word: quoted when it holds anything but plain characters. */
const shellWord = (w: string) => (/^[\w@%+=:,./-]+$/.test(w) ? w : `'${w.replace(/'/g, `'\\''`)}'`);

/** True when `path` is `dir` or below it. */
function inside(path: string, dir: string): boolean {
  const rel = relative(dir, path);
  return rel === '' || (!isAbsolute(rel) && rel !== '..' && !rel.startsWith(`..${sep}`));
}

function safeName(name: string): string {
  const base = basename(name).replace(/[\u0000-\u001f<>:"/\\|?*]/g, '_').trim();
  return base && base !== '.' && base !== '..' ? base : 'download';
}

/** `path`, or `name-2.ext`, `name-3.ext`… when it exists. */
function uniquePath(path: string): string {
  if (!existsSync(path)) return path;
  const ext = extname(path);
  const stem = path.slice(0, path.length - ext.length);
  for (let i = 2; ; i++) if (!existsSync(`${stem}-${i}${ext}`)) return `${stem}-${i}${ext}`;
}

export function withTimeout<T>(p: Promise<T>, ms: number, message = `timed out after ${ms} ms`): Promise<T> {
  let timer: ReturnType<typeof setTimeout>;
  return Promise.race([
    p.finally(() => clearTimeout(timer)),
    new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(message)), ms);
    }),
  ]);
}
