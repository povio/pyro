# lightningpanda

A Rust rewrite of the [Lightpanda](https://github.com/lightpanda-io/browser) headless browser on
**JavaScriptCore**. It never uses V8. It targets fast, low-memory browser E2E testing over CDP.

**Status: early bootstrap, not a usable browser yet.** The JavaScriptCore runtime, the event loop and the
page/realm lifecycle are real and tested. The DOM, networking, CDP and document loading are in progress.
[docs/PARITY.md](docs/PARITY.md) lists what exists and what is missing. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
describes the module contract.

## Build (macOS)

```sh
rustup toolchain install stable
cargo build --release
cargo test
./scripts/check-no-v8.sh
```

On macOS every linked executable (`lightningpanda`, the test binaries) is ad hoc re-signed with
`com.apple.security.cs.allow-jit` and nothing else; this runs through `scripts/macos/link-and-sign.sh`, which
`.cargo/config.toml` sets as the linker. Without that entitlement the system JavaScriptCore runs interpreter-only.
Cargo reads `.cargo/config.toml` from the current directory upwards, so run cargo from the repository: a build started
elsewhere (`cargo build --manifest-path ...`) links unsigned, and the `lightningpanda` build script warns about it. If a
binary is copied or re-signed outside Cargo, keep the entitlement:
`codesign -f -s - --entitlements scripts/macos/jit.entitlements <binary>`. With the JIT active, JSC's default
signal-based traps do not stop JIT-compiled loops, so `lp_js::configure_process` (called by `Engine::new` and at the
start of the CLI) sets `JSC_usePollingTraps=true` unless that variable is already set; script timeouts and termination
depend on it.

The release profile keeps `strip = "none"`: Cargo otherwise strips after linking when
`CARGO_PROFILE_RELEASE_DEBUG=0`, which removes the JIT entitlement added by the linker wrapper.
An explicit strip override requires re-signing the final binary afterward. Check this build contract with
`python3 scripts/macos/check-jit-signing.py` (macOS; compiles a small temporary executable).

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

### Fast E2E tests: `LP.run` and `lightningpanda-test`

Page sessions also accept a native test driver, `LP.run {steps}`: a whole
scenario (navigate, locate, click, fill, press, check, read, check requests)
runs on the browser thread and replies once, waiting in place instead of
polling from the client. [`packages/lightningpanda-test`](packages/lightningpanda-test)
is the `bun test` client for it, with Playwright-shaped locators and signed-in
page pools; its [LLM.md](packages/lightningpanda-test/LLM.md) is the guide for
coding agents writing tests with it. It has a builder for every step the
engine runs: pointer actions (click, dblclick, hover with button, position and
force; wheel; raw mouse), typing, blur, uploads, drag, iframe-scoped locators,
style, focus, accessible-name and enabled checks, network mocks (fulfil,
abort, passthrough, patch, hold and release), URL waits, reloads, request
checks on status, body and headers, downloads, popups, viewport and media
emulation, per-page init scripts, and `evaluate`, which awaits a returned
promise. Releases ship it in the archive
(`lightningpanda-test/`, beside the binary) and as
`lightningpanda-test-<version>.tgz` for `bun add`;
`scripts/pack-test-client.sh` builds both. TOS's positions-close test runs in about
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
to be held and returns `{"released": n}`); there is no clock control. A
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
