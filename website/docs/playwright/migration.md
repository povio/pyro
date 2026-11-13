# Migrating a suite

This page moves an existing Playwright suite to Pyro: install the package, rewrite the imports, point the runner at your config, update scripts and CI, then fix the first failures. Most suites need no changes beyond the imports. Plan time for re-baselining screenshots and for the [behaviour differences](/playwright/differences).

## 1. Install

```sh
bun add --dev @povio/pyro
# or
npm install --save-dev @povio/pyro
```

The package bundles the browser for macOS 14+ (Apple silicon and Intel) and Linux x64 / arm64 with glibc 2.39+ (for example Ubuntu 24.04). There is no browser download step and no `playwright install`.

`pyro run` runs your tests with `bun test`, so Bun must be on `PATH` even if the rest of your project uses Node. Check with `bun --version`. See [Installation](/guide/installation) for platform details.

::: info Before the first `@povio/pyro` release
The package is not on npm under this name yet. Until it is, install it from a checkout and point `PYRO_BIN` at the browser you built, or use `@povio/lightning-panda` 0.0.33. See [Before the first release](/guide/installation#try-today).
:::

Keep `@playwright/test` installed until the end of the migration if you want to [run both browsers on the same suite](/recipes/playwright-side-by-side) and compare.

## 2. Rewrite the imports

Every import of `@playwright/test`, `playwright`, `playwright/test` and `playwright-core` becomes `@povio/pyro/wright`. Type-only imports change the same way.

```ts
import { test, expect } from '@playwright/test';             // before
import { test, expect } from '@povio/pyro/wright'; // after

import type { Page, Locator } from '@playwright/test';           // before
import type { Page, Locator } from '@povio/pyro/wright'; // after

import { chromium, devices } from 'playwright';                // before
import { chromium, devices } from '@povio/pyro/wright'; // after
```

This command rewrites them in place. It matches the module name only when it is the whole quoted string, so it works for `import`, `import type`, `export ... from`, `import()` and `require()`. It works with both BSD (macOS) and GNU tools because it uses Perl:

```sh
grep -rlE "['\"](@playwright/test|playwright/test|playwright-core|playwright)['\"]" \
    e2e playwright.config.ts \
  | xargs perl -pi -e 's{(["\x27])(?:\@playwright/test|playwright/test|playwright-core|playwright)\1}{$1\@povio/pyro/wright$1}g'
```

Replace `e2e` with the directories that hold your tests, fixtures and page objects. Run `git diff` afterwards and review it.

It leaves subpath imports such as `@playwright/test/reporter` alone. Custom reporters do not run under `pyro run`, so remove those files or keep them out of the test directory.

::: warning Shared helpers count too
A helper module that still imports `@playwright/test` loads Playwright into the Bun test process. Search the whole repository, not only the test directory: `grep -rn "@playwright/test\|from 'playwright'" --include='*.ts' .`
:::

## 3. The config file

`pyro run` reads the first of these that exists:

1. The file named by `-c` / `--config`, or by `$PYRO_CONFIG` (`$PLAYWRIGHT_CONFIG` also works).
2. `pyro.config.ts`, `.mts`, `.mjs` or `.js` in the working directory.
3. `playwright.config.ts`, `.mts`, `.mjs` or `.js` in the working directory.

So your existing `playwright.config.ts` works after its import is rewritten:

```ts
import { defineConfig, devices } from '@povio/pyro/wright';

export default defineConfig({
  testDir: './e2e',
  timeout: 30_000,
  retries: process.env.CI ? 2 : 0,
  use: { baseURL: 'http://localhost:3000' },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: { command: 'bun run dev', url: 'http://localhost:3000', reuseExistingServer: !process.env.CI },
});
```

A `pyro.config.ts` beside it takes precedence. That is how both runners can share one repository during a migration (see [below](#running-both-side-by-side)).

Things to check in the config:

- **Projects for other browsers.** All projects run on Pyro, and a file runs in each project that matches it, as in Playwright. A config with `chromium`, `firefox` and `webkit` projects therefore runs each test three times on the same browser. Remove the other two projects, or select one with `--project`.
- **`devices`** is Playwright 1.59.1's table. A descriptor's `viewport`, `userAgent`, `deviceScaleFactor`, `isMobile` and `hasTouch` take effect; `defaultBrowserType` does not. See [Mobile emulation](/playwright/runner#mobile-emulation). A descriptor sets `userAgent`, so its project gets no [booted pages](/playwright/boot-pools).
- **Setup projects** work as in Playwright: project `dependencies` run first, and `teardown` projects last. See [Sign in once with a setup project](/recipes/playwright-setup-project-auth).
- **`globalSetup`** receives Playwright's resolved `FullConfig`, so `config.projects[0].use.baseURL` works.
- **`reporter`** is ignored. Pass `--reporter list|line|dot` on the command line.
- **`use.trace`** is accepted and records nothing. **`use.screenshot`** and **`use.video`** work; video needs an ffmpeg on the machine. See [Video](/playwright/runner#video).

[Config and the test runner](/playwright/runner) lists every field that is honoured.

## 4. Run it

```sh
bunx pyro run                 # all projects' files, half the CPUs as workers
bunx pyro run e2e/checkout    # files whose path matches the regex
bunx pyro run -g "closes" -j 4
```

You can also run a file with plain `bun test`, in one process, with `$PYRO_CONFIG` naming the config:

```sh
PYRO_CONFIG=playwright.config.ts bun test ./e2e/checkout.spec.ts
```

The difference: under plain `bun test`, Bun reports a test that calls `test.skip()` at run time as passed, because Bun cannot mark a running test as skipped. Workers, `fullyParallel` and the runner's own report also need `pyro run`. Use plain `bun test` for quick runs of one file.

## 5. package.json scripts

```json
{
  "scripts": {
    "e2e": "pyro run",
    "e2e:update": "pyro run -u",
    "e2e:one": "PYRO_CONFIG=playwright.config.ts bun test"
  }
}
```

Drop scripts that use `playwright show-report`, `playwright codegen`, `--ui` or `--trace`. They have no equivalent (see [Overview](/playwright/overview#what-is-not-there)).

## 6. Remove Playwright

Once nothing imports it:

```sh
bun remove @playwright/test playwright playwright-core
# or
npm uninstall @playwright/test playwright playwright-core
```

Also remove `npx playwright install` from setup scripts, Dockerfiles and CI, and any `@playwright/test` entry in `tsconfig.json` `types`.

## 7. CI

The browser comes with the npm package, so CI needs Node or Bun for the install, and Bun for the run. On GitHub Actions:

```yaml
jobs:
  e2e:
    runs-on: ubuntu-24.04          # glibc 2.39
    steps:
      - uses: actions/checkout@v4
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
      - run: bunx pyro run --reporter dot
```

Compared with a Playwright job:

- Remove the `npx playwright install --with-deps` step and any browser cache.
- Keep a `--shard=i/n` matrix if you have one: `pyro run --shard i/n` splits the tests as Playwright does. Remove the `merge-reports` job that follows it, since there are no blob reports. A suite often needs fewer jobs on Pyro; try more workers (`-j`) on one machine first.
- Remove steps that upload `playwright-report/` or traces. On failure, `test-results/` (the `outputDir`) still holds `-actual`, `-expected` and `-diff` images of screenshot mismatches.
- The runner exits with status 1 when a test fails.

[Sharding in CI](/recipes/playwright-ci-without-sharding) has a complete workflow with caching and a job matrix. [CI recipes](/e2e/ci) covers the native runner, `pyro e2e`.

## 8. Re-baseline screenshots

Pyro renders pages with its own CPU renderer. Screenshots differ from Chromium's at the pixel level, so every existing `toHaveScreenshot` baseline will fail. Write new ones once, review them, and commit them:

```sh
bunx pyro run -u all
```

Commit the `.json` file written beside each `.png`: it holds the render's display-list hash, which lets later runs pass without rasterizing. See [Screenshots and snapshots](/playwright/snapshots).

## Running both side by side

You do not have to switch the whole suite at once. Two ways work.

**Split the files between two configs.** `pyro run` prefers `pyro.config.ts`; `playwright test` only reads `playwright.config.ts`. Give each runner its own files:

```ts
// pyro.config.ts: the migrated files
import { defineConfig } from '@povio/pyro/wright';
export default defineConfig({
  testDir: './e2e',
  testMatch: ['checkout.spec.ts', 'positions/*.spec.ts'],
  use: { baseURL: 'http://localhost:3000' },
});
```

```ts
// playwright.config.ts: still on Playwright, skips the migrated files
import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './e2e',
  testIgnore: ['checkout.spec.ts', 'positions/*.spec.ts'],
  use: { baseURL: 'http://localhost:3000' },
});
```

Rewrite the imports of a file when you move it to the first list. Keep shared fixtures in two copies, or migrate them first.

**Switch one fixtures module.** If all tests import `test` and `expect` from a local module, that module can pick the implementation. The package's own parity suite works this way (`packages/pyro/test/wright/fixtures.ts`):

```ts
// e2e/fixtures.ts (an ES module project)
import * as pyro from '@povio/pyro/wright';

const stock = process.env.PW_STOCK ? await import('@playwright/test') : undefined;
export const test: typeof pyro.test = stock ? (stock.test as any) : pyro.test;
export const expect: typeof pyro.expect = stock ? (stock.expect as any) : pyro.expect;
```

`PW_STOCK=1 npx playwright test` runs the suite on Playwright and Chromium; `bunx pyro run` runs it on Pyro. Comparing the two runs shows which failures come from the migration.

[Run the same suite on Chromium and Pyro](/recipes/playwright-side-by-side) is a complete, tested version of this: one config per runner over shared options, separate screenshot baselines, a script that lists the tests whose results differ, and how to time both.

## Typical first failures

[Triage the first red run](/recipes/playwright-triage-first-failures) shows the most common of these as you will see them, how to find them before running, and how to group a large run's errors.

| Symptom | Cause | Fix |
| --- | --- | --- |
| `page.click is not a function` (also `page.fill`, `page.textContent(selector)`, `page.isVisible(selector)`, ...) | Playwright's selector-first page methods are not implemented. | Use locators: `page.locator(selector).click()`. |
| Every `toHaveScreenshot` fails | Baselines came from Chromium. | Re-baseline with `-u all` (step 8). |
| `unknown field 'animations'` (or `mask`, `caret`, `scale`) | `toHaveScreenshot` accepts only `threshold`, `maxDiffPixels`, `maxDiffPixelRatio`, `clip`, `fullPage`, `omitBackground` and `timeout`. | Remove the option. |
| An assertion on a loading spinner or "Saving..." text times out | Actions settle before they return; the transient state is already gone. | Assert the final state, or wait for the request. See [Behaviour differences](/playwright/differences#actions-settle-before-they-return). |
| A `confirm()` dialog acts as if Cancel was pressed | A dialog with no `page.on('dialog')` listener is dismissed. | Register a listener before the action, as Playwright requires. |
| `toHaveRole('button')` fails on a `<button>` | `toHaveRole` reads the `role` attribute only, not implicit roles. | Use `getByRole('button')` to find it, or drop the assertion. |
| An element inside an open shadow root is not found | CSS and `getByRole` locators do not enter shadow roots; `getByText` does. | Locate by text, or test through the host element's API. |
| Each test runs three times, once per browser project | All projects run on Pyro. | Remove the Firefox and WebKit projects, or use `--project` to pick one. |
| `await page.goto(url)` returns `null` | A [booted page](/playwright/boot-pools) turns `goto` into a route change, which returns `null`. | Check status with `waitForResponse`, or leave that route out of `boot`. |
| `browser.newContext: recordVideo encodes videos with ffmpeg, and there is none` | `video` or `recordVideo` is set and the machine has no ffmpeg. | Install one, or set `PYRO_FFMPEG`. See [Video](/playwright/runner#video). |
| `getByTestId(/regex/)` or `click({ clickCount: 3 })` throws "not supported yet" | Not implemented. | Use a string test id; use `dblclick()` or `evaluate` for triple clicks. |
| `ENOENT: no such file or directory, open 'e2e/.auth/user.json'` under plain `bun test` | Plain `bun test` runs no project dependencies, so the setup project did not write the storage state. | Use `pyro run`, or run the setup file first. |
| `Missing bundled browser: .../native/<platform>/pyro` | The package was installed from a checkout, which has no bundled browsers. | Set `PYRO_BIN`; see [Before the first release](/guide/installation#try-today). |
| App code reports an unsupported browser, or a web API is undefined | Pyro is not Chromium: its UA and API surface differ. | See [Web platform support](/reference/web-platform). |

## Checklist

- [ ] `@povio/pyro` installed; `bun --version` works locally and in CI.
- [ ] No imports of `@playwright/test`, `playwright`, `playwright/test` or `playwright-core` left (`grep -rn`).
- [ ] Config file found (`bunx pyro run --list` lists your tests).
- [ ] Browser-specific projects reviewed: Firefox and WebKit projects removed or left out with `--project`.
- [ ] Unsupported screenshot options removed; baselines rewritten with `-u all` and committed with their `.json` files.
- [ ] `page.click(selector)`-style calls rewritten as locators.
- [ ] Dialog listeners registered where the app uses `confirm`, `alert` or `prompt`.
- [ ] Assertions on transient states rewritten.
- [ ] `package.json` scripts and CI updated; `playwright install`, report uploads and `merge-reports` removed.
- [ ] Playwright packages removed.
- [ ] Optional: slow app boots moved to [booted pages](/playwright/boot-pools).

## Next

- Work through the first failures: [Triage the first red run](/recipes/playwright-triage-first-failures).
- Look up a behaviour before you change a test: [Behaviour differences](/playwright/differences).
- Every runner flag and config field: [Config and the test runner](/playwright/runner).
