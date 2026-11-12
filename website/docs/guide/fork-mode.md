# Connections and fork mode

By default a Pyro server runs one browser thread per CDP connection, and every connection boots its own pages. **Fork mode** (`serve --fork`) changes that: a template process boots each profile once, and every connection is served by a `fork()` of it. A test then starts with the app already booted, in about 20 ms however long the app takes to boot, and shares nothing with other tests.

## Thread mode: the default

```sh
pyro serve --port 9222
```

- Each connection is a browser thread in the server process, with its own JavaScriptCore engine.
- Profiles are per connection. Each connection boots its own pool of pages ([Profiles](/guide/profiles)).
- The native SDK can open several connections (`connect(ws, { connections: 4 })`) and spread tests over them.
- Up to 16 connections at once by default (`--cdp-max-connections`). Past that, new connections wait in the kernel's backlog until one closes.

Thread mode works on every platform, including Intel Macs.

## Fork mode

```sh
pyro serve --fork --prepare http://localhost:3000/
# or: PYRO_FORK=1 pyro serve ...
```

What happens:

1. At startup the server forks a **template process**. The server listens for connections while the template starts.
2. The template boots one page per profile: every `--prepare` URL and every profile a client registers with `Pyro.registerPreparedPage` (which `pyro.profile()` sends). It lets each page settle.
3. When a connection arrives, the server passes its socket to the template. Once the booted pages are loaded and quiet, the template calls `fork()`. The child process serves that one connection, with every profile's page already booted.
4. The child shares memory with the template copy-on-write: pages are copied only when the child writes to them.
5. When the connection closes, the child process exits. Nothing it did survives.

### Spares

Forking takes a few milliseconds, so the template keeps forked children waiting, with their pages running. A new connection is accepted directly by a waiting spare, and the template forks a replacement. The default is 2 spares per template; set `PYRO_FORK_SPARES` to change it. Registering a new profile discards the spares, since their pages are out of date.

### What a forked page costs

Measured on an M1 Max with a release build (`docs/ARCHITECTURE.md`):

| Measurement | Result |
| --- | --- |
| App whose startup runs 0.8 s of JavaScript, handed to a new connection, booted | about 20 ms |
| Connect to an evaluated expression in a booted page, with a spare waiting (8 MB test app) | 3.6 ms |

The cost is independent of how long your app takes to boot, because the boot happened once in the template. Linux browsers are fork-capable too, but no Linux numbers have been published.

### No optimizing JIT in forks

Forked children run without JavaScriptCore's DFG and FTL optimizing tiers. A connection lives for one test, too short for those tiers to repay their compile time. Interpreted and baseline-JIT code still run. If a test does heavy computation in page JavaScript, set `JSC_useDFGJIT` explicitly to keep the tiers on.

When the server serves an app with `--static`, the template also compiles the app's JavaScript modules once before its first page. Forked pages that load the same module text reuse the result without parsing it. `PYRO_FORK_PRECOMPILE=off` turns this off.

## Using fork mode from the SDK

The native SDK detects fork mode by itself (the server reports `Pyro.capabilities {fork: true}`). Each `pyro.page()` then opens its own connection, which is a fork:

```ts
import { Pyro, button, check, text } from '@povio/pyro';

const pyro = await Pyro.launch({ args: ['--fork'] });
await pyro.profile('employee', { url: 'http://localhost:3000/platform', localStorage: { token } });

await using page = await pyro.page({ as: 'employee', route: '/positions/42' });
await page.run([button('Settings').click(), check(text('Closed')).visible()]);
// closing the page closes its connection; the forked process exits
```

In fork mode `connections` and `pool` make no difference: every page is its own process, and the template keeps one page per profile. Raise `--cdp-max-connections` above the number of pages you run at once; `pyro e2e` sets it for you.

[Fork mode in tests](/e2e/fork) and [Profiles booted once (fork)](/scraping/fork-profiles) have complete examples.

## Setup and snapshots

Fork mode adds two profile fields that only make sense when a page can be copied.

**`setup`** runs steps once on the booted page in the template, before any test gets it. Every fork starts from the state the steps left. Use it for a sign-in flow, opening a view, or filling filters:

```ts
import { button, check, label, text } from '@povio/pyro';

await pyro.profile('signed-in', {
  url: 'http://localhost:3000/login',
  setup: [
    label('Email').fill('ada@example.com'),
    label('Password').fill('secret'),
    button('Sign in').click(),
    check(text('Dashboard')).visible(),
  ],
});
```

**`base`** builds a profile on top of another one's snapshot. Its page is forked from the base profile's page and only its own `setup` runs:

```ts
import { check, link, text } from '@povio/pyro';

await pyro.profile('positions', {
  base: 'signed-in',            // start from the signed-in snapshot
  url: 'http://localhost:3000/positions',
  setup: [link('Positions').click(), check(text('Open positions')).visible()],
});
```

This builds a tree: boot on the login page once, sign in once, then one snapshot per area of the app. Each derived snapshot lives in a derived template process, which serves forks for it.

## Shared profiles

A fork-mode server keeps profiles by **name** for all its clients. This lets one process launch the server and register profiles, and many worker processes use them without booting anything:

```ts
// launcher: the process that starts the test workers
const pyro = await Pyro.launch({ args: ['--fork', '--cdp-max-connections', '64'] });
await pyro.profile('app', { url: 'http://localhost:3000/', setup: signIn });
await pyro.warm('app');
// hand pyro.wsEndpoint to the workers, for example as PYRO_WS

// each worker
const pyro = await Pyro.connect(process.env.PYRO_WS!);
await using page = await pyro.page({ as: 'app' }); // adopts the launcher's profile
```

The rules:

- The same definition under the same name finds the profile already booted. A different definition is refused while another client holds the name, so give per-worker profiles unique names.
- `pyro.adopt(name)` holds a profile explicitly; `pyro.page({ as })` and `base: name` adopt a name they do not know.
- A client holds what it registered or adopted until `pyro.releaseProfile(name)` or until it disconnects.
- The server retires a profile once no client holds it and no other profile is built on it.

See [One server, many workers](/e2e/shared-server).

## Platform availability

Fork mode needs the patched JavaScriptCore that ships with some builds:

| Bundled browser | `version` reports | Fork mode |
| --- | --- | --- |
| macOS, Apple silicon | `fork: true` | yes |
| macOS, Intel | `fork: false` | no (uses the system JavaScriptCore) |
| Linux x64 and arm64 | `fork: true` | yes |

On a browser without it, `serve --fork` exits with `--fork needs the patched JavaScriptCore`. `pyro e2e` checks first and falls back to thread mode with a warning, so every page then boots its profile.

## When not to use fork mode

- **On Intel Macs.** It is not available.
- **When tests should share state in one browser.** Every connection is a separate process; cookies, storage and page state do not carry from one test to the next. That is usually what you want for tests, and the opposite of what a long-running scraper session needs.
- **When a single page runs heavy, long JavaScript.** Forks have no optimizing JIT. Use thread mode, or set `JSC_useDFGJIT`.
- **For one long-lived page.** Fork mode pays off when you open many short-lived pages of the same app. A script that opens one page and keeps it gains nothing.

## Inspecting forks

`PYRO_FORK_STATS=<file>` writes one JSON line per fork event: each fork, each child's CPU time, page faults and instructions at each step of its life, and each child that exits. Use it to see what a forked test costs. See [Environment variables](/reference/env).
