// Playwright's Browser and BrowserContext on one lightningpanda connection.
import { EventEmitter } from 'node:events';
import { readFileSync, writeFileSync } from 'node:fs';
import { Lightningpanda, type ConnectOptions } from '../index.js';
import { initScriptSource, Page } from './page.js';

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

/** The `browser.newContext()` options this client acts on; the rest are ignored. */
export type ContextOptions = {
  baseURL?: string;
  viewport?: { width: number; height: number } | null;
  storageState?: string | StorageState;
  extraHTTPHeaders?: Record<string, string>;
  userAgent?: string;
  colorScheme?: 'light' | 'dark' | 'no-preference' | null;
  reducedMotion?: 'reduce' | 'no-preference' | null;
  [option: string]: unknown;
};

const DEFAULT_VIEWPORT = { width: 1280, height: 720 };

type TestDefaults = { options: ContextOptions; timeout?: number; navigationTimeout?: number; testIdAttribute: string };
let testDefaults: TestDefaults | undefined;
/** The running test's `use` options (`@povio/lightning-panda/test`): `browser.newContext()` starts from them. */
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

  constructor(
    readonly _lp: Lightningpanda,
    private _owned = true,
  ) {
    _lp.cdp.onClose(() => this._events.emit('disconnected', this));
  }

  async newContext(options: ContextOptions = {}): Promise<BrowserContext> {
    const merged: ContextOptions = { ...this._defaults, ...options };
    const { browserContextId } = await this._lp.cdp.send('Target.createBrowserContext');
    const context = new BrowserContext(this, browserContextId, merged);
    await context._init();
    this._contexts.add(context);
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
    return 'lightningpanda';
  }
  browserType() {
    return { name: () => 'chromium' };
  }
  on(event: string, listener: (...args: any[]) => void): this {
    this._events.on(event, listener);
    return this;
  }
  once(event: string, listener: (...args: any[]) => void): this {
    this._events.once(event, listener);
    return this;
  }
  off(event: string, listener: (...args: any[]) => void): this {
    this._events.off(event, listener);
    return this;
  }
  async close(): Promise<void> {
    for (const c of [...this._contexts]) await c.close();
    if (this._owned) this._lp.close();
  }
}

export class BrowserContext {
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

  constructor(
    private _browser: Browser,
    readonly _id: string,
    /** Read by `expect(page).toHaveURL()` for `baseURL`. */
    readonly _options: ContextOptions,
  ) {
    this._defaultTimeout = _browser._defaultTimeout;
    this._defaultNavigationTimeout = _browser._defaultNavigationTimeout;
    this._testIdAttribute = _browser._testIdAttribute;
  }

  async _init(): Promise<void> {
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
        `(() => { try { const e = ${JSON.stringify(entries)}[location.origin]; if (!e || localStorage.getItem('__lpStorageState')) return; for (const [k, v] of e) localStorage.setItem(k, v); localStorage.setItem('__lpStorageState', '1'); } catch {} })();`,
      );
    }
  }

  private get cdp() {
    return this._browser._lp.cdp;
  }

  async newPage(): Promise<Page> {
    if (this._closed) throw new Error('browserContext.newPage: Target page, context or browser has been closed');
    const { targetId } = await this.cdp.send('Target.createTarget', { url: 'about:blank', browserContextId: this._id });
    const { sessionId } = await this.cdp.send('Target.attachToTarget', { targetId, flatten: true });
    const o = this._options;
    const viewport = o.viewport === null ? null : (o.viewport ?? DEFAULT_VIEWPORT);
    const page = new Page(this, this.cdp, sessionId, targetId, viewport);
    const send = (method: string, params: object = {}) => this.cdp.send(method, params, sessionId);
    // Pipelined: the server runs them in order, before any later command.
    const setup: Promise<unknown>[] = [send('Page.enable')];
    if (viewport) setup.push(send('Emulation.setDeviceMetricsOverride', { ...viewport, deviceScaleFactor: 1, mobile: false }));
    if (o.userAgent) setup.push(send('Emulation.setUserAgentOverride', { userAgent: o.userAgent }));
    if (o.extraHTTPHeaders) setup.push(page._enableNetwork(), send('Network.setExtraHTTPHeaders', { headers: o.extraHTTPHeaders }));
    if (this._cookies.length) setup.push(send('Network.setCookies', { cookies: this._cookies.map(cdpCookie) }));
    for (const source of this._initScripts) setup.push(send('Page.addScriptToEvaluateOnNewDocument', { source }));
    if (o.colorScheme || o.reducedMotion) {
      setup.push(send('LP.run', { steps: [{ media: { ...(o.colorScheme ? { colorScheme: o.colorScheme } : {}), ...(o.reducedMotion ? { reducedMotion: o.reducedMotion } : {}) } }] }));
    }
    await Promise.all(setup);
    this._cookies = []; // in the context's jar now
    this._pages.add(page);
    this._events.emit('page', page);
    return page;
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

  async addInitScript(script: string | Function | { path?: string; content?: string }, arg?: unknown): Promise<void> {
    const source = initScriptSource(script, arg);
    this._initScripts.push(source);
    await Promise.all(this.pages().map((p) => p._send('Page.addScriptToEvaluateOnNewDocument', { source })));
  }
  async addCookies(cookies: Cookie[]): Promise<void> {
    const page = this.pages()[0];
    if (page) await page._send('Network.setCookies', { cookies: cookies.map(cdpCookie) });
    else this._cookies.push(...cookies);
  }
  async cookies(urls?: string | string[]): Promise<Cookie[]> {
    const page = this.pages()[0];
    if (!page) return [...this._cookies];
    const list = urls === undefined ? undefined : Array.isArray(urls) ? urls : [urls];
    const { cookies } = await page._send('Network.getCookies', list ? { urls: list } : {});
    return cookies.map((c: any) => ({ name: c.name, value: c.value, domain: c.domain, path: c.path, expires: c.expires ?? -1, httpOnly: !!c.httpOnly, secure: !!c.secure, sameSite: c.sameSite ?? 'Lax' }));
  }
  async clearCookies(): Promise<void> {
    this._cookies = [];
    const page = this.pages()[0];
    if (page) await page._send('Network.clearBrowserCookies');
  }
  async storageState(o: { path?: string } = {}): Promise<StorageState> {
    const cookies = await this.cookies();
    const origins: NonNullable<StorageState['origins']> = [];
    for (const page of this.pages()) {
      if (!/^https?:/.test(page.url())) continue;
      const origin = new URL(page.url()).origin;
      if (origins.some((x) => x.origin === origin)) continue;
      const localStorage = await page.evaluate<{ name: string; value: string }[]>(
        `Object.keys(localStorage).filter((k) => k !== '__lpStorageState').map((name) => ({ name, value: localStorage.getItem(name) ?? '' }))`,
      );
      origins.push({ origin, localStorage });
    }
    const state = { cookies, origins };
    if (o.path) writeFileSync(o.path, JSON.stringify(state, null, 2));
    return state;
  }
  async setExtraHTTPHeaders(headers: Record<string, string>): Promise<void> {
    this._options.extraHTTPHeaders = headers;
    await Promise.all(this.pages().map(async (p) => {
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
  async route(): Promise<never> {
    throw new Error('lightning-panda: context.route is not supported yet');
  }
  async unroute(): Promise<void> {}
  async exposeFunction(): Promise<never> {
    throw new Error('lightning-panda: context.exposeFunction is not supported yet');
  }
  get tracing() {
    const noop = async () => {};
    return { start: noop, stop: noop, startChunk: noop, stopChunk: noop, group: noop, groupEnd: noop };
  }
  async waitForEvent(event: string, o: { timeout?: number; predicate?: (arg: any) => boolean } = {}): Promise<any> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`browserContext.waitForEvent: Timeout ${o.timeout ?? 30_000}ms exceeded`)), o.timeout ?? 30_000);
      const listener = (arg: any) => {
        if (o.predicate && !o.predicate(arg)) return;
        clearTimeout(timer);
        this._events.off(event, listener);
        resolve(arg);
      };
      this._events.on(event, listener);
    });
  }
  on(event: string, listener: (...args: any[]) => void): this {
    this._events.on(event, listener);
    return this;
  }
  once(event: string, listener: (...args: any[]) => void): this {
    this._events.once(event, listener);
    return this;
  }
  off(event: string, listener: (...args: any[]) => void): this {
    this._events.off(event, listener);
    return this;
  }
  async close(): Promise<void> {
    if (this._closed) return;
    this._closed = true;
    for (const p of [...this._pages]) await p.close();
    this.cdp.post('Target.disposeBrowserContext', { browserContextId: this._id });
    this._browser._contextClosed(this);
    this._events.emit('close', this);
  }
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
  async launch(o: ConnectOptions & { bin?: string; args?: string[] } = {}): Promise<Browser> {
    return new Browser(await Lightningpanda.launch(o));
  },
  async connectOverCDP(wsEndpoint: string | { wsEndpoint: string }): Promise<Browser> {
    const url = typeof wsEndpoint === 'string' ? wsEndpoint : wsEndpoint.wsEndpoint;
    return new Browser(await Lightningpanda.connect(await wsUrl(url)));
  },
  async connect(wsEndpoint: string | { wsEndpoint: string }): Promise<Browser> {
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
