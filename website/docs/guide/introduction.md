# What is Pyro?

Pyro is a headless browser written in Rust. It runs JavaScript on JavaScriptCore, the engine behind Safari, and never uses V8. Clients drive it over the Chrome DevTools Protocol (CDP). It is built for one job above all others: running browser end-to-end tests quickly and with little memory.

It ships as one npm package, `@povio/pyro`. The package holds the browser binaries for macOS and Linux, the `pyro` CLI, a typed client, a Playwright-compatible test API and a suite runner. You do not need a Rust toolchain, Chrome or Playwright to use it.

::: info Formerly Lightningpanda
Pyro was called Lightningpanda up to version 0.0.33, published as `@povio/lightning-panda` with the `lightningpanda` command. From the next release the package is `@povio/pyro`; every name moved with it (class `Pyro`, `PYRO_*` environment variables, the `Pyro.*` CDP domain, `@povio/pyro/wright` and `pyro run` for Playwright tests). The old package keeps its old names. See [Versions](/reference/versions#renamed-from-lightningpanda).
:::

## Who it is for

- **Teams with a Playwright suite** that runs too slowly or needs too many machines. You change the imports and keep the tests.
- **Teams writing new E2E tests** who want each test to finish in a few milliseconds of browser time. The native driver sends a whole test to the browser as one command.
- **Scripts that load pages and extract data**: scrapers, PDF and screenshot generators, and automation that needs a signed-in session.

## Three ways to use it

| Path | What you write | Start here |
| --- | --- | --- |
| Playwright drop-in | Your existing Playwright tests. Replace `@playwright/test` and `playwright` imports with `@povio/pyro/wright` and run `pyro run`. | [Coming from Playwright](/playwright/overview) |
| Native fast E2E | Tests that send one `page.run([...steps])` per scenario. The browser locates, acts, waits and checks on its own thread and replies once. | [Fastest E2E tests](/e2e/overview) |
| Scraping and automation | Scripts that use the native client, the `pyro fetch` CLI, Puppeteer, or raw CDP. | [Scraping and automation](/scraping/overview) |

The three paths share one browser and can be mixed in the same project. [Choosing an API](/guide/choosing-an-api) compares them in detail.

## What makes it fast

- **The browser does the waiting.** A native test step that does not hold yet is tried again when the DOM changes. Nothing polls from the test process. After an action, the next step starts as soon as the page is quiet. See [Settling](/guide/settling).
- **One round trip per test.** Locators, actionability checks and assertions run in Rust inside the browser. A test is one CDP command, not dozens.
- **Pages booted ahead.** A profile keeps signed-in pages of your app loaded before a test asks for one. See [Profiles](/guide/profiles).
- **Fork mode.** The browser boots your app once in a template process. Each test gets a `fork()` of it, already booted, in about 20 ms. See [Fork mode](/guide/fork-mode).

[Performance](/guide/performance) lists the measured numbers and where they come from.

## What it is not

- **It is not Chrome.** It speaks CDP and reports a Chrome-like version string to CDP clients, but the DOM, networking, layout and event loop are its own. Behaviour can differ from Chromium, and some web APIs are missing.
- **It does not run V8.** JavaScript runs on JavaScriptCore. Code that depends on V8-specific behaviour, such as the text of stack traces, may behave differently.
- **Web API coverage is partial.** Common application APIs work: DOM, events, fetch and XHR, Web Storage, IndexedDB, workers, WebSocket, Canvas 2D, custom elements, shadow DOM, observers. Others are absent, for example WebGL, WebGPU, WebRTC and `SharedWorker`. See [Web platform support](/reference/web-platform).
- **Screenshots are not Chromium pixels.** Rendering uses Blitz (Stylo, Taffy, Parley) and a CPU rasterizer. Baselines must come from Pyro itself. See [Rendering](/guide/rendering).
- **It has no headed mode, trace viewer, video recording or Playwright HTML report.**
- **It does not run on Windows.** Windows machines can connect to a browser running on macOS or Linux. See [Platforms](/guide/platforms).

## Status

Pyro is young and moves quickly. Versions are `0.0.x`; release candidates are published under the npm `next` tag. Expect behaviour differences from Chrome and check [Behaviour differences](/playwright/differences) and [Troubleshooting](/guide/troubleshooting) when a test fails only here. Every pull request runs a Web Platform Tests gate in CI that fails when a previously passing subtest regresses.

## Origins and license

Pyro is a Rust rewrite of [Lightpanda](https://github.com/lightpanda-io/browser), a headless browser written in Zig on V8. It is a derivative work and keeps Lightpanda's license, **AGPL-3.0-only**. The patched JavaScriptCore it links is LGPL, and its source is published with each GitHub release. The project is built by [Povio](https://github.com/povio/pyro).

## Next steps

- [Install the package](/guide/installation)
- [Run your first test in five minutes](/guide/quick-start)
- [Learn how the browser works](/guide/how-it-works)
