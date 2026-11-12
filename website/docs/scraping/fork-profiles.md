# Profiles booted once (fork)

A stored profile ([Stored profiles](/scraping/profiles)) saves cookies to a file and loads them into a new context for every job. Each job still loads the site's pages and boots its JavaScript from scratch. A server-side profile goes further: the server loads the page once, runs your sign-in steps once, and keeps that signed-in page in memory. In fork mode, every job then gets its own copy of that page, already signed in, in about 20 ms.

This page uses the native SDK (`@povio/lightning-panda`). For the concept in general, see [Profiles and pre-booted pages](/guide/profiles) and [Connections and fork mode](/guide/fork-mode).

## How fork mode works

```sh
lightningpanda serve --fork
# or: LIGHTNINGPANDA_FORK=1 lightningpanda serve
```

With `--fork`, the server starts a **template process**. When you register a profile, the template opens a page at the profile's URL, waits for it to load, and runs the profile's `setup` steps (fill the login form, click Sign in, wait for the account page). The result stays in the template.

Each new CDP connection is then served by a `fork()` of the template. The forked process starts with every profile's page already booted, and shares its memory with the template copy-on-write: a page uses new memory only for what it changes. When the connection closes, its process exits and all of its memory goes back to the operating system at once.

The SDK detects fork mode by itself (`Lightningpanda.capabilities` reports `{ fork: true }`). Each `lp.page({ as })` opens its own connection, forked from that profile's snapshot, and closing the page ends the process.

Fork mode is available on macOS on Apple silicon and on Linux x64 and arm64. The macOS Intel build has no fork mode.

## Sign in once, fork per job

```ts
import { Lightningpanda, button, css, evaluate, goto, label, waitForUrl } from '@povio/lightning-panda';

const BASE = 'http://127.0.0.1:4500';
const lp = await Lightningpanda.launch({ args: ['--fork'] });
try {
  for (const user of ['alice', 'bob']) {
    await lp.profile(user, {
      url: `${BASE}/login`,
      setup: [
        label('User').fill(user),
        label('Password').fill(process.env[`PASSWORD_${user.toUpperCase()}`] ?? ''),
        button('Sign in').click(),
        waitForUrl('/account'),
        css('.order').first().read('firstOrder'), // waits until the app rendered its data
      ],
    });
  }
  await lp.warm('alice', 10_000); // optional: wait until the template finished booting
  await lp.warm('bob', 10_000);

  const jobs = Array.from({ length: 8 }, (_, i) => ({ as: i % 2 ? 'bob' : 'alice', page: (i % 5) + 1 }));
  const results = await Promise.all(
    jobs.map(async (job) => {
      await using page = await lp.page({ as: job.as }); // a fresh fork, signed in, on /account
      const r = await page.run([
        css('#who').read('who'),
        goto(`${BASE}/products?page=${job.page}`),
        evaluate(`[...document.querySelectorAll('.product .name')].map((e) => e.textContent)`, 'names'),
      ]);
      return { ...job, who: r.values.who, n: (r.values.names as string[]).length };
    }),
  );
  console.table(results);
} finally {
  lp.close();
}
```

Measured on an M1 Max against a local test site: both profiles booted and signed in within 150 ms, and the eight jobs, run all at once, took 140 to 180 ms in total. Each `lp.page()` returned a signed-in page in 18 to 125 ms, rising with the number opened at the same moment.

What the profile options do:

| Option | Meaning |
| --- | --- |
| `url` | Where the profile's page boots. Must be an absolute URL. |
| `setup` | Steps run once on the booted page; every page of the profile starts from their result. Needs `--fork`. |
| `base` | Start from another profile's snapshot instead of booting `url`: its page is forked and only this profile's `setup` runs. Needs `--fork`. |
| `localStorage` | Entries written before the app's scripts run (for example a token you already have). |
| `initScripts` | Scripts run in every document before its own scripts. |
| `viewport` | `{ width, height }` of the page. |
| `pool` | Pages kept booted ahead per connection, 1 to 8 (default 2). Without `--fork` only; the fork template keeps one page per profile. |
| `prepare` | `false` boots each page when a job asks for it instead of ahead. |

A page taken with `lp.page({ as })` is where `setup` left it, here on `/account`. Pass `url` to navigate first (`lp.page({ as: 'alice', url: 'https://example.com/orders' })`), or `route` for a same-document navigation in a single-page app, which changes the route without booting the app again.

::: warning All forks share one session
Every fork starts from the same snapshot, so every page of a profile sends the same session cookie. To the site it is one signed-in user with many tabs open. That is usually what you want, but a site that limits concurrent requests per session, or rotates the session on each request, sees all jobs as one user. Register one profile per account to spread jobs over several sessions.
:::

### When `setup` fails

The template runs `setup` in the server, not in your script. A failing step is reported on the server's stderr, for example:

```
lightningpanda: template: setup of profile 0: step 4 (read Text {"css":".order"}): strict mode violation: ...
```

The client is not told. The profile is still offered, in whatever state the steps reached, so a job may get a page that is not signed in. Make your first step check that it is (`check(css('#who')).visible()` from the SDK fails the run with a clear error), and watch the server's stderr while you write the `setup`. A launched server's stderr is your script's stderr.

## Base chains: one sign-in, many starting points

`base` builds a snapshot on top of another. The sign-in runs once; each derived profile adds only its own steps on a fork of the signed-in page:

```ts
const signInSteps = (user: string) => [
  label('User').fill(user),
  label('Password').fill(process.env[`PASSWORD_${user.toUpperCase()}`] ?? ''),
  button('Sign in').click(),
  waitForUrl('/account'),
  css('.order').first().read('firstOrder'),
];

await lp.profile('alice', { url: `${BASE}/login`, setup: signInSteps('alice') });
await lp.profile('alice-products', {
  url: `${BASE}/login`, // required field; the page comes from the base
  base: 'alice',
  setup: [goto(`${BASE}/products?page=3`)],
});

await using page = await lp.page({ as: 'alice-products' }); // signed in, on products page 3
```

Use this for jobs that all start from the same view: a search form with filters applied, a dashboard with a date range set, a report already opened.

## Refreshing an expired session

A snapshot holds the session as it was when `setup` ran. When the site expires that session, every new fork is signed out. Register the profile again to sign in again:

```ts
await lp.releaseProfile('alice-products'); // derived profiles first
await lp.releaseProfile('alice');
await lp.profile('alice', { url: `${BASE}/login`, setup: signInSteps('alice') });
await lp.warm('alice', 10_000);
```

`releaseProfile` drops this client's hold, and the server retires a profile once no client holds it and no other profile derives from it. Registering the same name with the same definition while it is still held adopts the existing snapshot instead of booting again, so release first. Pages already open keep running from the old snapshot.

A simple policy: re-register on a timer shorter than the site's session lifetime, or when a job's first check finds the page signed out.

## One server, many worker processes

A fork-mode server keeps profiles by name for all its clients. Start the server and register the profiles in one launcher process, then hand its endpoint to worker processes. The launcher below uses `Bun.spawn`; with Node.js use `child_process.spawn`. `signInSteps` is the helper from the previous section.

```ts
// launcher.ts
import { Lightningpanda } from '@povio/lightning-panda';

const lp = await Lightningpanda.launch({ args: ['--fork', '--cdp-max-connections', '64'] });
await lp.profile('alice', { url: `${BASE}/login`, setup: signInSteps('alice') });
await lp.warm('alice', 10_000);

const workers = [1, 2].map((i) => Bun.spawn(['bun', 'worker.ts', lp.wsEndpoint, String(i)], { stdout: 'inherit', stderr: 'inherit' }));
await Promise.all(workers.map((w) => w.exited));
lp.close();
```

```ts
// worker.ts
import { Lightningpanda, css, goto } from '@povio/lightning-panda';

const [ws, id] = process.argv.slice(2);
const lp = await Lightningpanda.connect(ws);
try {
  const out = await Promise.all(
    [1, 2, 3].map(async (n) => {
      await using page = await lp.page({ as: 'alice' }); // adopts the launcher's profile: no boot
      const r = await page.run([goto(`http://127.0.0.1:4500/products?page=${n}`), css('h1').read('h')]);
      return r.values.h;
    }),
  );
  console.log(`worker ${id}`, out);
} finally {
  lp.close();
}
```

A worker's `lp.page({ as: 'alice' })` adopts the profile the launcher registered (`lp.adopt(name)` does it explicitly). A worker calling `lp.profile(name, def)` with the same definition adopts it too; a different definition under a name another client holds is refused. A client holds the profiles it registered or adopted until it calls `releaseProfile` or disconnects. `Lightningpanda.preparedProfiles` lists the names and how many clients hold each.

The same works with a long-running server you start yourself (`lightningpanda serve --fork --port 9222`) and clients that `Lightningpanda.connect('ws://127.0.0.1:9222/')`. See [One server, many workers](/e2e/shared-server).

## Without `--fork`: pools of booted pages

Without fork mode, `lp.profile()` still helps: each connection keeps `pool` pages booted at the profile's URL (with its `localStorage` and init scripts), and `lp.page({ as })` takes one. When one is taken, the connection boots a replacement once it is quiet. `setup` and `base` are refused without `--fork`, so a pool page is booted but not signed in by steps; use `localStorage` or init scripts for tokens. Every pooled page is a full copy of the app in memory. See [Profiles and pre-booted pages](/guide/profiles).

## Fork profiles or stored profiles?

| | Stored profiles (`storageState`) | Fork profiles |
| --- | --- | --- |
| Survives a server restart | yes, it is a file | no, it is in the server's memory |
| Cost per job | new context, full page load and app boot | a fork: about 20 ms, page already booted |
| Session state kept | cookies and `localStorage` only | everything: cookies, all storage, IndexedDB, the live JavaScript state |
| Sign-in | your script, once per expiry | `setup` steps in the server, once per registration |
| Sessions | one per file | one per profile, shared by all its forks |
| API | Playwright library or SDK | native SDK, `--fork` server |
| Platforms | all | macOS Apple silicon, Linux x64/arm64 |

Fork profiles win when you run many short jobs as the same identity against a heavy app: the boot you skip is most of the cost. Stored profiles win when sessions must outlive the process, when you need the Playwright API, or on Intel Macs. They combine well: keep credentials and state on disk, and feed a token from the file into a fork profile's `localStorage`, or let `setup` sign in with the password.

## Limits and tuning

- **Connections.** In fork mode each page is a connection, and the server accepts `--cdp-max-connections` at once (default 16). Further connections wait until one closes. Raise it to the number of jobs you run at once.
- **Spare forks.** Each template keeps `LIGHTNINGPANDA_FORK_SPARES` forks waiting (default 2) so a connection does not wait for its fork.
- **JIT tiers.** Forks run without JavaScriptCore's DFG and FTL optimizing tiers, because connections are usually too short for them to pay off. Set `JSC_useDFGJIT` in the server's environment to turn them back on for long jobs.
- **Profile ids are per server.** A restart forgets every profile; register them again on start.

## Related

- [Concurrency at scale](/scraping/concurrency): worker pools and measuring.
- [Fork mode in tests](/e2e/fork): the same machinery used for test suites.
- [Pyro CDP domain](/reference/cdp-domain): `registerPreparedPage`, `acquirePreparedPage`, `adoptPreparedPage` on the wire.
