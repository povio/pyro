# Config and the test runner

`pyro run` is the counterpart of `playwright test`. It reads your config, finds each project's test files, starts web servers and global setup once, then runs the projects in dependency order, in worker processes. Each worker is a `bun test` process with its own Pyro browser. This page lists every flag, every config field that is honoured, how projects and shards split a run, and how workers split the work.

## Usage

```sh
pyro run [options] [test-filter...]
```

A `test-filter` is a regular expression. A file runs when its absolute or working-directory-relative path matches any filter. Playwright's `file.spec.ts:42` line filters are not supported.

The command needs [Bun](https://bun.sh) on `PATH`. When the CLI is started by Node, it starts itself again under Bun, and fails with "needs Bun (https://bun.sh) on PATH" if Bun is missing.

## Flags

| Flag | Meaning |
| --- | --- |
| `-c, --config <file>` | Config file. Default: `pyro.config.*`, else `playwright.config.*` in the working directory. |
| `--project <name>` | Run only this project and the projects it depends on. Repeatable. The name matches in any case, and `*` matches any text. Without it, every project runs. |
| `--no-deps` | Do not run the projects that the selected ones depend on. |
| `-j, --workers <n\|n%>` | Worker processes. Default: the config's `workers`, else half the CPUs. `50%` is a share of the CPUs. |
| `-g, --grep <regex>` | Only tests whose title path (file, describes, title, tags) matches. |
| `--grep-invert <regex>` | Skip tests whose title path matches. |
| `--retries <n>` | Retries of a failing test. Overrides the config. |
| `--timeout <ms>` | Test timeout. Overrides the config and every project. |
| `-u, --update-snapshots [mode]` | Rewrite snapshots and screenshot baselines. `changed` (the default), `all`, `missing` or `none`. See [Snapshots](/playwright/snapshots#update-modes). |
| `--fully-parallel` | Run every file's tests in parallel, as `fullyParallel: true`. |
| `--shard <i/n>` | Run part `i` of `n` of the tests, one part per CI job. See [Sharding](#sharding). |
| `-x` | Stop after the first failure (same as `--max-failures 1`). |
| `--max-failures <n>` | Stop after `n` failures. |
| `--reporter <name>` | `list` (default), `line` or `dot`. |
| `--list` | List the tests without running them. |
| `-q, --quiet` | Do not print the tests' own output. |
| `-h, --help` | Print the help. |
| `--headed`, `--debug`, `--ui` | Print a warning and have no effect: Pyro is headless. |

Any other flag (`--trace`, `--repeat-each`, `--last-failed`, `--forbid-only`, `--output`, ...) is an error: the command prints `pyro run: unknown option <flag>` and the help, and exits with status 2.

## Config fields

`defineConfig(config, ...overrides)` merges later configs over earlier ones, with `use` and `expect` merged key by key, as Playwright does. A plain exported object works too.

### Honoured

| Field | Where | Notes |
| --- | --- | --- |
| `use` | config, project | The project's `use` is merged over the config's. See [use options](#use-options). |
| `projects` | config | Each has `name`, `use`, `testDir`, `testMatch`, `testIgnore`, `dependencies`, `teardown`, `fullyParallel`, `grep`, `grepInvert`, `outputDir`, `timeout`, `retries`, `snapshotDir`, `snapshotPathTemplate`, `expect`, `metadata`. See [Projects](#projects). |
| `testDir` | config, project | Default: the config file's directory. |
| `testMatch` | config, project | Default `**/*.@(spec|test).?(c|m)[jt]s?(x)`. A glob without `/` matches the file name only, so `*.e2e.ts` works. Regexes test the absolute path. |
| `testIgnore` | config, project | Same matching as `testMatch`. |
| `timeout` | config, project | Test timeout. Default 30 000 ms. |
| `retries` | config, project | Default 0. |
| `expect.timeout` | config | Default timeout of web-first assertions and `expect.poll`. Default 5 000 ms. |
| `expect.toHaveScreenshot` | config, project | `threshold`, `maxDiffPixels`, `maxDiffPixelRatio` defaults, and `pathTemplate`. |
| `expect.toMatchSnapshot` | config, project | `threshold`, `maxDiffPixels`, `maxDiffPixelRatio` defaults. |
| `expect.toMatchAriaSnapshot` | config, project | `children` (`contain`, `equal`, `deep-equal`) and `pathTemplate`. |
| `outputDir` | config, project | Default `test-results`. Each test writes into a sub-directory named after its title path. |
| `snapshotDir`, `snapshotPathTemplate` | config, project | Where snapshots live. See [Snapshots](/playwright/snapshots). |
| `updateSnapshots` | config | Default update mode when `-u` is not given. Default `missing`. |
| `webServer` | config | One or a list. See [below](#web-servers). |
| `globalSetup`, `globalTeardown` | config | A path or a list of paths. See [below](#global-setup-and-teardown). |
| `workers` | config | A number or a percentage string. |
| `fullyParallel` | config, project | Every test of every file is parallel. See [Workers](#workers). |
| `shard` | config | `{ current, total }`. `--shard` overrides it. See [Sharding](#sharding). |
| `metadata` | config, project | Merged into `testInfo.project.metadata`. |

A project's `expect` replaces the top-level `expect` instead of merging with it, as in Playwright.

### Accepted and ignored

These typecheck but do nothing: `reporter`, `globalTimeout`, `maxFailures` (use `--max-failures`), `forbidOnly`, `reportSlowTests`, `quiet`, `preserveOutput`, `captureGitInfo`, `tsconfig`, `repeatEach` (also on projects). In `webServer`, `gracefulShutdown`, `name` and `ignoreHTTPSErrors` are ignored.

## Projects

`pyro run` collects the files of every project (or those selected with `--project`). A file runs in each project whose `testDir`, `testMatch` and `testIgnore` include it, with that project's `use`, as in Playwright. A file that three projects match runs three times.

A project's `grep` and `grepInvert` filter its tests by title path, like the flags.

### Dependencies and teardown

```ts
projects: [
  { name: 'setup', testMatch: /.*\.setup\.ts/, teardown: 'cleanup' },
  { name: 'cleanup', testMatch: /.*\.teardown\.ts/ },
  { name: 'app', dependencies: ['setup'], use: { storageState: '.auth/user.json' } },
],
```

- **A project's `dependencies` run first, to completion.** The projects that depend on them start after their last test ended. So the usual sign-in setup works: the `setup` project saves the storage state, and `app` starts from that file. See [Sign in once (setup project)](/recipes/playwright-setup-project-auth).
- **Dependencies run in full.** File filters, `--grep`, `--grep-invert` and `--shard` apply to the selected projects only.
- **`--project app` takes `setup` along.** `--no-deps` leaves the dependencies out.
- **When a dependency has a failed test, the projects that depend on it do not run.** Their tests are counted as "did not run" in the summary.
- **A `teardown` project runs after every project that depends on its setup project**, whatever the result.

The config is checked as Playwright checks it. A dependency or teardown name that matches no project, a name that two projects share, a teardown project with dependencies, a project that depends on a teardown project and a circular dependency are errors, for example `pyro: project 'app' has unknown dependency 'setup'`.

Under plain `bun test` there are no project dependencies: the project is `$PYRO_PROJECT` if set, else the project whose files include the test file, else the first one.

## Sharding

```sh
pyro run --shard 2/4      # this CI job's quarter of the tests
```

`--shard i/n` (or `shard: { current: i, total: n }` in the config) runs part `i` of `n`. The split is Playwright's, so each CI job computes the same parts without talking to the others:

- The selected projects' tests are taken in order and cut into `n` parts balanced by test count.
- A file's tests that are not parallel stay together, in the part where the first of them falls. Parallel tests (`fullyParallel`, or a describe with `mode: 'parallel'`) are split one by one.
- Each shard runs the dependencies it needs, in full. A setup project therefore runs in every job.

The runner lists the tests first (one process, no browser) to compute the parts. A shard that gets no tests prints `pyro run: shard 4 of 4 has no tests` and exits with status 0. `--shard 1/1` is a run without shards.

Sharding is for splitting a run over machines. On one machine, raise `-j` instead.

## use options

All Playwright `use` options are accepted, with Playwright's meaning and defaults. `browser.newContext()` takes the same context options. Every page of the context has them from its first document on, popups included.

| Option | Effect |
| --- | --- |
| `baseURL` | Resolves relative URLs in `goto`, `waitForURL`, `toHaveURL`, routes and `request`. |
| `viewport`, `screen` | Default viewport `{ width: 1280, height: 720 }`. `null` leaves the browser's own size. |
| `deviceScaleFactor` | `devicePixelRatio`. Screenshots are in device pixels. |
| `isMobile` | Mobile emulation, as Chromium's: `<meta name="viewport">` decides the layout viewport. See [below](#mobile-emulation). |
| `hasTouch` | `navigator.maxTouchPoints`, and a coarse pointer without hover in media queries. |
| `storageState` | Cookies and `localStorage` (path or object). See [Network](/playwright/network#storagestate-and-cookies). |
| `extraHTTPHeaders` | Sent with every page request and by `request`. |
| `userAgent` | Overrides the page's user agent. |
| `colorScheme`, `reducedMotion` | Media emulation for `@media` and `matchMedia`. |
| `locale` | `navigator.language` and `languages`, `Accept-Language`, and the default locale of `Intl` and the `toLocale*String` methods. In documents only, not in workers. |
| `timezoneId` | `Date`, `Intl` and `Temporal`. An error on the Intel macOS browser. |
| `geolocation`, `permissions` | `navigator.geolocation` and the Permissions API, with Playwright's permission names. Without the `geolocation` permission, `navigator.geolocation` reports permission denied. |
| `proxy` | An HTTP proxy for the context: `server` (`host:port` or `http://host:port`), `bypass`, `username`, `password`. SOCKS and HTTPS proxies are an error. `context.request` and the `request` fixture do not go through it. |
| `httpCredentials` | A page's 401 challenge gets the Basic credentials once (from `origin` only, when set). The `request` fixture sends them after a 401 too, or with every request with `send: 'always'`. |
| `offline` | Requests fail as without a network. |
| `javaScriptEnabled` | `false`: no script of the page runs, event handlers and timers included, and `<noscript>` shows. `page.evaluate` still runs. |
| `ignoreHTTPSErrors` | Pages and the `request` fixture accept invalid certificates. |
| `bypassCSP` | See [What differs from Playwright](/playwright/differences#context-options): Pyro enforces no Content-Security-Policy to begin with. |
| `serviceWorkers` | `'block'`: `navigator.serviceWorker.register()` registers nothing. Default `'allow'`. |
| `acceptDownloads` | Default `true`. `false`: downloads are not saved; the `download` event still fires. |
| `recordHar`, `recordVideo` | A HAR file per context; a WebM per page. See [Video](#video). |
| `screenshot`, `video` | The runner's captures per test. See [Screenshots of failed tests](#screenshots-of-failed-tests) and [Video](#video). |
| `actionTimeout`, `navigationTimeout` | Default timeouts. With neither set, an action or navigation waits up to 30 s (Playwright waits until the test times out). |
| `testIdAttribute` | The attribute `getByTestId` matches. Default `data-testid`. |
| `contextOptions` | Merged under the named options. |
| `pyro` | How tests reach the browser. See [below](#use-pyro). |

These are accepted and have no effect: `forcedColors`, `strictSelectors`, and the worker options `channel`, `headless`, `launchOptions`, `connectOptions` and `trace` (nothing is recorded). `browserName` does not choose a browser, but it sets the `browserName` fixture, so `test.skip(({ browserName }) => browserName === 'webkit')` behaves as the config says.

A proxy for every context of a worker can also go on the browser: `pyro: { args: ['--proxy', 'http://host:3128'] }`.

### Mobile emulation

`isMobile: true` lays a page out as Chromium's mobile emulation does. The `devices` descriptors (`devices['iPhone 15']`, `devices['Pixel 7']`, ...) set it together with `viewport`, `deviceScaleFactor`, `hasTouch` and `userAgent`.

```ts
import { defineConfig, devices } from '@povio/pyro/wright';

export default defineConfig({
  projects: [
    { name: 'desktop', use: { ...devices['Desktop Chrome'] } },
    { name: 'phone', use: { ...devices['iPhone 15'] } },
  ],
});
```

- `<meta name="viewport">` decides the layout viewport: `width=device-width`, a width in px, `initial-scale`, `minimum-scale`, `maximum-scale` and `user-scalable`, also when a script adds or changes the tag.
- A page without the tag lays out 980 px wide, zoomed out to the device's width.
- `innerWidth`, `visualViewport`, media queries, `resize` events and screenshots follow, with Chrome's numbers, in portrait and in landscape.
- There is no pinch zoom, and `locator.tap()` is a click.

`isMobile` or `deviceScaleFactor` with `viewport: null` is an error, as in Playwright.

### Screenshots of failed tests

`use: { screenshot }` takes `'off'` (the default), `'on'`, `'only-on-failure'`, `'on-first-failure'`, or `{ mode, fullPage, omitBackground }`. Right after the test function, before the `afterEach` hooks, the runner captures every open page when the outcome asks for it: a failed or timed-out test for the failure modes, and the first attempt only for `'on-first-failure'`. The files are `test-failed-1.png`, `test-failed-2.png`, ... or `test-finished-1.png`, ... in `testInfo.outputDir`. They are attached to `testInfo.attachments` as `screenshot` and listed under the failure in the report.

A test that times out keeps its pages, so it is captured too. See [Timeouts](#timeouts).

### Video

`recordVideo: { dir, size }` on a context records a WebM of each page, popups included, as Playwright records it: VP8 at 25 frames a second, at `size` or else the viewport scaled down to fit 800x800. `page.video()` gives `path()`, `saveAs()` and `delete()`. `page.close()` and `context.close()` return once the file is written.

In the runner, `use: { video }` records the `context` fixture's pages for you. It takes `'off'` (the default), `'on'`, `'retain-on-failure'`, `'on-first-retry'`, or `{ mode, size }`. The files go into the test's output directory as `video.webm`, `video-1.webm`, ..., are attached to the test as `video`, and are listed under a failure in the report.

```ts
use: { video: 'retain-on-failure' },
```

- **Recording needs ffmpeg.** Pyro uses `$PYRO_FFMPEG`, else `ffmpeg` on `PATH`, else Playwright's own (`bunx playwright install ffmpeg`). Without one, creating the context fails with an error that starts `browser.newContext: recordVideo encodes videos with ffmpeg, and there is none` and says how to install one.
- **A frame is rendered when what the page shows changes**, at most 25 a second. A page that does not change costs nothing. Frames are rendered as screenshots are, so a CSS animation shows at its end state.
- **An ffmpeg that reads raw video is faster.** With Homebrew's or a Linux distribution's ffmpeg, frames are encoded while the page runs and the file is ready soon after the page closes. With Playwright's ffmpeg, and under `'retain-on-failure'`, frames wait as JPEGs in a temporary directory and are encoded when the page closes. Under `'retain-on-failure'`, the pages of a passing test are not encoded at all.
- `recordVideo.showActions` and `video.show` are accepted and draw no annotations.

### use.pyro

```ts
use: {
  pyro: {
    wsEndpoint: 'ws://127.0.0.1:9222/',     // connect to a running server instead of launching one
    bin: '/path/to/pyro',          // the binary to launch
    args: ['--static', 'http://app.test=./dist'], // extra `pyro serve` flags
    boot: [{ url: '/platform', routes: '/platform/**' }], // pages booted ahead
  },
},
```

| Field | Default | Meaning |
| --- | --- | --- |
| `wsEndpoint` | `$PYRO_WS` | A running `pyro serve` to connect to. Without it, each worker launches its own browser. |
| `bin` | `$PYRO_BIN`, else the bundled binary | The binary each worker launches. |
| `args` | none | Extra `pyro serve` flags, for example `--static`, `--proxy`, `--hyphen-digit-breaks on`. See [serve options](/reference/serve). |
| `boot` | none | Pages booted ahead of the tests. See [Booting pages ahead](/playwright/boot-pools). |

`pyro` is a worker option, so `test.use({ pyro: { ... } })` in a file works. Each distinct value gets its own browser in the worker.

With `wsEndpoint` (or `$PYRO_WS`), all workers share one server, one connection each. Start it yourself, for example `pyro serve --port 9222`, or in `globalSetup`.

## Environment variables

| Variable | Meaning |
| --- | --- |
| `PYRO_CONFIG` | Config file path. `PLAYWRIGHT_CONFIG` is read when it is unset. |
| `PYRO_PROJECT` | Project for plain `bun test` runs. |
| `PYRO_UPDATE_SNAPSHOTS` | Update mode (`all`, `changed`, `missing`, `none`; `1` or `true` mean `changed`). Set by `-u`. |
| `PYRO_RETRIES`, `PYRO_TIMEOUT` | Override retries and test timeout. Set by `--retries` and `--timeout`. |
| `PYRO_CONCURRENT` | `1` runs the tests of a parallel describe concurrently inside one worker (see [Workers](#workers)). |
| `PYRO_WS` | Connect to this server instead of launching one. |
| `PYRO_BIN` | Binary to launch. |
| `PYRO_FFMPEG` | The ffmpeg that encodes videos. See [Video](#video). |

See [Environment variables](/reference/env) for the browser's own variables.

## Workers

The runner starts `workers` processes at once. Each runs `bun test` with its own browser and its own module state. Files are dealt out whole: sorted by size, largest first, round robin over the workers.

- **A file's tests run in order, in one worker**, unless they are parallel.
- **Parallel tests** (`fullyParallel`, `--fully-parallel`, or a describe with `mode: 'parallel'`) spread over the workers by their position in the file. A describe's `beforeAll` hooks run with the first of its tests a worker runs and its `afterAll` hooks after the last, in each worker that runs some of its tests, and in no other.
- A describe in `serial` mode, or `default` mode set with `test.describe.configure`, keeps its tests on the file's worker.
- **Inside a worker, parallel tests still run one after another**, since they may share state set in hooks. Set `PYRO_CONCURRENT=1` to run a parallel describe's tests concurrently in the worker (Bun's `test.concurrent`).
- **Worker fixtures live as long as the worker**, across its files: one instance per worker and set of fixture parameters.
- **A file that several projects match** runs in a separate worker process for each project.
- **Web servers and global setup run once**, in the parent process, before any worker starts.
- A worker that gets no tests exits before it starts a browser.

`--list` and `--shard` list the tests in one process that starts no browser.

The worker count is not capped by the number of files, because parallel describes can spread one file over every worker. With few files and no parallel tests, lower `-j` to avoid idle processes.

## Web servers

`webServer` is one object or a list. For each:

1. If `url` (or `port`) already answers, the server is reused, unless `reuseExistingServer: false`, which is an error. Unlike Playwright, an unset `reuseExistingServer` reuses. Make sure nothing else listens on that port: the runner reuses whatever answers.
2. Otherwise `command` runs in a shell, in `cwd` (relative to the config file) with `env` added, in its own process group.
3. The runner polls `url` (any status below 404, or 405) or the TCP `port` every 100 ms until it answers, for up to `timeout` (default 60 000 ms). It fails if the command exits first.

`stdout: 'pipe'` shows the server's output; stderr is shown unless `stderr: 'ignore'`. The servers are stopped after the run, also when the run ends early.

## Global setup and teardown

`globalSetup` and `globalTeardown` take a path or a list of paths, relative to the config file. Each module's default export (or the module itself) is called with the config. A function returned by a global setup runs at teardown, as in Playwright. Teardowns run after the last worker ends.

Global setup runs after the web servers answer, in the `pyro run` process, once per run. Under plain `bun test`, it runs before the first file's tests.

The argument is Playwright's resolved `FullConfig`. `config.projects[0].use` is that project's `use` with the top-level `use` merged in, so Playwright's common `const { baseURL, storageState } = config.projects[0].use` works. The `FullConfig` and `FullProject` types are exported from `@povio/pyro/wright`.

```ts
import { chromium, type FullConfig } from '@povio/pyro/wright';

export default async function globalSetup(config: FullConfig) {
  const { baseURL, storageState } = config.projects[0].use;
  const browser = await chromium.launch();
  const page = await browser.newPage({ baseURL });
  // ... sign in ...
  await page.context().storageState({ path: storageState as string });
  await browser.close();
}
```

## Timeouts

A test that times out keeps its pages, as in Playwright.

1. The call the test function waits on is cancelled in the browser. It fails with, for example, `locator.click: Test timeout of 30000ms exceeded.` and its call log, reported after the timeout error. A page script that does not return is stopped.
2. The `screenshot` option's captures, the `afterEach` hooks and the fixture teardown run, in a time of their own: the larger of the project's and the test's timeout. Past it, the test fails with `Test timeout of 30000ms exceeded while running "afterEach" hook.`
3. If the browser does not answer the cancel within 2 s, the test's pages are closed instead and the run goes on.

So an `afterEach` hook that screenshots or evaluates the page works for a timed-out test too. `testInfo.status` is `'timedOut'` there.

## Reporting

The report shows each test as it finishes: passes, failures, skips (declared, or `test.skip()` at run time, with the reason in parentheses), flaky tests (passed on a retry) and expected failures (`test.fail()`). A test of a named project shows as `[project] › file › title`. The tests' own `console.log` output is printed per worker, prefixed with `[w0]`, `[w1]`, ... After the run come the failures with their errors and the files they attached (screenshots, videos), then errors outside any test (a worker fixture's teardown, for example), then a summary.

```text
Running 2 files using 4 workers

  ✓   1 [app] › e2e/todo.spec.ts › todos › adds a todo (359ms)
  ✘   2 [app] › e2e/todo.spec.ts › todos › ua and role (4962ms)
  -   3 [app] › e2e/todo.spec.ts › todos › runtime skip (needs FEATURE_X)

  1 failed
  1 skipped
  1 passed (5.9s)
```

The summary also counts flaky tests and tests that did not run: the tests of a project whose dependency failed, the tests after a failure in a serial group, and the other tests of a describe whose `beforeAll` hook failed. With `--shard`, the first line ends in `, shard 2 of 4`.

- `list` prints one line per test.
- `line` prints the same lines as `list` in this version.
- `dot` prints one character per test: `·` pass, `±` flaky, `°` skipped, `F` failed.

There is no HTML, JSON or JUnit reporter. A worker that crashes outside a test (a syntax error in a test file, for example) is reported with the last 40 lines of Bun's output.

The exit status is 1 when any test failed, an error happened outside a test or a worker crashed, else 0.

## Plain bun test

```sh
PYRO_CONFIG=playwright.config.ts bun test ./e2e/foo.spec.ts
```

This runs the files in one process with one browser. The config, web servers and global setup are loaded on the first file; teardowns run after the last file. Bun's own flags apply (`-t` for names, `--bail`, `-u` for snapshots).

Differences from `pyro run`: no workers or `fullyParallel` spreading, no project dependencies or shards, Bun's reporter instead of the runner's, and a run-time `test.skip()` is reported by Bun as a pass (it prints `↳ skipped: <title>` to the log).

## Next

- Sign in once with a setup project: [Sign in once (setup project)](/recipes/playwright-setup-project-auth).
- Run a large suite in CI: [Sharding in CI](/recipes/playwright-ci-without-sharding).
- Fixtures, hooks and retries: [Fixtures, hooks and steps](/playwright/fixtures).
