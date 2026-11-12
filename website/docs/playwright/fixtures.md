# Fixtures, hooks and steps

The runner implements Playwright's test API on top of `bun test`: Bun registers and runs the tests, and `@povio/lightning-panda/playwright` resolves their fixtures, runs their hooks, applies timeouts and retries, and reports. Code written for `@playwright/test` works as it is. This page lists what is supported and where behaviour differs.

## A complete example

```ts
import { test as base, expect, type Page } from '@povio/lightning-panda/playwright';

type Fixtures = {
  todoPage: Page;
  greeting: string;            // an option
  logConsole: void;            // an auto fixture
};
type WorkerFixtures = {
  account: { email: string };  // shared by a worker's tests
};

export const test = base.extend<Fixtures, WorkerFixtures>({
  greeting: ['hello', { option: true }],

  account: [async ({}, use) => {
    const account = { email: `user-${Date.now()}@example.com` };
    await use(account);
    // teardown runs after the file's last test
  }, { scope: 'worker' }],

  todoPage: async ({ page, account }, use) => {
    await page.goto(`/login?as=${account.email}`);
    await use(page);
  },

  // runs for every test, even when no test asks for it
  logConsole: [async ({ page }, use) => {
    page.on('console', (m) => console.log(m.text()));
    await use();
  }, { auto: true }],
});

test.describe('todos', () => {
  test.use({ greeting: 'bonjour' });

  test.beforeEach(async ({ todoPage }) => {
    await todoPage.getByRole('button', { name: 'Clear' }).click();
  });

  test('adds a todo', async ({ todoPage, greeting }, testInfo) => {
    await test.step('type and submit', async () => {
      await todoPage.getByLabel('New todo').fill(greeting);
      await todoPage.getByRole('button', { name: 'Add' }).click();
    });
    await expect(todoPage.getByRole('listitem')).toHaveText([greeting]);
    expect(testInfo.retry).toBe(0);
  });
});
```

## Built-in fixtures

| Fixture | Scope | Value |
| --- | --- | --- |
| `page` | test | A new page in `context`. |
| `context` | test | A new `BrowserContext` with the test's `use` options; closed after the test. |
| `browser` | worker | The worker's `Browser`, one per distinct `lightningpanda` option. Kept for the whole worker process. |
| `request` | test | An `APIRequestContext` with `baseURL`, `extraHTTPHeaders`, `httpCredentials`, `ignoreHTTPSErrors`, `userAgent` and `storageState`; disposed after the test. |
| `playwright` | worker | `{ chromium, request, devices, errors, selectors }`. |
| `browserName` | worker option | `'chromium'` unless `use` sets it. |
| every `use` option | test or worker option | `baseURL`, `viewport`, `storageState`, `testIdAttribute`, `actionTimeout`, ... and `lightningpanda`. |

`browser.newContext()` inside a test starts from the test's `use` options (viewport, `baseURL`, storage state, timeouts, test id attribute), as in Playwright.

## test.extend

Everything Playwright's `test.extend` does is supported:

- **Test fixtures**: `async ({ deps }, use, testInfo) => { ...; await use(value); ...teardown }`. Set up on first use in a test, torn down after it in reverse order.
- **Worker fixtures**: `[fn, { scope: 'worker' }]`. The second argument of `fn` is a worker info with `workerIndex`, `parallelIndex`, `project` and `config`.
- **Options**: `[defaultValue, { option: true }]`, set by `use` in the config, a project or `test.use()`. Overriding an option with a plain value in a later `extend` keeps it an option with a new default.
- **Auto fixtures**: `{ auto: true }`. Test-scoped autos run before each test; worker-scoped autos also run before `beforeAll` / `afterAll`.
- **Overrides**: an `extend` that redefines a fixture can take the original under the same name: `page: async ({ page }, use) => { ...; await use(page); }`.
- **Dependencies** are read from the names the fixture function destructures from its first parameter, as Playwright does. Write `async ({ page, account }, use) =>`, not `async (args, use) =>`.
- **Value fixtures**: a non-function value is used as is.

`{ timeout }` on a fixture is accepted and not applied separately; the fixture counts against the test's timeout.

### Where worker fixtures differ

A worker fixture's value is shared by the tests of a file in that worker, and **torn down after each file**, not at the end of the worker. A worker that runs three files sets up a worker fixture three times. The built-in `browser` is the exception: it lives as long as the worker.

## Hooks

```ts
test.beforeAll(async ({ browser }) => { /* once per describe, per worker */ });
test.beforeEach(async ({ page }, testInfo) => { /* before each test */ });
test.afterEach(async ({ page }, testInfo) => { /* after each test, also after a failure */ });
test.afterAll(async () => { /* after the describe's last test in this worker */ });
```

- Hooks take an optional title first: `test.beforeEach('sign in', async ({ page }) => {})`.
- `beforeEach` and `afterEach` get the test's fixtures and `testInfo`. They run outer describe first (`beforeEach`) and inner first (`afterEach`).
- `beforeAll` and `afterAll` get worker fixtures only. Asking for a test fixture such as `page` throws: `"page" is a test fixture: beforeAll and afterAll get worker fixtures only`. Create a page from `browser` instead.
- A describe's `beforeAll` / `afterAll` run only in workers that run some of its tests.
- An error in `afterEach` fails the test. All `afterEach` hooks still run.

## test.step

```ts
const id = await test.step('create the order', async () => {
  const r = await page.request.post('/api/orders', { data: { sku: 'A1' } });
  return (await r.json()).id;
});
```

`test.step` runs its body and returns its value. The `timeout` option fails the step with `TimeoutError: Step timeout of <ms>ms exceeded: <title>`. Differences from Playwright:

- Steps are not shown in the report. There is no report that would show them.
- `box: true` is accepted and has no effect.
- `step.skip()` inside the body skips the **whole test**, not only the step.
- `test.step.skip(title, body)` does not run the body and returns `undefined`.

## test.info()

`test.info()` (or the second argument of a test or hook) returns the running test's `TestInfo`:

| Member | Notes |
| --- | --- |
| `title`, `titlePath`, `file`, `testId` | `titlePath` starts with the file path relative to the working directory. |
| `project`, `config` | The resolved project (`name`, `use`, `outputDir`, `testDir`, `timeout`, `retries`, `metadata`) and the config object. |
| `retry`, `workerIndex`, `parallelIndex` | `parallelIndex` equals `workerIndex`. |
| `timeout`, `setTimeout(ms)` | Changing it re-arms the running test's timer. |
| `annotations` | Filled by `skip`, `fixme`, `fail`, `slow`. |
| `attach(name, { body, path, contentType })` | Stored in `attachments`. No report shows them. |
| `outputDir`, `outputPath(...parts)` | `<outputDir>/<title path slug>[-retryN]`; `outputPath` creates the directory. |
| `snapshotDir`, `snapshotPath(name, { kind })`, `snapshotSuffix` | Playwright's snapshot paths. See [Snapshots](/playwright/snapshots). |
| `skip()`, `fixme()`, `fail()`, `slow()` | As the `test.*` forms. |
| `expectedStatus` | `'passed'`, or `'skipped'` / `'failed'` after `skip` / `fail`. |
| `status`, `error`, `errors`, `duration` | Not updated while the test runs: `status` stays `'passed'` in `afterEach`. |
| `repeatEachIndex`, `line`, `column` | Always 0. |

## skip, fixme, fail, slow

All of Playwright's forms work:

```ts
test.skip('not ready', async ({ page }) => {});                   // declared skip
test.fixme('broken on staging', async ({ page }) => {});
test.fail('known bug #123', async ({ page }) => {});               // must fail
test.fail.only('focus on this known bug', async () => {});

test('only with the feature flag', async ({ page }) => {
  test.skip(!process.env.FEATURE_X, 'needs FEATURE_X');            // at run time
  test.slow();                                                     // timeout x3
});

test.describe('admin', () => {
  test.skip(({ browserName }) => browserName !== 'chromium', 'chromium only'); // condition with fixtures
  test.fixme(process.env.STAGE === 'prod');
});
```

- `test.skip()` / `test.fixme()` in a test stop it there; it is reported as skipped. Under plain `bun test`, Bun counts it as a pass.
- `test.fail()` marks the test as expected to fail. A failure is reported as a pass with "(expected to fail)"; a pass fails with `Expected to fail, but passed.`
- `test.slow()` triples the timeout.
- `test.slow(title, fn)` is an error, as it is not a Playwright form.

::: warning Unset boolean options are undefined
Context options that you do not set (`isMobile`, `hasTouch`, `javaScriptEnabled`, `offline`, ...) are `undefined`, not `false` as in Playwright. `test.skip(undefined)` skips, like `test.skip()`. So `test.skip(isMobile)` skips every test unless `isMobile` is set in `use`. Write `test.skip(isMobile === true)` or set the option explicitly.
:::

## describe modes

```ts
test.describe('checkout', () => {
  test.describe.configure({ mode: 'serial', retries: 1, timeout: 60_000 });
  // ...
});

test.describe.serial('wizard', () => { /* ... */ });
test.describe.parallel('independent pages', () => { /* ... */ });
test.describe.only('focus', () => { /* ... */ });
test.describe.skip('later', () => { /* ... */ });
test.describe.fixme('broken', () => { /* ... */ });   // same as skip
```

| Mode | Behaviour |
| --- | --- |
| `default` | Tests run in order in the file's worker. A failure does not stop the others. |
| `serial` | Tests run in order in the file's worker. After a failure (retries used up), the rest of the group is skipped with "an earlier test of its serial group failed". |
| `parallel` | Tests spread over the workers. Within one worker they still run one after another unless `LP_CONCURRENT=1`. |

`configure({ retries, timeout })` applies to the describe and the describes inside it.

In Playwright, a failed test in a serial group with retries reruns the whole group. Here only the failed test is retried.

## Retries

The number of retries comes from the innermost `test.describe.configure({ retries })`, else the project's `retries`, else the config's, else `--retries`. A test that passes on a retry is reported as flaky. Each attempt gets a new `TestInfo` with `retry` set, and its own fixtures. A snapshot written in "missing" mode fails the test without a retry, as in Playwright.

## Tags and titles

`test('title', { tag: '@smoke' }, fn)` and `{ tag: ['@a', '@b'] }` are supported. Tags are appended to the name Bun shows and are part of what `--grep` matches. `annotation` in the details object is accepted and not recorded.

## Timeouts

| Timeout | Default | Set with |
| --- | --- | --- |
| Test | 30 000 ms | `timeout` in config or project, `--timeout`, `test.setTimeout()`, `describe.configure({ timeout })`, `testInfo.setTimeout()` |
| `expect` | 5 000 ms | `expect.timeout` in config, `expect.configure({ timeout })`, per call |
| Action, navigation | 30 000 ms cap | `actionTimeout`, `navigationTimeout`, `page.setDefaultTimeout()`, per call |

A timed-out test is reported as "timed out" with `Test timeout of <ms>ms exceeded.` Its fixtures are torn down at once.
