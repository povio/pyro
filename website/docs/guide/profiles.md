# Profiles and pre-booted pages

Most of an E2E test's time is often the app booting: downloading and parsing its JavaScript, running it, fetching the signed-in user. A **profile** moves that out of the test. The browser keeps pages of your app booted ahead, signed in, and hands one to a test when it asks. This page explains the concept; [Profiles and page pools](/e2e/profiles) has the testing recipes and [Stored profiles](/scraping/profiles) the scraping ones.

## The idea

A profile is a recipe for a page:

- the URL it boots at;
- what is in `localStorage` before the app's scripts run (auth tokens, user settings);
- init scripts that run in every document before its scripts;
- the viewport;
- how many booted pages to keep waiting (the pool).

Each booted page lives in its own browser context. When a test takes one, it gets a page that already ran the app. The server boots a replacement for the next test.

## Three ways to declare one

### With the native SDK: `pyro.profile`

```ts
import { Pyro } from '@povio/pyro';

const pyro = await Pyro.launch();
await pyro.profile('employee', {
  url: 'http://localhost:3000/platform',       // where pages boot
  localStorage: { token: process.env.TOKEN! }, // written before the app runs
  initScripts: ['window.__E2E__ = true'],      // run in every document first
  viewport: { width: 1440, height: 900 },
  pool: 2,                                     // pages kept booted (1-8)
});

await using page = await pyro.page({ as: 'employee', route: '/positions/42' });
```

| Field | Meaning |
| --- | --- |
| `url` | Where the profile's pages boot and wait. Any page of the app works. |
| `localStorage` | Entries written to `localStorage` before the app's scripts run. The SDK installs them as the first init script, so they are written again in every document the page loads, whatever its origin. |
| `initScripts` | Sources run in every document of the page before its own scripts, popups included. |
| `viewport` | `{ width, height }` of the page. |
| `pool` | Booted pages to keep waiting per connection, 1 to 8. The SDK's default is 2. |
| `prepare` | `false` boots each page only when a test asks for it, instead of ahead. |
| `setup` | Steps run once on the booted page before tests get it. Fork mode only. |
| `base` | Start from another profile's snapshot. Fork mode only. |

`setup` and `base` are covered in [Fork mode](/guide/fork-mode#setup-and-snapshots).

### With a server flag: `--prepare`

```sh
pyro serve --prepare http://localhost:3000/dashboard \
  --local-storage auth.json
```

Every connection keeps one page booted at that URL, seeded from the `--local-storage` file (`{"http://localhost:3000": {"token": "..."}}`), before any client connects. `--prepare` is repeatable, and also reads `PYRO_PREPARE` (`;`-separated). A client takes the page with the CDP command `Pyro.acquirePreparedPage {url}`. The page is announced like a new target, so Playwright sees it as a page of its default context, already loaded.

### With CDP: `Pyro.registerPreparedPage`

The SDK's `pyro.profile` sends this command. Any CDP client can:

```json
{ "method": "Pyro.registerPreparedPage",
  "params": { "url": "http://localhost:3000/platform",
              "initScripts": ["localStorage.setItem('token', 'abc')"],
              "viewport": { "width": 1440, "height": 900 },
              "pool": 2 } }
```

It replies with a `profileId`. `Pyro.preparedPages` lists what is waiting, and `Pyro.unregisterPreparedPage` removes a profile. The parameters are described in the [Pyro CDP domain](/reference/cdp-domain).

For the Playwright module, the equivalent is `use: { pyro: { boot: [...] } }` in the config. See [Booting pages ahead](/playwright/boot-pools).

## `route` versus `url`

`pyro.page()` takes the page and adds a first step to its first `run`:

| Option | First step | What happens |
| --- | --- | --- |
| `route: '/positions/42'` | `route` | A same-document navigation: `history.pushState` then a `popstate` event, settled. The app does not boot again; its router renders the new route. |
| `url: 'http://…/positions/42'` | `goto` | A full navigation. The app boots again, so the profile saves only the context setup. |
| neither | none | The page stays where it booted. |

Use `route` for single-page apps whose router listens to `popstate` (React Router, Vue Router and similar). Use `url` only when a full page load is the point of the test.

::: warning The app booted before your test seeded data
A booted page ran the app's startup requests before the test existed. Anything fetched at boot (the user, feature flags, a list cached in a store) is as old as the page. `route` works best for routes that fetch their own data when they mount. If a test creates data and then needs the app to see it at boot, use `url`, or reload.
:::

If you pass `initScripts` to `pyro.page()` itself, the page cannot come from the pool (a booted page already ran its document), so a fresh one boots for that test.

## When the replacement boots: refill policies

When a test takes a page, the connection boots a replacement. When it does so is a trade between speed and memory:

| Policy | Replacement boots | Booted apps per profile and connection |
| --- | --- | --- |
| `quiet` (default) | as soon as the connection has no open page, or after 15 ms without commands | pool + 1 (the pool plus the page in use) |
| `after-close` | only once the taken page's browser context closes | at most pool |

Set it per server with `--prepare-refill after-close` (env `PYRO_PREPARE_REFILL`), or per profile with `refillAfterClose: true` in `Pyro.registerPreparedPage`.

The default boots during the gaps between tests, so the next test usually finds a page ready. Its cost is one extra booted app. A boot runs on the connection's browser thread, so a test running on the same connection can queue behind it. That is why the SDK spreads tests over several connections: `Pyro.connect(ws, { connections: 4 })` (or `launch({ connections: 4 })`) opens four browser threads, takes each page from the connection with the fewest open pages, and lets one thread boot while another runs a test.

`pyro.stats` counts how pages were served: `prepared` (taken booted), `preparedBooting` (taken while still booting) and `fresh` (booted for the test because none was waiting). If `fresh` grows, raise `pool` or `connections`.

## Memory

Every booted page is a full copy of the app. A pool of N holds N copies plus the one in use, per connection and per profile. For example, 4 connections, 2 profiles and a pool of 2 keep up to 24 apps in memory with the default policy.

To cap memory:

- lower `pool`, or use `--prepare-refill after-close`;
- use fewer profiles per connection;
- use [fork mode](/guide/fork-mode), where each profile is booted once in a template and every connection shares its memory copy-on-write.

Closed pages are collected and their memory returned to the operating system at once. The repository README measured a 20-test loop on a 2 MB app plateauing at 275 MB instead of 323 MB with this on. `PYRO_RELEASE_ON_CLOSE=0` turns it off, for comparisons.

## Always close pages

An open page counts as in use: with the default policy a connection with an open page waits before refilling. Close pages when the test ends. The SDK's `Page` is disposable, so `await using` does it for you:

```ts
test('closes a position', async () => {
  await using page = await pyro.page({ as: 'employee', route: '/positions/42' });
  await page.run([/* ... */]);
}); // page closed here, even when the run throws
```

## Releasing profiles

`pyro.releaseProfile(name)` drops a profile you no longer need, so the server stops keeping pages booted for it. In fork mode, profiles are shared by name between clients of one server; see [Fork mode](/guide/fork-mode#shared-profiles).
