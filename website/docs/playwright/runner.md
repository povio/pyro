# Config and the test runner

`pyro run` is the counterpart of `playwright test`. It reads your config, finds each project's test files, starts web servers and global setup once, then runs the files in worker processes. Each worker is a `bun test` process with its own Pyro browser. This page lists every flag, every config field that is honoured, and how workers split the work.

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
| `--project <name>` | Run only this project. Repeatable. Without it, every project's files run. |
| `-j, --workers <n\|n%>` | Worker processes. Default: the config's `workers`, else half the CPUs. `50%` is a share of the CPUs. |
| `-g, --grep <regex>` | Only tests whose title path (file, describes, title, tags) matches. |
| `--grep-invert <regex>` | Skip tests whose title path matches. |
| `--retries <n>` | Retries of a failing test. Overrides the config. |
| `--timeout <ms>` | Test timeout. Overrides the config and every project. |
| `-u, --update-snapshots [mode]` | Rewrite snapshots and screenshot baselines. `changed` (the default), `all`, `missing` or `none`. See [Snapshots](/playwright/snapshots#update-modes). |
| `--fully-parallel` | Run every file's tests in parallel, as `fullyParallel: true`. |
| `-x` | Stop after the first failure (same as `--max-failures 1`). |
| `--max-failures <n>` | Stop after `n` failures. |
| `--reporter <name>` | `list` (default), `line` or `dot`. |
| `--list` | List the tests without running them. |
| `-q, --quiet` | Do not print the tests' own output. |
| `-h, --help` | Print the help. |
| `--headed`, `--debug`, `--ui` | Print a warning and have no effect: Pyro is headless. |

Any other flag (`--shard`, `--trace`, `--repeat-each`, `--last-failed`, `--forbid-only`, `--output`, ...) is an error: the command prints the help and exits with status 2.

## Config fields

`defineConfig(config, ...overrides)` merges later configs over earlier ones, with `use` and `expect` merged key by key, as Playwright does. A plain exported object works too.

### Honoured

| Field | Where | Notes |
| --- | --- | --- |
| `use` | config, project | The project's `use` is merged over the config's. See [use options](#use-options). |
| `projects` | config | Each has `name`, `use`, `testDir`, `testMatch`, `testIgnore`, `outputDir`, `timeout`, `retries`, `snapshotDir`, `snapshotPathTemplate`, `expect`, `metadata`. |
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
| `fullyParallel` | config | Top level only. |
| `metadata` | config, project | Merged into `testInfo.project.metadata`. |

A project's `expect` replaces the top-level `expect` instead of merging with it, as in Playwright.

### Accepted and ignored

These typecheck but do nothing: `reporter`, `globalTimeout`, `maxFailures` (use `--max-failures`), `forbidOnly`, `reportSlowTests`, `quiet`, `preserveOutput`, `captureGitInfo`, `tsconfig`, `shard`, and on projects `grep`, `grepInvert`, `dependencies`, `repeatEach`, `teardown` and `fullyParallel`. In `webServer`, `gracefulShutdown`, `name` and `ignoreHTTPSErrors` are ignored.

Project `dependencies` are worth a note: a "setup" project that logs in and saves `storageState` does not run first. Run that code in `globalSetup` instead.

## Projects

`pyro run` collects the files of every project (or those named with `--project`). Each file runs once, under the first project whose `testDir`, `testMatch` and `testIgnore` include it. Playwright would run such a file once per project.

Under plain `bun test`, the project is `$PYRO_PROJECT` if set, else the project whose files include the test file, else the first one.

## use options

All Playwright `use` options are accepted. These take effect:

| Option | Effect |
| --- | --- |
| `baseURL` | Resolves relative URLs in `goto`, `waitForURL`, `toHaveURL`, routes and `request`. |
| `viewport` | Default `{ width: 1280, height: 720 }`. `null` leaves the browser's own size. |
| `storageState` | Cookies and `localStorage` (path or object). See [Network](/playwright/network#storagestate-and-cookies). |
| `extraHTTPHeaders` | Sent with every page request and by `request`. |
| `userAgent` | Overrides the page's user agent. |
| `colorScheme`, `reducedMotion` | Media emulation for `@media` and `matchMedia`. |
| `httpCredentials` | Used by the `request` fixture only (Basic auth); pages do not use it. |
| `ignoreHTTPSErrors` | Used by the `request` fixture only. |
| `actionTimeout`, `navigationTimeout` | Default timeouts. With neither set, an action or navigation waits up to 30 s (Playwright waits until the test times out). |
| `testIdAttribute` | The attribute `getByTestId` matches. Default `data-testid`. |
| `contextOptions` | Merged under the named options. |
| `acceptDownloads` | Default `true`. |
| `pyro` | How tests reach the browser. See below. |

These are accepted and have no effect on pages: `bypassCSP`, `deviceScaleFactor`, `geolocation`, `hasTouch`, `isMobile`, `javaScriptEnabled`, `locale`, `offline`, `permissions`, `proxy`, `timezoneId`, `serviceWorkers`, and the worker options `browserName`, `channel`, `headless`, `launchOptions`, `connectOptions`, `screenshot`, `video`, `trace`. A proxy goes on the browser instead: `pyro: { args: ['--proxy', 'http://host:3128'] }`.

`browserName` still sets the `browserName` fixture, so `test.skip(({ browserName }) => browserName === 'webkit')` behaves as the config says.

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

See [Environment variables](/reference/env) for the browser's own variables.

## Workers

The runner starts `workers` processes at once. Each runs `bun test` with its own browser and its own module state. Files are dealt out whole: sorted by size, largest first, round robin over the workers.

- **A file's tests run in order, in one worker**, unless they are parallel.
- **Parallel tests** (`fullyParallel`, `--fully-parallel`, or a describe with `mode: 'parallel'`) spread over the workers by their position in the file. A describe's `beforeAll` and `afterAll` run in each worker that runs some of its tests, and in no other.
- A describe in `serial` mode, or `default` mode set with `test.describe.configure`, keeps its tests on the file's worker.
- **Inside a worker, parallel tests still run one after another**, since they may share state set in hooks. Set `PYRO_CONCURRENT=1` to run a parallel describe's tests concurrently in the worker (Bun's `test.concurrent`).
- **Web servers and global setup run once**, in the parent process, before any worker starts.
- A worker that gets no tests exits before it starts a browser.

`--list` uses one worker.

The worker count is not capped by the number of files, because parallel describes can spread one file over every worker. With few files and no parallel tests, lower `-j` to avoid idle processes.

## Web servers

`webServer` is one object or a list. For each:

1. If `url` (or `port`) already answers, the server is reused, unless `reuseExistingServer: false`, which is an error. Unlike Playwright, an unset `reuseExistingServer` reuses. Make sure nothing else listens on that port: the runner reuses whatever answers.
2. Otherwise `command` runs in a shell, in `cwd` (relative to the config file) with `env` added, in its own process group.
3. The runner polls `url` (any status below 404, or 405) or the TCP `port` every 100 ms until it answers, for up to `timeout` (default 60 000 ms). It fails if the command exits first.

`stdout: 'pipe'` shows the server's output; stderr is shown unless `stderr: 'ignore'`. The servers are stopped after the run, also when the run ends early.

## Global setup and teardown

`globalSetup` and `globalTeardown` take a path or a list of paths, relative to the config file. Each module's default export (or the module itself) is called with the config. A function returned by a global setup runs at teardown, as in Playwright. Teardowns run after the last worker ends.

## Reporting

The report shows each test as it finishes: passes, failures, skips (declared, or `test.skip()` at run time, with the reason in parentheses), flaky tests (passed on a retry) and expected failures (`test.fail()`). The tests' own `console.log` output is printed per worker, prefixed with `[w0]`, `[w1]`, ... After the run come the failures with their errors and a summary.

```text
Running 2 files using 4 workers

  ✓   1 e2e/todo.spec.ts › todos › adds a todo (359ms)
  ✘   2 e2e/todo.spec.ts › todos › ua and role (4962ms)
  -   3 e2e/todo.spec.ts › todos › runtime skip (needs FEATURE_X)

  1 failed
  1 skipped
  1 passed (5.9s)
```

- `list` prints one line per test.
- `line` prints the same lines as `list` in this version.
- `dot` prints one character per test: `·` pass, `±` flaky, `°` skipped, `F` failed.

There is no HTML, JSON or JUnit reporter. A worker that crashes outside a test (a syntax error in a test file, for example) is reported with the last 40 lines of Bun's output.

The exit status is 1 when any test failed or a worker crashed, else 0.

## Plain bun test

```sh
PYRO_CONFIG=playwright.config.ts bun test ./e2e/foo.spec.ts
```

This runs the files in one process with one browser. The config, web servers and global setup are loaded on the first file; teardowns run after the last file. Bun's own flags apply (`-t` for names, `--bail`, `-u` for snapshots).

Differences from `pyro run`: no workers or `fullyParallel` spreading, Bun's reporter instead of the runner's, and a run-time `test.skip()` is reported by Bun as a pass (it prints `↳ skipped: <title>` to the log).
