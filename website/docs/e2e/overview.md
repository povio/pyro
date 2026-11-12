# Why a native driver

The native driver runs a whole test scenario inside the browser as one command. You build the steps in TypeScript, `page.run([...])` sends them as one `LP.run` CDP command, and the browser locates, acts, waits and checks on its own thread before it replies once. This section shows how to write tests that way and how to run a large suite with it.

If you already have a Playwright suite, you do not need this section to get started: change the imports and run it unchanged (see [Coming from Playwright](/playwright/overview)). Come back here for the tests whose last milliseconds matter, or for new suites.

## Where the time goes in a classic E2E test

A Playwright-style test sends one command per action or assertion and waits for each answer:

```ts
await page.goto(`/positions/${id}`);
await page.getByRole('button', { name: 'Settings', exact: true }).press('Enter');
const done = page.waitForResponse((r) => r.url().endsWith('/close') && r.status() === 200);
await page.getByRole('dialog').filter({ hasText: 'will be closed' })
  .getByRole('button', { name: 'Close Position', exact: true }).click();
await done;
await expect(page.getByRole('button', { name: 'Reopen Position' })).toHaveCount(1);
```

Each line is at least one round trip between the test process and the browser. Auto-waiting assertions poll from the client until they pass. The browser context and page are created per test, and the app boots again in every test.

The native driver removes each of these costs:

| Cost | Classic driver | Native driver |
|---|---|---|
| Round trips | One or more per action and assertion | One per `page.run` |
| Waiting | The client polls until an assertion holds | A waiting step is tried again when the page's DOM changes; nothing polls |
| Waiting after an action | Fixed waits, `waitForResponse`, retry helpers | The action settles: the next step starts when the page is quiet |
| App boot | Every test loads the app | A [profile](/e2e/profiles) keeps signed-in pages booted ahead, or [fork mode](/e2e/fork) forks one booted copy per test |
| Locating and actionability | JavaScript injected into the page | Rust, against the browser's own DOM |

## What runs in the browser

`page.run(steps)` sends the steps as one `LP.run` command. For each step the browser:

1. Resolves the locator against the live DOM. An action on a locator that matches several elements fails at once ([strict mode](/e2e/locators#strict-mode)).
2. Checks actionability for actions: attached, visible, enabled where the action needs it, not covered by another element, scrolled into view.
3. Acts, or evaluates the check. A step that does not hold yet waits until the DOM changes and tries again, up to its timeout (5 s by default).
4. After an action, settles: it waits until no task is queued, no request the action started is in flight, no timer is due within 50 ms, no animation frame is pending and no navigation is half done. Intervals, later timers and animation loops do not count. Settling gives up after 1 s and carries on, recording what was still pending in the step's result.

The reply carries the values your `read` and `evaluate` steps produced, one result per step, and every request the page made during the run. See [Settling and auto-waiting](/guide/settling) for the details of the quiet check.

## The model: seed through the API, then one run

A native test has two parts:

1. **Seed** the data the test needs through your backend's API, in plain test code.
2. **One `page.run([...])`** that drives the UI from that state and asserts the outcome.

```ts
test.concurrent('closes a position', async () => {
  const position = await api.createPosition();                 // seed through the API
  await using page = await lp.page({ as: 'employee', route: `/positions/${position.id}` });
  await page.run([
    check(text(position.number)).visible(),
    button('Settings', { exact: true }).press('Enter'),
    dialog().hasText('will be closed').button('Close Position', { exact: true }).click(),
    check.request('POST', `/positions/${position.id}/close`).status(200),
    check(button('Reopen Position')).count(1),
  ]);
});
```

The page comes from the `employee` [profile](/e2e/profiles): it is already loaded and signed in. The `route` option changes the URL in place (`pushState` plus `popstate`), so the app does not boot again. The request check looks at the requests the page made during this run, so it sits in the same `run` as the click that sends the request.

Split a scenario into several `run` calls only when plain code must happen between them, for example an API call that changes data mid-test.

## Measured numbers

All numbers below were measured on an Apple M1 Max under load from other work. They show the order of magnitude, not a guarantee for your app.

**One test, TOS close flow** (`bench/results/lp-tos-e501cef`, median of 5 rounds, TOS-shaped app in the bench site):

| How the test runs | Median per test |
|---|---:|
| Playwright over CDP, same browser | 34.2 ms |
| Native driver, one CDP command per step, fresh page | 9.2 ms |
| One `LP.run`, fresh page | 8.6 ms |
| One `LP.run`, page booted ahead | 6.8 ms |
| One `LP.run`, booted pages over 4 connections, back to back | 2.7 ms |

The same flow written as Playwright code and run through `@povio/lightning-panda/playwright` took 14.7 ms per test (median of 40, fresh page) against 41 to 44 ms for stock Playwright driving the same browser (package README).

**A whole suite** (TOS, 2,158 cases in 309 files, PR #289):

| Run | Wall time | Passed |
|---|---:|---:|
| `bun test --parallel=8 --max-concurrency=1` (8 browsers) | 380 s | 2,158/2,158 |
| `lightningpanda e2e -j 12` (1 browser) | 362 s | 2,157/2,158 |
| `lightningpanda e2e -j 16` (1 browser) | 313 s | 2,154/2,158 |

The suite numbers are wall time for an app whose own JavaScript dominates the cost; the failures were app-level step timeouts and a known flaky test. [Performance](/guide/performance) collects the other measurements.

## When not to use it

- **You need Playwright's HTML report, trace viewer or video.** Neither layer provides them.
- **You must assert a transient state between an action and its result**, such as a spinner that shows only while a request is in flight. An action settles before the next step runs, so the spinner may be gone by then. Hold the request with a [mock](/e2e/network#holding-a-request) to keep the state on screen while you check it.
- **The test needs Chromium-specific behaviour or pixels.** Pyro is its own engine. See [Behaviour differences](/playwright/differences) and [Web platform support](/reference/web-platform).

## Where to go next

- [Your first test](/e2e/first-test): install, a tiny app, one test, end to end.
- [Locators](/e2e/locators) and [Steps and checks](/e2e/steps): everything a run can do.
- [Profiles and page pools](/e2e/profiles) and [Fork mode in tests](/e2e/fork): how pages start booted.
- [Suite runner: lightningpanda e2e](/e2e/runner): one process and one browser for a whole suite.
