# lightningpanda

A Rust rewrite of the [Lightpanda](https://github.com/lightpanda-io/browser) headless browser on
**JavaScriptCore**. It never uses V8. It targets fast, low-memory browser E2E testing over CDP.

**Status: early bootstrap, not a usable browser yet.** The JavaScriptCore runtime, the event loop and the
page/realm lifecycle are real and tested. The DOM, networking, CDP and document loading are in progress.
[docs/PARITY.md](docs/PARITY.md) lists what exists and what is missing. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
describes the module contract.

## npm package

The npm distribution is `@povio/lightning-panda`: one package with the browser
CLI and testing API. It bundles macOS 14+ binaries for Apple Silicon and Intel.

```sh
npm install --save-dev @povio/lightning-panda
npx lightningpanda serve --port 9222
```

Install `@povio/lightning-panda@next` for release candidates. The API supports
Node.js 22+ and Bun; remote CDP connections work from other operating systems.
The bundled browsers use system JavaScriptCore; fork mode requires a separately
built patched browser selected with `LIGHTNINGPANDA_BIN`.
See [the package README](packages/lightning-panda/README.md) and
[publishing instructions](docs/NPM_PUBLISHING.md).

## Build (macOS)

```sh
rustup toolchain install stable
cargo build --release
cargo test
./scripts/check-no-v8.sh
```

Screenshots (`Page.captureScreenshot`, `LP.run` `matchScreenshot`) need the `render` feature, off by default:
`cargo build --release --features render`. It adds lp-render (Blitz, Stylo, Vello's CPU rasterizer: about 200
crates), about a minute to a release build and 15 MB to the binary (15 to 30 MB on an M1 Max). Release archives
that ship the test client should be built with it. `lp-render` is not a default workspace member, so test it with
`cargo test -p lp-render -p lp-cdp --features lp-cdp/render`. See [docs/automation/SCREENSHOTS.md](docs/automation/SCREENSHOTS.md).

On macOS every linked executable (`lightningpanda`, the test binaries) is ad hoc re-signed with
`com.apple.security.cs.allow-jit` and nothing else; `.cargo/config.toml` passes `-C link-arg=-Bscripts/macos/linker`,
so `cc` links through `scripts/macos/linker/ld`, which does this. Without that entitlement the system JavaScriptCore
runs interpreter-only. Cargo reads `.cargo/config.toml` from the current directory upwards, so run cargo from the
repository: a build started elsewhere (`cargo build --manifest-path ...`) links unsigned, and so does one with
`RUSTFLAGS` set, which replaces the config's flags (add `-Clink-arg=-Bscripts/macos/linker` to it). The
`lightningpanda` build script warns in both cases. If a binary is copied or re-signed outside Cargo, keep the
entitlement: `codesign -f -s - --entitlements scripts/macos/jit.entitlements <binary>`. With the JIT active, JSC's default
signal-based traps do not stop JIT-compiled loops, so `lp_js::configure_process` (called by `Engine::new` and at the
start of the CLI) sets `JSC_usePollingTraps=true` unless that variable is already set; script timeouts and termination
depend on it.

The release profile keeps `strip = "none"`: Cargo otherwise strips after linking when
`CARGO_PROFILE_RELEASE_DEBUG=0`, which removes the JIT entitlement added by the linker wrapper.
An explicit strip override requires re-signing the final binary afterward. Check this build contract with
`python3 scripts/macos/check-jit-signing.py` (macOS; compiles a small temporary executable).

### Sharing dependency builds across worktrees

Each checkout or worktree has its own `target/`, so a new one compiles every dependency again.
[sccache](https://github.com/mozilla/sccache) caches those compiles across checkouts; the build passes no
checkout path to dependencies' rustc commands, so the cache hits from any path. One-time setup:

```sh
brew install sccache
printf '[build]\nrustc-wrapper = "sccache"\n' >> ~/.cargo/config.toml   # or merge into an existing [build]
```

Measured with `cargo build --workspace --tests` on an M1 Max (10 cores, other builds running): a second worktree at another path gets 83% cache
hits and builds in 29-38 s instead of 44-46 s. The misses are dependencies whose inputs carry the checkout path:
crates that include build-script output from `OUT_DIR`, and users of proc-macros, which sccache cannot cache and
whose dylibs record where they were linked. Workspace crates are compiled incrementally, which
sccache passes through, so edit rebuilds are unchanged (lp-dom edit: 4.6-5.3 s with it, 5.8-5.9 s without).
`sccache --show-stats` shows the hit rate.

## Usage

```sh
lightningpanda version
lightningpanda eval -e 'setTimeout(() => console.log("hi"), 10)'
lightningpanda eval script.js --timeout-ms 5000
lightningpanda serve --port 9222
# Test runs that need no front-end server and no per-context auth setup:
# serve a build directory in place of its origin and preload localStorage.
lightningpanda serve --static http://localhost:3000=/path/to/dist \
  --local-storage auth.json   # {"http://localhost:3000": {"token": "..."}}
```

`serve --help` lists every option. `--static` and `--local-storage` also read
`LIGHTNINGPANDA_STATIC` (entries separated by `;`) and `LIGHTNINGPANDA_LOCAL_STORAGE`
from the environment, for launchers that cannot pass flags.

To hide the app's own boot time as well, `--prepare http://localhost:3000/dashboard`
(env `LIGHTNINGPANDA_PREPARE`) keeps one page per connection booted at that URL
before any client connects and boots the next one whenever the thread is quiet
or the connection has no open page.
A test takes it with the browser-level CDP command
`Lightningpanda.acquirePreparedPage {url}`: the page is announced like a new
target, so Playwright sees it as a page of its default context, already loaded.
`Lightningpanda.registerPreparedPage {url, initScripts, viewport, pool}` declares
a profile at run time, including the scripts a test would otherwise install
with `addInitScript`; `Lightningpanda.preparedPages` lists what is waiting.

Memory: every booted page is a full copy of the app, so a pool of N holds N
of them plus one in use. `--prepare-refill after-close` (or `refillAfterClose`
in the profile) boots the replacement only once the acquired page's context
closes, capping the count at the pool size; the default boots it as soon as
the thread is quiet. Closed pages are collected and their memory returned to
the OS right away (`LIGHTNINGPANDA_RELEASE_ON_CLOSE=0` disables that for
comparisons); a 20-test loop on a 2 MB app plateaus at 275 MB instead of 323.

`--fork` (env `LIGHTNINGPANDA_FORK=1`, patched JavaScriptCore only:
`scripts/jsc/build.sh`) goes further: one template process boots each
prepared profile once, and every connection is a `fork()` of it, starting
with those pages booted and sharing their memory copy-on-write. Open one
connection per test; it costs about 20 ms however long the app takes to boot,
and its process exits when the connection closes. `@povio/lightning-panda` does
this by itself when the server reports `Lightningpanda.capabilities {fork: true}`.

### Fast E2E tests: `LP.run` and `@povio/lightning-panda`

Page sessions also accept a native test driver, `LP.run {steps}`: a whole
scenario (navigate, locate, click, fill, press, check, read, check requests)
runs on the browser thread and replies once, waiting in place instead of
polling from the client. [`packages/lightning-panda`](packages/lightning-panda)
is the `bun test` client for it, with Playwright-shaped locators and signed-in
page pools; its [LLM.md](packages/lightning-panda/LLM.md) is the guide for
coding agents writing tests with it. It has a builder for every step the
engine runs: pointer actions (click, dblclick, hover with button, position and
force; wheel; raw mouse), typing, blur, uploads, drag, iframe-scoped locators,
style, focus, accessible-name and enabled checks, network mocks (fulfil,
abort, passthrough, patch, hold and release), URL waits, reloads, request
checks on status, body and headers, downloads, popups, viewport and media
emulation, per-page init scripts, and `evaluate`, which awaits a returned
promise. The npm package `@povio/lightning-panda` ships the client and browser
together. `scripts/pack-test-client.sh` packs staged macOS builds into
`povio-lightning-panda-<version>.tgz` and can unpack it into a legacy release
archive's `lightning-panda/` directory. TOS's positions-close test runs in about
3 ms per test this way, against about 34 ms in Playwright
(`bench/results/lp-tos-e501cef`).

Network steps on the wire (engine side; the client builds them):

```json
{"mock": {"url": "/api/orders", "passthrough": true,
          "patch": {"status": 201, "headers": {"x-a": "1"}, "json": {"total": 0, "items": null}}}}
{"mock": {"url": {"regex": "/api/orders(?!/draft)"}, "hold": "save", "json": {"ok": true}}}
{"release": {"hold": "save"}}
{"request": {"method": "PATCH", "path": "/api/orders", "state": "started", "count": 1,
             "body": {"regex": "\"remarks\""}, "bodyKeys": ["remarks"],
             "headers": {"x-idempotency-key": {"regex": "^k\\d$"}}}}
```

`patch` implies `passthrough` (the real response is rewritten; `json` is a
JSON merge patch onto a JSON object body); `hold` parks the request, or with
`passthrough` its real response, until `release` (which waits for something
to be held and returns `{"released": n}`). Explicit `{"advanceTime":{"ms":2000}}`
steps advance page Date and timers, bounded to one day per step. A
`request` step (default `state: "finished"`) and the run's `requests` carry
`requestHeaders`, `postData` (`postDataBase64` / `postDataTruncated` past
1 MiB) and `responseHeaders`. `{"regex", "flags"}` patterns have ECMAScript
semantics (look-around, backreferences; flags `i`, `m`, `s`).

After each action a step waits until the page is quiet: no queued task, no
request the action started, no `setTimeout` due within 50 ms, no pending
animation frame (animation loops excepted) and no navigation short of
DOMContentLoaded. The same wait is an `idle` step, or `LP.idle {timeout}` on
its own; when it gives up, the result says what was still `pending`.

Pointer actions follow Playwright: a locator `act` with `click`, `dblclick`,
`hover` or `wheel` aims at the center of the element's visible part (clipped
to the viewport and its overflow containers) after scrolling it into view;
`button: "right" | "middle"` (click, dblclick; a right click fires
`contextmenu` after the press, both fire `auxclick` after the release), `position: {x, y}` (an offset from
the element's top-left) and `force: true` (no hit test or enabled check)
adjust that, and `wheel` takes `deltaX`/`deltaY`. Only click, dblclick, fill,
check, uncheck, press and type need an enabled element; hover does not. The
`mouse` step is `page.mouse.*` at viewport coordinates:
`{"mouse": {"action": "click" | "dblclick" | "move" | "down" | "up" | "wheel",
"x", "y", "button", "deltaX", "deltaY", "steps"}}`, where `move` with `steps`
sends intermediate moves and `down`, `up` and `wheel` default to the current
pointer position.

Viewport, media and page steps:

```json
{"viewport": {"width": 390, "height": 844}}
{"media": {"colorScheme": "dark", "reducedMotion": "reduce", "media": "print"}}
{"popup": {"as": "tab"}}
{"page": {"target": "tab"}}
{"page": {"target": "main"}}
```

`viewport` resizes the page mid-run: `innerWidth`/`innerHeight` and layout
follow at once, `resize` and the `change` of every `matchMedia` list it
flipped fire, and the step settles like an action (its result is
`{"width", "height", "settleMs"}`). `media` replaces the page's media
emulation, as `Emulation.setEmulatedMedia` does: `colorScheme` (`light`,
`dark`, `no-preference`), `reducedMotion` (`reduce`, `no-preference`) and
`media` (`screen`, `print`); an omitted field resets to its default, and
`@media` rules, `matchMedia(...).matches` and its `change` listeners
(`MediaQueryListEvent`) follow. `page` makes the following steps drive
another page of the run's browser context: the `as` key of an earlier
`popup` step, a target id, or `main` for the page the run started on;
`request` and `download` steps then see that page's activity, and the
reply's `requests` stay the starting page's. A popup paused by
`Target.setAutoAttach {waitForDebuggerOnStart: true}` navigates only after
the client's `Runtime.runIfWaitingForDebugger`.

## License

AGPL-3.0-only. This is a derivative of Lightpanda; see [NOTICE](NOTICE).
