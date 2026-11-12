# Troubleshooting

Common errors, what they mean, and how to fix them. For failing test steps in particular, see also [Debugging failures](/e2e/debugging).

## The browser does not start

### `Lightningpanda has no bundled browser for <platform>-<arch>`

You are on a platform without a bundled browser, such as Windows or Linux on another architecture. Run the browser on a supported machine and connect to it with `Lightningpanda.connect(ws)` or `LIGHTNINGPANDA_WS`, or point `LIGHTNINGPANDA_BIN` at a browser you built. See [Platforms](/guide/platforms).

### `Missing bundled browser: …/native/<platform>-<arch>/lightningpanda`

The package is installed but its binary is missing. This happens when a tool prunes "unneeded" files from `node_modules`, or when the package was copied without `native/`. Reinstall `@povio/lightning-panda`, or set `LIGHTNINGPANDA_BIN`.

### `lightningpanda serve did not start: …`

`Lightningpanda.launch()` could not start the server or reach it. The message carries the cause. Run the binary yourself to see its output:

```sh
bunx lightningpanda version
bunx lightningpanda serve --port 9222
```

On Linux, an error such as `version 'GLIBC_2.39' not found` means the system's glibc is too old. Use Ubuntu 24.04, Debian 13 or newer. An error about `libfontconfig.so.1` means fontconfig is missing: install `libfontconfig1` (Debian, Ubuntu).

### `lightningpanda test: needs Bun (https://bun.sh) on PATH`

`lightningpanda test` and `lightningpanda e2e` run on Bun. Install Bun, or on CI add `oven-sh/setup-bun`. The client library itself works under Node 22+.

### `Address already in use`

Another process listens on the port. `lightningpanda serve` defaults to 9222, which Chrome's remote debugging also uses. Pick another `--port`. `Lightningpanda.launch()` always picks a free port, so this only affects servers you start yourself.

## Fork mode

### `lightningpanda serve: --fork needs the patched JavaScriptCore`

The binary has no fork mode. The Intel macOS build uses the system JavaScriptCore and cannot fork. Check with:

```sh
bunx lightningpanda version   # look for fork: true
```

On an Intel Mac, drop `--fork` and use [profiles](/guide/profiles) with several connections. `lightningpanda e2e` falls back by itself and prints `has no fork mode; every page boots its profile`.

### `no profile <name>: adopting a profile needs lightningpanda serve --fork`

`lp.adopt(name)`, or `lp.page({ as })` for a name this client never declared, only works against a fork-mode server, where profiles are shared between clients. Against a thread-mode server, declare the profile in each client with `lp.profile()`.

### A profile definition is refused

In fork mode, profile names are shared by every client of the server. Registering a different definition under a name another client holds is refused. Give per-worker or per-file profiles unique names, for example `feature-${process.pid}`.

## Slow tests on macOS

If everything works but JavaScript-heavy pages are many times slower than expected, check that the binary still has its JIT entitlement:

```sh
codesign -d --entitlements - node_modules/@povio/lightning-panda/native/darwin-arm64/lightningpanda
# Intel Macs: native/darwin-x64/lightningpanda
```

Without `com.apple.security.cs.allow-jit`, JavaScriptCore runs only its interpreter. This happens when a binary was re-signed or stripped. See [Platforms](/guide/platforms#the-jit-entitlement-on-macos) to restore it.

Other causes of slow tests:

- **Pages boot for every test.** `lp.stats.fresh` grows. Declare a [profile](/guide/profiles), raise `pool`, or open more `connections`.
- **A step waits the full settle second.** An action started work that never finishes (polling with a short `setTimeout`, a long-lived request). Add `idle({ timeout: 2000 })` after it and read `pending`, then use `{ settle: false }` on the action. See [Settling](/guide/settling).
- **The dev server is slow.** Serve a production build with `--static`. See [Serving a build](/e2e/static).

## `RunError`: a step failed

`page.run` throws a `RunError` with the index of the failing step (`err.step`) and why it was still waiting when it timed out:

```text
step 0 (Click {"name":"Save","role":"button"}): timed out after 5000 ms:
  Click {"name":"Save","role":"button"}: {"name":"Save","role":"button"} matched no element
```

| Reason | Meaning | Fix |
| --- | --- | --- |
| `matched no element` | The locator found nothing before the timeout. | Check the locator, and the step before it: the UI may never have reached this state. |
| `strict mode violation: … matched 3 elements` | An action needs exactly one element. | Scope the locator (`dialog().button(...)`), filter it, use `{ exact: true }`, or `.first()` when any match will do. |
| `<div> intercepts pointer events` | Another element covers the target at its click point. | Close the overlay first, or click what a user would click. |
| `element is disabled`, `not editable`, `not visible` | The element exists but cannot take the action. | Wait for the state with a check, or fix the precondition. |
| `no matching request yet` / `last match was {… status: 500 …}` | A `check.request` did not see the expected request. | The reply's `requests` lists everything the page requested; compare paths and methods. Request checks only see requests made in the same `run`. |
| `expected Visible …, got false` | A check's condition never held. | The app never reached that state, or reached it and left it (see [transient states](/guide/settling#transient-states)). |

Do not fix a `RunError` by raising the timeout. A step waits only for its own condition, so a timeout means the condition never held.

## Missing web APIs

A page that works in Chrome throws `ReferenceError: X is not defined`, or a feature silently does nothing. Pyro's web API coverage is partial. Known absences include WebGL, WebGPU, WebRTC and `SharedWorker`. Check [Web platform support](/reference/web-platform). Options:

- Feature-detect in the app and fall back, as you would for an older browser.
- Stub the API with an init script (`initScripts` in a profile, `page.addInitScript`) when the test does not depend on it.
- Open an issue on the [repository](https://github.com/povio/lightningpanda) with the API and the page that needs it.

CDP commands that are not implemented return an explicit error (`… is not supported by lightningpanda`) rather than a fake success. See [CDP coverage](/reference/cdp).

## Screenshots and PDFs

### Every screenshot fails against my baselines

Baselines taken with Chromium never match: pixels come from Pyro's renderer. Regenerate them with Pyro (`LP_UPDATE_SNAPSHOTS=1`, `bun test -u`, or `lightningpanda test -u`). See [Rendering](/guide/rendering).

### Screenshots differ between my machine and CI

Text uses system fonts when the page does not load its own, and machines have different fonts. Generate baselines on the same OS and font set as CI, or load web fonts in the app.

### `captureScreenshot` or `printToPDF` fails with an explicit "render" error

The browser was built from source without the `render` feature. All bundled browsers have it. Use the bundled browser, or rebuild with rendering enabled.

### Text is missing from screenshots or PDFs on Linux

The container has no fonts. Install fontconfig and a font package (for example `libfontconfig1 fonts-dejavu-core`).

### PDF ignores my print styles

The page prints with its current emulated media, `screen` by default. Emulate `print` media before printing. See [Rendering](/guide/rendering#pdf).

## Connections

### Clients hang when many tests start at once

The server accepts 16 connections at a time by default; more wait in the kernel's backlog until one closes. In fork mode every page is a connection. Raise the limit:

```sh
lightningpanda serve --fork --cdp-max-connections 64
```

`lightningpanda e2e` sets it to at least 64 (or four times the concurrency plus 8) unless you pass your own.

### A browser keeps running after tests

`Lightningpanda.launch()` tells the server to exit with the launching process, however it ends. A server you started yourself with `lightningpanda serve` runs until you stop it. To list running browsers without printing their arguments:

```sh
pgrep -x lightningpanda
```
