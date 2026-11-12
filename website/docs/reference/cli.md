# CLI

The `pyro` command ships in the npm package `@povio/pyro` (its `bin`). It has two halves: the native browser binary (`version`, `eval`, `serve`, `fetch`) and two test runners written in TypeScript that need [Bun](https://bun.sh) (`test`, `e2e`).

```sh
bunx pyro <command> [options]
# or, installed in a project
bun add -d @povio/pyro
./node_modules/.bin/pyro <command> [options]
```

| Command | What it does | Runs on |
| --- | --- | --- |
| [`version`](#version) | Prints the version and engine capabilities | native binary |
| [`eval`](#eval) | Runs JavaScript in an `about:blank` page until idle | native binary |
| [`serve`](#serve) | CDP server over WebSocket | native binary |
| [`fetch`](#fetch) | Loads a URL (scripts run) and prints the DOM after `load` | native binary |
| [`test`](#test) | Playwright-style test runner (`playwright test` replacement) | Bun |
| [`e2e`](#e2e) | One-process runner for `@povio/pyro/e2e` specs | Bun |
| `help`, `--help`, `-h`, no command | Prints the usage summary | native binary |

## How the npm `pyro` command dispatches {#dispatch}

The npm `bin` is a small Node/Bun script (`dist/cli.js`):

- `test` and `e2e` run in-process when the script runs under Bun. Under Node it re-launches itself with `bun` on `PATH`; with no Bun it prints `pyro run: needs Bun (https://bun.sh) on PATH` and exits 1.
- Every other command is forwarded, arguments unchanged, to the native binary. `SIGINT`, `SIGTERM` and `SIGHUP` are forwarded to it, and its exit code (or signal) becomes the wrapper's.

The native binary is chosen in this order:

1. `$PYRO_BIN` ([env](/reference/env#pyro-bin)).
2. A `pyro` file next to the package directory (the layout of a release archive).
3. The bundled binary `native/<platform>-<arch>/pyro` (`darwin-arm64`, `darwin-x64`, `linux-arm64`, `linux-x64`).

On any other platform it fails with: `Pyro has no bundled browser for <platform>-<arch>. The browser supports macOS 14+ and Linux (glibc 2.39+) on arm64 and x64.` See [Platforms and limits](/guide/platforms).

## Exit codes {#exit-codes}

Native commands:

| Code | Meaning |
| --- | --- |
| `0` | Success (`help` too) |
| `1` | Runtime failure: engine or page creation failed; `eval`: an uncaught exception or unhandled promise rejection; `fetch`: the navigation failed; `serve`: could not bind or start (a `--fork` template that fails to start) |
| `2` | Usage error: unknown command, unknown or malformed option, unreadable `eval` file; `serve --fork` on a build without fork support |
| `124` | `eval` / `fetch`: the `--timeout-ms` deadline passed |

`test` and `e2e` exit `0` when every test passed, `1` on any failure, and `2` on a usage error. `e2e` interrupted by a signal exits `128 + signal` (130 for `SIGINT`).

## version {#version}

```sh
pyro version      # or: pyro --version
```

Prints the version and what the linked JavaScriptCore supports:

```text
pyro X.Y.Z
engine: JavaScriptCore; capabilities: Capabilities { weak_refs: true, modules: true, import_maps: true, typed_modules: true, unhandled_rejections: true, termination: true, hidden_properties: true, proxy_targets: true, object_global_context: true, fork: true, global_constants: true }
```

`fork: true` means the binary supports [`serve --fork`](/reference/serve#fork); the `e2e` runner checks this line before it adds `--fork`.

## eval {#eval}

```text
pyro eval [--timeout-ms N] (-e CODE | FILE | -)
```

Runs JavaScript in a fresh `about:blank` page, then keeps the event loop running until the page is idle (timers, promises and tasks done) or the deadline passes. No network is set up: use [`fetch`](#fetch) or [`serve`](#serve) to load pages.

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `--timeout-ms N` | integer (ms) | `30000` | Deadline for the evaluation and everything it schedules. Covers the first evaluation too: a watchdog terminates running JavaScript (loops included). Exceeding it exits `124`. |
| `-e CODE` | string | | The source to run (reported as `eval.js`). |
| `FILE` | path | | Read the source from a file (reported under its path). |
| `-` | | | Read the source from standard input (reported as `stdin.js`). |

Exactly one source is needed; if several are given the last wins. Any other argument is taken as a file path (so `eval --help` reports `--help: No such file or directory`).

Output: `console.log`/`info`/`debug` go to stdout; `console.error`, `warn` and `assert` go to stderr. Strings print as-is and objects as JSON. An uncaught exception prints `Uncaught <message>` and its stack to stderr; an unhandled rejection prints `Unhandled promise rejection: <message>`. Either makes the exit code `1`, but the run still continues until idle.

```sh
pyro eval -e 'console.log(1 + 1, { a: 1 }); setTimeout(() => console.log("later"), 10)'
# 2 {"a":1}
# later

echo 'console.log(navigator.userAgent)' | pyro eval -
# Mozilla/5.0 (compatible; pyro/X.Y.Z)

pyro eval --timeout-ms 100 -e 'setInterval(() => {}, 10)'
# pyro: timed out after 100 ms   (exit 124)
```

## serve {#serve}

```text
pyro serve [OPTIONS]
```

Starts the CDP server: Chrome DevTools Protocol over WebSocket on `ws://HOST:PORT/` (default `ws://127.0.0.1:9222/`), with `GET /json/version` and `GET /json/list`. This is what `Pyro.launch()`, the Playwright drop-in, Puppeteer and raw CDP clients connect to. It runs until killed (or until `--parent-pid` exits).

```sh
pyro serve --port 9222
pyro serve --fork --static http://app.test=./dist --prepare http://app.test/
```

Every option, with types, defaults and environment equivalents, is in [serve options](/reference/serve). `pyro serve --help` prints the same list for the installed version.

On start it logs to stderr (unless `--log-level` is above `info`):

```text
pyro: CDP server listening on ws://127.0.0.1:9222/
pyro: CDP server listening on ws://127.0.0.1:9222/ (fork mode)
```

## fetch {#fetch}

```text
pyro fetch [--dump html|none] [--wait-ms N] [--timeout-ms N] URL
```

Loads `URL` with the network enabled, runs its scripts, waits for the `load` event, and prints the serialized DOM to stdout.

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `URL` | URL | required | `http:`, `https:`, `data:` or `file:` (exactly one; an argument starting with `-` is an option). |
| `--dump html\|none` | enum | `html` | `html`: print `<!DOCTYPE html>` (when the document has a doctype) and the `outerHTML` of the root element. `none`: print nothing (exit code only). |
| `--wait-ms N` | integer (ms) | `0` | After `load`, keep running the page until it is idle or `N` ms pass (for content rendered after `load`). Bounded by `--timeout-ms`. |
| `--timeout-ms N` | integer (ms) | `30000` | Overall deadline. If `load` has not fired by then, or the wait runs past it, the command exits `124`. |

Console output of the page goes to stdout/stderr as for [`eval`](#eval). A failed navigation prints `pyro: <url>: <error>` (for example `net::ERR_CONNECTION_REFUSED`) and exits `1`. `fetch` has no `--help` (it is a usage error).

```sh
pyro fetch https://example.com > page.html
pyro fetch --wait-ms 2000 https://spa.example.com/
pyro fetch --dump none https://example.com && echo reachable
```

See [One-off fetches](/scraping/fetch) for recipes.

## test {#test}

```text
pyro run [options] [test-filter...]
```

Runs tests written against `@povio/pyro/wright` the way `playwright test` does: it reads the config, finds each project's test files (`testDir`, `testMatch`, `testIgnore`), starts `webServer`s and runs `globalSetup` once, then runs the files with `bun test` in worker processes, one browser per worker. Needs Bun. Guide: [Config and the test runner](/playwright/runner).

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `-c`, `--config <file>` | path | `pyro.config.{ts,mts,mjs,js}`, else `playwright.config.*` in the working directory | Config file. Sets `PYRO_CONFIG` for the workers. |
| `--project <name>` | string | all projects | Only this project (repeatable). An unknown name exits `1`. |
| `-j`, `--workers <n\|n%>` | count or percentage of CPUs | the config's `workers`, else half the CPUs | Worker processes. |
| `-g`, `--grep <regex>` | regex | | Only tests whose title path matches. |
| `--grep-invert <regex>` | regex | | Skip tests whose title path matches. |
| `--retries <n>` | integer | config's `retries` | Retries of a failing test (sets `PYRO_RETRIES`). |
| `--timeout <ms>` | integer | config's `timeout` (30000) | Test timeout (sets `PYRO_TIMEOUT`). |
| `-u`, `--update-snapshots [mode]` | `changed` \| `all` \| `missing` \| `none` | `changed` when the flag is given | Rewrite snapshots and screenshot baselines (sets `PYRO_UPDATE_SNAPSHOTS` to the mode). |
| `--fully-parallel` | flag | config's `fullyParallel` | Run every file's tests in parallel across workers. |
| `-x` | flag | | Stop after the first failure (as `--max-failures=1`). |
| `--max-failures <n>` | integer | | Stop after `n` failures (workers get `SIGTERM`). |
| `--reporter <name>` | `list` \| `line` \| `dot` | `list` | Console reporter. |
| `--list` | flag | | List the tests without running them (one worker, no web servers or global setup). |
| `-q`, `--quiet` | flag | | Do not print the tests' own output. |
| `--headed`, `--debug`, `--ui` | flag | | Accepted; print `has no effect (pyro is headless)`. |
| `-h`, `--help` | flag | | Print the usage and exit `0`. |

Options also take the `--name=value` form. `test-filter` arguments are regular expressions; a file runs when its absolute or relative path matches any of them. With no files found the command prints `pyro run: no tests found` and exits `1`.

The browser is `$PYRO_WS` when set (an already running `serve`), else each worker launches its own. Test output lines are prefixed `[wN]`; colors are off when stdout is not a TTY or `NO_COLOR` is set.

```sh
pyro run                        # every project
pyro run -j 8 --project chromium login
pyro run -u all e2e/visual      # rewrite all baselines of matching files
```

## e2e {#e2e}

```text
pyro e2e [options] [path | dir | glob | filter ...]
```

Runs spec files written against `@povio/pyro/e2e` in one Bun process: tests of every file share one queue (up to `--concurrency` at a time, so cases of different files interleave) and one fork-mode browser, launched on first use and always closed. Needs Bun. Guide: [Suite runner](/e2e/runner).

Without paths it runs every `*.test.*` / `*.spec.*` file (also `_test`/`_spec`; extensions `js`, `jsx`, `ts`, `tsx`, `mjs`, `cjs`, `mts`, `cts`) under the working directory, skipping `node_modules` and dot-directories. An argument that is a file, a directory or a glob (contains `*`, `?`, `[` or `{`) selects files; any other argument filters the spec paths by substring, as with `bun test`.

| Option | Value | Default | Description |
| --- | --- | --- | --- |
| `-j`, `--concurrency <n>` | integer ≥ 1 | the CPU count | Tests at a time across all files. |
| `--max-concurrency <n>` | integer ≥ 1 | unlimited | Tests of one file at a time (as `bun test`'s). |
| `--timeout <ms>` | integer | `5000` | Default timeout of tests and hooks without their own. |
| `-t`, `--test-name-pattern <re>` | regex | | Only tests whose name (describes and test joined by spaces) matches. |
| `--only` | flag | | Only tests and describes marked `.only`. |
| `--shard <i>/<n>` | `i/n`, `1 ≤ i ≤ n` | | Run shard `i` of `n`. Files are split by `--timings` (longest first onto the least-loaded shard; files without a timing count as the average). |
| `--timings <file>` | path | | Per-file durations, `{"version":1,"files":{"<path>":ms}}`. Without `--shard`, files start longest first. |
| `--update-timings` | flag | | Write this run's per-file durations to `--timings` (needs it; skipped with `-t`, `--only` or a stopped run). |
| `--setup <module>` | path | | Module exporting `setup()`, `browser(result)`, `teardown(result)` (or a default export with them). See [`Setup`](/reference/sdk#e2e-setup). |
| `--preload <module>` | path | | Loaded before the specs; its top-level hooks apply to every test (repeatable). |
| `--browser-arg <arg>` | string | | Extra `pyro serve` argument (repeatable), for example `--browser-arg=--static --browser-arg=http://app.test=./dist`. |
| `--reporter junit` | `junit` \| `console` | `console` | Also write a JUnit report (needs `--reporter-outfile`). |
| `--reporter-outfile <file>` | path | | Where the JUnit report goes. |
| `--junit <file>` | path | | `--reporter junit --reporter-outfile <file>` in one. |
| `-u`, `--update-snapshots` | flag | | Write screenshot baselines instead of comparing (sets `PYRO_UPDATE_SNAPSHOTS=1`). |
| `--bail [n]` | integer | `1` when given | Stop after `n` failures. |
| `-h`, `--help` | flag | | Print the usage and exit `0`. |

Options also take the `--name=value` form.

The browser it launches is `pyro serve --fork --cdp-max-connections <max(64, 4 × concurrency + 8)>` plus the setup module's `browser().args` and every `--browser-arg` (a `--cdp-max-connections` of yours replaces the computed one). If the binary's `version` does not report `fork: true` it warns `has no fork mode; every page boots its profile` and launches without `--fork`.

Exit code: `1` when no spec files are found, no tests match, or any test, hook or file load failed (or a test did not run); else `0`. An empty shard exits `0`. On `SIGINT`/`SIGTERM`/`SIGHUP` it stops starting tests, closes the browser, runs `teardown`, and exits `128 + signal`; a second signal exits at once. `NODE_ENV` defaults to `test`.

```sh
pyro e2e                                    # every spec under the cwd
pyro e2e e2e/positions -t 'closes'          # one directory, matching names
pyro e2e --shard 2/4 --timings e2e/timings.json --junit reports/e2e.xml
pyro e2e --setup e2e/setup.ts --browser-arg=--static --browser-arg=http://app.test=./dist
```
