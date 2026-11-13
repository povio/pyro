# Fork mode in tests

With `pyro serve --fork`, one template process boots each profile once, and every connection is served by a `fork()` of it. A test's page therefore starts with the app booted and signed in, shares memory with the template copy-on-write, and its process exits when the page closes. The SDK detects fork mode by itself and opens one connection per page. This page shows how to use it from tests and what it costs; [Connections and fork mode](/guide/fork-mode) explains the mechanism.

## Turning it on

```ts
const pyro = await Pyro.launch({ args: ['--fork'] });
```

Or start the server yourself and connect:

```sh
bunx pyro serve --fork --port 9222
```

```ts
const pyro = await Pyro.connect('ws://127.0.0.1:9222/');
```

`--fork` can also be set with `PYRO_FORK=1`. [`pyro e2e`](/e2e/runner) adds it for you.

### Which builds support it

| Platform | Fork mode |
|---|---|
| macOS, Apple silicon | Yes (bundled build with patched JavaScriptCore) |
| macOS, Intel | No: it uses the system JavaScriptCore |
| Linux x64 and arm64 | Yes (bundled patched JavaScriptCore) |

`pyro version` prints the engine capabilities; a fork-capable build shows `fork: true`:

```sh
$ bunx pyro version
pyro X.Y.Z
engine: JavaScriptCore; capabilities: Capabilities { ..., fork: true, ... }
```

On the Intel build, `pyro e2e` warns `has no fork mode; every page boots its profile` and runs without it.

## Auto-detection

After connecting, the SDK sends `Pyro.capabilities`. When the server answers `{ fork: true }`:

- `pyro.profile()` registers the profile once, in the template, instead of on every connection.
- `pyro.page()` opens a **new WebSocket connection** for the page. The server forks it from the template with the profile's page already booted, and the page attaches in the same call.
- `page.close()` closes that connection, which ends the forked process.
- The `connections` and `pool` options make no difference.

Nothing in the test changes. The same test file runs against a forking and a non-forking server.

## A test against a forking server

The app is [the example app](/e2e/first-test#the-example-app): its sign-in screen has an `Email` field and a `Sign in` button that store `tok-<email>` as `token` in `localStorage`, reload, and show `Signed in`.

```ts
// fork.test.ts
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { Pyro, button, check, evaluate, label, text } from '@povio/pyro';
import { startServer } from './server';

const server = startServer();
const APP = server.url.origin;
let pyro: Pyro;

beforeAll(async () => {
  pyro = await Pyro.launch({ args: ['--fork'] });
  // Boot the sign-in screen once.
  await pyro.profile('signed-out', { url: `${APP}/` });
  // Sign in once, on a fork of that page.
  await pyro.profile('app', {
    base: 'signed-out',
    url: `${APP}/`,
    setup: [label('Email').fill('ada@example.com'), button('Sign in').click(), check(text('Signed in')).visible()],
  });
});

afterAll(() => {
  pyro?.close();
  server.stop(true);
});

test.concurrent('starts signed in', async () => {
  await using page = await pyro.page({ as: 'app' });
  const r = await page.run([evaluate('localStorage.getItem("token")', 'token')]);
  expect(r.values.token).toBe('tok-ada@example.com');
});
```

Each page of `app` starts from the state the sign-in left: the token in `localStorage`, the app rendered. The setup steps ran once, not once per test.

## Setup steps

`setup` is a list of steps, the same builders a run takes. They run once in the template after the profile's page loads, and every connection for that profile starts from the state they leave. Use them for work every test of a profile would otherwise repeat:

- signing in through the UI (cookie sessions, OAuth redirects that end on your app)
- opening a view, selecting a tenant, dismissing a cookie banner
- filling filters that a group of tests shares

`pyro.profile()` resolves once the setup ran. When a step fails it rejects with a `ProfileSetupError` (exported by `@povio/pyro`) that names the profile, the step and the reason:

```text
profile app: setup step 0 (expect Visible {"text":"missing"}): timed out after 300 ms: expect Visible {"text":"missing"}: expected Visible null, got false
```

The error has `.profile` (the name) and `.step` (the index of the failed step in this profile's `setup`, from 0; `undefined` when the base profile failed instead). No page of a failed profile is handed out: `pyro.page({ as })` and `pyro.warm()` throw the same error, and profiles with `base` on it fail with `profile <name>: base profile <base> failed: ...`. Declaring the profile again runs the setup again. The server also prints the failure on its standard error, as `pyro: template: profile 2: setup step 0 (...): ...`.

With `pyro.profile()` in a `beforeAll`, a failed setup fails every test of that scope with the step's reason. End a setup with a check of the state it should reach (as `check(text('Signed in')).visible()` above), so that a sign-in that did not work is a setup failure and not a later, less obvious one.

In releases up to 0.0.33 a failed setup does not make `pyro.profile()` throw. The server prints `pyro: template: setup of profile 2: step 0 (...)` on its standard error and pages of the profile still open, from wherever the setup stopped. There, keep the server's stderr visible in CI and look there first when every test of one profile fails.

## Base chains

`base: 'other'` starts a profile from another profile's snapshot. Its page is forked from that profile's page, and only its own `setup` runs on it. Chains are how a suite pays for each piece of work once:

```ts
await pyro.profile('signed-out', { url: `${APP}/` });                                    // boot once
await pyro.profile('app', { base: 'signed-out', url: `${APP}/`, setup: signIn });          // sign in once
await pyro.profile('todos-file', { base: 'app', url: `${APP}/`, setup: [label('New todo').fill('draft')] });  // per file
```

A typical layout is one root profile for the app, one signed-in profile per user role, and one snapshot per test file or `describe` with that file's shared setup. Release per-file profiles when their tests are done (`pyro.releaseProfile(name)`, or `defer` in [`pyro e2e`](/e2e/runner)), so their template processes exit.

A derived profile's page is the base's page as it was after the base's setup: it does not navigate to its own `url`. Pass the same `url` as the base (the option is still required), and move to another route in the derived `setup` or with `pyro.page({ as, route })`.

Registering a profile whose base is still booting waits for the base (up to 90 s), and so do connections for it.

## What tests still share

Fork mode copies the browser, not your backend. Every page of a profile starts from the same snapshot, so it carries the same session token, and every test talks to the same API and database. Tests stay independent when:

- **Each test creates its own data** through the API, with names no other test uses, and removes it afterwards. See [API seeding and cleanup](/recipes/e2e-api-seeding-and-cleanup).
- **No test ends the shared session.** A test that signs out of a profile's page revokes the token on the server for every page of that profile, including pages of other tests and the setup of profiles based on it. Sign in inside the test instead, on a page of the signed-out profile. [Signing in once](/recipes/e2e-login-once#tests-that-sign-out) shows the failure and the fix.
- **No test changes what a snapshot shows for others.** A snapshot holds the data the app fetched when it was taken. Data a test creates later appears only after the app fetches again, so take the page with `route` (or `url`) to make it fetch.
- **Tests that must share server state are serial.** `test.serial` and `describe.serial` in [`pyro e2e`](/e2e/runner#serial-tests).

## Cost model

| Cost | When | Size |
|---|---|---|
| Profile boot | Once per profile, in the template | Your app's load time |
| `setup` | Once per profile | Your steps' time |
| Fork | Per page | The template keeps `PYRO_FORK_SPARES` forks waiting (default 2), so a page rarely waits for one. A template `fork()` measured 3.4 ms median on ToS (PR #291) |
| Page open | Per page | About 20 ms from `pyro.page()` to a ready page, however long the app takes to boot (README) |
| The test | Per page | Your app's JavaScript and layout. On ToS, about 0.57 s of user CPU per page (PR #291) |
| Close | Per page | The process exits without tearing the page down |

In a 10-page loop on that sign-in app, `pyro.page()` plus a two-step run took 1.2 to 1.6 ms per page on an idle Apple silicon machine, and between 3.5 and 19 ms in an earlier run on a busy M1 Max.

What fork mode buys:

- **No boot per test** and no pool to refill: the app boots once per profile, not once per page or per connection.
- **Isolation in the browser**: each page is its own process. What a test does to its page (DOM, `localStorage`, cookies, mocks, timers) does not reach another test. Your backend is not isolated: see [What tests still share](#what-tests-still-share).
- **Memory**: pages share the template's memory until they write to it.

What it costs:

- **A process per page.** Concurrency is bounded by CPU, not by browser threads. Size `-j` to your cores (see [CI recipes](/e2e/ci#concurrency)).
- **One connection per page.** The server's `--cdp-max-connections` (default 16) caps open pages. Raise it when more tests run at once: `args: ['--fork', '--cdp-max-connections', '64']`. `pyro e2e` sets it to at least 64.
- **JavaScriptCore's optimizing tiers are off in forks.** Forked pages run without the DFG and FTL JITs, because connections are too short for them to pay off. Set `JSC_useDFGJIT=true` in the server's environment to override.
- **Templates run the app.** Every template and spare process runs your app's timers and polling while it waits, so many profiles cost CPU even when idle. Release the ones you no longer need.

## Platform limits

- Fork mode needs the patched JavaScriptCore: macOS on Apple silicon and Linux. The macOS Intel build has no fork mode.
- On macOS, `fork()` copies JavaScriptCore's executable-memory reservation. The template uses a 32 MB JIT pool to keep forks cheap (a fork took 15 ms of the template's time with 512 MB, 3.5 ms with 32 MB).
- Fork mode is for one host: the template and its forks are local processes. Remote clients connect over WebSocket as usual.

## Several processes on one forking server

A forking server keeps profiles by name for all of its clients, so `bun test --parallel` workers can share profiles that one launcher booted. See [One server, many workers](/e2e/shared-server). The simpler route for large suites is [`pyro e2e`](/e2e/runner): one process, one forking browser, one queue.

## Next

- [Signing in once](/recipes/e2e-login-once): a sign-in profile per role, one snapshot per feature, and the independence rules above in a running suite.
- [Suite runner: pyro e2e](/e2e/runner): fork mode is its default.
