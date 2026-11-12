# Environment variables

Every environment variable Pyro reads, by the component that reads it. The browser binary reads the `LIGHTNINGPANDA_*` and `JSC_*` variables, the SDK and the runners read `LIGHTNINGPANDA_BIN`, `LIGHTNINGPANDA_WS` and the `LP_*` variables. Variables marked *diagnostic* or *tuning* exist to measure or compare behaviour. They are stable enough to use, but you should not need them.

## Quick reference {#quick-reference}

| Variable | Read by | Default | Purpose |
| --- | --- | --- | --- |
| [`LIGHTNINGPANDA_BIN`](#lightningpanda-bin) | SDK, CLI, runners | bundled binary | Browser binary to run |
| [`LIGHTNINGPANDA_WS`](#lightningpanda-ws) | Playwright drop-in | launch one | Connect to a running server |
| [`LP_CONFIG`](#lp-config) | Playwright drop-in | `lightningpanda.config.*`, `playwright.config.*` | Config file |
| [`PLAYWRIGHT_CONFIG`](#lp-config) | Playwright drop-in | | Fallback for `LP_CONFIG` |
| [`LP_PROJECT`](#lp-project) | Playwright drop-in | first project | Project to use |
| [`LP_RETRIES`](#lp-retries) | Playwright drop-in | config | Retries |
| [`LP_TIMEOUT`](#lp-timeout) | Playwright drop-in | config | Test timeout, ms |
| [`LP_UPDATE_SNAPSHOTS`](#lp-update-snapshots) | SDK, Playwright drop-in, `e2e` | unset | Write screenshot baselines |
| [`LP_CONCURRENT`](#lp-concurrent) | Playwright drop-in | unset | Run parallel describes concurrently in a worker |
| [`NO_COLOR`](#no-color) | `test`, `e2e` reporters | unset | Disable colors |
| [`LIGHTNINGPANDA_LOCAL_STORAGE`](#lightningpanda-local-storage) | `serve` | | `--local-storage` |
| [`LIGHTNINGPANDA_STATIC`](#lightningpanda-static) | `serve` | | `--static` entries |
| [`LIGHTNINGPANDA_PREPARE`](#lightningpanda-prepare) | `serve` | | `--prepare` URLs |
| [`LIGHTNINGPANDA_PREPARE_REFILL`](#lightningpanda-prepare-refill) | `serve` | `quiet` | `--prepare-refill` |
| [`LIGHTNINGPANDA_FORK`](#lightningpanda-fork) | `serve` | off | `--fork` |
| [`LIGHTNINGPANDA_FORK_SPARES`](#lightningpanda-fork-spares) | `serve --fork` | `2` | Forks kept waiting |
| [`LIGHTNINGPANDA_FORK_PRECOMPILE`](#lightningpanda-fork-precompile) | `serve --fork` | `modules` | Precompile static module scripts |
| [`LIGHTNINGPANDA_SPARE_ENGINE`](#lightningpanda-spare-engine) | `serve` | on with `--prepare` | `--spare-engine` |
| [`LIGHTNINGPANDA_ENGINE_REUSE`](#lightningpanda-engine-reuse) | `serve` (main) | `on` | `--engine-reuse` |
| [`LIGHTNINGPANDA_RENDER_PREWARM`](#lightningpanda-render-prewarm) | `serve` | on with `--prepare` | `--render-prewarm` |
| [`LIGHTNINGPANDA_HYPHEN_DIGIT_BREAKS`](#lightningpanda-hyphen-digit-breaks) | `serve` | `off` | `--hyphen-digit-breaks` |
| [`LIGHTNINGPANDA_PARENT_PID`](#lightningpanda-parent-pid) | `serve` | set by `launch()` | `--parent-pid` |
| [`LIGHTNINGPANDA_VIRTUAL_TIME`](#lightningpanda-virtual-time) | browser | off | Jump the clock to the next timer while a run waits |
| [`LIGHTNINGPANDA_FILE_URLS`](#lightningpanda-file-urls) | browser | on | Serve `file:` URLs |
| [`LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB`](#lightningpanda-shared-asset-cache-mb) | browser | off | Cross-context cache of public static assets |
| [`LIGHTNINGPANDA_PDF_SHADOWS`](#lightningpanda-pdf-shadows) | PDF renderer | on | Draw blurred box shadows in PDFs |
| [`JSC_*`](#jsc) | JavaScriptCore | see below | Engine options |

The memory, tuning and diagnostic variables are listed in their own sections below.

## SDK and CLI {#sdk}

### `LIGHTNINGPANDA_BIN` {#lightningpanda-bin}

Path to a `lightningpanda` binary. The `lightningpanda` npm command, `Lightningpanda.launch()` (when no `bin` option is passed), the Playwright drop-in and the `e2e` runner use it in place of the bundled binary. Use it for a binary you built yourself, or on a platform with no bundled one. Default: a `lightningpanda` next to the package (the release-archive layout), else `native/<platform>-<arch>/lightningpanda` in the package.

```sh
LIGHTNINGPANDA_BIN=$HOME/src/lightningpanda/target/release/lightningpanda bun test
```

### `LIGHTNINGPANDA_WS` {#lightningpanda-ws}

The WebSocket endpoint of a running `lightningpanda serve` (for example `ws://127.0.0.1:9222/`). The Playwright drop-in (`@povio/lightning-panda/playwright` and `lightningpanda test`) connects to it instead of launching a browser per worker, unless the config's `lightningpanda.wsEndpoint` option is set. `Lightningpanda.connect()` does not read it, but scripts often pass it: `Lightningpanda.connect(process.env.LIGHTNINGPANDA_WS!)`. See [One server, many workers](/e2e/shared-server).

### `LP_CONFIG` {#lp-config}

Path of the Playwright-style config file the drop-in loads (absolute, or relative to the working directory). `lightningpanda test -c <file>` sets it for its workers. Fallback: `PLAYWRIGHT_CONFIG`, then `lightningpanda.config.{ts,mts,mjs,js}`, then `playwright.config.{ts,mts,mjs,js}` in the working directory. Set it when you run a file with plain `bun test`.

### `LP_PROJECT` {#lp-project}

Name of the config project whose settings apply. Default: the first project. An unknown name throws `no project named "<name>"`. When it is unset and the config has several projects, a file takes the project whose `testDir`/`testMatch` include it.

### `LP_RETRIES` {#lp-retries}

Retries of a failing test, overriding the config and project values. `lightningpanda test --retries <n>` sets it.

### `LP_TIMEOUT` {#lp-timeout}

Test timeout in ms for every project, overriding the config. `lightningpanda test --timeout <ms>` sets it.

### `LP_UPDATE_SNAPSHOTS` {#lp-update-snapshots}

Write screenshot baselines instead of comparing.

| Reader | Values |
| --- | --- |
| SDK `matchScreenshot` steps (`updateMode()`) | `1`, `true`, `all` or `changed` turn update mode on. Passing `--update-snapshots` or `-u` to the process (`bun test -u`) does the same. |
| Playwright drop-in `toHaveScreenshot` / `toMatchSnapshot` | `all`, `changed`, `missing`, `none` (Playwright's `updateSnapshots`). `lightningpanda test -u [mode]` sets it (default `changed`). |
| `lightningpanda e2e -u` | Sets it to `1`. |

### `NO_COLOR` {#no-color}

Any value turns off ANSI colors in the `lightningpanda test` and `lightningpanda e2e` console output. Colors are also off when stdout is not a TTY.

### `NODE_ENV` {#node-env}

`lightningpanda e2e` sets it to `test` if it is unset, as `bun test` does.

### Set by the runners {#runner-internal}

`lightningpanda test` passes these to its `bun test` workers. They are internal: do not set them yourself.

`LP_WORKER`, `LP_WORKERS`, `LP_WORKER_INDEX`, `LP_RESULTS`, `LP_FILE_PROJECTS`, `LP_FILE_OWNERS`, `LP_FULLY_PARALLEL`, `LP_GREP`, `LP_GREP_INVERT` and `LP_LIST`.

### `LP_CONCURRENT` {#lp-concurrent}

`1` makes the Playwright drop-in run the tests of a `mode: 'parallel'` describe concurrently inside one worker. By default a file's tests run one after another in their worker, since they may share module state that hooks set. *Experimental.*

## serve {#serve}

Each variable is the default for its flag, and the flag wins. Switches take `on`/`off`, `1`/`0` or `true`/`false`. A value that does not parse is ignored. Full descriptions are in [serve options](/reference/serve).

### `LIGHTNINGPANDA_LOCAL_STORAGE` {#lightningpanda-local-storage}

Path of the JSON file for [`--local-storage`](/reference/serve#local-storage). Empty means unset.

### `LIGHTNINGPANDA_STATIC` {#lightningpanda-static}

[`--static`](/reference/serve#static) entries `ORIGIN=DIR`, separated by `;`. `--static` flags add to them. Malformed entries are skipped.

```sh
LIGHTNINGPANDA_STATIC='http://app.test=./dist;http://cdn.test=./public'
```

### `LIGHTNINGPANDA_PREPARE` {#lightningpanda-prepare}

[`--prepare`](/reference/serve#prepare) URLs, separated by `;`. `--prepare` flags add to them.

### `LIGHTNINGPANDA_PREPARE_REFILL` {#lightningpanda-prepare-refill}

`after-close` means [`--prepare-refill after-close`](/reference/serve#prepare-refill). Any other value means `quiet`.

### `LIGHTNINGPANDA_FORK` {#lightningpanda-fork}

`1` means [`--fork`](/reference/serve#fork). Other values are ignored.

### `LIGHTNINGPANDA_FORK_SPARES` {#lightningpanda-fork-spares}

How many forked processes each fork-mode template keeps waiting for connections. Default `2`. Raise it when many connections open at once.

### `LIGHTNINGPANDA_FORK_PRECOMPILE` {#lightningpanda-fork-precompile}

What the root fork-mode template does with the module scripts of the [`--static`](/reference/serve#static) roots before its first page. Every fork then loads them already compiled. *Tuning.*

| Value | Effect |
| --- | --- |
| `modules` (default) | Each module is parsed and its top-level code compiled. Functions compile at their first call, in the calling process. |
| `functions` | The code of every function is compiled too. This saves a little CPU per page but costs more memory and page faults. |
| `off`, `0` | Nothing is precompiled. |

### `LIGHTNINGPANDA_SPARE_ENGINE` {#lightningpanda-spare-engine}

`on`/`off` for [`--spare-engine`](/reference/serve#spare-engine).

### `LIGHTNINGPANDA_ENGINE_REUSE` {#lightningpanda-engine-reuse}

`on`, `off`, `thread` or `always` for [`--engine-reuse`](/reference/serve#engine-reuse). Only on `main`, newer than 0.0.33.

### `LIGHTNINGPANDA_RENDER_PREWARM` {#lightningpanda-render-prewarm}

`on`/`off` for [`--render-prewarm`](/reference/serve#render-prewarm).

### `LIGHTNINGPANDA_HYPHEN_DIGIT_BREAKS` {#lightningpanda-hyphen-digit-breaks}

`on`/`off` for [`--hyphen-digit-breaks`](/reference/serve#hyphen-digit-breaks). Default `off`.

### `LIGHTNINGPANDA_PARENT_PID` {#lightningpanda-parent-pid}

The PID for [`--parent-pid`](/reference/serve#parent-pid). `Lightningpanda.launch()` sets it to its own PID in the server's environment. A fork-mode template removes it from its own environment, so the template ends with its server instead.

## Browser behaviour {#browser}

These are read by the browser in any mode (`serve`, `fetch`, `eval`).

### `LIGHTNINGPANDA_VIRTUAL_TIME` {#lightningpanda-virtual-time}

`1` turns on virtual time. While an `LP.run` waits and its page has nothing left but timers (no task, request or navigation), the clock jumps to the next timer instead of sleeping until it is due. A debounce or a delayed save then costs no wall time. The jump applies to the whole browser thread, so it is meant for fork mode (one page per process). Time never goes back. Default: off.

### `LIGHTNINGPANDA_FILE_URLS` {#lightningpanda-file-urls}

`0` turns off `file:` URLs. They then fail with `net::ERR_UNKNOWN_URL_SCHEME`. Default: on, so navigations to `file:` and the subresources of `file:` documents read the local filesystem, as Chrome does.

### `LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB` {#lightningpanda-shared-asset-cache-mb}

A positive integer turns on a process-wide cache of public static assets shared across browser contexts, with that many MiB (each entry up to 16 MiB). It is read once per process. Default: off, so every context keeps its own HTTP cache.

### `LIGHTNINGPANDA_PDF_SHADOWS` {#lightningpanda-pdf-shadows}

`0` skips blurred outset `box-shadow`s when printing to PDF. Default: drawn.

## Memory and tuning {#tuning}

*Tuning.* These were added to compare behaviours in benchmarks. The defaults are the measured best choice.

| Variable | Default | Effect of setting it |
| --- | --- | --- |
| `LIGHTNINGPANDA_RELEASE_ON_CLOSE` | on | `0`: closing a page no longer releases its memory (collection and free pages returned). The memory waits for the engine's next collection |
| `LIGHTNINGPANDA_RELEASE_GROWTH` | `32:25` | `<MB>[:<percent>]`: how far the footprint must grow past its low point before a close collects. `0` collects on every close |
| `LIGHTNINGPANDA_RELEASE_PUT_OFF` | `1` | How many closes past the gate may leave their collection to an idle moment before one collects at once |
| `LIGHTNINGPANDA_DECOMMIT` | patched behaviour | `madvise`: keep JavaScriptCore's stock page release. By default, freed pages leave the resident set at once |
| `LIGHTNINGPANDA_HEAP_ARENA` (main, macOS) | on | `0`: use mimalloc's own arenas for the Rust heap instead of one reserved range whose freed pages leave the resident set at once |
| `LIGHTNINGPANDA_FORK_FAST_EXIT` | on | `0`: a forked connection process tears down as a shared process does instead of exiting at once |
| `LIGHTNINGPANDA_STATIC_DIRECT` | on | `0`: `--static` hits go through the I/O thread instead of being answered on the browser thread |

## Diagnostics {#diagnostics}

*Diagnostic.* Output formats may change between releases.

| Variable | Output |
| --- | --- |
| `LIGHTNINGPANDA_FORK_STATS=<file>` | JSON lines appended to `<file>`, one per fork, reaped child, child life step and precompile in `serve --fork` |
| `LIGHTNINGPANDA_ENGINE_REUSE_LOG=1` (main) | One stderr line per connection end: what became of its engine |
| `LIGHTNINGPANDA_TRACE_MODULES=1` | One stderr line per step of every ES module load |
| `LIGHTNINGPANDA_FLOW_STATS=1` | Layout flow statistics |
| `LP_RUN_STEP_TRACE=1` | Per-step trace records of each `LP.run` (not `LP.act`/`LP.expect`) |
| `LP_POINTER_WAIT_DIAGNOSTICS` (any value) | Pointer-action timeout messages include why the action was not ready |

## JavaScriptCore options {#jsc}

JavaScriptCore reads `JSC_<option>` variables once, when it creates its first VM. Pyro sets some defaults for itself unless the variable is already set in the environment:

| Variable | Pyro's default | Why |
| --- | --- | --- |
| `JSC_usePollingTraps` | `true` | The watchdog (`--timeout-ms`, termination) must reach JIT-compiled loops |
| `JSC_heapGrowthMaxIncrease` | `1` (`3` in fork-mode forks) | Lower heap peaks. In forks, fewer collections for short-lived processes |
| `JSC_smallHeapSize` | `4194304` | 4 MB between collections instead of 1 MB. `1048576` restores the old trade-off |
| `JSC_useMachForExceptions` | `false` (processes that fork) | Mach exception ports do not survive `fork()` |
| `JSC_useDFGJIT` | `false` in fork-mode forks | DFG/FTL compilation rarely pays off in a short connection. `JSC_useDFGJIT=true` restores the default tiers |
| `JSC_jitMemoryReservationSize` | `33554432` in fork-mode forks | 32 MB JIT pool instead of 512 MB, so `fork()` is faster on macOS |

Any other `JSC_*` option reaches JavaScriptCore unchanged. Options you set this way are outside what Pyro tests.
