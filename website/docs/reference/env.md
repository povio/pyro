# Environment variables

Every environment variable Pyro reads, by the component that reads it. The browser binary reads the `PYRO_*` serve, browser and tuning variables and `JSC_*`; the SDK and the runners read `PYRO_BIN`, `PYRO_WS`, the Playwright drop-in's `PYRO_*` variables, `NO_COLOR` and `NODE_ENV`. Releases up to 0.0.33 (`@povio/lightning-panda`) read the same variables as `LIGHTNINGPANDA_*` (browser, `LIGHTNINGPANDA_BIN`, `LIGHTNINGPANDA_WS`) and `LP_*` (Playwright drop-in); see [Renamed from Lightningpanda](/reference/versions#renamed-from-lightningpanda). Variables marked *diagnostic* or *tuning* exist to measure or compare behaviour. They are stable enough to use, but you should not need them.

## Quick reference {#quick-reference}

| Variable | Read by | Default | Purpose |
| --- | --- | --- | --- |
| [`PYRO_BIN`](#pyro-bin) | SDK, CLI, runners | bundled binary | Browser binary to run |
| [`PYRO_WS`](#pyro-ws) | Playwright drop-in | launch one | Connect to a running server |
| [`PYRO_CONFIG`](#pyro-config) | Playwright drop-in | `pyro.config.*`, `playwright.config.*` | Config file |
| [`PLAYWRIGHT_CONFIG`](#pyro-config) | Playwright drop-in | | Fallback for `PYRO_CONFIG` |
| [`PYRO_PROJECT`](#pyro-project) | Playwright drop-in | first project | Project to use |
| [`PYRO_RETRIES`](#pyro-retries) | Playwright drop-in | config | Retries |
| [`PYRO_TIMEOUT`](#pyro-timeout) | Playwright drop-in | config | Test timeout, ms |
| [`PYRO_UPDATE_SNAPSHOTS`](#pyro-update-snapshots) | SDK, Playwright drop-in, `e2e` | unset | Write screenshot baselines |
| [`PYRO_CONCURRENT`](#pyro-concurrent) | Playwright drop-in | unset | Run parallel describes concurrently in a worker |
| [`PYRO_FFMPEG`](#pyro-ffmpeg) | Playwright drop-in | `ffmpeg` on `PATH` | The ffmpeg that encodes `recordVideo` videos |
| [`NO_COLOR`](#no-color) | `run`, `e2e` reporters | unset | Disable colors |
| [`PYRO_LOCAL_STORAGE`](#pyro-local-storage) | `serve` | | `--local-storage` |
| [`PYRO_STATIC`](#pyro-static) | `serve` | | `--static` entries |
| [`PYRO_PREPARE`](#pyro-prepare) | `serve` | | `--prepare` URLs |
| [`PYRO_PREPARE_REFILL`](#pyro-prepare-refill) | `serve` | `quiet` | `--prepare-refill` |
| [`PYRO_BYTECODE_CACHE`](#pyro-bytecode-cache) | `serve`, `fetch` (newer than 0.0.33) | off | `--bytecode-cache` directory |
| [`PYRO_FORK`](#pyro-fork) | `serve` | off | `--fork` |
| [`PYRO_FORK_SPARES`](#pyro-fork-spares) | `serve --fork` | `2` | Forks kept waiting |
| [`PYRO_FORK_PRECOMPILE`](#pyro-fork-precompile) | `serve --fork` | `modules` | Precompile static module scripts |
| [`PYRO_SPARE_ENGINE`](#pyro-spare-engine) | `serve` | on with `--prepare` | `--spare-engine` |
| [`PYRO_ENGINE_REUSE`](#pyro-engine-reuse) | `serve` (newer than 0.0.33) | `always` | `--engine-reuse` |
| [`PYRO_RENDER_PREWARM`](#pyro-render-prewarm) | `serve` | on with `--prepare` | `--render-prewarm` |
| [`PYRO_HYPHEN_DIGIT_BREAKS`](#pyro-hyphen-digit-breaks) | `serve` | `off` | `--hyphen-digit-breaks` |
| [`PYRO_PARENT_PID`](#pyro-parent-pid) | `serve` | set by `launch()` | `--parent-pid` |
| [`PYRO_VIRTUAL_TIME`](#pyro-virtual-time) | browser | off | Jump the clock to the next timer while a run waits |
| [`PYRO_FILE_URLS`](#pyro-file-urls) | browser | on | Serve `file:` URLs |
| [`PYRO_SHARED_ASSET_CACHE_MB`](#pyro-shared-asset-cache-mb) | browser | off | Cross-context cache of public static assets |
| [`PYRO_PDF_SHADOWS`](#pyro-pdf-shadows) | PDF renderer | on | Draw blurred box shadows in PDFs |
| [`JSC_*`](#jsc) | JavaScriptCore | see below | Engine options |

The memory, tuning and diagnostic variables are listed in their own sections below.

## SDK and CLI {#sdk}

### `PYRO_BIN` {#pyro-bin}

Path to a `pyro` binary. The `pyro` npm command, `Pyro.launch()` (when no `bin` option is passed), the Playwright drop-in and the `e2e` runner use it in place of the bundled binary. Use it for a binary you built yourself, or on a platform with no bundled one. Default: a `pyro` next to the package (the release-archive layout), else `native/<platform>-<arch>/pyro` in the package.

```sh
PYRO_BIN=$HOME/src/pyro/target/release/pyro bun test
```

### `PYRO_WS` {#pyro-ws}

The WebSocket endpoint of a running `pyro serve` (for example `ws://127.0.0.1:9222/`). The Playwright drop-in (`@povio/pyro/wright` and `pyro run`) connects to it instead of launching a browser per worker, unless the config's `pyro.wsEndpoint` option is set. `Pyro.connect()` does not read it, but scripts often pass it: `Pyro.connect(process.env.PYRO_WS!)`. See [One server, many workers](/e2e/shared-server).

### `PYRO_CONFIG` {#pyro-config}

Path of the Playwright-style config file the drop-in loads (absolute, or relative to the working directory). `pyro run -c <file>` sets it for its workers. Fallback: `PLAYWRIGHT_CONFIG`, then `pyro.config.{ts,mts,mjs,js}`, then `playwright.config.{ts,mts,mjs,js}` in the working directory. Set it when you run a file with plain `bun test`.

### `PYRO_PROJECT` {#pyro-project}

Name of the config project whose settings apply. Default: the first project. An unknown name throws `pyro: no project named "<name>" in <config file>`. When it is unset and the config has several projects, a file takes the project whose `testDir`/`testMatch` include it.

### `PYRO_RETRIES` {#pyro-retries}

Retries of a failing test, overriding the config and project values. `pyro run --retries <n>` sets it.

### `PYRO_TIMEOUT` {#pyro-timeout}

Test timeout in ms for every project, overriding the config. `pyro run --timeout <ms>` sets it.

### `PYRO_UPDATE_SNAPSHOTS` {#pyro-update-snapshots}

Write screenshot baselines instead of comparing.

| Reader | Values |
| --- | --- |
| SDK `matchScreenshot` steps (`updateMode()`) | `1`, `true`, `all` or `changed` turn update mode on. Passing `--update-snapshots` or `-u` to the process (`bun test -u`) does the same. |
| Playwright drop-in `toHaveScreenshot` / `toMatchSnapshot` | `all`, `changed`, `missing`, `none` (Playwright's `updateSnapshots`). `pyro run -u [mode]` sets it (default `changed`). |
| `pyro e2e -u` | Sets it to `1`. |

### `NO_COLOR` {#no-color}

Any value turns off ANSI colors in the `pyro run` and `pyro e2e` console output. Colors are also off when stdout is not a TTY.

### `NODE_ENV` {#node-env}

`pyro e2e` sets it to `test` if it is unset, as `bun test` does.

### Set by the runners {#runner-internal}

`pyro run` passes these to its `bun test` workers. They are internal: do not set them yourself.

`PYRO_WORKER`, `PYRO_PLAN`, `PYRO_WORKER_INDEX`, `PYRO_WORKER_ID`, `PYRO_RESULTS`, `PYRO_RUN_WORKERS`, `PYRO_SHARD`, `PYRO_GREP`, `PYRO_GREP_INVERT` and `PYRO_LIST`. A run started inside another run does not inherit them.

### `PYRO_CONCURRENT` {#pyro-concurrent}

`1` makes the Playwright drop-in run the tests of a `mode: 'parallel'` describe concurrently inside one worker. By default a file's tests run one after another in their worker, since they may share module state that hooks set. *Experimental.*

### `PYRO_FFMPEG` {#pyro-ffmpeg}

Path of the ffmpeg executable that encodes videos (`recordVideo`, the runner's `video` option). Default: `ffmpeg` on `PATH`, else the one Playwright installs (`playwright install ffmpeg`). When it is set to something that is not an executable, recording fails and the error says so. See [Video](/playwright/runner#video).

## serve {#serve}

Each variable is the default for its flag, and the flag wins. Switches take `on`/`off`, `1`/`0` or `true`/`false`. A value that does not parse is ignored. Full descriptions are in [serve options](/reference/serve).

### `PYRO_LOCAL_STORAGE` {#pyro-local-storage}

Path of the JSON file for [`--local-storage`](/reference/serve#local-storage). Empty means unset.

### `PYRO_STATIC` {#pyro-static}

[`--static`](/reference/serve#static) entries `ORIGIN=DIR`, separated by `;`. `--static` flags add to them. Malformed entries are skipped.

```sh
PYRO_STATIC='http://app.test=./dist;http://cdn.test=./public'
```

### `PYRO_PREPARE` {#pyro-prepare}

[`--prepare`](/reference/serve#prepare) URLs, separated by `;`. `--prepare` flags add to them.

### `PYRO_PREPARE_REFILL` {#pyro-prepare-refill}

`after-close` means [`--prepare-refill after-close`](/reference/serve#prepare-refill). Any other value means `quiet`.

### `PYRO_BYTECODE_CACHE` {#pyro-bytecode-cache}

The directory for [`--bytecode-cache`](/reference/serve#bytecode-cache). `pyro serve` and `pyro fetch` read it. Unset means no cache. Newer than 0.0.33.

### `PYRO_FORK` {#pyro-fork}

`1` means [`--fork`](/reference/serve#fork). Other values are ignored.

### `PYRO_FORK_SPARES` {#pyro-fork-spares}

How many forked processes each fork-mode template keeps waiting for connections. Default `2`. Raise it when many connections open at once.

### `PYRO_FORK_PRECOMPILE` {#pyro-fork-precompile}

What the root fork-mode template does with the module scripts of the [`--static`](/reference/serve#static) roots before its first page. Every fork then loads them already compiled. *Tuning.*

| Value | Effect |
| --- | --- |
| `modules` (default) | Each module is parsed and its top-level code compiled. Functions compile at their first call, in the calling process. |
| `functions` | The code of every function is compiled too. This saves a little CPU per page but costs more memory and page faults. |
| `off`, `0` | Nothing is precompiled. |

### `PYRO_SPARE_ENGINE` {#pyro-spare-engine}

`on`/`off` for [`--spare-engine`](/reference/serve#spare-engine).

### `PYRO_ENGINE_REUSE` {#pyro-engine-reuse}

`on`, `off`, `thread` or `always` (also `1`/`0`, `true`/`false`) for [`--engine-reuse`](/reference/serve#engine-reuse). Default `always`. Newer than 0.0.33: since the first `@povio/pyro` release.

### `PYRO_RENDER_PREWARM` {#pyro-render-prewarm}

`on`/`off` for [`--render-prewarm`](/reference/serve#render-prewarm).

### `PYRO_HYPHEN_DIGIT_BREAKS` {#pyro-hyphen-digit-breaks}

`on`/`off` for [`--hyphen-digit-breaks`](/reference/serve#hyphen-digit-breaks). Default `off`.

### `PYRO_PARENT_PID` {#pyro-parent-pid}

The PID for [`--parent-pid`](/reference/serve#parent-pid). `Pyro.launch()` sets it to its own PID in the server's environment. A fork-mode template removes it from its own environment, so the template ends with its server instead.

## Browser behaviour {#browser}

These are read by the browser in any mode (`serve`, `fetch`, `eval`).

### `PYRO_VIRTUAL_TIME` {#pyro-virtual-time}

`1` turns on virtual time. While an `Pyro.run` waits and its page has nothing left but timers (no task, request or navigation), the clock jumps to the next timer instead of sleeping until it is due. A debounce or a delayed save then costs no wall time. The jump applies to the whole browser thread, so it is meant for fork mode (one page per process). Time never goes back. Default: off.

### `PYRO_FILE_URLS` {#pyro-file-urls}

`0` turns off `file:` URLs. They then fail with `net::ERR_UNKNOWN_URL_SCHEME`. Default: on, so navigations to `file:` and the subresources of `file:` documents read the local filesystem, as Chrome does.

### `PYRO_SHARED_ASSET_CACHE_MB` {#pyro-shared-asset-cache-mb}

A positive integer turns on a process-wide cache of public static assets shared across browser contexts, with that many MiB (each entry up to 16 MiB). It is read once per process. Default: off, so every context keeps its own HTTP cache.

### `PYRO_PDF_SHADOWS` {#pyro-pdf-shadows}

`0` skips blurred outset `box-shadow`s when printing to PDF. Default: drawn.

## Memory and tuning {#tuning}

*Tuning.* These were added to compare behaviours in benchmarks. The defaults are the measured best choice.

| Variable | Default | Effect of setting it |
| --- | --- | --- |
| `PYRO_RELEASE_ON_CLOSE` | on | `0`: closing a page no longer releases its memory (collection and free pages returned). The memory waits for the engine's next collection |
| `PYRO_RELEASE_GROWTH` | `32:25` | `<MB>[:<percent>]`: how far the footprint must grow past its low point before a close collects. `0` collects on every close |
| `PYRO_RELEASE_PUT_OFF` | `1` | How many closes past the gate may leave their collection to an idle moment before one collects at once |
| `PYRO_DECOMMIT` | patched behaviour | `madvise`: keep JavaScriptCore's stock page release. By default, freed pages leave the resident set at once |
| `PYRO_HEAP_ARENA` (macOS, newer than 0.0.33) | on | `0`: use mimalloc's own arenas for the Rust heap instead of one reserved range whose freed pages leave the resident set at once |
| `PYRO_FORK_FAST_EXIT` | on | `0`: a forked connection process tears down as a shared process does instead of exiting at once |
| `PYRO_STATIC_DIRECT` | on | `0`: `--static` hits go through the I/O thread instead of being answered on the browser thread |
| `PYRO_IDLE_COLLECT` (newer than 0.0.33) | on, off in fork-mode forks | `0`: a browser thread that goes quiet with its pages still open no longer collects garbage then. `1`: forked connection processes do it too |
| `PYRO_RASTER_THREADS` (newer than 0.0.33) | one less than the cores, at most 8 | Threads that rasterize large frames (4 million device pixels or more, such as a 3840x2160 screenshot). `0`: every frame is rasterized on the render thread |
| `PYRO_BYTECODE_CACHE_MIN_BYTES` (newer than 0.0.33) | `4096` | Sources shorter than this are not written to the [bytecode cache](/reference/serve#bytecode-cache) |

## Diagnostics {#diagnostics}

*Diagnostic.* Output formats may change between releases.

| Variable | Output |
| --- | --- |
| `PYRO_FORK_STATS=<file>` | JSON lines appended to `<file>`, one per fork, reaped child, child life step and precompile in `serve --fork` |
| `PYRO_ENGINE_REUSE_LOG=1` (newer than 0.0.33) | One stderr line per connection end (`pyro: engine reuse: …`): what became of its engine |
| `PYRO_BYTECODE_CACHE_LOG` (any value, newer than 0.0.33) | One stderr line per [bytecode cache](/reference/serve#bytecode-cache) lookup (`pyro: bytecode cache: hit …`, `miss …`) |
| `PYRO_TRACE_MODULES=1` | One stderr line per step of every ES module load |
| `PYRO_FLOW_STATS=1` | Layout flow statistics |
| `PYRO_GEOMETRY_PROFILE` (any value) | Time spent in layout syncs, whole-document reads and box reads, printed to stderr every 250 syncs |
| `PYRO_GEOMETRY_DEBUG` (any value) | Describes elements whose layout box is empty |
| `PYRO_MIRROR_FULL_LAYOUT` (any value) | The render mirror relays out the whole document on every render instead of incrementally (tells a layout difference from a mirroring one) |
| `PYRO_MIRRORDBG` (any value) | Logs mirror nodes that arrive with unknown children |
| `PYRO_RUN_STEP_TRACE=1` | Per-step trace records of each `Pyro.run` (not `Pyro.act`/`Pyro.expect`) |
| `PYRO_POINTER_WAIT_DIAGNOSTICS` (any value) | Pointer-action timeout messages include why the action was not ready |

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

## Build-time variables {#build}

Read when building Pyro from source, not by a built binary: `PYRO_JSC=system` (link the system JavaScriptCore framework instead of the patched build), `PYRO_JSC_DIR=<dir>` (link the patched JavaScriptCore in `<dir>/lib`) and `PYRO_JSC_CACHE` (the artifact root `scripts/jsc/build.sh` writes to). `PYRO_HEAP_PROFILE` and `PYRO_HEAP_PROFILE_INTERVAL` only work in binaries built with the `heap-profile` feature, which release builds are not.
