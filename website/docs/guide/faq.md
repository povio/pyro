# FAQ

Short answers to common questions, with links to the details.

## Is it Chrome?

No. Lightningpanda is its own browser, written in Rust. It speaks the Chrome DevTools Protocol, and to CDP clients it reports a Chrome version (`Chrome/124.0.6367.29` in `/json/version`) so that Playwright takes its normal code paths. The DOM, network stack, layout and event loop are not Chromium's, and behaviour can differ. See [How it works](/guide/how-it-works).

## Does it run V8?

No. JavaScript runs on JavaScriptCore, the engine in Safari. The project forbids V8 and checks its dependency graph for it. On Apple silicon and Linux it uses a patched JavaScriptCore that ships with the package; on Intel Macs it uses the one built into macOS.

## What User-Agent do pages see?

`Mozilla/5.0 (compatible; lightningpanda/<version>)`, in `navigator.userAgent` and in request headers. Override it per context (Playwright's `userAgent` option) or per page (`Emulation.setUserAgentOverride`) if a site serves different content to unknown browsers. Google Fonts is one such site; see [Rendering](/guide/rendering#fonts).

## Do I need Playwright installed?

No. `@povio/lightning-panda/playwright` implements Playwright's API itself, including the test runner and `expect`. Replace the imports and remove Playwright from your dependencies. See [Migrating a suite](/playwright/migration).

## Can I use stock Playwright with it?

Yes. `chromium.connectOverCDP('ws://127.0.0.1:9222/')` from the real `playwright` package works against `lightningpanda serve`. It is slower than the Playwright module, because stock Playwright sends many CDP commands per action and polls from the client: the project measured 34 ms per test for stock Playwright against 2.75 ms for the native driver on the same flow.

## Can I use Puppeteer?

Basic flows work: `puppeteer.connect({ browserWSEndpoint: 'ws://127.0.0.1:9222/' })`, new page, `goto`, `$eval` and `click` were checked with `puppeteer-core` 25. Puppeteer is not in the project's tested matrix, and CDP commands Lightningpanda does not implement return an explicit error. See [Puppeteer and raw CDP](/scraping/cdp).

## Does it work on Windows?

The browser does not. Run it on macOS or Linux (a Linux VM or WSL 2 with glibc 2.39+ works) and connect from Windows with `Lightningpanda.connect()` or `LIGHTNINGPANDA_WS`. See [Platforms](/guide/platforms#windows).

## Does it work in CI?

Yes. GitHub-hosted `ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-15` and `macos-15-intel` runners run the package without extra setup; the project's release workflow tests on all four. Containers need glibc 2.39+ and fonts. See [CI recipes](/e2e/ci) and [Installation](/guide/installation#ci).

## Is there a headed mode?

No. Lightningpanda has no window and no GPU. To see a page, take a screenshot (`Page.captureScreenshot`, `page.screenshot()`, or a `matchScreenshot` step).

## Is there a trace viewer, video recording or HTML report?

No. The Playwright module accepts the `trace`, `video` and `screenshot` options so existing configs load, but records nothing. `lightningpanda test` reports with `list`, `line` or `dot`. `lightningpanda e2e` writes JUnit XML (`--junit`). For failures, the native `RunError` names the failing step and why it was waiting, and every run reply lists the requests the page made. See [Debugging failures](/e2e/debugging).

## Can I shard a suite across machines?

With `lightningpanda e2e`, yes: `--shard i/n` splits files by recorded durations (`--timings`). `lightningpanda test` (the Playwright runner) has no sharding. See [Suite runner](/e2e/runner).

## Why does my spinner assertion fail?

Actions settle: a click returns only after the page is quiet, including the requests the click started. By then a loading spinner has usually come and gone. Hold the request with a mock, check the spinner, then release it. See [Transient states](/guide/settling#transient-states).

## Why does my test pass here but fail in Chrome, or the reverse?

Common causes: a web API Lightningpanda lacks ([Web platform support](/reference/web-platform)), a layout difference that changes what is visible or covered, or a test that relied on timing that settling removes. [Behaviour differences](/playwright/differences) lists the known ones.

## Do I still need `waitForTimeout`, `waitForResponse` or retry loops?

No. Steps wait in the browser for their condition and actions settle. In native tests, `check.request(method, path)` replaces `waitForResponse`. Existing Playwright waits still work in the Playwright module; they just have nothing left to wait for.

## Can screenshots match my existing Chromium baselines?

No. Pixels come from Lightningpanda's CPU renderer (Blitz and Vello). Regenerate baselines with Lightningpanda and keep them apart from Chromium ones. See [Rendering](/guide/rendering).

## Can it generate PDFs?

Yes, through `Page.printToPDF` (`page.pdf()` in the Playwright module). Text in the PDF copies and extracts correctly, links are clickable, and the document is tagged. There is no `@page` support and no header or footer templates. See [Rendering](/guide/rendering#pdf).

## How much memory does it use?

It depends on your app, since every booted page is a full copy of it. On a small HTTP workload (50 navigations, 20 browser contexts) a process peaked at 15.9 MB resident. A 20-test loop on a 2 MB app plateaued at 275 MB in the project's measurement. Profiles keep `pool` pages per connection booted, so they trade memory for speed; fork mode shares a profile's memory between tests copy-on-write. See [Profiles](/guide/profiles#memory).

## Why is fork mode not available on my Mac?

Fork mode needs the patched JavaScriptCore, which is bundled for Apple silicon and Linux only. The Intel macOS build uses macOS's own JavaScriptCore. `bunx lightningpanda version` shows `fork: true` or `fork: false`. See [Fork mode](/guide/fork-mode#platform-availability).

## How is it related to Lightpanda?

Lightningpanda is a Rust rewrite of [Lightpanda](https://github.com/lightpanda-io/browser), a headless browser written in Zig on V8. It started from Lightpanda's design and CDP behaviour, replaced V8 with JavaScriptCore, and added the native test driver, profiles, fork mode and rendering. It is a derivative work, credited in the `NOTICE` file, and keeps Lightpanda's license. It is built by Povio and contributors, not by the Lightpanda team.

## What does the AGPL license mean for me?

The package, including the TypeScript client, is licensed under AGPL-3.0-only, as Lightpanda is. The bundled JavaScriptCore is LGPL, and its source is published with each GitHub release. The project's `NOTICE` states the key AGPL point: network users of a modified version must be offered its source (AGPL section 13). AGPL's obligations are about distributing the software or offering a modified version of it to users over a network: then you must offer the corresponding source of your version. This is not legal advice; if you plan to modify Lightningpanda, ship it to others, or run it as a service for third parties, check with your legal team.

## Which version should I install?

`latest` for stable releases, `next` for release candidates. The project is at `0.0.x` and moves quickly; pin an exact version in CI and upgrade deliberately. See [Changelog and versions](/reference/versions).

## Can AI coding agents write tests for it?

Yes. The package ships `LLM.md`, a complete guide for coding agents writing native tests. Point your agent at `node_modules/@povio/lightning-panda/LLM.md`. See [Writing tests with AI agents](/e2e/ai-agents).
