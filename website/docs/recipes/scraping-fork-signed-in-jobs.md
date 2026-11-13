# Fork-mode signed-in jobs

The [identity pool](/recipes/scraping-identity-pool) loads a fresh context from a state file for every job, and every job loads and boots the site again. On a server you can skip both. Start `pyro serve --fork`, register one named profile per identity whose `setup` steps sign in, and every job gets a `fork()` of that identity's signed-in page, already booted, in a few milliseconds.

This recipe uses the native SDK (`@povio/pyro`). Fork mode runs on macOS on Apple silicon and on Linux x64 and arm64, not on Intel Macs.

## When to choose this over state files

| | State files ([identity pool](/recipes/scraping-identity-pool)) | Fork profiles (this recipe) |
| --- | --- | --- |
| Cost per job | new context, page load, app boot | a fork of a booted page: a few ms |
| Survives a server restart | yes, the files are on disk | no; register the profiles again at start |
| Session state kept | cookies and `localStorage` | everything: cookies, all storage, IndexedDB, live JavaScript state |
| Sessions per identity | one per file | one per profile, shared by every fork |
| Per-identity proxy | the context's `proxy` option | the profile's `proxyServer` (releases up to 0.0.33: one fork server per proxy) |
| Per-identity user agent | context option, sent at sign-in too | the profile's `userAgent`, sent at sign-in too (releases up to 0.0.33: set on each forked page, and the sign-in uses Pyro's default user agent) |
| API | Playwright library or SDK | native SDK only |
| Platforms | all | macOS Apple silicon, Linux x64/arm64 |

Choose fork profiles when you run many short jobs per identity against a site whose pages are slow to boot, on a long-running server. Choose state files when sessions must outlive the process, when the site checks that the user agent at sign-in matches later requests, or when you need the Playwright API. You can combine them: sign in with a state file's token by passing it as the profile's `localStorage`.

## Files

```
fork/
  site.ts          test site: sign-in, session cookie + localStorage token, expiry
  fork-jobs.ts     the server, the profiles and the jobs
```

```sh
mkdir fork && cd fork
bun init -y
bun add @povio/pyro
```

Until `@povio/pyro` is published under that name, install it as [Installation](/guide/installation#try-today) describes.

## The test site

The same site as in the [identity pool recipe](/recipes/scraping-identity-pool): `/login` sets an `httpOnly` session cookie and a `localStorage` token, `/account` needs both and loads the orders with `fetch` after `load`, `/admin/expire?user=<name>` ends a user's sessions, and `/whoami` echoes the user agent. It also logs each sign-in with the user agent it came from.

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

```sh
SESSION_TTL_MS=3600000 bun site.ts 4601
```

## The script

```ts
// fork-jobs.ts: bun fork-jobs.ts
// One fork-mode server, one named profile per identity, signed in once by `setup`.
// Every job is a fork of its identity's signed-in page.
import { Pyro, RunError, button, css, evaluate, goto, label, waitForUrl, type Page } from '@povio/pyro';

const BASE = process.env.BASE ?? 'http://127.0.0.1:4601';
const IDENTITIES: Record<string, { userAgent: string }> = {
  alice: { userAgent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36' },
  bob: { userAgent: 'MyCrawler/1.0 (+https://example.com/bot)' },
};

const pyro = await Pyro.launch({ args: ['--fork', '--cdp-max-connections', '32'] });

/** Registers (or re-registers) the identity's profile: the template signs in once. */
async function signIn(name: string) {
  const password = process.env[`PASSWORD_${name.toUpperCase()}`];
  if (!password) throw new Error(`set PASSWORD_${name.toUpperCase()}`);
  await pyro.releaseProfile(name); // no-op the first time; drops the stale snapshot after
  await pyro.profile(name, {
    url: `${BASE}/login`,
    setup: [
      label('User').fill(name),
      label('Password').fill(password),
      button('Sign in').click(),
      waitForUrl('/account'),
      css('.order').first().read('firstOrder'), // waits until the app loaded its data
    ],
  });
  await pyro.warm(name, 10_000);
}

/** A forked, signed-in page for `name`, with the identity's user agent. */
async function pageAs(name: string): Promise<Page> {
  const page = await pyro.page({ as: name });
  await page.send('Emulation.setUserAgentOverride', { userAgent: IDENTITIES[name].userAgent });
  return page;
}

const signingIn = new Map<string, Promise<void>>();
/** One job as `name`; on a signed-out page, signs in again (once per identity) and retries. */
async function job(name: string, url: string) {
  for (let attempt = 0; ; attempt++) {
    await using page = await pageAs(name);
    try {
      const r = await page.run(
        [
          goto(url),
          waitForUrl('/account', { timeout: 300 }), // on /login when the session is gone
          css('.order').first().read('first'),
          evaluate(`fetch('/whoami').then((r) => r.json())`, 'seen'),
        ],
        { timeout: 2_000 },
      );
      return { name, first: r.values.first, ua: (r.values.seen as { ua: string }).ua.slice(0, 24), ms: Math.round(r.ms) };
    } catch (e) {
      if (!(e instanceof RunError) || e.step !== 1 || attempt > 0) throw e;
      console.error(`[fork] ${name}: signed out, signing in again`);
      if (!signingIn.has(name)) signingIn.set(name, signIn(name).finally(() => signingIn.delete(name)));
      await signingIn.get(name);
    }
  }
}

try {
  let t = performance.now();
  await Promise.all(Object.keys(IDENTITIES).map(signIn));
  console.log(`signed in ${Object.keys(IDENTITIES).length} identities in ${Math.round(performance.now() - t)} ms`);

  t = performance.now();
  const names = Object.keys(IDENTITIES);
  console.table(await Promise.all(Array.from({ length: 8 }, (_, i) => job(names[i % names.length], `${BASE}/account`))));
  console.log(`8 jobs in ${Math.round(performance.now() - t)} ms`);

  await fetch(`${BASE}/admin/expire?user=alice`); // the site ends alice's session
  t = performance.now();
  console.table(await Promise.all(Array.from({ length: 4 }, () => job('alice', `${BASE}/account`))));
  console.log(`4 jobs after expiry in ${Math.round(performance.now() - t)} ms`);
} finally {
  pyro.close();
}
```

## Run it

```
$ PASSWORD_ALICE=pw-alice PASSWORD_BOB=pw-bob bun fork-jobs.ts
pyro: CDP server listening on ws://127.0.0.1:53811/ (fork mode)
signed in 2 identities in 52 ms
┌───┬───────┬─────────┬──────────────────────────┬────┐
│   │ name  │ first   │ ua                       │ ms │
├───┼───────┼─────────┼──────────────────────────┼────┤
│ 0 │ alice │ alice-1 │ Mozilla/5.0 (X11; Linux  │ 5  │
│ 1 │ bob   │ bob-1   │ MyCrawler/1.0 (+https:// │ 5  │
│ 2 │ alice │ alice-1 │ Mozilla/5.0 (X11; Linux  │ 5  │
│ 3 │ bob   │ bob-1   │ MyCrawler/1.0 (+https:// │ 5  │
│ 4 │ alice │ alice-1 │ Mozilla/5.0 (X11; Linux  │ 5  │
│ 5 │ bob   │ bob-1   │ MyCrawler/1.0 (+https:// │ 5  │
│ 6 │ alice │ alice-1 │ Mozilla/5.0 (X11; Linux  │ 5  │
│ 7 │ bob   │ bob-1   │ MyCrawler/1.0 (+https:// │ 5  │
└───┴───────┴─────────┴──────────────────────────┴────┘
8 jobs in 23 ms
[fork] alice: signed out, signing in again
[fork] alice: signed out, signing in again
[fork] alice: signed out, signing in again
[fork] alice: signed out, signing in again
┌───┬───────┬─────────┬──────────────────────────┬────┐
│   │ name  │ first   │ ua                       │ ms │
├───┼───────┼─────────┼──────────────────────────┼────┤
│ 0 │ alice │ alice-1 │ Mozilla/5.0 (X11; Linux  │ 7  │
│ 1 │ alice │ alice-1 │ Mozilla/5.0 (X11; Linux  │ 7  │
│ 2 │ alice │ alice-1 │ Mozilla/5.0 (X11; Linux  │ 6  │
│ 3 │ alice │ alice-1 │ Mozilla/5.0 (X11; Linux  │ 6  │
└───┴───────┴─────────┴──────────────────────────┴────┘
4 jobs after expiry in 365 ms
```

On an M1 Max: both identities signed in within about 50 ms, and eight jobs ran in 23 ms of wall-clock time, about 5 ms of browser time each. After the site ended alice's session, the four alice jobs found the login page, shared one new sign-in, and finished. Most of the 365 ms is the 300 ms the `waitForUrl` check waited before it gave up.

The site's log shows where each sign-in came from:

```
site on http://127.0.0.1:4601
sign-in alice from Mozilla/5.0 (compatible; pyro/0.0.17)
sign-in bob from Mozilla/5.0 (compatible; pyro/0.0.17)
sign-in alice from Mozilla/5.0 (compatible; pyro/0.0.17)
```

The sign-in ran in the server's template with Pyro's default user agent; the jobs then sent each identity's own. That is what this recipe's code does: it sets the user agent on each forked page. Newer than 0.0.33, a profile has a `userAgent` option that covers the sign-in too. See [User agent](#how-the-parts-work) below.

## How the parts work

**One profile per identity.** Every fork of a profile starts from the same snapshot, so all of alice's jobs share one session cookie. To the site, that is one user with many tabs. Register one profile per account to spread jobs over several sessions.

**Signing in.** `pyro.profile(name, { url, setup })` boots `url` in the template and runs `setup` there once. `pyro.warm(name)` waits until the snapshot is ready. When a `setup` step fails, `pyro.profile()` rejects with a `ProfileSetupError` that names the step and the reason, and no page of the profile is handed out. In releases up to 0.0.33 the failure is only printed on the server's stderr (your script's stderr with `Pyro.launch`) and the profile is still offered in whatever state the steps reached. Either way a session can end after the sign-in, which is why every job checks first.

**Detecting a signed-out page.** The first steps of every job are `goto(url)` and `waitForUrl('/account', { timeout: 300 })`. On a signed-out page the site redirects to `/login`, the check fails, and `page.run` throws a `RunError` whose `step` is `1`. Other failures (a missing element, a slow page) have other step numbers and are not taken for an expired session. Use whatever check fits your site; keep its timeout short, since the page is already loaded when it runs.

**Re-sign-in.** `releaseProfile(name)` drops this client's hold, and the server retires the old snapshot once no client holds it; `profile(name, ...)` then signs in again. The `signingIn` map makes jobs that fail at the same moment share one sign-in. Pages already open keep running from the old snapshot. If other processes adopted the profile (see below), they hold it too; then register the new sign-in under a new name, such as `alice-2`, and point new jobs at it.

**User agent.** `page.send('Emulation.setUserAgentOverride', ...)` on each forked page sets the `User-Agent` header and `navigator.userAgent` for everything the job does after it. Newer than 0.0.33, put the user agent in the profile instead (`pyro.profile(name, { url, userAgent: IDENTITIES[name].userAgent, setup })`) and drop the call: the `setup` steps and every fork then send it, so the site sees the same user agent at sign-in and in the jobs. That variant was not run for this page.

**Proxies.** A fork server's `--proxy` is the default for every profile; forks and their `setup` use it. Newer than 0.0.33, a profile's `proxyServer` (`http://user:pass@host:port`) replaces it for that profile, so one fork server holds identities behind different proxies. See [Proxy and user agent per identity](/scraping/fork-profiles#proxy-and-user-agent-per-identity). In releases up to 0.0.33, launch one fork server per proxy and register each identity on its proxy's server:

```ts
const de = await Pyro.launch({ args: ['--fork', '--proxy', 'http://user:pass@proxy-de.example.net:3128'] });
const us = await Pyro.launch({ args: ['--fork', '--proxy', 'http://user:pass@proxy-us.example.net:3128'] });
```

A fork server launched with `--proxy` sent the profile's boot request and the forked job's requests through that proxy in a test with the test proxy from the identity pool recipe.

## On a server

- **Connections.** Each forked page is one connection. `--cdp-max-connections` (default 16) caps the pages open at once; further connections wait. Set it to the number of jobs you run at once.
- **Many worker processes.** Start the server and register the profiles in one launcher process, then give `pyro.wsEndpoint` to workers that `Pyro.connect()` and call `pyro.page({ as: 'alice' })`. They adopt the profiles without signing in again. See [One server, many worker processes](/scraping/fork-profiles#one-server-many-worker-processes).
- **Restarts.** Profiles live in the server's memory. After a restart, register them again; a systemd unit with `Restart=always` brings the server back, not the profiles. See [Running as a service](/scraping/service).
- **Refresh on a timer.** Instead of waiting for a job to find the login page, re-register each profile on a timer shorter than the site's session lifetime.
- **Memory.** A fork shares memory with the template until it writes, and its process exits when the page closes. Nothing builds up across jobs. Measure the template and a few forks under your own load; see [Concurrency at scale](/scraping/concurrency#memory).

## Next

- The concepts behind this: [Profiles booted once (fork)](/scraping/fork-profiles).
- The file-based alternative: [A pool of signed-in identities](/recipes/scraping-identity-pool).
