# Profiles and page pools

A profile is a kind of page your tests ask for by name: "a signed-in employee on the dashboard", "a guest on the pricing page". The browser boots pages of a profile before any test asks, so `lp.page({ as })` returns one that has already loaded the app, run its scripts and read its tokens. This page lists every option of `lp.profile()` and `lp.page()` and explains pools, connections, refills and memory. [Fork mode in tests](/e2e/fork) covers the fork-only options in depth; [Profiles and pre-booted pages](/guide/profiles) explains the concept.

```ts
await lp.profile('employee', {
  url: `${APP}/platform`,
  localStorage: { accessToken: tokens.access, refreshToken: tokens.refresh },
  viewport: { width: 1440, height: 900 },
  pool: 4,
});

await using page = await lp.page({ as: 'employee', route: `/positions/${id}` });
```

## `lp.profile(name, profile)`

Declares the profile and starts booting its pages. It resolves once the server has registered it, not when the pages are booted (use [`warm`](#warm) for that).

| Option | Type | Default | Meaning |
|---|---|---|---|
| `url` | `string` | required | Where its pages boot, and stay until a test takes one. Any page of the app works; a test moves on with `route` |
| `localStorage` | `Record<string, string>` | | Written to `localStorage` before the app's scripts run, in every document the page loads |
| `initScripts` | `string[]` | | Scripts run in every document before its own scripts, after the `localStorage` writes |
| `viewport` | `{ width, height }` | 1920 × 1080 | The viewport in CSS px (device scale factor 1) |
| `pool` | `number` (1 to 8) | `2` | Pages kept booted per connection. Ignored in fork mode |
| `prepare` | `boolean` | `true` | `false` declares the profile without booting ahead: each `lp.page({ as })` then boots a fresh page |
| `setup` | `StepLike[]` | | Steps run once on the booted page before tests get it. Fork mode only |
| `base` | `string` | | Start from another profile's snapshot. Fork mode only |

Declaring `setup` or `base` against a server without `--fork` fails with `setup and base need serve --fork`. A `pool` outside 1 to 8 fails with `pool must be between 1 and 8`.

### Signing in with `localStorage`

Most single-page apps keep their session in `localStorage`. Get tokens from your auth API once, in `beforeAll`, and give them to the profile:

```ts
beforeAll(async () => {
  const tokens = await getAuthTokens();                      // your API, not the UI
  lp = await Lightningpanda.launch();
  await lp.profile('employee', {
    url: `${APP}/platform`,
    localStorage: {
      'auth.tokens': JSON.stringify(tokens),
      'user.store': JSON.stringify(await getEmployeeAccount()),
    },
  });
});
```

The entries are written by an init script, so they are written again in every document the page loads, including after a reload. Values are strings: serialize objects yourself.

There is no cookie option. For cookie sessions, either sign in once in a `setup` (fork mode, see below) or have an init script call your app's own sign-in logic. A server-wide seed for every context exists too: [`--local-storage`](/e2e/static#seeding-localstorage-for-every-context).

### `initScripts`

Code that must run before the app in every document: feature flags, a fixed `Math.random`, test hooks.

```ts
await lp.profile('flags', {
  url: `${APP}/`,
  initScripts: ['window.__FEATURES__ = { newCheckout: true };'],
});
```

## `lp.page(options)`

| Option | Meaning |
|---|---|
| `as` | The profile to take a page from. Without it you get a blank page (`about:blank`) in a new browser context |
| `route` | Same-document navigation run as the first step of the first `run`: `pushState` plus `popstate`, as a client-side router sees a link. The app does not boot again |
| `url` | A full navigation (`goto`) run as the first step of the first `run` |
| `initScripts` | Extra scripts for this page, run after the profile's. A pooled page already ran its document, so with this option the page boots fresh instead of coming from the pool |

`route` and `url` are not sent until your first `page.run`, so they cost no extra round trip. A failure there is reported as `opening the page (...)` instead of a step index.

Use `route` for an app whose routes fetch their data when they mount. Use `url` when the test needs a full load, for example because the app reads the data it shows only once at boot.

A page asked for while no booted page is waiting is booted for the test: a new context, the profile's scripts and viewport, then a navigation to the profile's `url`. A page taken while it is still booting gets a `loaded()` step in front of your steps, so your first step sees the finished document.

`lp.stats` counts what happened: `prepared` (pages taken from a pool), `preparedBooting` (of those, still booting when taken) and `fresh` (booted on demand).

## Pools and refills

Each connection keeps `pool` booted pages per profile. When a test takes one, the connection boots its replacement in the background:

- as soon as the connection has no open page, or
- after 15 ms without commands on that connection.

Close every page when its test ends (`await using`). An open page keeps its connection busy, and the pool does not refill behind it. Back to back on one connection, the replacement boot shares the browser thread with the next test.

## Connections

```ts
const lp = await Lightningpanda.launch({ connections: 4 });
// or
const lp = await Lightningpanda.connect(wsUrl, { connections: 4 });
```

Each connection is its own browser thread with its own pools. `lp.page()` takes the page from the connection with the fewest open pages (round robin among equals), and a connection refills while tests run on the others. With 4 connections the TOS close flow took 2.7 ms per test back to back, against 7.4 ms on one connection (`bench/results/lp-tos-e501cef`).

Every connection boots `pool` pages of every profile, so memory grows with `connections × pool × profiles`. `lp.profile()` registers the profile on every connection.

In fork mode `connections` and `pool` make no difference: every page gets a connection of its own.

## Memory

Every booted page is a full copy of the app. A pool of N holds N booted pages plus the one in use, per connection and per profile. Two ways to cap it:

- **`--prepare-refill after-close`**: the replacement boots only once the taken page's context closes, so a connection never holds more than `pool` copies. Pass it to the server: `Lightningpanda.launch({ args: ['--prepare-refill', 'after-close'] })`. It applies to every profile; the SDK has no per-profile option for it.
- **Fork mode**: the profile boots once in a template process, and each page is a copy-on-write fork that exits when the page closes. See [Fork mode in tests](/e2e/fork).

## Setup steps and base profiles

In fork mode, a profile can run steps once on its booted page, and every page of it starts from their result:

```ts
const lp = await Lightningpanda.launch({ args: ['--fork'] });
await lp.profile('signed-out', { url: `${APP}/` });
await lp.profile('app', {
  base: 'signed-out',
  url: `${APP}/`,
  setup: [label('Email').fill('ada@example.com'), button('Sign in').click(), check(text('Signed in')).visible()],
});
```

`base` forks the new profile's page from the base profile's snapshot, and only the new profile's own `setup` runs on it. Chains work: boot the login page once, sign in once, then one snapshot per test file on top. A `base` the client does not know fails with `no prepared profile <name> to start from`. Details and the cost model are on [Fork mode in tests](/e2e/fork).

## warm()

```ts
await lp.warm('employee');          // default timeout 5,000 ms
await lp.warm('employee', 30_000);
```

Waits until every connection's pool of the profile is fully booted (in fork mode: until the profile's template page is ready). It throws `no booted page for <name> after <ms> ms` on timeout. Use it before timing tests, or in a launcher before workers start. Tests do not need it: `lp.page()` takes a page that is still booting and waits for it in the browser.

## releaseProfile() and redeclaring

`await lp.releaseProfile(name)` drops the profile: the server unregisters it and retires its waiting pages. Pages already taken stay independent until they close. In fork mode with several clients, it drops only this client's hold (see [One server, many workers](/e2e/shared-server)).

Declaring the same name again with a different definition replaces it.

## Recipes

**One profile per kind of user.**

```ts
await lp.profile('admin', { url: `${APP}/admin`, localStorage: adminTokens });
await lp.profile('employee', { url: `${APP}/platform`, localStorage: employeeTokens });
```

**A page per test, seeded through the API.**

```ts
test.concurrent('shows the order', async () => {
  const order = await api.createOrder();
  await using page = await lp.page({ as: 'employee', route: `/orders/${order.id}` });
  await page.run([check(heading(`Order ${order.number}`)).visible()]);
});
```

**A test that needs extra init code.**

```ts
await using page = await lp.page({ as: 'employee', initScripts: ['window.__TEST_CLOCK__ = 0'] });
```

This page boots fresh, so it costs a full app boot. Prefer a second profile when many tests need the same script.
