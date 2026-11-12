# Fork mode in tests

With `lightningpanda serve --fork`, one template process boots each profile once, and every connection is served by a `fork()` of it. A test's page therefore starts with the app booted and signed in, shares memory with the template copy-on-write, and its process exits when the page closes. The SDK detects fork mode by itself and opens one connection per page. This page shows how to use it from tests and what it costs; [Connections and fork mode](/guide/fork-mode) explains the mechanism.

## Turning it on

```ts
const lp = await Lightningpanda.launch({ args: ['--fork'] });
```

Or start the server yourself and connect:

```sh
bunx lightningpanda serve --fork --port 9222
```

```ts
const lp = await Lightningpanda.connect('ws://127.0.0.1:9222/');
```

`--fork` can also be set with `LIGHTNINGPANDA_FORK=1`. [`lightningpanda e2e`](/e2e/runner) adds it for you.

### Which builds support it

| Platform | Fork mode |
|---|---|
| macOS, Apple silicon | Yes (bundled build with patched JavaScriptCore) |
| macOS, Intel | No: it uses the system JavaScriptCore |
| Linux x64 and arm64 | Yes (bundled patched JavaScriptCore) |

`lightningpanda version` prints the engine capabilities; a fork-capable build shows `fork: true`:

```sh
$ bunx lightningpanda version
lightningpanda 0.0.33
engine: JavaScriptCore; capabilities: Capabilities { ..., fork: true, ... }
```

On the Intel build, `lightningpanda e2e` warns `has no fork mode; every page boots its profile` and runs without it.

## Auto-detection

After connecting, the SDK sends `Lightningpanda.capabilities`. When the server answers `{ fork: true }`:

- `lp.profile()` registers the profile once, in the template, instead of on every connection.
- `lp.page()` opens a **new WebSocket connection** for the page. The server forks it from the template with the profile's page already booted, and the page attaches in the same call.
- `page.close()` closes that connection, which ends the forked process.
- The `connections` and `pool` options make no difference.

Nothing in the test changes. The same test file runs against a forking and a non-forking server.

## A test against a forking server

```ts
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { Lightningpanda, button, check, evaluate, label, text } from '@povio/lightning-panda';

let lp: Lightningpanda;

beforeAll(async () => {
  lp = await Lightningpanda.launch({ args: ['--fork'] });
  // Boot the sign-in screen once.
  await lp.profile('signed-out', { url: `${APP}/` });
  // Sign in once, on a fork of that page.
  await lp.profile('app', {
    base: 'signed-out',
    url: `${APP}/`,
    setup: [label('Email').fill('ada@example.com'), button('Sign in').click(), check(text('Signed in')).visible()],
  });
});

afterAll(() => lp?.close());

test.concurrent('starts signed in', async () => {
  await using page = await lp.page({ as: 'app' });
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

A setup that fails does **not** make `lp.profile()` throw. The setup runs in the template after registration, and the server prints the failure on its standard error:

```text
lightningpanda: template: setup of profile 2: step 0 (expect Visible {"text":"missing"}): timed out after 300 ms: expect Visible {"text":"missing"}: expected Visible null, got false
```

Pages of that profile still open, from wherever the setup stopped, so the tests fail later with less obvious messages. End a setup with a check of the state it should reach (as `check(text('Signed in')).visible()` above), keep the server's stderr visible in CI, and look there first when every test of one profile fails.

## Base chains

`base: 'other'` starts a profile from another profile's snapshot. Its page is forked from that profile's page, and only its own `setup` runs on it. Chains are how a suite pays for each piece of work once:

```ts
await lp.profile('signed-out', { url: `${APP}/` });                                    // boot once
await lp.profile('app', { base: 'signed-out', url: `${APP}/`, setup: signIn });          // sign in once
await lp.profile('todos-file', { base: 'app', url: `${APP}/`, setup: [label('New todo').fill('draft')] });  // per file
```

A typical layout is one root profile for the app, one signed-in profile per user role, and one snapshot per test file or `describe` with that file's shared setup. Release per-file profiles when their tests are done (`lp.releaseProfile(name)`, or `defer` in [`lightningpanda e2e`](/e2e/runner)), so their template processes exit.

A derived profile's page is the base's page as it was after the base's setup: it does not navigate to its own `url`. Pass the same `url` as the base (the option is still required), and move to another route in the derived `setup` or with `lp.page({ as, route })`.

Registering a profile whose base is still booting waits for the base (up to 90 s), and so do connections for it.

## Cost model

| Cost | When | Size |
|---|---|---|
| Profile boot | Once per profile, in the template | Your app's load time |
| `setup` | Once per profile | Your steps' time |
| Fork | Per page | The template keeps `LIGHTNINGPANDA_FORK_SPARES` forks waiting (default 2), so a page rarely waits for one. A template `fork()` measured 3.4 ms median on ToS (PR #291) |
| Page open | Per page | About 20 ms from `lp.page()` to a ready page, however long the app takes to boot (README) |
| The test | Per page | Your app's JavaScript and layout. On ToS, about 0.57 s of user CPU per page (PR #291) |
| Close | Per page | The process exits without tearing the page down |

In a 10-page loop on the small todo app from [Your first test](/e2e/first-test), on a busy M1 Max, `lp.page()` plus a two-step run took between 3.5 and 19 ms per page.

What fork mode buys:

- **No boot per test** and no pool to refill: the app boots once per profile, not once per page or per connection.
- **Isolation**: each page is its own process. Nothing a test does reaches another test.
- **Memory**: pages share the template's memory until they write to it.

What it costs:

- **A process per page.** Concurrency is bounded by CPU, not by browser threads. Size `-j` to your cores (see [CI recipes](/e2e/ci#concurrency)).
- **One connection per page.** The server's `--cdp-max-connections` (default 16) caps open pages. Raise it when more tests run at once: `args: ['--fork', '--cdp-max-connections', '64']`. `lightningpanda e2e` sets it to at least 64.
- **JavaScriptCore's optimizing tiers are off in forks.** Forked pages run without the DFG and FTL JITs, because connections are too short for them to pay off. Set `JSC_useDFGJIT` in the server's environment to override.
- **Templates run the app.** Every template and spare process runs your app's timers and polling while it waits, so many profiles cost CPU even when idle. Release the ones you no longer need.

## Platform limits

- Fork mode needs the patched JavaScriptCore: macOS on Apple silicon and Linux. The macOS Intel build has no fork mode.
- On macOS, `fork()` copies JavaScriptCore's executable-memory reservation. The template uses a 32 MB JIT pool to keep forks cheap (it took 14 ms per fork with 512 MB).
- Fork mode is for one host: the template and its forks are local processes. Remote clients connect over WebSocket as usual.

## Several processes on one forking server

A forking server keeps profiles by name for all of its clients, so `bun test --parallel` workers can share profiles that one launcher booted. See [One server, many workers](/e2e/shared-server). The simpler route for large suites is [`lightningpanda e2e`](/e2e/runner): one process, one forking browser, one queue.
