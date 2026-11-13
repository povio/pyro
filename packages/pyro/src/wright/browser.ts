// Playwright's Browser and BrowserContext on one pyro connection.
import { Clock } from './clock.js';
import { EventEmitter } from 'node:events';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Pyro, type ConnectOptions } from '../index.js';
import { CdpError } from '../cdp.js';
import { urlMatches, resolveUrl, type UrlMatch } from './match.js';
import { Download, type PageFunction } from './handles.js';
import { APIRequestContext } from './request.js';
import { HarRecorder } from './har.js';
import { CDPSession } from './cdp-session.js';
import {
  Frame,
  type ConsoleMessage,
  type Request,
  type Response,
  bindingName,
  bindingScript,
  harHandler,
  initScriptSource,
  Page,
  callOwner,
  sameRoute,
  WORKER_AUTO_ATTACH,
  type Attachment,
  type HarOptions,
  type RouteEntry,
  type RouteHandler,
} from './page.js';

type Cookie = {
  name: string;
  value: string;
  url?: string;
  domain?: string;
  path?: string;
  expires?: number;
  httpOnly?: boolean;
  secure?: boolean;
  sameSite?: 'Strict' | 'Lax' | 'None';
};
type StorageState = { cookies?: Cookie[]; origins?: { origin: string; localStorage: { name: string; value: string }[] }[] };
/** What `storageState()` returns: both lists, always. */
type SavedState = { cookies: Cookie[]; origins: { origin: string; localStorage: { name: string; value: string }[] }[] };

/**
 * `browser.newContext()` options: Playwright's. The ones pyro does not
 * emulate (geolocation, permissions, locale, ...) are accepted and have no effect.
 */
export type BrowserContextOptions = {
  baseURL?: string;
  viewport?: { width: number; height: number } | null;
  screen?: { width: number; height: number };
  storageState?: string | StorageState;
  extraHTTPHeaders?: { [key: string]: string };
  userAgent?: string;
  colorScheme?: 'light' | 'dark' | 'no-preference' | null;
  reducedMotion?: 'reduce' | 'no-preference' | null;
  forcedColors?: 'active' | 'none' | null;
  acceptDownloads?: boolean;
  bypassCSP?: boolean;
  deviceScaleFactor?: number;
  geolocation?: { latitude: number; longitude: number; accuracy?: number };
  hasTouch?: boolean;
  httpCredentials?: { username: string; password: string; origin?: string; send?: 'unauthorized' | 'always' };
  ignoreHTTPSErrors?: boolean;
  isMobile?: boolean;
  javaScriptEnabled?: boolean;
  locale?: string;
  offline?: boolean;
  permissions?: string[];
  proxy?: { server: string; bypass?: string; username?: string; password?: string };
  serviceWorkers?: 'allow' | 'block';
  strictSelectors?: boolean;
  timezoneId?: string;
  /** Accepted; pyro records no video. */
  recordVideo?: { dir: string; size?: { width: number; height: number } };
  /** Recorded as `routeFromHAR(path, { update: true })` does. */
  recordHar?: { path: string; content?: 'omit' | 'embed' | 'attach'; mode?: 'full' | 'minimal'; urlFilter?: string | RegExp; omitContent?: boolean };
};
/** @internal */
export type ContextOptions = BrowserContextOptions;

const DEFAULT_VIEWPORT = { width: 1280, height: 720 };

export interface BrowserEvents {
  disconnected: Browser;
}
export interface BrowserContextEvents {
  close: BrowserContext;
  console: ConsoleMessage;
  page: Page;
  request: Request;
  requestfailed: Request;
  requestfinished: Request;
  response: Response;
}

/**
 * Pages booted ahead (`use: { pyro: { boot: [...] } }`): a test
 * whose page's first `goto` is `url`, or matches `routes` on the same origin,
 * gets a page that already loaded `url` with the context's init scripts,
 * storage and viewport, and the goto becomes a same-document route change
 * (`pushState` + `popstate`, settled). Booted before the test runs: the
 * app's shell and boot requests come from before the test seeded anything.
 */
export type BootSpec = { url: string; routes?: UrlMatch | UrlMatch[]; pool?: number };

/** Contexts not closed yet, of every browser of this process. */
const openContextSet = new Set<BrowserContext>();
/** Every open context (Playwright's `_allContexts()`), oldest first. */
export function openContexts(): BrowserContext[] {
  return [...openContextSet];
}
const closingHooks = new Set<(context: BrowserContext) => Promise<void>>();
/** Calls `fn` when a context starts closing, before its pages close (`pyro run`: their screenshots); returns the unsubscribe. */
export function beforeContextClose(fn: (context: BrowserContext) => Promise<void>): () => void {
  closingHooks.add(fn);
  return () => void closingHooks.delete(fn);
}

type TestDefaults = { options: ContextOptions; timeout?: number; navigationTimeout?: number; testIdAttribute: string };
let testDefaults: TestDefaults | undefined;
/** The running test's `use` options (`@povio/pyro/test`): `browser.newContext()` starts from them. */
export function setTestDefaults(d: TestDefaults | undefined) {
  testDefaults = d;
}

export class Browser {
  private _contexts = new Set<BrowserContext>();
  private _events = new EventEmitter();
  get _defaults(): ContextOptions {
    return testDefaults?.options ?? {};
  }
  get _defaultTimeout(): number | undefined {
    return testDefaults?.timeout;
  }
  get _defaultNavigationTimeout(): number | undefined {
    return testDefaults?.navigationTimeout;
  }
  get _testIdAttribute(): string {
    return testDefaults?.testIdAttribute ?? 'data-testid';
  }

  /** Booted-ahead page profiles on this connection, by profile key. */
  private _profiles = new Map<string, Promise<number>>();
  /** Pages taken from a pool and pages created fresh (`browser._stats`). */
  readonly _stats = { pooled: 0, fresh: 0 };

  /** Where downloads land (named by guid) until `saveAs` copies them; removed on close. */
  private _downloadDir = mkdtempSync(join(tmpdir(), 'pyro-downloads-'));
  private _downloads = new Map<string, Download>();

  constructor(
    readonly _lp: Pyro,
    private _owned = true,
    readonly _boot: BootSpec[] = [],
  ) {
    const cdp = _lp.cdp;
    cdp.onClose(() => this._events.emit('disconnected', this));
    // Popups: targets announced with an opener become pages of the opener's context.
    cdp.on('Target.targetCreated', (p) => {
      const info = p.targetInfo;
      if (info?.type !== 'page' || !info.openerId) return;
      for (const context of this._contexts) {
        const opener = context.pages().find((page) => page._targetId === info.openerId);
        if (opener) return void context._adoptPopup(info.targetId, opener);
      }
    });
    cdp.on('Browser.downloadWillBegin', (p) => {
      for (const context of this._contexts) {
        const page = context.pages().find((page) => page._mainFrameIdValue === p.frameId) ?? (p.frameId ? undefined : context.pages()[0]);
        if (!page) continue;
        const download = new Download(page, p.url, p.suggestedFilename);
        this._downloads.set(p.guid, download);
        page._emit('download', download);
        return;
      }
    });
    cdp.on('Browser.downloadProgress', (p) => {
      const download = this._downloads.get(p.guid);
      if (!download || p.state === 'inProgress') return;
      this._downloads.delete(p.guid);
      download._finish(p.state === 'completed' ? { path: p.filePath ?? join(this._downloadDir, p.guid), failure: null } : { path: null, failure: 'canceled' });
    });
    cdp.post('Target.setDiscoverTargets', { discover: true });
    cdp.post('Browser.setDownloadBehavior', { behavior: 'allowAndName', downloadPath: this._downloadDir, eventsEnabled: true });
  }

  /** The boot spec covering a first navigation to `target`, and its boot URL. */
  async _bootFor(target: string, baseURL?: string): Promise<{ spec: BootSpec; url: string } | undefined> {
    for (const spec of this._boot) {
      const url = resolveUrl(baseURL, spec.url);
      if (url === target) return { spec, url };
      if (!spec.routes || new URL(url).origin !== new URL(target).origin) continue;
      for (const r of Array.isArray(spec.routes) ? spec.routes : [spec.routes]) {
        if (await urlMatches(baseURL, target, r)) return { spec, url };
      }
    }
    return undefined;
  }

  /** The prepared-page profile for these settings, registered on first use. */
  _profile(url: string, initScripts: string[], viewport: { width: number; height: number } | null, pool: number): Promise<number> {
    const key = JSON.stringify([url, initScripts, viewport, pool]);
    let id = this._profiles.get(key);
    if (!id) {
      id = this._lp.cdp
        .send('Pyro.registerPreparedPage', { url, initScripts, ...(viewport ? { viewport } : {}), pool })
        .then((r: { profileId: number }) => r.profileId);
      id.catch(() => this._profiles.delete(key));
      this._profiles.set(key, id);
    }
    return id;
  }

  async newContext(options: ContextOptions = {}): Promise<BrowserContext> {
    const merged: ContextOptions = { ...this._defaults, ...options };
    const context = new BrowserContext(this, merged);
    await context._init();
    this._contexts.add(context);
    openContextSet.add(context);
    return context;
  }
  /** A page in a context of its own, closed with the page. */
  async newPage(options: ContextOptions = {}): Promise<Page> {
    const context = await this.newContext(options);
    const page = await context.newPage();
    context._ownedBy = page;
    return page;
  }
  contexts(): BrowserContext[] {
    return [...this._contexts];
  }
  _contextClosed(context: BrowserContext) {
    this._contexts.delete(context);
  }
  isConnected(): boolean {
    return !this._lp.cdp.isClosed;
  }
  version(): string {
    return 'pyro';
  }
  browserType() {
    return { name: () => 'chromium' };
  }
  on<K extends keyof BrowserEvents>(event: K, listener: (arg: BrowserEvents[K]) => unknown): this {
    this._events.on(event, listener as (...args: any[]) => void);
    return this;
  }
  once<K extends keyof BrowserEvents>(event: K, listener: (arg: BrowserEvents[K]) => unknown): this {
    this._events.once(event, listener as (...args: any[]) => void);
    return this;
  }
  off<K extends keyof BrowserEvents>(event: K, listener: (arg: BrowserEvents[K]) => unknown): this {
    this._events.off(event, listener as (...args: any[]) => void);
    return this;
  }
  async close(): Promise<void> {
    for (const c of [...this._contexts]) await c.close();
    if (this._owned) this._lp.close();
    rmSync(this._downloadDir, { recursive: true, force: true });
  }
}

export class BrowserContext {
  /** The fake clock this context's pages share (`page.clock` too). */
  readonly clock: Clock = new Clock(this);
  _routes: RouteEntry[] = [];
  /** `routeFromHAR(…, { update: true })` recorders: written when the context closes. */
  _har: HarRecorder[] = [];
  /** `exposeBinding` callbacks of every page of this context, by binding name. */
  _bindings = new Map<string, { name: string; callback: (source: any, ...args: any[]) => unknown; script: string }>();
  /** The browser context id: made on the first page that needs one, or a booted page's. */
  private _idPromise?: Promise<string>;
  private _id?: string;
  private _pages = new Set<Page>();
  private _initScripts: string[] = [];
  private _cookies: Cookie[] = [];
  private _closed = false;
  private _events = new EventEmitter();
  _defaultTimeout?: number;
  _defaultNavigationTimeout?: number;
  _testIdAttribute: string;
  /** `browser.newPage()`: the page whose close closes this context. */
  _ownedBy?: Page;
  /** Who made it (`setCallOwner`; `pyro run`: the test running then). */
  readonly _owner = callOwner();

  constructor(
    private _browser: Browser,
    /** Read by `expect(page).toHaveURL()` for `baseURL`. */
    readonly _options: ContextOptions,
  ) {
    this._defaultTimeout = _browser._defaultTimeout;
    this._defaultNavigationTimeout = _browser._defaultNavigationTimeout;
    this._testIdAttribute = _browser._testIdAttribute;
  }

  async _init(): Promise<void> {
    const har = this._options.recordHar;
    if (har) {
      const content = har.omitContent ? 'omit' : (har.content ?? (har.path.endsWith('.zip') ? 'attach' : 'embed'));
      await this._recordHar(har.path, { update: true, updateContent: content, updateMode: har.mode, ...(har.urlFilter ? { url: har.urlFilter } : {}) });
    }
    const state = this._options.storageState;
    if (!state) return;
    const s: StorageState = typeof state === 'string' ? JSON.parse(readFileSync(state, 'utf8')) : state;
    if (s.cookies?.length) this._cookies.push(...s.cookies);
    const origins = (s.origins ?? []).filter((o) => o.localStorage?.length);
    if (origins.length) {
      // Once per context, as Playwright seeds storage: a marker keeps later
      // documents from writing the entries again over the app's changes.
      const entries = Object.fromEntries(origins.map((o) => [o.origin, o.localStorage.map((e) => [e.name, e.value])]));
      this._initScripts.push(
        `(() => { try { const e = ${JSON.stringify(entries)}[location.origin]; if (!e || localStorage.getItem('__pyroStorageState')) return; for (const [k, v] of e) localStorage.setItem(k, v); localStorage.setItem('__pyroStorageState', '1'); } catch {} })();`,
      );
    }
  }

  private get cdp() {
    return this._browser._lp.cdp;
  }

  async newPage(): Promise<Page> {
    if (this._closed) throw new Error('browserContext.newPage: Target page, context or browser has been closed');
    const o = this._options;
    const viewport = o.viewport === null ? null : (o.viewport ?? DEFAULT_VIEWPORT);
    // No target yet: the page's first command decides which (see `_attachPage`).
    const page = new Page(this, this.cdp, viewport);
    this._pages.add(page);
    this._events.emit('page', page);
    return page;
  }

  private contextId(): Promise<string> {
    return (this._idPromise ??= this.cdp.send('Target.createBrowserContext').then((r: { browserContextId: string }) => {
      this._id = r.browserContextId;
      return r.browserContextId;
    }));
  }

  /** A target for `page`: a booted-ahead one for a covered first `goto`, else a new one. */
  async _attachPage(page: Page, gotoUrl?: string): Promise<Attachment> {
    const o = this._options;
    const viewport = page.viewportSize();
    const scripts = [...this._initScripts, ...page._pendingScripts];
    // A booted page brings its own browser context: only the context's first page can take one.
    const plain = !this._idPromise && !this._cookies.length && !o.extraHTTPHeaders && !o.userAgent && !media(o);
    const boot = plain && gotoUrl ? await this._browser._bootFor(gotoUrl, o.baseURL) : undefined;
    if (boot && !this._idPromise) {
      let acquired: { sessionId: string; targetId: string; browserContextId: string; ready: boolean } | undefined;
      const taking = (async () => {
        const profileId = await this._browser._profile(boot.url, scripts, viewport, boot.spec.pool ?? 2);
        try {
          acquired = await this.cdp.send('Pyro.acquirePreparedPage', { profileId, attach: true });
          return acquired!.browserContextId;
        } catch (e) {
          if (!(e instanceof CdpError)) throw e;
          return null;
        }
      })();
      this._idPromise = taking.then((id) => id ?? this.cdp.send('Target.createBrowserContext').then((r: any) => r.browserContextId));
      this._id = await this._idPromise;
      if (acquired) {
        this._browser._stats.pooled++;
        const { frameTree } = await this.cdp.send('Page.getFrameTree', {}, acquired.sessionId);
        page._bind(acquired.sessionId, acquired.targetId, frameTree.frame.id, frameTree.frame.url);
        page._seedFrames(frameTree);
        await this.setUp(page, false);
        return { pooled: true, ready: acquired.ready, url: boot.url };
      }
    }
    const browserContextId = await this.contextId();
    const { targetId } = await this.cdp.send('Target.createTarget', { url: 'about:blank', browserContextId });
    const { sessionId } = await this.cdp.send('Target.attachToTarget', { targetId, flatten: true });
    page._bind(sessionId, targetId);
    this._browser._stats.fresh++;
    await this.setUp(page, true, scripts);
    return { pooled: false, ready: true };
  }

  /** Domains and emulation for a page's new session; pipelined, the server runs them in order. */
  private async setUp(page: Page, fresh: boolean, scripts: string[] = []): Promise<void> {
    const o = this._options;
    const send = (method: string, params: object = {}) => page._sendNow(method, params);
    const setup: Promise<unknown>[] = [send('Page.enable')];
    const viewport = page.viewportSize();
    if (fresh && viewport) setup.push(send('Emulation.setDeviceMetricsOverride', { ...viewport, deviceScaleFactor: 1, mobile: false }));
    if (o.userAgent) setup.push(send('Emulation.setUserAgentOverride', { userAgent: o.userAgent }));
    if (this._har.some((r) => r.covers(page)) || this._wants.network) page._wants.network = true;
    if (this._wants.runtime) page._wants.runtime = true;
    if (page._wants.network || o.extraHTTPHeaders) setup.push((page._network = send('Network.enable').then(() => undefined)));
    if (o.extraHTTPHeaders) setup.push(send('Network.setExtraHTTPHeaders', { headers: o.extraHTTPHeaders }));
    if (page._wants.runtime) setup.push((page._runtime = send('Runtime.enable').then(() => undefined)));
    if (page._wants.fetch || page._routes.length || this._routes.length) {
      setup.push((page._fetch = send('Fetch.enable', { patterns: [{ urlPattern: '*' }] }).then(() => undefined)));
    }
    if (fresh && this._cookies.length) setup.push(send('Network.setCookies', { cookies: this._cookies.map(cdpCookie) }));
    for (const source of scripts) setup.push(send('Page.addScriptToEvaluateOnNewDocument', { source }));
    // The clock's init scripts only reach the next document: catch up the current one.
    const clock = this.clock._bundle();
    if (clock) setup.push(send('Runtime.evaluate', { expression: clock }).catch(() => {}));
    const m = media(o);
    if (fresh && m) setup.push(send('Pyro.run', { steps: [{ media: m }] }));
    if (page._wantsDialogs) setup.push(send('Pyro.dialogs', { block: true }));
    if (page._wants.fileChooser) setup.push(send('Page.setInterceptFileChooserDialog', { enabled: true }));
    if (page._wants.workers) setup.push(send('Target.setAutoAttach', WORKER_AUTO_ATTACH));
    for (const recorder of this._har) if (recorder.covers(page)) recorder.attach(page);
    for (const b of [...this._bindings.values(), ...page._bindings.values()]) setup.push(page._installBinding(b));
    await Promise.all(setup);
    if (fresh) this._cookies = []; // in the context's jar now
  }

  /** A popup `opener` opened: a page of this context on the existing target. */
  async _adoptPopup(targetId: string, opener: Page): Promise<void> {
    const o = this._options;
    const page = new Page(this, this.cdp, o.viewport === null ? null : (o.viewport ?? DEFAULT_VIEWPORT));
    this._pages.add(page);
    const attaching = (async (): Promise<Attachment> => {
      const { sessionId } = await this.cdp.send('Target.attachToTarget', { targetId, flatten: true });
      page._bind(sessionId, targetId);
      await this.setUp(page, true, this._initScripts);
      const { frameTree } = await page._sendNow('Page.getFrameTree');
      page._bind(sessionId, targetId, frameTree.frame.id, frameTree.frame.url);
      page._seedFrames(frameTree);
      return { pooled: false, ready: true };
    })();
    page._adopt(attaching);
    try {
      await attaching;
    } catch {
      this._pages.delete(page);
      return;
    }
    this._events.emit('page', page);
    opener._emit('popup', page);
  }

  _pageClosed(page: Page) {
    this._pages.delete(page);
    if (this._ownedBy === page) void this.close();
  }
  pages(): Page[] {
    return [...this._pages];
  }
  browser(): Browser {
    return this._browser;
  }

  async addInitScript<Arg>(script: PageFunction<Arg, any> | { path?: string; content?: string }, arg?: Arg): Promise<void> {
    const source = initScriptSource(script as string | Function | { path?: string; content?: string }, arg);
    this._initScripts.push(source);
    await Promise.all(this.attached().map((p) => p._send('Page.addScriptToEvaluateOnNewDocument', { source })));
  }
  /** Pages that have a target (the others pick up context state when they get one). */
  private attached(): Page[] {
    return this.pages().filter((p) => p._isAttached);
  }
  async addCookies(cookies: Cookie[]): Promise<void> {
    const page = this.attached()[0];
    if (page) await page._send('Network.setCookies', { cookies: cookies.map(cdpCookie) });
    else this._cookies.push(...cookies);
  }
  async cookies(urls?: string | string[]): Promise<Cookie[]> {
    const page = this.attached()[0];
    if (!page) return [...this._cookies];
    const list = urls === undefined ? undefined : Array.isArray(urls) ? urls : [urls];
    const { cookies } = await page._send('Network.getCookies', list ? { urls: list } : {});
    return cookies.map((c: any) => ({ name: c.name, value: c.value, domain: c.domain, path: c.path, expires: c.expires ?? -1, httpOnly: !!c.httpOnly, secure: !!c.secure, sameSite: c.sameSite ?? 'Lax' }));
  }
  async clearCookies(): Promise<void> {
    this._cookies = [];
    const page = this.attached()[0];
    if (page) await page._send('Network.clearBrowserCookies');
  }
  async storageState(o: { path?: string; indexedDB?: boolean } = {}): Promise<SavedState> {
    const cookies = await this.cookies();
    const origins: NonNullable<StorageState['origins']> = [];
    for (const page of this.pages()) {
      if (!/^https?:/.test(page.url())) continue;
      const origin = new URL(page.url()).origin;
      if (origins.some((x) => x.origin === origin)) continue;
      const localStorage = await page.evaluate<{ name: string; value: string }[]>(
        `Object.keys(localStorage).filter((k) => k !== '__pyroStorageState').map((name) => ({ name, value: localStorage.getItem(name) ?? '' }))`,
      );
      origins.push({ origin, localStorage });
    }
    const state = { cookies, origins };
    if (o.path) writeFileSync(o.path, JSON.stringify(state, null, 2));
    return state;
  }
  async setExtraHTTPHeaders(headers: Record<string, string>): Promise<void> {
    this._options.extraHTTPHeaders = headers;
    await Promise.all(this.attached().map(async (p) => {
      await p._enableNetwork();
      await p._send('Network.setExtraHTTPHeaders', { headers });
    }));
  }
  setDefaultTimeout(timeout: number): void {
    this._defaultTimeout = timeout;
  }
  setDefaultNavigationTimeout(timeout: number): void {
    this._defaultNavigationTimeout = timeout;
  }
  async grantPermissions(): Promise<void> {}
  async clearPermissions(): Promise<void> {}
  async setGeolocation(): Promise<void> {}
  async setOffline(): Promise<void> {}
  async route(url: UrlMatch, handler: RouteHandler, options: { times?: number } = {}): Promise<void> {
    this._routes.unshift({ url, handler, times: options.times });
    await Promise.all(this.pages().map((p) => p._enableFetch()));
  }
  async unroute(url: UrlMatch, handler?: RouteHandler): Promise<void> {
    this._routes = this._routes.filter((r) => !sameRoute(r, url, handler));
  }
  async unrouteAll(_options: { behavior?: 'wait' | 'ignoreErrors' | 'default' } = {}): Promise<void> {
    this._routes = [];
  }
  async routeFromHAR(har: string, options: HarOptions = {}): Promise<void> {
    if (options.update) return this._recordHar(har, options);
    await this.route(options.url ?? '**/*', harHandler(har, options));
  }
  /** Records the requests of `only` (else every page of the context) into `har`, written on close. */
  async _recordHar(har: string, options: HarOptions, only?: Page): Promise<void> {
    const recorder = new HarRecorder(har, options, this._options.baseURL, only);
    this._har.push(recorder);
    for (const page of this.pages()) if (recorder.covers(page)) recorder.attach(page);
  }
  async exposeFunction(name: string, fn: (...args: any[]) => unknown): Promise<void> {
    await this.exposeBinding(name, (_source: unknown, ...args: unknown[]) => fn(...args));
  }
  async exposeBinding(name: string, callback: (source: any, ...args: any[]) => unknown): Promise<void> {
    if (this._bindings.has(bindingName(name))) throw new Error(`browserContext.exposeBinding: Function "${name}" has been already registered`);
    const entry = { name, callback, script: bindingScript(name) };
    this._bindings.set(bindingName(name), entry);
    await Promise.all(this.attached().map((p) => p._attach().then(() => p._installBinding(entry))));
  }
  get tracing() {
    const noop = async () => {};
    return { start: noop, stop: noop, startChunk: noop, stopChunk: noop, group: noop, groupEnd: noop };
  }
  async waitForEvent<K extends keyof BrowserContextEvents>(
    event: K,
    optionsOrPredicate: ((arg: BrowserContextEvents[K]) => boolean) | { timeout?: number; predicate?: (arg: BrowserContextEvents[K]) => boolean } = {},
  ): Promise<BrowserContextEvents[K]> {
    const o = typeof optionsOrPredicate === 'function' ? { predicate: optionsOrPredicate } : optionsOrPredicate;
    this.enableFor(event);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`browserContext.waitForEvent: Timeout ${o.timeout ?? 30_000}ms exceeded`)), o.timeout ?? 30_000);
      const listener = (arg: any) => {
        if (o.predicate && !o.predicate(arg)) return;
        clearTimeout(timer);
        this._events.off(event, listener as (...args: any[]) => void);
        resolve(arg);
      };
      this._events.on(event, listener as (...args: any[]) => void);
    });
  }
  on<K extends keyof BrowserContextEvents>(event: K, listener: (arg: BrowserContextEvents[K]) => unknown): this {
    this.enableFor(event);
    this._events.on(event, listener as (...args: any[]) => void);
    return this;
  }
  /** Request and console events of every page, now and later. */
  private enableFor(event: string): void {
    if (['request', 'response', 'requestfinished', 'requestfailed'].includes(event)) {
      this._wants.network = true;
      for (const p of this.pages()) void p._enableNetwork();
    }
    if (event === 'console') {
      this._wants.runtime = true;
      for (const p of this.pages()) p.on('console', () => {});
    }
  }
  _wants = { network: false, runtime: false };
  /** A page's event, re-emitted by its context. */
  _emit(event: string, arg: unknown): void {
    this._events.emit(event, arg);
  }
  once<K extends keyof BrowserContextEvents>(event: K, listener: (arg: BrowserContextEvents[K]) => unknown): this {
    this.enableFor(event);
    this._events.once(event, listener as (...args: any[]) => void);
    return this;
  }
  off<K extends keyof BrowserContextEvents>(event: K, listener: (arg: BrowserContextEvents[K]) => unknown): this {
    this._events.off(event, listener as (...args: any[]) => void);
    return this;
  }
  private _request?: APIRequestContext;
  /** API requests with this context's base URL and extra headers (`context.request`, `page.request`). */
  get request(): APIRequestContext {
    const o = this._options;
    return (this._request ??= new APIRequestContext({ baseURL: o.baseURL, extraHTTPHeaders: o.extraHTTPHeaders, httpCredentials: o.httpCredentials, ignoreHTTPSErrors: o.ignoreHTTPSErrors, userAgent: o.userAgent, storageState: o.storageState }));
  }
  /** A raw CDP session on `page` (pyro's CDP: its commands, not all of Chrome's). */
  async newCDPSession(page: Page | Frame): Promise<CDPSession> {
    const p = page instanceof Page ? page : page.page();
    await p._attach();
    return new CDPSession(this.cdp, p._sessionId);
  }
  async close(_options: { reason?: string } = {}): Promise<void> {
    if (this._closed) return;
    this._closed = true;
    for (const hook of [...closingHooks]) await hook(this).catch(() => {});
    openContextSet.delete(this);
    for (const recorder of this._har) await recorder.save();
    for (const p of [...this._pages]) await p.close();
    const id = this._id ?? (this._idPromise ? await this._idPromise.catch(() => undefined) : undefined);
    if (id) this.cdp.post('Target.disposeBrowserContext', { browserContextId: id });
    this._browser._contextClosed(this);
    this._events.emit('close', this);
  }
}

/** Media emulation the options ask for beyond the defaults (Playwright's `colorScheme: 'light'` is one). */
function media(o: ContextOptions): Record<string, string> | undefined {
  const m: Record<string, string> = {};
  if (o.colorScheme && o.colorScheme !== 'light') m.colorScheme = o.colorScheme;
  if (o.reducedMotion && o.reducedMotion !== 'no-preference') m.reducedMotion = o.reducedMotion;
  return Object.keys(m).length ? m : undefined;
}

function cdpCookie(c: Cookie): Record<string, unknown> {
  const out: Record<string, unknown> = { name: c.name, value: c.value };
  for (const k of ['url', 'domain', 'path', 'httpOnly', 'secure', 'sameSite'] as const) if (c[k] !== undefined) out[k] = c[k];
  if (c.expires !== undefined && c.expires >= 0) out.expires = c.expires;
  if (!out.url && !out.domain) throw new Error(`cookie ${c.name}: give url or domain`);
  return out;
}

/** `chromium` for scripts that launch a browser themselves (`chromium.launch()`). */
export const chromium = {
  name: () => 'chromium',
  async launch(o: ConnectOptions & { bin?: string; args?: string[]; boot?: BootSpec[] } = {}): Promise<Browser> {
    return new Browser(await Pyro.launch(o), true, o.boot);
  },
  /** A running pyro's CDP endpoint (`ws://…` or `http://host:port`). */
  async connectOverCDP(endpointURL: string, _options: { timeout?: number; headers?: Record<string, string> } = {}): Promise<Browser> {
    return new Browser(await Pyro.connect(await wsUrl(endpointURL)));
  },
  async connect(wsEndpoint: string, _options: { timeout?: number; headers?: Record<string, string> } = {}): Promise<Browser> {
    return this.connectOverCDP(wsEndpoint);
  },
};

/** An `http://host:port` endpoint as its WebSocket URL. */
async function wsUrl(url: string): Promise<string> {
  if (!/^https?:/.test(url)) return url;
  const r = await fetch(new URL('/json/version', url));
  const v = (await r.json()) as { webSocketDebuggerUrl?: string };
  return v.webSocketDebuggerUrl ?? url.replace(/^http/, 'ws');
}
