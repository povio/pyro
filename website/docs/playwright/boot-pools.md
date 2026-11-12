# Booting pages ahead

Most of an E2E test's time often goes into booting the app: downloading and parsing its JavaScript, running its startup code and its first requests. With `use.pyro.boot`, the browser keeps pages that have already loaded your app, and a test's first `goto` becomes a route change inside one of them. The test code does not change.

## Configure it

```ts
// playwright.config.ts
import { defineConfig } from '@povio/pyro/wright';

export default defineConfig({
  use: {
    baseURL: 'http://localhost:3000',
    pyro: {
      boot: [{ url: '/platform', routes: '/platform/**' }],
    },
  },
});
```

```ts
// unchanged test
test('opens a position', async ({ page }) => {
  await page.goto('/platform/positions/42');   // a route change in a booted app
  await expect(page.getByRole('heading', { name: 'Position 42' })).toBeVisible();
});
```

Each `boot` entry is a `BootSpec`:

| Field | Meaning |
| --- | --- |
| `url` | The page to boot, resolved against `baseURL`. |
| `routes` | A glob, RegExp or predicate, or a list of them. A first `goto` to a matching URL on the same origin as `url` takes a booted page. Optional: without it, only a `goto` to `url` itself does. |
| `pool` | How many booted pages to keep ready. Default 2. |

`boot` can be set per file with `test.use({ pyro: { boot: [...] } })`, since `pyro` is a worker option. Each distinct `pyro` value gets its own browser in the worker.

## When goto becomes a route change

A page has no browser target until its first command. That first command decides what the page gets. A page takes a booted page when all of these hold:

1. It is the **first page of its context** to get a target. Later pages of the same context, and popups, are fresh.
2. Its **first call is `page.goto(url)`**. Any other first call that talks to the page (`evaluate`, `setContent`, `setViewportSize`, a locator action) gives a fresh page. Calls that only register state, such as `page.route()`, `page.on()`, `waitForResponse()` and `addInitScript()`, do not count as the first call.
3. The target URL **equals a spec's `url`, or matches its `routes` on the same origin**.
4. The context has **no cookies** (from `storageState` or `addCookies`), **no `extraHTTPHeaders`**, **no `userAgent`**, and **no media emulation** beyond the defaults (`colorScheme` other than `light`, `reducedMotion` other than `no-preference`).

Then:

- The booted page has already loaded `url` with the context's init scripts (`context.addInitScript`, `page.addInitScript` called before `goto`, and the `localStorage` part of `storageState`) and the context's viewport.
- If the target is `url` itself, nothing else happens. Otherwise the browser changes the route in place: `history.pushState` to the target, then a `popstate` event, then it waits for the page to settle. Your router renders the target route without booting the app again.
- `waitUntil: 'networkidle'` also waits for network idle.
- **`goto` returns `null`**, not a `Response`. Code that checks `(await page.goto(url))!.ok()` fails on booted pages.

If no booted page can be taken (the browser refuses), the page falls back to a fresh one.

::: warning device descriptors set userAgent
`...devices['Desktop Chrome']` sets `userAgent`, which rule 4 excludes. A project that spreads a device descriptor gets no booted pages. Set `viewport` directly instead.
:::

## Profiles

The browser keeps one pool per combination of boot URL, init scripts, viewport and pool size. The first context with a new combination registers it, and the browser boots its pages; later contexts with the same combination take from the pool, and the browser boots replacements.

Init scripts must therefore be **the same in every test** to share a pool. A script that embeds a per-test value (a freshly created token, a timestamp) makes a new pool for each test, so each test waits for its own boot and gains nothing. Put per-test data in the requests your routes make after the route change, or seed it on the server.

`page.clock.install()` before `goto` also adds init scripts that record the current time, so a test that installs the clock first gets a pool of its own.

## What it is good for

The booted app ran its startup code **before the test seeded anything**. That suits routes that fetch their data when they mount: the route change triggers those requests after your test's setup.

It does not suit:

- Apps that load per-user data once at startup (a "current user" request at boot). The booted app has whatever it loaded before the test ran. Use a signed-in state that is the same for every test (fixed `storageState` local storage), or skip booting for those tests.
- Apps without client-side routing for the target. `pushState` + `popstate` must make the router render the target.
- Tests that assert on the boot itself (first paint, loading screens).

To see whether pages came from the pool, read `browser._stats` (internal, may change): `{ pooled, fresh }`.

```ts
test.afterAll(async ({ browser }) => {
  console.log((browser as any)._stats); // { pooled: 12, fresh: 1 }
});
```

## Fork mode

Pyro's fork mode (`pyro serve --fork`) serves each connection from a fork of a process that booted the profiles once. The Playwright layer opens one connection per worker and keeps it, so it does not get a fork per test.

Measured with version 0.0.33: with `use: { pyro: { args: ['--fork'], boot: [...] } }`, every page was fresh (`{ pooled: 0, fresh: 4 }` for four tests that took booted pages without `--fork`). Do not combine `boot` with `--fork`.

Fork mode pays off with the native API, where each test opens its own connection: see [Connections and fork mode](/guide/fork-mode) and [Fork mode in tests](/e2e/fork). Pre-booted pages there are called profiles; see [Profiles and pre-booted pages](/guide/profiles).
