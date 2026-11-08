// Playwright's APIRequestContext and APIResponse on `fetch`: the `request`
// fixture, `request.newContext()`, `context.request` and `page.request`.
import { readFileSync, writeFileSync } from 'node:fs';
import { basename } from 'node:path';
import { resolveUrl } from './match.js';

type Cookie = { name: string; value: string; domain: string; path: string; expires: number; httpOnly: boolean; secure: boolean; sameSite: 'Strict' | 'Lax' | 'None' };
type FilePayload = { name: string; mimeType: string; buffer: Buffer };
export type RequestOptions = {
  method?: string;
  headers?: Record<string, string>;
  params?: Record<string, string | number | boolean> | URLSearchParams | string;
  data?: unknown;
  form?: Record<string, string | number | boolean> | FormData;
  multipart?: Record<string, string | number | boolean | FilePayload | Blob> | FormData;
  timeout?: number;
  failOnStatusCode?: boolean;
  ignoreHTTPSErrors?: boolean;
  maxRedirects?: number;
  maxRetries?: number;
};
export type NewContextOptions = {
  baseURL?: string;
  extraHTTPHeaders?: Record<string, string>;
  storageState?: string | { cookies?: Partial<Cookie>[] };
  timeout?: number;
  failOnStatusCode?: boolean;
  ignoreHTTPSErrors?: boolean;
  userAgent?: string;
  httpCredentials?: { username: string; password: string };
};

export class APIResponse {
  private _disposed = false;
  constructor(
    private _url: string,
    private _status: number,
    private _statusText: string,
    private _headers: [string, string][],
    private _body: Buffer,
  ) {}
  url(): string {
    return this._url;
  }
  status(): number {
    return this._status;
  }
  statusText(): string {
    return this._statusText;
  }
  ok(): boolean {
    return this._status >= 200 && this._status <= 299;
  }
  headers(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const [k, v] of this._headers) out[k.toLowerCase()] = out[k.toLowerCase()] === undefined ? v : `${out[k.toLowerCase()]}, ${v}`;
    return out;
  }
  headersArray(): { name: string; value: string }[] {
    return this._headers.map(([name, value]) => ({ name, value }));
  }
  async body(): Promise<Buffer> {
    if (this._disposed) throw new Error('apiResponse.body: Response has been disposed');
    return this._body;
  }
  async text(): Promise<string> {
    return (await this.body()).toString('utf8');
  }
  async json(): Promise<any> {
    return JSON.parse(await this.text());
  }
  async dispose(): Promise<void> {
    this._disposed = true;
  }
  async [Symbol.asyncDispose](): Promise<void> {
    await this.dispose();
  }
}

export class APIRequestContext {
  private cookies: Cookie[] = [];
  private disposed = false;
  constructor(private o: NewContextOptions = {}) {
    const state = o.storageState;
    if (state) {
      const s = typeof state === 'string' ? JSON.parse(readFileSync(state, 'utf8')) : state;
      for (const c of s.cookies ?? []) this.cookies.push({ path: '/', expires: -1, httpOnly: false, secure: false, sameSite: 'Lax', domain: '', ...c } as Cookie);
    }
  }

  get(url: string, o: RequestOptions = {}) {
    return this.fetch(url, { ...o, method: 'GET' });
  }
  post(url: string, o: RequestOptions = {}) {
    return this.fetch(url, { ...o, method: 'POST' });
  }
  put(url: string, o: RequestOptions = {}) {
    return this.fetch(url, { ...o, method: 'PUT' });
  }
  patch(url: string, o: RequestOptions = {}) {
    return this.fetch(url, { ...o, method: 'PATCH' });
  }
  delete(url: string, o: RequestOptions = {}) {
    return this.fetch(url, { ...o, method: 'DELETE' });
  }
  head(url: string, o: RequestOptions = {}) {
    return this.fetch(url, { ...o, method: 'HEAD' });
  }

  async fetch(urlOrRequest: string | { url(): string; method(): string; headers(): Record<string, string>; postDataBuffer(): Buffer | null }, o: RequestOptions = {}): Promise<APIResponse> {
    if (this.disposed) throw new Error('apiRequestContext.fetch: Request context disposed');
    let url: string;
    let method = o.method;
    let headers: Record<string, string> = {};
    let body: BodyInit | undefined;
    if (typeof urlOrRequest !== 'string') {
      url = urlOrRequest.url();
      method ??= urlOrRequest.method();
      headers = { ...urlOrRequest.headers() };
      const b = urlOrRequest.postDataBuffer();
      if (b) body = b as unknown as BodyInit;
    } else {
      url = resolveUrl(this.o.baseURL, urlOrRequest);
    }
    method = (method ?? 'GET').toUpperCase();
    const u = new URL(url);
    if (o.params) {
      const p = typeof o.params === 'string' ? new URLSearchParams(o.params) : o.params instanceof URLSearchParams ? o.params : new URLSearchParams(Object.entries(o.params).map(([k, v]) => [k, String(v)]));
      for (const [k, v] of p) u.searchParams.append(k, v);
    }
    for (const [k, v] of Object.entries({ ...this.o.extraHTTPHeaders, ...o.headers })) headers[k.toLowerCase()] = v;
    if (this.o.userAgent && !headers['user-agent']) headers['user-agent'] = this.o.userAgent;
    if (this.o.httpCredentials && !headers['authorization']) {
      headers['authorization'] = `Basic ${Buffer.from(`${this.o.httpCredentials.username}:${this.o.httpCredentials.password}`).toString('base64')}`;
    }
    if (o.data !== undefined) {
      if (typeof o.data === 'string' || Buffer.isBuffer(o.data) || o.data instanceof Uint8Array) {
        body = o.data as BodyInit;
        if (typeof o.data === 'string' && !headers['content-type']) headers['content-type'] = 'text/plain';
      } else {
        body = JSON.stringify(o.data);
        headers['content-type'] ??= 'application/json';
      }
    } else if (o.form) {
      body = o.form instanceof FormData ? new URLSearchParams(o.form as any) : new URLSearchParams(Object.entries(o.form).map(([k, v]) => [k, String(v)]));
      headers['content-type'] ??= 'application/x-www-form-urlencoded';
    } else if (o.multipart) {
      const fd = o.multipart instanceof FormData ? o.multipart : new FormData();
      if (!(o.multipart instanceof FormData)) {
        for (const [k, v] of Object.entries(o.multipart)) {
          if (v && typeof v === 'object' && 'buffer' in (v as FilePayload)) {
            const f = v as FilePayload;
            fd.append(k, new Blob([new Uint8Array(f.buffer)], { type: f.mimeType }), f.name ?? basename(k));
          } else fd.append(k, v instanceof Blob ? v : String(v));
        }
      }
      body = fd;
      delete headers['content-type'];
    }
    const cookie = this.cookieHeader(u);
    if (cookie && !headers['cookie']) headers['cookie'] = cookie;
    const timeout = o.timeout ?? this.o.timeout ?? 30_000;
    const signal = timeout > 0 ? AbortSignal.timeout(timeout) : undefined;
    const r = await fetch(u, {
      method,
      headers,
      body: method === 'GET' || method === 'HEAD' ? undefined : body,
      redirect: o.maxRedirects === 0 ? 'manual' : 'follow',
      signal,
      ...(o.ignoreHTTPSErrors || this.o.ignoreHTTPSErrors ? ({ tls: { rejectUnauthorized: false } } as object) : {}),
    });
    const pairs: [string, string][] = [];
    r.headers.forEach((v, k) => pairs.push([k, v]));
    for (const c of (r.headers as any).getSetCookie?.() ?? []) this.storeCookie(new URL(r.url || u), c);
    const response = new APIResponse(r.url || u.toString(), r.status, r.statusText, pairs, Buffer.from(await r.arrayBuffer()));
    if ((o.failOnStatusCode ?? this.o.failOnStatusCode) && !response.ok()) {
      throw new Error(`apiRequestContext.${method.toLowerCase()}: ${response.status()} ${response.statusText()}\nCall log:\n  - → ${method} ${u}`);
    }
    return response;
  }

  private cookieHeader(u: URL): string {
    const now = Date.now() / 1000;
    return this.cookies
      .filter((c) => (c.expires < 0 || c.expires > now) && (!c.domain || u.hostname === c.domain.replace(/^\./, '') || u.hostname.endsWith(c.domain.startsWith('.') ? c.domain : `.${c.domain}`)) && u.pathname.startsWith(c.path || '/') && (!c.secure || u.protocol === 'https:'))
      .map((c) => `${c.name}=${c.value}`)
      .join('; ');
  }
  private storeCookie(u: URL, header: string): void {
    const [pair, ...attrs] = header.split(';').map((s) => s.trim());
    const eq = pair.indexOf('=');
    if (eq < 0) return;
    const c: Cookie = { name: pair.slice(0, eq), value: pair.slice(eq + 1), domain: u.hostname, path: '/', expires: -1, httpOnly: false, secure: false, sameSite: 'Lax' };
    for (const a of attrs) {
      const [k, v = ''] = a.split('=');
      const key = k.toLowerCase();
      if (key === 'domain') c.domain = v;
      else if (key === 'path') c.path = v || '/';
      else if (key === 'expires') c.expires = Date.parse(v) / 1000;
      else if (key === 'max-age') c.expires = Date.now() / 1000 + Number(v);
      else if (key === 'httponly') c.httpOnly = true;
      else if (key === 'secure') c.secure = true;
      else if (key === 'samesite') c.sameSite = (v[0].toUpperCase() + v.slice(1).toLowerCase()) as Cookie['sameSite'];
    }
    this.cookies = this.cookies.filter((x) => !(x.name === c.name && x.domain === c.domain && x.path === c.path));
    if (c.expires < 0 || c.expires > Date.now() / 1000) this.cookies.push(c);
  }

  async storageState(o: { path?: string } = {}): Promise<{ cookies: Cookie[]; origins: never[] }> {
    const state = { cookies: [...this.cookies], origins: [] as never[] };
    if (o.path) writeFileSync(o.path, JSON.stringify(state, null, 2));
    return state;
  }
  async dispose(): Promise<void> {
    this.disposed = true;
  }
  async [Symbol.asyncDispose](): Promise<void> {
    await this.dispose();
  }
}

/** Playwright's `request` (`playwright.request`). */
export const request = {
  async newContext(o: NewContextOptions = {}): Promise<APIRequestContext> {
    return new APIRequestContext(o);
  },
};
