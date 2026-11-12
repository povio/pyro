# Debugging failures

When a run fails, the browser tells you which step failed and why it was still waiting when its timeout passed. Most failures are fixed by reading that one line. This page explains the error's parts, lists the reasons the browser reports, and shows how to look at the page when the reason is not enough.

## Anatomy of a RunError

```text
RunError: step 2 (request {"method":"POST","path":"/api/items","status":200}): timed out after 300 ms: request {"method":"POST","path":"/api/items","status":200}: last match was {"error":null,"method":"POST",...,"status":201,"url":"http://localhost:51142/api/items"}
```

| Part | Meaning |
|---|---|
| `step 2` | The index of the failed step in the array you passed to `page.run` (0-based). Also `error.step` |
| `(request {...})` | The step, described: its kind and its locator or matcher as JSON. Actions show as `Click {...}`, `Fill {...}`; checks as `expect Visible {...}`, `expect Text {...}`; reads as `read Text {...}` |
| `timed out after 300 ms` | The step waited its full timeout. Missing for failures that do not wait (see below) |
| `last match was {...}` | **Why it was still waiting**: the reason from the last attempt |

```ts
import { RunError } from '@povio/pyro';

try {
  await page.run(steps);
} catch (e) {
  if (e instanceof RunError) console.log(e.step, e.message);
  throw e;
}
```

When the failure happens in a step that `pyro.page({ route, url })` added in front of yours, the message starts with `opening the page (...)` instead, and `error.step` is negative:

```text
opening the page ({"goto":{"url":"http://127.0.0.1:1/","waitUntil":"load"}}) (goto http://127.0.0.1:1/): the navigation failed before its document loaded
```

A `matchScreenshot` mismatch throws `ScreenshotMismatchError` instead (see [Visual tests](/e2e/screenshots#when-a-screenshot-does-not-match)).

## Reasons a step waits

These come from the last attempt before the timeout.

### Locating

| Reason | What to do |
|---|---|
| `{...} matched no element` | The locator found nothing. Check it against the page (below), or the step before it never got the UI there |
| `{...} matched no frame` / `{...} is not an iframe` | The frame locator of `.frame()` / `inFrame()` is wrong |
| `the frame has no document yet` | The iframe has not loaded |
| `element is detached` | The element was removed while the step ran; the step retries on the new DOM |

### Actionability

| Reason | What to do |
|---|---|
| `element is not visible (box x,y wxh)` / `element is not visible (not rendered)` | It has no box with width and height: hidden, collapsed, or not laid out yet |
| `element is outside the viewport (...)` | It could not be scrolled into view; the box and viewport are printed |
| `<div> intercepts pointer events` | Another element covers the click point: an overlay, a toast, a modal backdrop. Close it first, or click what a user would click. `force: true` skips the check when the cover is intended |
| `element is disabled` | Wait for the state that enables it (a `check(...).enabled()` before), or the form is not valid yet |
| `element is not editable` | `fill` on something that is not an input, textarea or contenteditable element |
| `element is not a checkbox or radio button` | `check()`/`uncheck()`/`.checked()` on another kind of element |
| `element is not an <input type=file>` | `upload` on the wrong element |
| `drop target is not visible` / `{...} matched no drop target` | `drag(target)`: the target is wrong or hidden |
| `unknown key "Foo"` / `cannot type '…'` | A key name `press` or `type` does not know (see [Keys](/e2e/steps#keys)) |
| `malformed time value` | `fill` on `<input type=time>` with a bad value |

### Checks

| Reason | What to do |
|---|---|
| `expected Text "Something", got "Todos"` | The check's assertion and what it saw. The kind (`Text`, `Visible`, `Count`, `Value`, ...) and `not ` appear as in the step |
| `expected Visible null, got false` | `.visible()` on a locator whose match has no box, or with no match |
| `url is http://…` | `waitForUrl`: the URL the page is on |

### Requests, mocks, popups, downloads

| Reason | What to do |
|---|---|
| `no matching request yet` | No request with this method and path in this run. The path is exact and has no query string; the request check must be in the same run as the action |
| `the matching request is still in flight` | It was sent, no response yet |
| `last match was {...}` | It finished with another status; the request is printed |
| `N matching requests (M finished), want n` / `N matching requests started, want n` | `.count(n)` saw another number |
| `matching request status differs: {...}` | With `.count(n)`, one match has another status |
| `context request history was truncated; an exact assertion is unavailable` | `.scope('context')` on a page that dropped old log entries |
| `the page has no request log` | The page has no request log to check |
| `nothing is held under "save" yet` | `release('save')` before any request matched the holding mock |
| `held response statuses [...], want n matching ...` | The wire-only `held` step |
| `no popup yet` / `no download yet` | `popup()` / `download()`: the page opened none during the run |
| `no page <id> in this context` | `switchTo` with an unknown name or id |
| `page is gone` | The page closed during the run |
| `the page is still on about:blank, its navigation pending` | `route` on a pooled page whose navigation has not started its document |

## Failures that do not wait

These fail the step at once, without `timed out after`:

| Message | Cause |
|---|---|
| `strict mode violation: {...} matched 3 elements: <button> "x", ...` | The locator is ambiguous; up to five matches are listed. Scope, filter or make it exact ([Strict mode](/e2e/locators#strict-mode)) |
| `rejected: <message>` | An `evaluate` whose promise rejected |
| `timed out after N ms waiting for the promise to settle` | An `evaluate` whose promise did not settle within the step's timeout |
| `timed out after N ms waiting for load` (or `DOMContentLoaded`) | `goto`, `reload` or `loaded` |
| `the navigation failed before its document loaded` | `goto`/`reload` to a URL that could not load (refused, DNS failure) |
| `the page is on about:blank: route needs a document (goto a URL first, or loaded on a prepared page)` | `route` on a blank page |
| `invalid css selector`, `invalid xpath: ...`, `invalid regex ...`, `unsupported regex flag ...` | A malformed locator or pattern |
| `step 0: unknown variant 'bogus', expected one of ...` | A malformed step (only with hand-written wire steps) |
| `mock takes body or json, not both` and the other mock errors | See [Requests and mocks](/e2e/network#options) |

Errors from the SDK itself, before anything reaches the browser:

| Message | Cause |
|---|---|
| `no profile X: declare it with pyro.profile()` | `pyro.page({ as: 'X' })` for an undeclared profile |
| `filter before first() / last() / nth()` | A filter chained after a pick |
| `read(as, 'css', property) needs the property` | `read(..., 'css')` without the property |
| `screenshot name '...': use letters, digits, ...` | An invalid screenshot name |
| `no booted page for X after N ms` | `pyro.warm()` timed out |
| `pyro serve did not start: ...` | `Pyro.launch()` could not start the server within 15 s |
| `setup and base need serve --fork` | Fork-only profile options on a plain server |

## Timeouts

A step waits only for its own condition: an element to appear, a request to finish, a check to hold. It does not wait for the page in general. So a timed-out step means its condition never held in that time, and the reason says what it saw instead.

- Do not raise timeouts to fix a failure. Read the reason first. Raise a step's timeout only for a slow operation you expect, such as an export that takes 10 s.
- Do not add `idle()` or `evaluate('new Promise(r => setTimeout(r, 500))')` between steps "to be safe". Actions settle on their own.
- Do not catch and retry steps. A step that sometimes fails is a race in the test or in the app; find which step and why.

## Reading results and requests

A run that passes but asserts the wrong thing is debugged from its reply:

```ts
const r = await page.run(steps);
console.log(JSON.stringify(r.results, null, 2));   // one entry per step
console.log(r.requests.map((q) => `${q.method} ${q.url} ${q.status ?? q.error}`));
```

- `results[i]` is what step `i` saw: `{ actual }` for checks, the matched request for request checks, `{ settleMs }` for actions.
- An action's result with `settled: false` and `pending` (for example `{ "requests": 1 }`) means the page did not go quiet within 1 s after it. That is a common cause of the next step failing: a request that never finishes, a held mock, or a long animation.
- `requests` lists every request with its status, error, headers and body, so a `no matching request yet` can be compared with what was actually sent.

## Looking at the page

The page stays open after a failed run, so you can inspect it in the `catch`:

```ts
try {
  await page.run(steps);
} catch (e) {
  if (e instanceof RunError) {
    const r = await page.run([
      evaluate('location.href', 'url'),
      evaluate('document.body.innerText', 'text'),
      evaluate('document.documentElement.outerHTML', 'html'),
    ]);
    console.log(r.values.url, r.values.text);
  }
  throw e;
}
```

Other ways to look:

- `css('...').read('box', 'box')`, `read('v', 'visible')`, `read('n', 'name')`, `read('c', 'count')` show how the browser sees one element.
- `check(loc).count(n)` with a wrong `n` fails with the real count.
- A `matchScreenshot` with a throwaway name writes a PNG of the state (and fails, which is fine while debugging).
- `page.send(method, params)` sends any CDP command on the page's session, for example `page.send('DOM.getDocument')`.

### Console output

The server does not print the page's console. To see it, subscribe to `Runtime.consoleAPICalled` on the CDP connection and enable the `Runtime` domain. On a server without fork mode, the page lives on `pyro.cdp`:

```ts
await using page = await pyro.page({ url });
pyro.cdp.on('Runtime.consoleAPICalled', (p, sessionId) => {
  if (sessionId === page.sessionId) console.log('console:', p.type, p.args.map((a: any) => a.value).join(' '));
});
await page.send('Runtime.enable');
```

Messages logged before `Runtime.enable` are delivered when it is enabled. In fork mode each page has its own connection, which the SDK does not expose; debug such a test against a server without `--fork`, or read state with `evaluate`.

## Server logs

`pyro serve --log-level <debug|info|warn|error|fatal>` (default `info`) controls the server's own messages on standard error: at `info` it prints what `--static` serves, what `--local-storage` seeds, which pages `--prepare` boots, and the listening address. Errors such as a failed fork-mode profile `setup` are printed there too:

```text
pyro: template: setup of profile 2: step 0 (expect Visible {"text":"missing"}): ...
```

`Pyro.launch()` passes the server's standard error through to your test process, so these lines appear in the test output. Keep them visible in CI. The log level does not add page console output or per-request logs.

## A checklist

1. Read the reason after the last colon.
2. `matched no element`: check the step before; check the locator with `count`.
3. `strict mode violation`: scope or filter.
4. `intercepts pointer events`: something covers the element.
5. A request check: compare with `r.requests`; keep the check in the same run as the action.
6. Every test of one profile fails: look for a `setup of profile` line on the server's stderr.
7. Only under load: look for `settled: false` in the results before the failing step.
