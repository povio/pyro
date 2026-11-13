# Settling and auto-waiting

Pyro tests contain no sleeps, no polling loops and no `waitForResponse`. Two mechanisms replace them, both running inside the browser: **steps that wait for their condition**, and **actions that settle**. This page states the rules precisely, because knowing them tells you exactly when a test can rely on the page being up to date.

## Steps wait for their condition

Every locator action and every check has a timeout (5 seconds by default). Until the timeout, a step that does not hold yet is parked and tried again whenever the page's DOM changes or its document is replaced. Nothing polls on a clock.

| Step | Waits until |
| --- | --- |
| `loc.click()`, `dblclick`, `hover`, ... | the locator matches exactly one element that is visible, enabled (for actions that need it), not covered, then scrolls it into view and acts |
| `loc.fill(v)`, `press`, `type`, `check`, ... | one matching element that is visible, enabled and editable |
| `check(loc).visible()`, `.text(...)`, `.count(n)`, ... | the assertion holds |
| `check.request(method, path)` | a matching request made during this run has finished (or started, with `.state('started')`) |
| `goto(url)` | the new document reaches `load` |
| `waitForUrl(url)` | the page URL contains or matches it |

Set a timeout per run with `page.run(steps, { timeout })`, or per step with `{ timeout }` on the step. A timeout means the condition never held: raising it rarely helps.

## Actions settle

After an action (`click`, `dblclick`, `fill`, `press`, `type`, `check`, `uncheck`, `route`, `viewport`, and the other actions), the next step starts only when the page is **quiet**. A page is quiet when all of these are true at the same moment:

| Condition | Counted as pending work |
| --- | --- |
| No queued task | any task queued on the browser thread, including results posted from the network thread that have not run yet |
| No request the action started | finite requests in flight beyond those that were already in flight before the action |
| No timer due soon | a `setTimeout` due within the next **50 ms** |
| No animation frame pending | a `requestAnimationFrame` callback, unless it is part of an animation loop |
| No ResizeObserver work | queued ResizeObserver gathering or notifications |
| No scroll work | a `scroll` event the next rendering update owes, or scroll offsets to clamp again after a DOM change (newer than 0.0.33) |
| No navigation in progress | a frame whose navigation has not reached `DOMContentLoaded` |

Microtasks (promise reactions, MutationObserver records) always run to completion before a task ends, so they never appear as pending. Readiness is checked between event loop tasks, so the next step starts right after the last piece of work.

An action waits at most **1 second** for the page to go quiet. After that the run carries on, and the step's result records what was still pending.

Newer than 0.0.33, a settle does not wait for the frame clock. When only `scroll` events or ResizeObserver work are left, which a browser delivers at its next rendering update, the settle runs that update at once.

### What does not hold up settling

These are excluded on purpose, because they would otherwise keep a page busy forever:

- `setInterval` timers, and `setTimeout` timers due more than 50 ms in the future (polling, debounces, toasts that hide after 5 s);
- requests that were already in flight before the action (long polls). A poll that the page sends again while the action settles counts as started by the action;
- established `text/event-stream` responses (server-sent events);
- open WebSockets;
- animation loops: a frame callback requested from a frame callback that was itself requested from one. A single frame, or the double-frame "after paint" idiom, still counts;
- a paused fake clock (`page.clock` in the Playwright module).

If your test needs something on that list to happen, assert its outcome with a check. The check waits for it.

### Turning settling off

Pass `{ settle: false }` to an action whose effect you do not want to wait for, such as one that starts a stream or polling that never finishes:

```ts
await page.run([
  button('Start live updates').click({ settle: false }),
  check(text('Connected')).visible(),
]);
```

An action with `settle: false` still gets one event loop turn before the next step. The tasks it queued run in that turn, and the `scroll` events it caused are delivered. `focus()` and `blur()` never settle and get the same turn. `route(path, { settle: false })` changes the route without waiting for the requests it starts.

Newer than 0.0.33, an `evaluate` step gets that turn only when its expression left the page something to run: a task, a timer, a request, a DOM change or observer work. After an `evaluate` that only read, the next step starts directly. To wait for something a later step needs, use a check, not an `evaluate`.

## The `idle` step

`idle({ timeout })` waits for the same quiet state as an action's settle, with no requests in flight allowed, up to its timeout. It never fails. When it gives up, its result says what was still pending:

```ts
import { evaluate, idle } from '@povio/pyro';

const r = await page.run([
  evaluate('(function tick() { setTimeout(tick, 10) })(), 1', 'started'),
  idle({ timeout: 300 }),
]);
console.log(r.results[1]);
// { pending: { timers: 1 }, settleMs: 304.2, settled: false }
```

A quiet page gives `{ settleMs: 0.4 }` or similar. Use `idle` after `evaluate` steps that start work, or to find out why a page never settles. The CDP command `Pyro.idle {timeout}` does the same on its own.

## Settling in the Playwright module

The Playwright module sends each awaited call as one `Pyro.run`, so the same rules apply: `await locator.click()` returns after the page has settled. Web-first assertions (`await expect(locator).toHaveText(...)`) wait in the browser the same way a native check does.

### Compared with Playwright's auto-waiting

| | Playwright on Chrome | Pyro |
| --- | --- | --- |
| Before an action | actionability checks (attached, visible, stable, enabled, receives events), retried by the client | the same checks, retried in the browser on DOM changes |
| After an action | returns once the input is dispatched; it does not wait for the app to react | returns once the page is quiet, as defined above |
| Assertions | retried by polling until the timeout | retried in the browser on DOM changes until the timeout |
| Waiting for a response | `waitForResponse`, in parallel with the action | not needed after an action; `check.request` reads the run's request log |

The practical difference: code written for Playwright that adds `waitForLoadState('networkidle')`, `waitForResponse` or retries after a click still works, but has nothing left to wait for.

## Transient states

Because an action returns only after the app has reacted, a state that exists only **between** the action and its result can be over by the time the next step runs. The common case is a loading spinner:

```ts
// Playwright on Chrome: usually sees the spinner. On Pyro the click
// settles after the request finishes, so the spinner is already gone.
await page.getByRole('button', { name: 'Save' }).click();
await expect(page.getByRole('progressbar')).toBeVisible(); // can fail
```

To test a transient state, hold the work that ends it. With the native SDK, hold the request until you have checked, and do not let the click settle on a request that cannot finish yet:

```ts
import { button, check, mock, release, role, text } from '@povio/pyro';

await page.run([
  mock('/api/save', { hold: 'save', json: { ok: true } }),
  button('Save').click({ settle: false }),
  check(role('progressbar')).visible(),   // the request is held, so the spinner shows
  release('save'),                        // now answer it
  check(role('progressbar')).hidden(),
  check(text('Saved')).visible(),
]);
```

Without `settle: false` the run still passes, but the click waits the full second for its held request before the next step starts. Newer than 0.0.33, the step after `release` starts once the page has the released reply. A request that `release` sends on to the backend is not waited for. In the Playwright module, a `page.route` handler that waits on a promise you resolve after the assertion does the same job.

## Debugging a step that never settles

- Add `idle({ timeout: 2000 })` after the action and read its `pending` field.
- `timers` that never clears: a `setTimeout` chain shorter than 50 ms (a polling loop written with `setTimeout`). Use `settle: false` on the action, or check the outcome directly.
- `requests` that never clears: a request the action started that does not finish, such as a long poll started by the click. Use `settle: false`.
- `navigations`: the page started a navigation that has not reached `DOMContentLoaded`.
- `PYRO_RUN_STEP_TRACE=1` in the server's environment logs each step, its stages and what it waited on to stderr.

See [Debugging failures](/e2e/debugging) for reading `RunError` messages.
