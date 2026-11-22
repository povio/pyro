---
layout: home

hero:
  name: Pyro
  text: The headless browser built for tests
  tagline: A Rust browser on JavaScriptCore. Run your Playwright suite unchanged, write E2E tests that finish in milliseconds, or drive signed-in scrapers from stored profiles.
  image:
    src: /logo.svg
    alt: Pyro
  actions:
    - theme: brand
      text: Get started
      link: /guide/quick-start
    - theme: alt
      text: Coming from Playwright
      link: /playwright/overview
    - theme: alt
      text: Fastest E2E tests
      link: /e2e/overview
    - theme: alt
      text: Scraping
      link: /scraping/overview

features:
  - icon: 🔁
    title: Playwright, unchanged
    details: Swap the import of playwright or @playwright/test for @povio/pyro/wright. Your tests, config, fixtures and expect calls keep working, and Playwright itself is no longer a dependency.
    link: /playwright/migration
    linkText: Migrate a suite
  - icon: ⚡
    title: One command per test
    details: The native driver sends a whole scenario as one Pyro.run command. Locating, acting, waiting and checking happen inside the browser, so a test is one round trip.
    link: /e2e/overview
    linkText: Why a native driver
  - icon: 🧊
    title: Pages booted before the test
    details: Profiles keep signed-in pages loaded ahead of time. In fork mode every test gets a copy-on-write fork of a page that booted once, in about 20 ms.
    link: /guide/fork-mode
    linkText: Fork mode
  - icon: 🗂️
    title: Stored profiles for scrapers
    details: Save cookies and localStorage per identity, give each its own proxy and user agent, and reuse them across runs and processes.
    link: /scraping/profiles
    linkText: Stored profiles
  - icon: 🖼️
    title: Screenshots and PDFs in the browser
    details: Pages render on the CPU with Blitz, Stylo and Vello. Visual baselines compare inside the browser; Page.printToPDF writes tagged text you can extract.
    link: /guide/rendering
    linkText: Rendering
  - icon: 📦
    title: One npm package
    details: "@povio/pyro ships the browser for macOS (Apple silicon, Intel) and Linux (x64, arm64) with the CLI and typed APIs. No downloads at install time."
    link: /guide/installation
    linkText: Install
---

[Pyro 0.0.50-rc.6 compatibility progress and editing results →](/reference/round-seven)

[Round six compatibility progress: 749 more CSS test pages pass in full →](/reference/round-six)

## Measured on a real app suite

| Test style | Per test | Against |
|---|---|---|
| Native `page.run([...])` on a pre-booted page | **~3 ms** | ~34 ms in Playwright |
| Playwright code through `@povio/pyro/wright` | **14.7 ms** | 41–44 ms for stock Playwright on the same browser |
| A fresh fork-mode connection with the app already booted | **~20 ms** | however long your app takes to boot |

Numbers come from the TOS "close a position" flow in this repository's benchmarks (median of 40 runs, fresh page per test). See [Performance](/guide/performance) for the method and for what makes your own suite faster.

```sh
bun add --dev @povio/pyro      # or: npm install --save-dev @povio/pyro
bunx pyro serve --port 9222    # a CDP server any client can connect to
```
