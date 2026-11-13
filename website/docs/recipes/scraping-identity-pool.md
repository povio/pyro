# A pool of signed-in identities

You have several accounts. Each one should keep its session on disk, go out through its own proxy, send its own user agent, and never run more than a set number of jobs at once. When a session expires, the account should sign in again without you stepping in. This recipe builds that as one module, `identity-pool.ts`, on the Playwright library (`@povio/pyro/wright`).

What the pool does:

- **One identity per file pair.** `profiles/<name>.json` holds the session (Playwright's `storageState` format). `profiles/<name>.identity.json` says how the identity looks: user agent, headers, viewport, proxy.
- **Assignment.** A job gets the identity that has been idle longest (least recently used), or the next one in order (round robin).
- **A limit per identity.** At most `perIdentity` jobs run as one account at the same time. Other jobs wait for a free identity.
- **Re-sign-in on expiry.** A job throws `SignedOut` when the page shows the session is gone. The pool signs that identity in once (other jobs of the same identity wait for that sign-in), then retries the job once.
- **Atomic saves.** State files are written to a temporary file and renamed, mode `600`. A crash or a parallel save never leaves a half-written file.
- **One browser server per proxy.** The Playwright library ignores the `proxy` context option, so the pool starts one `pyro serve --proxy ...` per distinct proxy and opens each identity's contexts there.

## Files

```
pool/
  site.ts               test site: sign-in, session cookie + localStorage token, expiry
  proxy.ts              test HTTP proxy that tags requests with its name
  identity-pool.ts      the pool
  scrape.ts             a job list that uses it
  profiles/             mode 700
    alice.identity.json
    bob.identity.json
    carol.identity.json
    alice.json          written by the pool, mode 600
    ...
```

Set it up:

```sh
mkdir pool && cd pool
bun init -y
bun add @povio/pyro
```

Until `@povio/pyro` is published under that name, install it as [Installation](/guide/installation#try-today) describes.

## The test site and proxies

The site signs a user in with an `httpOnly` session cookie and puts an API token in `localStorage`. Its `/account` page needs both: the cookie for the page, the token for the `fetch` that loads the orders after `load`. Sessions end after `SESSION_TTL_MS`, or at once through `/admin/expire?user=<name>`. `/whoami` echoes the user agent and the proxy a request came through.

::: details site.ts
```ts
// site.ts: a local test site with sign-in, a session that expires, and a page
// that needs both the session cookie and a localStorage token.
// bun site.ts 4601
const PORT = Number(process.argv[2] ?? 4601);
const TTL_MS = Number(process.env.SESSION_TTL_MS ?? 60_000);
const USERS: Record<string, string> = { alice: 'pw-alice', bob: 'pw-bob', carol: 'pw-carol' };
const sessions = new Map<string, { user: string; expires: number; token: string }>();

const html = (body: string, status = 200, headers: Record<string, string> = {}) =>
  new Response(`<!doctype html><meta charset="utf-8">${body}`, { status, headers: { 'content-type': 'text/html', ...headers } });
const session = (req: Request) => {
  const sid = /(?:^|; )sid=([^;]+)/.exec(req.headers.get('cookie') ?? '')?.[1];
  const s = sid ? sessions.get(sid) : undefined;
  return s && s.expires > Date.now() ? s : undefined;
};

Bun.serve({
  port: PORT,
  hostname: '127.0.0.1',
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === '/login' && req.method === 'GET') {
      return html(`<title>Sign in</title><form method="post">
        <label>User <input name="user"></label>
        <label>Password <input name="password" type="password"></label>
        <button>Sign in</button></form>`);
    }
    if (url.pathname === '/login' && req.method === 'POST') {
      const form = await req.formData();
      const user = String(form.get('user'));
      if (USERS[user] !== form.get('password')) return html('<p>Wrong password</p>', 401);
      const sid = crypto.randomUUID();
      const token = crypto.randomUUID();
      sessions.set(sid, { user, expires: Date.now() + TTL_MS, token });
      console.log(`sign-in ${user} from ${req.headers.get('user-agent')}`);
      // The app keeps an API token in localStorage, next to the httpOnly cookie.
      return html(`<script>localStorage.setItem('apiToken', ${JSON.stringify(token)}); location.href = '/account';</script>`, 200, {
        'set-cookie': `sid=${sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${Math.ceil(TTL_MS / 1000)}`,
      });
    }
    if (url.pathname === '/account') {
      const s = session(req);
      if (!s) return new Response(null, { status: 302, headers: { location: '/login' } });
      // Orders come from an API call that needs the localStorage token.
      return html(`<title>Account</title><h1 id="who">${s.user}</h1><ul id="orders"></ul>
        <script>
          fetch('/api/orders', { headers: { authorization: 'Bearer ' + localStorage.getItem('apiToken') } })
            .then((r) => r.json())
            .then((orders) => { for (const o of orders) document.querySelector('#orders').insertAdjacentHTML('beforeend', '<li class="order">' + o + '</li>'); });
        </script>`);
    }
    if (url.pathname === '/api/orders') {
      const s = session(req);
      if (!s || req.headers.get('authorization') !== `Bearer ${s.token}`) return Response.json({ error: 'unauthorized' }, { status: 401 });
      return Response.json([`${s.user}-1`, `${s.user}-2`]);
    }
    if (url.pathname === '/whoami') {
      return Response.json({ ua: req.headers.get('user-agent'), via: req.headers.get('x-test-proxy') });
    }
    if (url.pathname === '/admin/expire') {
      // Ends every session of one user, as a site does when it signs you out.
      for (const [sid, s] of sessions) if (s.user === url.searchParams.get('user')) sessions.delete(sid);
      return new Response('ok');
    }
    return new Response('not found', { status: 404 });
  },
});
console.log(`site on http://127.0.0.1:${PORT}`);
```
:::

`proxy.ts` is a minimal forward proxy for plain HTTP. It adds an `x-test-proxy` header with its name, so you can see which proxy a request used. Use your real proxies instead.

::: details proxy.ts
```ts
// proxy.ts: a test HTTP forward proxy that tags each request with its name.
// bun proxy.ts 4611 de
const [port, name] = [Number(process.argv[2]), process.argv[3]];
Bun.serve({
  port,
  hostname: '127.0.0.1',
  async fetch(req) {
    // A forward proxy gets plain-HTTP requests in absolute form: req.url is the target.
    const headers = new Headers(req.headers);
    headers.delete('proxy-authorization');
    headers.set('x-test-proxy', name);
    return fetch(req.url, { method: req.method, headers, body: req.body, redirect: 'manual' });
  },
});
console.log(`proxy ${name} on http://127.0.0.1:${port}`);
```
:::

Start the site and two proxies in separate terminals:

```sh
SESSION_TTL_MS=3600000 bun site.ts 4601
bun proxy.ts 4611 de
bun proxy.ts 4612 us
```

## Identity files

One file per identity. Passwords do not go here; the sign-in function reads them from the environment.

```sh
mkdir -p profiles && chmod 700 profiles
```

`profiles/alice.identity.json`:

```json
{
  "userAgent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36",
  "extraHTTPHeaders": { "accept-language": "de-DE,de;q=0.9" },
  "proxy": "http://127.0.0.1:4611",
  "proxyBypass": "<-loopback>"
}
```

`profiles/bob.identity.json`:

```json
{
  "userAgent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
  "proxy": "http://127.0.0.1:4612",
  "proxyBypass": "<-loopback>"
}
```

`profiles/carol.identity.json` (no proxy: she connects directly):

```json
{ "userAgent": "MyCrawler/1.0 (+https://example.com/bot)" }
```

`proxyBypass: "<-loopback>"` is only needed because the test site is on `127.0.0.1`, which skips the proxy by default. Leave it out for real sites. A proxy with credentials goes in the URL: `http://user:pass@proxy.example.net:3128`. Only `http://` proxies work; see [Proxies](/scraping/identity#proxies).

## The pool

```ts
// identity-pool.ts: N stored identities, each with its own proxy and user agent.
// Jobs borrow an identity, run in a fresh context signed in from its file,
// and sign in again (once per identity) when the session has expired.
import { chromium, type Browser, type BrowserContext, type Page } from '@povio/pyro/wright';
import { existsSync } from 'node:fs';
import { chmod, mkdir, readdir, readFile, rename, writeFile } from 'node:fs/promises';

/** Throw this from a job when the page shows that the session is gone. */
export class SignedOut extends Error {}

/** profiles/<name>.identity.json */
export type IdentityFile = {
  userAgent?: string;
  viewport?: { width: number; height: number };
  extraHTTPHeaders?: Record<string, string>;
  /** http://[user:pass@]host:port. One browser server per distinct proxy. */
  proxy?: string;
  proxyBypass?: string;
};

export type PoolOptions = {
  dir: string;
  /** Fill the site's sign-in form. Runs in a fresh context; the pool saves the result. */
  signIn: (page: Page, name: string) => Promise<void>;
  /** Jobs one identity may run at the same time (default 1). */
  perIdentity?: number;
  /** 'lru' (default): the identity idle the longest. 'round-robin': the next one in order. */
  pick?: 'lru' | 'round-robin';
};

type Slot = {
  name: string;
  file: IdentityFile;
  running: number;
  lastUsed: number;
  /** Bumped by every sign-in; a job that saw an older one does not sign in again. */
  generation: number;
  signingIn?: Promise<void>;
};

type State = Awaited<ReturnType<BrowserContext['storageState']>>;

export class IdentityPool {
  private slots: Slot[] = [];
  private browsers = new Map<string, Promise<Browser>>();
  private waiting: (() => void)[] = [];
  private turn = 0;

  private constructor(private opts: Required<PoolOptions>) {}

  /** Every profiles/<name>.identity.json in `dir` is one identity. */
  static async open(opts: PoolOptions): Promise<IdentityPool> {
    const pool = new IdentityPool({ perIdentity: 1, pick: 'lru', ...opts });
    await mkdir(opts.dir, { recursive: true, mode: 0o700 });
    for (const f of (await readdir(opts.dir)).sort()) {
      const m = /^([\w.-]+)\.identity\.json$/.exec(f);
      if (!m) continue;
      const file = JSON.parse(await readFile(`${opts.dir}/${f}`, 'utf8')) as IdentityFile;
      pool.slots.push({ name: m[1], file, running: 0, lastUsed: 0, generation: 0 });
    }
    if (!pool.slots.length) throw new Error(`no *.identity.json in ${opts.dir}`);
    return pool;
  }

  /** Runs `job` as some identity, with at most `perIdentity` jobs per identity at once. */
  async run<T>(job: (page: Page, identity: string) => Promise<T>): Promise<T> {
    const slot = await this.acquire();
    try {
      for (let attempt = 0; ; attempt++) {
        if (!existsSync(this.statePath(slot.name))) await this.signIn(slot, slot.generation);
        const generation = slot.generation;
        const context = await this.newContext(slot, await this.readState(slot.name));
        try {
          const page = await context.newPage();
          const result = await job(page, slot.name);
          await this.save(slot.name, context); // keeps sliding sessions and rotated cookies fresh
          return result;
        } catch (e) {
          if (!(e instanceof SignedOut) || attempt > 0) throw e;
          await this.signIn(slot, generation); // then retry the job once
        } finally {
          await context.close();
        }
      }
    } finally {
      slot.running--;
      this.waiting.shift()?.();
    }
  }

  async close() {
    for (const b of this.browsers.values()) await (await b).close();
  }

  private async acquire(): Promise<Slot> {
    for (;;) {
      const free = this.slots.filter((s) => s.running < this.opts.perIdentity);
      if (free.length) {
        let slot: Slot;
        if (this.opts.pick === 'round-robin') {
          const order = [...this.slots.slice(this.turn), ...this.slots.slice(0, this.turn)];
          slot = order.find((s) => free.includes(s))!;
          this.turn = (this.slots.indexOf(slot) + 1) % this.slots.length;
        } else {
          slot = free.reduce((a, b) => (b.lastUsed < a.lastUsed ? b : a));
        }
        slot.running++;
        slot.lastUsed = performance.now();
        return slot;
      }
      await new Promise<void>((resolve) => this.waiting.push(resolve)); // every identity is at its limit
    }
  }

  /** Signs `slot` in again, unless another job already did since `seen`. */
  private async signIn(slot: Slot, seen: number) {
    if (slot.generation !== seen) return;
    slot.signingIn ??= (async () => {
      const context = await this.newContext(slot);
      try {
        await this.opts.signIn(await context.newPage(), slot.name);
        await this.save(slot.name, context, { replace: true });
        slot.generation++;
        console.error(`[pool] ${slot.name}: signed in`);
      } finally {
        await context.close();
        slot.signingIn = undefined;
      }
    })();
    await slot.signingIn;
  }

  private async newContext(slot: Slot, storageState?: State): Promise<BrowserContext> {
    const { proxy, proxyBypass, ...options } = slot.file;
    return (await this.browserFor(proxy, proxyBypass)).newContext({ ...options, ...(storageState ? { storageState } : {}) });
  }

  /** The proxy is a server flag: one browser server per distinct proxy, started on first use. */
  private browserFor(proxy?: string, bypass?: string): Promise<Browser> {
    const key = `${proxy ?? ''}|${bypass ?? ''}`;
    let browser = this.browsers.get(key);
    if (!browser) {
      const args = proxy ? ['--proxy', proxy, ...(bypass ? ['--proxy-bypass', bypass] : [])] : [];
      browser = chromium.launch({ args });
      this.browsers.set(key, browser);
    }
    return browser;
  }

  private statePath(name: string) {
    return `${this.opts.dir}/${name}.json`;
  }

  private async readState(name: string): Promise<State> {
    return JSON.parse(await readFile(this.statePath(name), 'utf8'));
  }

  /**
   * Writes the context's state atomically (temp file, then rename), mode 600.
   * Cookies are the whole jar. localStorage is read from the open pages only,
   * so origins the job did not end on are kept from the old file.
   */
  private async save(name: string, context: BrowserContext, o: { replace?: boolean } = {}) {
    const state = await context.storageState();
    if (!o.replace && existsSync(this.statePath(name))) {
      const old = await this.readState(name);
      const seen = new Set(state.origins.map((x) => x.origin));
      state.origins.push(...old.origins.filter((x) => !seen.has(x.origin)));
    }
    const tmp = `${this.statePath(name)}.${process.pid}.${crypto.randomUUID()}.tmp`;
    await writeFile(tmp, JSON.stringify(state, null, 2), { mode: 0o600 });
    await chmod(tmp, 0o600); // in case the umask widened it
    await rename(tmp, this.statePath(name)); // readers see the old file or the new one, never half
  }
}
```

## A job list

`scrape.ts` runs 12 jobs, two at a time per identity. Each job opens `/account`, waits for the orders the app fetches after `load`, and asks `/whoami` what the site saw. The job decides when a session is gone: a redirect to `/login` means the cookie expired; a `401` from the API means the token was refused.

```ts
// scrape.ts: bun scrape.ts
import { IdentityPool, SignedOut } from './identity-pool';

const BASE = process.env.BASE ?? 'http://127.0.0.1:4601';

const pool = await IdentityPool.open({
  dir: 'profiles',
  perIdentity: 2,
  async signIn(page, name) {
    const password = process.env[`PASSWORD_${name.toUpperCase()}`];
    if (!password) throw new Error(`set PASSWORD_${name.toUpperCase()}`);
    await page.goto(`${BASE}/login`);
    await page.getByLabel('User').fill(name);
    await page.getByLabel('Password').fill(password);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await page.waitForURL(`${BASE}/account`);
    await page.locator('.order').first().waitFor(); // the token works: the app loaded its data
  },
});

const jobs = Array.from({ length: 12 }, (_, i) => i + 1);
const t0 = performance.now();
try {
  const results = await Promise.all(
    jobs.map((n) =>
      pool.run(async (page, identity) => {
        if (n === 7 && process.env.EXPIRE) await fetch(`${BASE}/admin/expire?user=${process.env.EXPIRE}`); // the site ends a session mid-run
        // goto returns at `load`; the app fetches its data after that. Wait for that response.
        const api = page.waitForResponse((r) => r.url().endsWith('/api/orders')).catch(() => null);
        await page.goto(`${BASE}/account`);
        if (page.url().startsWith(`${BASE}/login`)) throw new SignedOut(); // cookie gone: redirected
        if ((await api)?.status() === 401) throw new SignedOut(); // token refused
        await page.locator('.order').first().waitFor();
        const orders = await page.locator('.order').allTextContents();
        const seen = await page.evaluate(() => fetch('/whoami').then((r) => r.json()));
        return { job: n, identity, orders: orders.join(','), proxy: seen.via ?? '-', ua: seen.ua.slice(0, 24) };
      }),
    ),
  );
  console.table(results);
  console.log(`${jobs.length} jobs in ${Math.round(performance.now() - t0)} ms`);
} finally {
  await pool.close();
}
```

## Run it

First run, no state files yet. Each identity signs in once, and the files are written:

```
$ PASSWORD_ALICE=pw-alice PASSWORD_BOB=pw-bob PASSWORD_CAROL=pw-carol bun scrape.ts
pyro: CDP server listening on ws://127.0.0.1:53404/
pyro: CDP server listening on ws://127.0.0.1:53402/
pyro: CDP server listening on ws://127.0.0.1:53403/
[pool] bob: signed in
[pool] alice: signed in
[pool] carol: signed in
┌────┬─────┬──────────┬─────────────────┬───────┬──────────────────────────┐
│    │ job │ identity │ orders          │ proxy │ ua                       │
├────┼─────┼──────────┼─────────────────┼───────┼──────────────────────────┤
│  0 │ 1   │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│  1 │ 2   │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
│  2 │ 3   │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│  3 │ 4   │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│  4 │ 5   │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
│  5 │ 6   │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│  6 │ 7   │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│  7 │ 8   │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│  8 │ 9   │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
│  9 │ 10  │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
│ 10 │ 11  │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│ 11 │ 12  │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
└────┴─────┴──────────┴─────────────────┴───────┴──────────────────────────┘
12 jobs in 121 ms
```

Three `pyro:` lines: one server per distinct proxy (de, us, none). Each job ran through its identity's proxy, with its user agent.

Second run, without passwords. Every session comes from its file:

```
$ bun scrape.ts
pyro: CDP server listening on ws://127.0.0.1:53436/
pyro: CDP server listening on ws://127.0.0.1:53437/
pyro: CDP server listening on ws://127.0.0.1:53439/
┌────┬─────┬──────────┬─────────────────┬───────┬──────────────────────────┐
│    │ job │ identity │ orders          │ proxy │ ua                       │
├────┼─────┼──────────┼─────────────────┼───────┼──────────────────────────┤
│  0 │ 1   │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│  1 │ 2   │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
│  2 │ 3   │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│  3 │ 4   │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│  4 │ 5   │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
│  5 │ 6   │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│  6 │ 7   │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│  7 │ 8   │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│  8 │ 9   │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│  9 │ 10  │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
│ 10 │ 11  │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│ 11 │ 12  │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
└────┴─────┴──────────┴─────────────────┴───────┴──────────────────────────┘
12 jobs in 107 ms
```

Third run: job 7 ends alice's sessions on the site in the middle of the run. The alice jobs that hit the signed-out page throw `SignedOut`; the pool signs alice in once and retries them:

```
$ EXPIRE=alice PASSWORD_ALICE=pw-alice bun scrape.ts
pyro: CDP server listening on ws://127.0.0.1:53462/
pyro: CDP server listening on ws://127.0.0.1:53461/
pyro: CDP server listening on ws://127.0.0.1:53465/
[pool] alice: signed in
┌────┬─────┬──────────┬─────────────────┬───────┬──────────────────────────┐
│    │ job │ identity │ orders          │ proxy │ ua                       │
├────┼─────┼──────────┼─────────────────┼───────┼──────────────────────────┤
│  0 │ 1   │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│  1 │ 2   │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
│  2 │ 3   │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│  3 │ 4   │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│  4 │ 5   │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
│  5 │ 6   │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│  6 │ 7   │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│  7 │ 8   │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│  8 │ 9   │ alice    │ alice-1,alice-2 │ de    │ Mozilla/5.0 (X11; Linux  │
│  9 │ 10  │ carol    │ carol-1,carol-2 │ -     │ MyCrawler/1.0 (+https:// │
│ 10 │ 11  │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
│ 11 │ 12  │ bob      │ bob-1,bob-2     │ us    │ Mozilla/5.0 (Macintosh;  │
└────┴─────┴──────────┴─────────────────┴───────┴──────────────────────────┘
12 jobs in 131 ms
```

Only `PASSWORD_ALICE` was needed: bob and carol still had valid sessions. If the sessions in the files have expired by the time of the next run (here after `SESSION_TTL_MS`), the site redirects to `/login`, and the pool signs in again the same way.

## How the parts work

**Detecting an expired session.** Only your job knows what "signed out" looks like on your site. Check what the site does when the session is gone, and throw `SignedOut` for that:

| The site... | Check |
| --- | --- |
| redirects to a login page | `page.url()` after `goto` |
| answers API calls with `401` or `403` | `page.waitForResponse(...)` started before `goto`, then its `status()` |
| shows a sign-in form on the same URL | `await page.getByRole('button', { name: 'Sign in' }).count()` |
| keeps the page but empties the data | wait for the data with a timeout, and treat the timeout as signed out |

Do not wait for data right after `goto` without a check. `goto` returns at the `load` event. Data the app fetches after that is not there yet; wait for its element or its response (see [Data from single-page apps](/recipes/scraping-spa-data)).

**One sign-in per identity.** Each identity has a `generation` number. A job remembers it before opening its context. When the job fails with `SignedOut`, the pool signs in only if no other job did since then; jobs that fail at the same moment share one sign-in. Without this, two parallel jobs would sign in twice, and the second sign-in could end the session of the first.

**What is saved.** After every successful job the pool saves the context's state again, so a site that rolls its session cookie keeps the file current. `storageState()` reads `localStorage` only from the origins of the open pages. A job that ends on another origin would drop the token from the file, so the pool keeps the old file's origins that the new state does not have. A sign-in replaces the file whole.

**Proxies.** The proxy is a server flag, so identities with the same proxy share a server and identities with different proxies do not. A server with no pages costs little (about 8 MB on macOS). Keep each identity on the same proxy across runs: many sites end a session when it shows up from a new IP address.

**Round robin or least recently used.** With `pick: 'lru'` (the default), the identity idle the longest gets the next job; with equal jobs this is a round robin, and when one identity is slow the others take more jobs. `pick: 'round-robin'` keeps a fixed order and skips identities at their limit.

## Adapting it

- **Your sign-in.** Replace the body of `signIn`. Wait for proof that the session works (an element only signed-in users see), not just for the URL.
- **Concurrency.** `perIdentity` limits jobs per account. All identities on one server share one CPU core, because the pool opens one connection per server (see [Concurrency at scale](/scraping/concurrency)). For CPU-heavy sites, run several pools in separate processes, each on its own set of identities.
- **Locale and time zone.** Add them to the identity file and set them with a CDP session after `newContext`, as the [profile store](/scraping/profiles#a-profile-store) does. A time zone applies to a whole connection.
- **Captchas and two-factor prompts.** A script cannot always sign in. Sign in once by hand in another tool that exports `storageState` JSON, put the file in `profiles/`, and make `signIn` throw, so you hear when the session ends.
- **Secrets.** The state files are credentials. To keep them encrypted at rest and move them between machines, see [Moving profiles between machines](/recipes/scraping-move-profiles-securely).

## Next

- Many short jobs per identity against a heavy app? [Fork-mode signed-in jobs](/recipes/scraping-fork-signed-in-jobs) signs in once in the server and forks a ready page per job.
- Sizing the server: [Concurrency at scale](/scraping/concurrency#how-many-jobs-per-server).
