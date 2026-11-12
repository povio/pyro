# How it works

This page explains the parts of Pyro a test author or script writer runs into: the engine, the threads, where a test's steps run, and why that makes tests fast. It is a user's view of the architecture; the full module contract is `docs/ARCHITECTURE.md` in the repository.

## The pieces

```text
  your test / script                       pyro serve
 ┌─────────────────────┐   WebSocket    ┌──────────────────────────────────────────────┐
 │ Playwright module   │   (CDP JSON)   │ I/O thread: accepts sockets, HTTP discovery  │
 │ native SDK          │ ─────────────► │                                              │
 │ Puppeteer, raw CDP  │                │ browser thread per connection:               │
 └─────────────────────┘                │   JavaScriptCore engine (one VM)             │
                                        │   DOM (Rust arena), Stylo styles             │
                                        │   event loop, CDP domains, Pyro.run driver     │
                                        │                                              │
                                        │ network thread (HTTP, cookies, cache)        │
                                        │ render thread per connection (Blitz, Vello)  │
                                        └──────────────────────────────────────────────┘
```

| Part | What it is |
| --- | --- |
| Language | Rust. V8 is forbidden; a script in the repository checks the dependency graph for it. |
| JavaScript | JavaScriptCore. On Apple silicon and Linux, a patched build that ships with the browser; on Intel Macs, macOS's system framework. |
| DOM | A Rust arena DOM parsed with html5ever. JavaScript objects are thin wrappers over Rust nodes. |
| Styles | Stylo, the style engine Firefox and Servo use, computes the cascade (`getComputedStyle`, `:hover`, media queries). |
| Layout | Blitz with Taffy (flex, grid, block) and Parley (text). Layout queries such as `getBoundingClientRect`, scrolling and hit testing are answered from the same layout that screenshots and PDFs paint. |
| Painting | Vello's CPU rasterizer for PNG screenshots; a PDF writer for `Page.printToPDF`. See [Rendering](/guide/rendering). |
| Network | An HTTP client on its own I/O thread: cookies, CORS, redirects, proxies, interception for `page.route` and mocks. |
| Protocol | A CDP server compatible with what Playwright 1.59 sends, plus the `Pyro.*` and `Pyro.*` domains. |

## One browser thread per connection

Each CDP connection gets its own browser thread. That thread owns a JavaScriptCore engine (one VM and heap), the DOM of every page opened on the connection, and an event loop. Nothing in JavaScript or the DOM is shared between threads, so nothing needs locks.

The browser thread also serves its own socket. It reads a command, runs it, and writes the reply and events, all in order, on the same thread. While JavaScript is running, a watchdog keeps reading the socket, so a client that closes or times out stops a runaway script.

Consequences for you:

- **Connections run in parallel.** Two connections are two threads and use two cores. The native SDK's `connect(ws, { connections: 4 })` opens four. A server accepts 16 connections by default (`--cdp-max-connections`).
- **Pages on one connection share a thread.** A page booting on the same connection can delay a test there. Spread work over connections, or use fork mode.
- **A connection's state ends with it.** When a connection closes, its pages, contexts and timers are disposed.

Dedicated workers run on threads of their own, each with its own engine.

## Where `Pyro.run` runs

The native SDK's `page.run([...steps])` is one CDP command, `Pyro.run`. It runs on the page's browser thread, between event loop tasks:

1. Each step's locator is resolved against the Rust DOM, with no JavaScript involved. Role and accessible-name matching, text matching, visibility and hit testing are Rust code.
2. If a step cannot finish yet (the element does not exist, is covered, the text does not match), the run parks. It is tried again only when the page's DOM version changes or its document is replaced. Nothing polls.
3. After an action, the next step waits until the page is quiet: no queued task, no request the action started, no timer due within 50 ms, no pending animation frame, no navigation short of DOMContentLoaded. See [Settling](/guide/settling).
4. When the last step holds, or one fails or times out, the browser replies once with values, per-step results and the requests the page made.

Input is delivered like Chromium delivers it: each input event of a pointer action (the move, the press, the release) is dispatched in its own task, with an event loop turn between them, so handlers that schedule work between press and release see it run.

The Playwright module uses the same machinery: each awaited Playwright call is one `Pyro.run`.

## Why it is fast

A Playwright test against Chrome sends many CDP messages per action: resolve a selector, check visibility, scroll, compute a click point, dispatch mouse events, then poll for an assertion. Each message crosses a socket and wakes two processes. Pyro removes most of that:

- **Fewer round trips.** A native test is usually one command. A Playwright-module test is one command per awaited call.
- **No polling.** Waiting steps resume on DOM changes, from inside the browser.
- **Locators in Rust.** No injected script and no JavaScript per lookup.
- **No app boot per test.** [Profiles](/guide/profiles) keep pages booted ahead; a test that takes one changes the route in place with `pushState`, so the app does not load again.
- **No boot at all with fork mode.** [Fork mode](/guide/fork-mode) boots each profile once in a template process and forks it for every connection, copy-on-write.
- **Small processes.** No GPU process, no renderer processes, no compositor. Screenshots and PDFs are rendered only when asked for.

## Fork mode in one paragraph

With `serve --fork`, the server starts a template process that boots one page per profile and waits. Each new connection is handed to the template, which calls `fork()`. The child serves that one connection with every profile's page already booted, shares memory with the template until it writes to it, and exits when the connection closes. JavaScriptCore's DFG and FTL optimizing tiers are off in forks, since a connection lives too briefly for them to pay off. Details, numbers and limits are in [Fork mode](/guide/fork-mode).

## Lifetimes

- `Pyro.launch()` passes its own process id with `--parent-pid`. The server exits when that process exits, however it ends, so a crashed test runner does not leave browsers behind.
- A fork-mode server also stops its template and removes its socket files when it exits, and cleans up those of servers that were killed.
- Closed pages are collected and their memory returned to the operating system right away. `PYRO_RELEASE_ON_CLOSE=0` turns that off, for comparisons.

## Where it differs from Chrome

The architecture explains most behaviour differences:

- Layout comes from Blitz, not Blink. Positions and sizes are close to Chrome's for common layouts and can differ in edge cases. Table column widths are a known difference.
- JavaScript runs on JavaScriptCore, not V8.
- Settling means an action returns later than in Chrome, after the app has reacted. A test that wants to see a short-lived state between the action and its result can miss it. See [Settling](/guide/settling#transient-states).

See [Behaviour differences](/playwright/differences) and [Web platform support](/reference/web-platform).
