# Coming from Playwright

`@povio/pyro/wright` is Playwright's test API and library API implemented on Pyro. You replace the imports of `@playwright/test` and `playwright` with it, run `pyro run` instead of `playwright test`, and keep your tests. Playwright itself is not installed or used.

This section is for teams with an existing Playwright suite. It covers what carries over unchanged, what behaves differently, and what is missing.

## The one-line change

```ts
// before
import { test, expect, type Page } from '@playwright/test';

// after
import { test, expect, type Page } from '@povio/pyro/wright';
```

```sh
# before
npx playwright test
# after
npx pyro run        # or: bunx pyro run
```

The same module replaces both `@playwright/test` (the runner, fixtures and `expect`) and `playwright` (the library: `chromium.launch()`, `Browser`, `Page`, ...). `@povio/pyro/test` is an alias of the same module.

A test like this one runs unchanged:

```ts
import { test, expect } from '@povio/pyro/wright';

test('closes a position', async ({ page }) => {
  await page.goto('/positions/42');
  await page.getByRole('button', { name: 'Settings', exact: true }).press('Enter');
  const done = page.waitForResponse((r) => r.url().endsWith('/close') && r.status() === 200);
  await page
    .getByRole('dialog')
    .filter({ hasText: 'will be closed' })
    .getByRole('button', { name: 'Close Position', exact: true })
    .click();
  await done;
  await expect(page.getByRole('button', { name: 'Reopen Position' })).toHaveCount(1);
});
```

## What you get

- **The runner** on `bun test`: `test`, `test.describe` (`only`, `skip`, `fixme`, `serial`, `parallel`, `configure`), the four hooks with fixtures, `test.extend` (test and worker fixtures, options, `auto`, overrides), `test.use`, `test.step`, `test.info()`, `skip` / `fixme` / `fail` / `slow`, retries and timeouts. See [Fixtures, hooks and steps](/playwright/fixtures).
- **The config file**: `playwright.config.ts` is read as it is, with `use`, `projects`, `webServer`, `globalSetup`, `testDir` / `testMatch` / `testIgnore`, `workers`, `fullyParallel` and more. See [Config and the test runner](/playwright/runner).
- **`expect`** with every web-first matcher on locators, pages and API responses, plus `.not`, `.soft`, `.poll`, `.configure`, `.extend` and `toPass`. See [Assertions](/playwright/assertions).
- **The library**: `Browser`, `BrowserContext`, `Page`, `Frame`, `Locator`, `FrameLocator`, handles, `Request`, `Response`, `Route`, `Download`, `Dialog`, `Worker`, `FileChooser`, `WebSocket`, `request`, `devices`, `errors`, `selectors`. See [API coverage](/playwright/api-coverage) for a method-by-method table.
- **Network control**: `page.route()`, `context.route()`, `routeFromHAR` (replay and recording), `recordHar`, `waitForRequest` / `waitForResponse`, WebSocket events. See [Network, routes and HAR](/playwright/network).
- **Snapshots**: `toHaveScreenshot`, `toMatchSnapshot`, `toMatchAriaSnapshot` at Playwright's snapshot paths. See [Screenshots and snapshots](/playwright/snapshots).
- **Clock, coverage, ARIA snapshots** including `mode: 'ai'` and `aria-ref=` locators. See [Clock, coverage, ARIA](/playwright/advanced).
- **Playwright's types**: `expect()` offers Locator, Page or APIResponse matchers by what it is given, `evaluate` infers argument and return types, and `getByRole` takes the ARIA role union.

## What stays the same

- Test files, fixtures, page objects and helpers, as long as they import from the new module.
- The config file. `pyro run` looks for `pyro.config.*` first, then `playwright.config.*`.
- Locators, auto-waiting, actionability checks and web-first assertions. Default timeouts are Playwright's: 30 s per test, 5 s per `expect`.
- Snapshot file paths (`<test file>-snapshots/<name>-<project>-<platform>.png`, or your `snapshotPathTemplate`).
- Most command-line flags: `-c`, `--project`, `-j`, `-g`, `--grep-invert`, `--retries`, `--timeout`, `-u`, `-x`, `--max-failures`, `--list`, `--reporter list|line|dot`.

## What changes

- **The browser is Pyro, not Chromium.** Every project runs on it, whatever `browserName` says. Its user agent is `Mozilla/5.0 (compatible; pyro/<version>)`. Its web platform support is broad but partial. See [Web platform support](/reference/web-platform).
- **Actions settle before they return.** A click returns once the page is quiet: no queued task, no request the action started, no timer due within 50 ms. An assertion on a transient state between the action and its result (a spinner, a "Saving..." label) can miss it. See [Behaviour differences](/playwright/differences).
- **Screenshots come from Pyro's CPU renderer.** Chromium baselines do not match. Re-baseline once when you migrate. See [Screenshots and snapshots](/playwright/snapshots).
- **Tests run on `bun test`.** `pyro run` needs [Bun](https://bun.sh) on `PATH`. The library API (`chromium.launch()` in a script) also runs on Node.js 22+.
- **Each awaited call is one command to the browser.** Locating, actionability checks, waiting and settling run inside the browser, in one round trip, instead of the many CDP calls Playwright makes per action.
- **Some Playwright APIs are left out.** The selector-first page methods (`page.click(selector)`, `page.fill(selector, value)`, ...) are not implemented; use locators. Deprecated APIs are type errors. See [API coverage](/playwright/api-coverage).

## Speed

The package's README reports this measurement for TOS's "close a position" flow (`packages/pyro/test/wright/tos-flow.ts`, Playwright code as TOS writes it):

| How the flow runs | Time per test |
| --- | --- |
| `@povio/pyro/wright` on Pyro | 14.7 ms (median of 40, fresh page) |
| Stock Playwright driving the same Pyro over CDP | 41-44 ms |

The flow seeds a position over HTTP, opens a context, navigates, clicks through a dialog and waits for two responses. The gain comes from sending each awaited call as one browser command.

Pages booted ahead cut more: with `use.pyro.boot`, a test's first `goto` becomes a route change in an app that has already loaded. See [Booting pages ahead](/playwright/boot-pools).

The native driver (`page.run([...])`, one command per test) is faster still. In the bench run recorded in `bench/results/pyro-tos-e501cef`, the same flow took 8.6 ms on a fresh page and 2.7 ms per test over four connections, against 34.2 ms for stock Playwright over CDP on the same busy host. If you are willing to rewrite tests, see [Why a native driver](/e2e/overview).

::: tip Measure your own suite
These numbers come from one flow on one machine. Run your suite both ways and compare wall time and CPU. Your app's own JavaScript is often the larger cost.
:::

## What is not there

| Playwright feature | Status |
| --- | --- |
| HTML report (`--reporter html`) | Not available. Reporters are `list`, `line` and `dot`. The config's `reporter` field is ignored. |
| Trace viewer, `trace` option | Not available. `context.tracing` methods exist and do nothing. |
| Video, `video` option | Not available. `page.video()` returns `null`. |
| Screenshot on failure, `screenshot` option | Accepted, no effect. |
| Sharding (`--shard`) | Not available in `pyro run` (unknown option). The native suite runner `pyro e2e` has `--shard`; see [Suite runner](/e2e/runner). |
| UI mode, `--headed`, `--debug` | Print a warning and have no effect. Pyro is headless. |
| Codegen, inspector, `page.pause()` | Not available. `page.pause()` returns at once. |
| Firefox and WebKit | Every project runs on Pyro. |
| Custom selector engines (`selectors.register`) | Throws. |
| Component testing | Not available. |
| JUnit / JSON reporters | Not in `pyro run`. |

If one of these is essential, keep Playwright for that job. Pyro and Playwright can run side by side during a migration; see [Migrating a suite](/playwright/migration#running-both-side-by-side).

## Next steps

1. [Migrate a suite](/playwright/migration) step by step.
2. Read [Behaviour differences](/playwright/differences) before you debug the first failures.
3. Check [API coverage](/playwright/api-coverage) for any API your suite depends on.
