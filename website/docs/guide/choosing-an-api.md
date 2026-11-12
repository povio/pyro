# Choosing an API

Lightningpanda has four ways in: the Playwright module, the native SDK, the `lightningpanda e2e` suite runner, and raw CDP (which includes Puppeteer and other CDP clients). They all talk to the same browser. This page compares them and shows when to pick each.

## At a glance

| | Playwright module | Native SDK | `lightningpanda e2e` | Raw CDP / Puppeteer |
| --- | --- | --- | --- | --- |
| Import | `@povio/lightning-panda/playwright` | `@povio/lightning-panda` | `@povio/lightning-panda/e2e` plus the SDK | any CDP client |
| Test runner | `lightningpanda test` (Playwright's CLI shape, on `bun test` workers) | `bun test`, or none | `lightningpanda e2e` (one process for all files) | your own |
| How a test is written | Playwright code, one `await` per call | one `page.run([...steps])` per scenario | same as the SDK | one CDP command per operation |
| Commands per test | one `LP.run` per awaited call | usually one | usually one | dozens to hundreds |
| Waiting | in the browser; actions settle before they return | in the browser; steps retry on DOM changes, actions settle | same as the SDK | yours |
| Booted-ahead pages | `use.lightningpanda.boot` | profiles (`lp.profile`) | profiles, every page a fork | `Lightningpanda.*` CDP commands |
| Fork mode | not used | automatic against `serve --fork` | always (when the binary supports it) | `ws://host/?profile=<id>` |
| Rewriting existing tests | none | yes | yes | yes |
| Measured speed (TOS close flow) | 14.7 ms per test vs 41-44 ms for stock Playwright | about 3 ms per test vs about 34 ms for Playwright | as the SDK, plus fork per test | depends on the client |

The speed figures come from the package README and `bench/results/lp-tos-e501cef`; see [Performance](/guide/performance) for the conditions.

## Playwright module

Use it when you already have Playwright tests, or when your team knows Playwright and the speed it gives is enough.

```ts
import { test, expect } from '@povio/lightning-panda/playwright';

test('saves', async ({ page }) => {
  await page.goto('/settings');
  await page.getByLabel('Name').fill('Ada');
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText('Saved')).toBeVisible();
});
```

The module is a reimplementation of Playwright's API, not a wrapper around Playwright: the library (`chromium.launch`, `connectOverCDP`, contexts, pages, locators, routes, ...), the test runner with fixtures and projects, and `expect`. Each awaited call becomes one `LP.run` command, so locating, actionability checks and settling run in the browser. Playwright's HTML report, trace viewer, video and sharding are not available. See [Coming from Playwright](/playwright/overview) and [API coverage](/playwright/api-coverage).

## Native SDK

Use it for new tests where speed matters, and for scripts that need a signed-in page fast.

```ts
import { Lightningpanda, button, check, label, text } from '@povio/lightning-panda';

const lp = await Lightningpanda.launch();
await lp.profile('user', { url: 'http://localhost:3000/', localStorage: { token } });

await using page = await lp.page({ as: 'user', route: '/settings' });
await page.run([
  label('Name').fill('Ada'),
  button('Save').click(),
  check(text('Saved')).visible(),
  check.request('PUT', '/api/settings').status(200),
]);
```

A scenario is data: locators and steps are built in the test process and sent once. The browser runs them on its own thread and replies with the values read, the result of each step, and every request the page made. You lose the ability to run arbitrary test code between two steps of the same `run`; split the run where you need it. See [Why a native driver](/e2e/overview) and [SDK reference](/reference/sdk).

## `lightningpanda e2e`

Use it for a large native suite. `bun test` runs one file's tests after another in each worker process, and each worker needs its own browser and profiles. `lightningpanda e2e` runs every spec file in one process instead: tests from all files share one queue and one fork-mode browser, so each profile boots once and each test forks.

```sh
lightningpanda e2e e2e --setup e2e/setup.ts --concurrency 16 --junit junit.xml
```

Spec files import `describe`, `test`, hooks and `browser()` from `@povio/lightning-panda/e2e` and write steps with the SDK. It adds sharding by recorded durations (`--shard`, `--timings`) and JUnit output. See [Suite runner](/e2e/runner).

## Raw CDP and Puppeteer

Use it when you have an existing CDP tool, or need a command no higher layer exposes.

```sh
lightningpanda serve --port 9222
```

```js
import puppeteer from 'puppeteer-core';

const browser = await puppeteer.connect({ browserWSEndpoint: 'ws://127.0.0.1:9222/' });
const page = await browser.newPage();
await page.goto('https://example.com/');
console.log(await page.$eval('h1', (e) => e.textContent));
await browser.disconnect();
```

Lightningpanda implements most of the CDP commands Playwright 1.59 sends (stock Playwright's `chromium.connectOverCDP` works against it), plus its own `Lightningpanda.*` and `LP.*` domains. A basic Puppeteer flow (connect, new page, `goto`, `$eval`, `click`) works with `puppeteer-core` 25, but Puppeteer is not part of the project's tested matrix, and commands outside the implemented set return an explicit error rather than a fake success. Every round trip costs a hop over the socket, so raw CDP is the slowest way to drive a test. See [Puppeteer and raw CDP](/scraping/cdp), [CDP coverage](/reference/cdp) and the [Lightningpanda CDP domain](/reference/cdp-domain).

## Mixing them

All four share one browser and one protocol, so you can mix them:

- **Port a Playwright suite gradually.** Run it unchanged with the Playwright module, then rewrite the slowest tests to `page.run` one at a time. The native client and the Playwright module can live in the same repository.
- **Drop to CDP from the SDK.** `page.send(method, params)` sends any CDP command on a native page, for example `Page.printToPDF` or `Emulation.setUserAgentOverride`.
- **Drop to CDP from Playwright.** `context.newCDPSession(page)` returns a raw session on Lightningpanda's CDP.
- **Share one fork-mode server.** A launcher can start `serve --fork`, register profiles, and hand `lp.wsEndpoint` to worker processes, which `Lightningpanda.connect()` to it. See [One server, many workers](/e2e/shared-server).

## A rule of thumb

1. You have Playwright tests: start with the [Playwright module](/playwright/overview). Measure.
2. Tests you write from now on, or the slowest few hundred: the [native SDK](/e2e/first-test).
3. More than a few hundred native tests: the [suite runner](/e2e/runner) with fork mode.
4. Tools that already speak CDP: [connect them directly](/scraping/cdp).
