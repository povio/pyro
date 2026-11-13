# SDK: Pyro and Page

The TypeScript API of `@povio/pyro`: launching or connecting to a browser, declaring profiles, opening pages, and running steps in one `Pyro.run` round trip. It runs on Bun and on Node 22 or later, using the runtime's global `WebSocket` and `fetch`. Locators and step builders are documented in [SDK: locators and steps](/reference/steps).

```ts
import { Pyro, button, check, dialog } from '@povio/pyro';

const pyro = await Pyro.launch();
await pyro.profile('employee', { url: 'http://localhost:5173/platform', localStorage: { token } });
await using page = await pyro.page({ as: 'employee', route: `/positions/${id}` });
const r = await page.run([
  button('Settings').click(),
  dialog().hasText('will be closed').button('Close Position').click(),
  check.request('POST', `/positions/${id}/close`).status(200),
  check(button('Reopen Position')).count(1),
]);
pyro.close();
```

## Entry points {#entry-points}

| Import | Contents | Documented in |
| --- | --- | --- |
| `@povio/pyro` | `Pyro`, `Page`, `RunError`, types, every step and locator builder, `Cdp`, `CdpError`, screenshot helpers | this page, [steps](/reference/steps) |
| `@povio/pyro/e2e` | Spec-file API for `pyro e2e` | [below](#e2e) |
| `@povio/pyro/wright` | Playwright-compatible API (`chromium`, `Browser`, `Page`, `Locator`, `expect`, `test`, ...) | [Coming from Playwright](/playwright/overview) |
| `@povio/pyro/test` | `@playwright/test` replacement (`test`, `expect`, config helpers) | [Config and the test runner](/playwright/runner) |

Under Bun the package's `bun` export condition loads the TypeScript sources directly. Other runtimes load the built `dist/*.js`.

## Pyro {#pyro}

A client of one `pyro serve`: one or more CDP connections, the profiles this client declared, and, when it launched the server, the server process.

### Pyro.launch() {#launch}

```ts
static launch(opts?: ConnectOptions & { bin?: string; args?: string[] }): Promise<Pyro>
```

Starts `pyro serve --host 127.0.0.1 --port <free port> ...args` and connects to it.

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `bin` | `string` | `$PYRO_BIN`, else the bundled binary | Browser binary. |
| `args` | `string[]` | `[]` | Extra [serve options](/reference/serve), for example `['--fork']`, `['--static', 'http://app.test=./dist']`, `['--hyphen-digit-breaks', 'on']`. |
| `connections` | `number` | `1` | See [`ConnectOptions`](#connect-options). |

- The server inherits the environment plus `PYRO_PARENT_PID=<this pid>`, so it exits with this process however that ends ([`--parent-pid`](/reference/serve#parent-pid)).
- It runs in its own process group (on POSIX). Its stdout is discarded and its stderr is inherited.
- `launch()` polls `GET /json/version` every 5 ms for up to 15 s. If the process exits or the deadline passes, it throws `pyro serve did not start: ...` and kills the process group.
- After connecting it sends `Pyro.capabilities` to learn whether the server forks connections (`--fork`) and shares profiles.

```ts
const pyro = await Pyro.launch({ args: ['--fork', '--static', 'http://app.test=./dist'] });
```

### Pyro.connect() {#connect}

```ts
static connect(wsUrl: string, opts?: ConnectOptions): Promise<Pyro>
```

Connects to a running server (for example another process's `pyro.wsEndpoint`). `close()` then closes only the connections. A server that does not know `Pyro.capabilities` (an older one) is treated as having no fork mode.

```ts
const pyro = await Pyro.connect(process.env.PYRO_WS!, { connections: 4 });
```

### ConnectOptions {#connect-options}

```ts
type ConnectOptions = {
  /** CDP connections to open (default 1). */
  connections?: number;
};
```

Each connection is a browser thread of its own with its own page pools. `pyro.page()` picks the connection with the fewest open pages (round-robin among equals). A connection refills its pools as soon as it has no open page, while tests run on the others. Against `serve --fork` every page opens a connection of its own anyway, and the extra connections only carry browser-level commands.

### Properties {#properties}

| Property | Type | Description |
| --- | --- | --- |
| `wsEndpoint` | `string` (getter) | The server's CDP endpoint. Hand it to other processes so they `connect()` to the same server and share its profiles (`--fork`). |
| `cdp` | [`Cdp`](#cdp) (getter) | The first connection, for raw browser-level commands. |
| `stats` | `{ prepared: number; preparedBooting: number; fresh: number }` | Counters: pages taken from a pool, of those the ones still booting when taken, and pages booted on demand because none was waiting. |

### pyro.profile() {#profile}

```ts
profile(name: string, profile: Profile): Promise<void>
```

Declares a kind of page that tests ask for by name (`pyro.page({ as: name })`). The server keeps pages of it booted ahead (`Pyro.registerPreparedPage` on every connection, or once in the template with `--fork`).

- `localStorage` entries become the first init script, followed by `initScripts`.
- Declaring a name again with a different definition replaces this client's own.
- Against `serve --fork` the name is shared by every client of the server. If another client registered the same definition under the name, this client adopts it and nothing boots again. A different definition is refused while another client holds the name.
- `base` must name a profile this client knows or can adopt, else it throws `no prepared profile <base> to start from`.

Calls to `profile`, `adopt` and `releaseProfile` run one after another, in call order.

### Profile {#profile-type}

```ts
type Profile = {
  url: string;
  localStorage?: Record<string, string>;
  initScripts?: string[];
  viewport?: { width: number; height: number };
  pool?: number;
  prepare?: boolean;
  setup?: StepLike[];
  base?: string;
};
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `url` | `string` | required | Where its pages boot (and stay until a test takes one). |
| `localStorage` | `Record<string, string>` | | Written to `localStorage` before the app's scripts run (for example auth tokens). Applied by an init script, so it applies to every origin the page visits. |
| `initScripts` | `string[]` | | Scripts run in every document before its own scripts. |
| `viewport` | `{ width, height }` | server default (1920×1080) | CSS px. |
| `pool` | `number` | `2` | Pages kept booted per connection (1-8). Ignored with `--fork`: the template keeps one snapshot per profile. |
| `prepare` | `boolean` | `true` | `false`: nothing is booted ahead; each `page({ as })` boots a fresh page and navigates to `url`. |
| `setup` | [`StepLike[]`](/reference/steps) | | `serve --fork` only: steps run once on the booted page in the template. Every page of the profile starts from their result (for example a signed-in state). |
| `base` | `string` | | `serve --fork` only: start from another profile's snapshot. Its page forks from that profile's, and only this profile's `setup` runs on it. The base may be a profile another client of the server registered. |

### pyro.adopt() {#adopt}

```ts
adopt(name: string): Promise<void>
```

Holds the profile another client of this server registered as `name` (`serve --fork`), so `page({ as: name })` and `base: name` work here too. Nothing boots: pages fork from the server's snapshot. `page()` and `profile()` adopt unknown names by themselves. Without `--fork` it throws `no profile <name>: adopting a profile needs pyro serve --fork`. If the server has no such profile it throws `no profile <name> on this server: declare it with pyro.profile()`.

### pyro.releaseProfile() {#release-profile}

```ts
releaseProfile(name: string): Promise<void>
```

Releases one name. Against `serve --fork` this drops this client's hold, and the server retires the profile once no client holds it and no other profile derives from it. Otherwise the connections' prepared pages are unregistered, except where another of this client's names (an alias of the same definition) still uses them. Pages already taken stay open. Unknown names are ignored.

### pyro.warm() {#warm}

```ts
warm(name: string, timeoutMs = 5000): Promise<void>
```

Waits until profile `name` is fully booted. Without `--fork` it waits for each connection's pool to have `pool` ready pages. With `--fork` it waits until a new connection, forked from the template, finds the profile's page ready. Throws `no booted page for <name> after <ms> ms` past the timeout. Does nothing for an unknown name or a `prepare: false` profile.

### pyro.page() {#page}

```ts
page(opts?: { as?: string; route?: string; url?: string; initScripts?: string[] }): Promise<Page>
```

| Option | Description |
| --- | --- |
| `as` | Profile name. A page is taken from its pool (`Pyro.acquirePreparedPage`, attached in the same call). Without a ready one it boots a fresh page in a new context, with the profile's scripts and viewport, and navigates to the profile's `url`. Without `as`: a blank page in a new browser context. |
| `route` | Same-document navigation, run as the first step of the first `run()` (a [`route`](/reference/steps#route) step). |
| `url` | Full navigation, run as the first step of the first `run()` (a [`goto`](/reference/steps#goto) step). Replaces the navigation to the profile `url` of a fresh page. |
| `initScripts` | Run in every document of this page after the profile's scripts. A booted page already ran its document, so with `initScripts` the page always boots fresh instead of coming from the pool. |

A pooled page that is still booting when taken gets a [`loaded()`](/reference/steps#loaded) step first. With `--fork` the page gets a connection of its own (`ws://…/?profile=<id>`), forked from the profile's snapshot. Closing the page ends that process. An unknown `as` throws `no profile <name>: declare it with pyro.profile()`.

### pyro.close() {#close}

```ts
close(): void
```

Closes every connection. When this client launched the server, it also sends `SIGTERM` to the server's process group (the server and its forks). Pages opened with `--fork` have their own connections, and closing those pages closes them.

## Page {#page-class}

One page, attached over flat CDP sessions. Created by `pyro.page()`.

| Member | Type | Description |
| --- | --- | --- |
| `sessionId` | `string` | CDP session of the page. |
| `targetId` | `string` | Target id (its main frame id). |
| `browserContextId` | `string` | The page's own browser context. |
| `prefix` | `StepLike[]` | Steps the next `run()` starts with (the page's `route`/`url`, `loaded()`). Cleared by that run. |

### page.run() {#run}

```ts
run(steps: StepLike[], opts?: { timeout?: number }): Promise<RunResult>
```

Sends every step to the browser as one [`Pyro.run`](/reference/pyro-run) command. The browser runs the steps in order, waiting in place, and replies once. `timeout` is the per-step timeout in ms for steps without their own (server default 5000, maximum 600000).

- Throws [`RunError`](#run-error) at the first failing step.
- Throws [`ScreenshotMismatchError`](#screenshot-mismatch-error) for a `matchScreenshot` step that does not match. The `.actual.png` and `.diff.png` files are written before it throws.
- A `matchScreenshot` step with no baseline (or in update mode) writes the baseline. Outside update mode it then throws `ScreenshotMismatchError` with the message `the baseline did not exist; wrote <path>` (or `was replaced`).
- Other failures (a closed connection, a CDP protocol error not tied to a step) are thrown unchanged.

### RunResult {#run-result}

```ts
type RunResult = {
  values: Record<string, unknown>;
  screenshots: Record<string, ScreenshotResult>;
  results: unknown[];
  requests: { method: string; url: string; status: number | null; error: string | null; ms: number | null }[];
  ms: number;
};
```

| Field | Description |
| --- | --- |
| `values` | Keyed by `as`: values of `read`, `evaluate`, `download`, `downloadCount` and `popup` steps (the wire `reads`). |
| `screenshots` | `matchScreenshot` results by step name. |
| `results` | One entry per step you passed (prefix steps removed): what a check saw (`{ actual }`), an action's `{ settleMs }`, a matched request, and so on. See [step results](/reference/pyro-run#step-results). |
| `requests` | Requests the page made during the run. The wire objects also carry `requestHeaders`, `responseHeaders` and `postData`. See [request log](/reference/pyro-run#request-log). |
| `ms` | Time the run took in the browser. |

```ts
const r = await page.run([css('h1').read('h'), evaluate('1 + 1', 'two')]);
r.values; // { h: 'Hi', two: 2 }
r.results; // ['Hi', 2]
```

### RunError {#run-error}

```ts
class RunError extends Error {
  readonly step: number;
}
```

`step` is the index of the failing step in the array you passed. It is negative when a prefix step (the page's opening `route`, `url` or `loaded()`) failed. The message starts with `step <n> (<what>):` or `opening the page (<step JSON>):`, followed by the browser's reason:

```text
step 0 (Click {"name":"Nope","role":"button"}): timed out after 100 ms: Click {"name":"Nope","role":"button"}: {"name":"Nope","role":"button"} matched no element
```

### page.matchScreenshot() {#match-screenshot}

```ts
matchScreenshot(name: string, opts?: ScreenshotOptions): Promise<ScreenshotResult>
```

Runs one [`matchScreenshot`](/reference/steps#matchscreenshot) step and returns its result. Throws `ScreenshotMismatchError` on a mismatch. Guide: [Visual tests](/e2e/screenshots).

### page.addInitScript() {#add-init-script}

```ts
addInitScript(source: string): Promise<void>
```

Runs `source` in every document this page loads from now on, before its scripts (`Page.addScriptToEvaluateOnNewDocument`, like Playwright's `page.addInitScript`). The current document is not affected: follow it with `goto` or `reload`.

### page.send() {#send}

```ts
send<T = any>(method: string, params?: object): Promise<T>
```

Any other CDP command on this page's session, for example `page.send('Page.captureScreenshot', { format: 'png' })`. See [CDP coverage](/reference/cdp).

### page.close() and await using {#page-close}

```ts
close(): void
[Symbol.asyncDispose](): Promise<void>
```

Closes the page and its browser context without waiting (`Target.closeTarget` and `Target.disposeBrowserContext`). For a fork-mode page it closes the page's own connection, which ends its process. Calling it again does nothing. `await using page = await pyro.page(...)` closes the page when the scope ends.

## Screenshots {#screenshots}

### ScreenshotOptions {#screenshot-options}

See [`matchScreenshot`](/reference/steps#matchscreenshot). Baselines live in `<dir>/<name>.png` with `<name>.json` (`{ sceneHash, width, height }`). `dir` defaults to `__screenshots__` next to the test file that built the step.

### ScreenshotResult {#screenshot-result}

```ts
type ScreenshotResult = {
  name: string;
  pass: boolean;
  hashHit: boolean;
  recorded: boolean;
  diffPixels: number;
  sceneHash: string;
  width: number;
  height: number;
  ms: Record<string, number>;
  files?: { baseline?: string; actual?: string; diff?: string };
};
```

| Field | Description |
| --- | --- |
| `pass` | Matched (or recorded). |
| `hashHit` | Passed on the display-list hash alone: no raster and no pixel compare. |
| `recorded` | No baseline was compared. The render was written as the baseline. |
| `diffPixels` | Pixels over `threshold`. |
| `sceneHash` | Hex hash of the display list, stored in `<name>.json` to skip the raster next time. |
| `width`, `height` | Image size in px. |
| `ms` | Browser-side timings (`snapshot`, `layout`, `scene`, `raster`, `compare`, `encode`, `total`, ...). |
| `files` | The written baseline (recorded), or `baseline`, `actual` and `diff` on a mismatch. |

### ScreenshotMismatchError {#screenshot-mismatch-error}

```ts
class ScreenshotMismatchError extends Error {
  readonly result: ScreenshotResult;
}
```

The message names the differing pixel count and the `actual:` and `diff:` paths.

### updateMode() {#update-mode}

```ts
function updateMode(): boolean
```

`true` when baselines should be written instead of compared: `PYRO_UPDATE_SNAPSHOTS` is `1`, `true`, `all` or `changed`, or the process was started with `--update-snapshots` or `-u` (`bun test -u`). See [env](/reference/env#pyro-update-snapshots).

### screenshotMatchers {#screenshot-matchers}

A matcher for `expect.extend`:

```ts
import { expect } from 'bun:test';
import { screenshotMatchers } from '@povio/pyro';

expect.extend(screenshotMatchers);
await expect(page).toMatchScreenshot('home', { fullPage: true });
```

`toMatchScreenshot(page, name, opts?)` passes when `page.matchScreenshot` passes, fails with the mismatch message, and rethrows other errors.

## Cdp {#cdp}

The minimal CDP client the SDK uses: flat sessions, one promise per command, event listeners by method. Use it for raw commands through `pyro.cdp` or on its own.

```ts
class Cdp {
  static open(url: string): Promise<Cdp>;
  send<T = any>(method: string, params?: object, sessionId?: string): Promise<T>;
  post(method: string, params?: object, sessionId?: string): void;
  on(method: string, fn: (params: any, sessionId?: string) => void): () => void;
  onClose(fn: (error: Error) => void): void;
  get isClosed(): boolean;
  close(): void;
}
```

| Member | Description |
| --- | --- |
| `open(url)` | Opens a WebSocket. Rejects with `cannot connect to <url>`. |
| `send` | Sends a command and resolves with its `result`. Rejects with `CdpError` on an error reply, or with `CDP connection closed`/`failed` when the socket ends. |
| `post` | `send` without waiting. Failures are ignored (for cleanup). |
| `on` | Calls `fn` for every event `method` on any session. Returns the unsubscribe function. |
| `onClose` | Calls `fn` once when the connection closes or fails (at once if it already has). |
| `isClosed` | Whether the connection has ended. |
| `close` | Closes the socket. |

```ts
const { pages } = await pyro.cdp.send('Pyro.preparedPages');
```

### CdpError {#cdp-error}

```ts
class CdpError extends Error {
  readonly method: string;
  readonly code: number;
  readonly data?: unknown;
}
```

The message is `<method>: <server message>`. `code` is the JSON-RPC error code (`-32000` server error, `-32602` invalid params, `-32601` unknown method). `data` is the error's `data`. A failed `matchScreenshot` carries its images there.

## e2e module {#e2e}

`@povio/pyro/e2e` is the spec-file API of [`pyro e2e`](/reference/cli#e2e). The functions have `bun:test`'s signatures, but tests from every file share the run's queue and its one fork-mode browser. They only work in files that `pyro e2e` loads: elsewhere they throw `... is for spec files run by pyro e2e`. Guide: [Suite runner](/e2e/runner).

```ts
import { browser, describe, expect, test, beforeAll, defer } from '@povio/pyro/e2e';
import { button, check } from '@povio/pyro';

describe('positions', () => {
  beforeAll(async () => {
    const pyro = await browser();
    await pyro.profile('positions', { url: 'http://app.test/positions' });
    defer(() => pyro.releaseProfile('positions'));
  });

  test('closes a position', async () => {
    await using page = await (await browser()).page({ as: 'positions' });
    await page.run([button('Close').click(), check(button('Reopen')).visible()]);
  }, 30_000);
});
```

| Export | Description |
| --- | --- |
| `test(name, fn, timeout?)`, `it` | Registers a test. `timeout` is a number or `{ timeout }`. Without `fn` it is a todo. Every test may run alongside any other unless it is serial. |
| `test.serial` | Runs alone in its describe: after every test declared before it, before any declared after it. |
| `test.concurrent` | Same as `test`. |
| `test.skip`, `test.todo`, `test.only` | `.only` tests run only with `--only`. |
| `test.if(cond)`, `test.skipIf(cond)` | Conditional registration (skipped when the condition fails). |
| `describe(name, fn)` | A group. `fn` must register synchronously (returning a promise throws). |
| `describe.serial` | Its tests run one at a time, in order. |
| `describe.skip`, `.todo`, `.only`, `.if(cond)`, `.skipIf(cond)` | As for `test`. |
| `beforeAll`, `afterAll`, `beforeEach`, `afterEach` | `(fn, timeout?)`. `beforeEach` runs outer hooks first and `afterEach` inner hooks first. At the top level of a file they apply to that file. In a `--preload` module they apply to every test. |
| `setDefaultTimeout(ms)` | Timeout of tests and hooks without their own (default 5000, or `--timeout`). |
| `defer(fn)` | Runs `fn` once what is running now is done. From a test (or its `beforeEach`/`afterEach`): after that test. From a describe's `beforeAll`/`afterAll`, or while it registers: after that describe's last test and its `afterAll` hooks. Later calls run first. |
| `browser()` | `Promise<Pyro>`: the run's browser, `pyro serve --fork` launched on first use with the setup module's `browser()` options and closed when the run ends. Pages a test opens through it close when the test ends, including a test that timed out. |
| `setupResult<T>()` | What the setup module's `setup()` returned. Throws without `--setup`. |
| `testInfo()` | `{ file, name, titlePath } \| undefined`: the test running now. |
| `expect` | `bun:test`'s `expect`. |

Hooks and tests take an `async` function, a sync function or a `done` callback (a function with a parameter). A test that exceeds its timeout fails with `TimeoutError`.

### Setup {#e2e-setup}

The module passed with `--setup <module>`: a default export object, or the same names exported one by one.

```ts
import type { Setup } from '@povio/pyro/e2e';

export default {
  async setup() {
    const api = await startApi();
    return { api };
  },
  browser: ({ api }) => ({ args: ['--static', 'http://app.test=./dist'] }),
  async teardown({ api }) {
    await api.stop();
  },
} satisfies Setup<{ api: Api }>;
```

```ts
type Setup<T = unknown> = {
  setup?: () => T | Promise<T>;
  browser?: (result: T) => BrowserOptions | Promise<BrowserOptions>;
  teardown?: (result: T) => unknown;
};

type BrowserOptions = {
  /** Extra `pyro serve` arguments (`--static`, `--proxy`, ...); `--fork` is added. */
  args?: string[];
  /** The browser binary (default: `$PYRO_BIN`, else the bundled one). */
  bin?: string;
};
```

`setup()` runs before any spec file loads. Specs read its result with `setupResult()`. `browser()` gives the run's browser its options, and `--browser-arg` values are appended. `teardown()` runs after the run, also when it is interrupted.
