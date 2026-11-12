# Requests and mocks

The browser records every request a page makes, so a test asserts on requests instead of waiting for them. Mocks answer, delay, fail, rewrite or hold requests from inside the browser, without a round trip to the test. Both are ordinary steps of a run.

```ts
import { check, mock, unmock, release } from '@povio/pyro';
```

## Checking requests

```ts
await page.run([
  label('New todo').fill('Buy milk'),
  button('Add').click(),
  check.request('POST', '/api/items').status(201),
]);
```

`check.request(method, path)` looks at the requests the page made during this run and waits until a matching one has finished. Put it in the same run as the action that sends the request: a request check does not see earlier runs unless you ask for it with `.scope('context')`.

### Matching

| Part | Rule |
|---|---|
| `method` | Case-insensitive: `'post'` and `'POST'` are the same |
| `path` as a string | The URL's path, exactly, without query string or fragment: `'/api/items'` matches `https://app.test/api/items?page=2` |
| `path` as a `RegExp` | Tested against the full URL: `check.request('GET', /\/api\/items\?page=2$/)` |

### Modifiers

Each returns a new check, so they chain:

| Modifier | Meaning |
|---|---|
| `.status(n)` / `.status([200, 304])` | The response status is `n`, or any of the list |
| `.count(n)` | Exactly `n` matching requests, all finished (with `.status`, all with that status) |
| `.body(text)` | The request body contains `text` (case-sensitive substring of the raw body), or matches the `RegExp` |
| `.bodyKeys(['a', 'b'])` | The body is a JSON object with exactly these top-level keys |
| `.headers({ 'X-Key': /^k\d$/ })` | Each named request header is present (names case-insensitive) and its value contains the string (case-sensitive) or matches the `RegExp` |
| `.state('started')` | Count requests as soon as they are sent, in flight or done. The default, `'finished'`, counts only requests with a response or an error |
| `.scope('context')` | Also look at earlier runs of the same page. The default, `'batch'`, is this run only |
| `.timeout(ms)` | This step's timeout |

```ts
check.request('POST', '/api/save')
  .status(200)
  .body('"a":1')
  .bodyKeys(['a', 'b'])
  .headers({ 'X-Key': /^k\d$/ })
  .count(1);
```

Without `.count`, the step holds as soon as at least one matching request has finished with an accepted status, and its result is that request (the latest one). With `.count(n)`, its result is `{ count: n }`.

`.state('started')` is what you want when a mock delays or holds the request: `.count(1).state('started')` holds while the request is still parked.

### When a request check fails

The run fails with the reason the step was still waiting:

| Reason | Meaning |
|---|---|
| `no matching request yet` | Nothing with this method and path was requested during the run. Check the path (exact, no query) and that the action is in the same run |
| `the matching request is still in flight` | It was sent but has no response yet |
| `last match was {...}` | It finished, but with another status. The request is printed in full |
| `N matching requests (M finished), want n` | `.count(n)` saw a different number |
| `N matching requests started, want n` | The same, with `.state('started')` |
| `matching request status differs: {...}` | With `.count(n)`, one of the matches has another status |
| `context request history was truncated; an exact assertion is unavailable` | `.scope('context')` on a page whose log dropped old entries (see below) |

## The request log in the reply

`r.requests` lists every request the page made during the run, in the order they started:

```json
{
  "method": "POST",
  "url": "http://localhost:51142/api/items",
  "status": 201,
  "error": null,
  "ms": 0.29,
  "postData": "{\"title\":\"X\"}",
  "requestHeaders": { "authorization": "[REDACTED]", "content-type": "application/json", "...": "..." },
  "responseHeaders": { "content-type": "application/json;charset=utf-8", "...": "..." }
}
```

| Field | Contents |
|---|---|
| `method`, `url` | As sent |
| `status` | The response status, `null` while in flight or on failure |
| `error` | A network error such as `net::ERR_FAILED`, else `null` |
| `ms` | Duration, `null` while in flight |
| `requestHeaders` | The headers as sent, cookies and extra headers included |
| `responseHeaders` | Once the response head arrived, else `null` |
| `postData` | The request body as text; base64 with `postDataBase64: true` when it is not UTF-8 |
| `postDataTruncated` | `true` when the body was longer than 1 MiB and was cut there |

The values of `authorization`, `proxy-authorization`, `cookie`, `cookie2`, `set-cookie`, `set-cookie2`, `x-api-key`, `x-auth-token`, `x-csrf-token` and `x-xsrf-token` are replaced by `[REDACTED]` in the log and in error messages. `.headers({...})` still matches the real values.

`.bodyKeys` parses the logged body, so it cannot match a body that was truncated at 1 MiB. A page keeps its last 2,000 requests; older ones are dropped in halves.

The `RunResult` type declares `method`, `url`, `status`, `error` and `ms`; the other fields are present at run time.

The reply is assembled when the last step holds. A request that was still finishing at that moment can be missing from `requests` or show `status: null`; assert on requests with `check.request`, which waits.

## Mocks

```ts
mock(url: string | RegExp, options?: MockOptions)
```

A `mock` step registers a rule on the page's browser context. Requests whose URL matches are answered from inside the browser. The step itself does not wait; its result is `{ mockId, mocks }` (the rule's id and how many rules the context has).

```ts
await page.run([
  mock('/api/items', { method: 'POST', status: 500, json: { error: 'down' } }),
  label('New todo').fill('Z'),
  button('Add').click(),
  check(text('Error 500')).visible(),
]);
```

### URL matching and order

- A string matches as a **case-insensitive substring of the full URL**: `'/api/items'` matches `http://localhost:3000/api/items?x=1` and `/api/items/7`.
- A `RegExp` is tested against the full URL with JavaScript semantics. Look-ahead works, so `/\/api\/orders(?!\/draft)/` excludes one path.
- `method` restricts a rule to one method (case-insensitive).
- When several rules match, **the most recently added one wins**. Register broad rules first and exceptions after them.

Rules live on the browser context until `unmock` removes them, their `times` are used up, or the context is closed. They therefore stay active for later runs on the same page. Each page from `pyro.page()` has its own context, so mocks never leak between tests.

### Options

| Option | Default | Meaning |
|---|---|---|
| `method` | any | Only requests with this method |
| `status` | `200` | Response status |
| `json` | | Serialized as the body; content type `application/json` |
| `body` | | Text body; content type `text/plain; charset=utf-8` |
| `contentType` | from `json`/`body` | Overrides the content type |
| `headers` | | Extra response headers; values must be strings |
| `delay` | `0` | Milliseconds before the response (loading states) |
| `abort` | | Fail the request with a CDP `Network.ErrorReason`, such as `'Failed'` or `'ConnectionRefused'`; no response |
| `times` | unlimited | Answer only the first `n` matching requests, then remove the rule |
| `passthrough` | `false` | Let matching requests reach the network unchanged (an exception to a broader mock) |
| `patch` | | Send the request to the real backend, then rewrite its response: `{ status?, headers?, json?, body? }` |
| `hold` | | Park matching requests under this name until a `release(name)` step |

The browser rejects contradictory options when the run starts:

| Combination | Error |
|---|---|
| `body` and `json` | `mock takes body or json, not both` |
| `abort` with `status`, `body` or `json` | `an aborted mock has no response` |
| `times: 0` | `times must be at least 1` |
| `patch` with `passthrough: false` | `patch rewrites the real response: it cannot take passthrough: false` |
| `passthrough` (or `patch`) with `status`, `body`, `json`, `abort`, `contentType` or `headers` | `a passthrough mock takes patch, delay and hold, not a response` |
| `patch` with both `body` and `json` | `patch takes body or json, not both` |
| `hold: ''` | `hold must be a name` |

### Examples

All of these were run against a small app whose Save button posts to `/api/save` and prints the JSON reply.

```ts
// Answer once, then let the next request through to the server.
mock('/api/save', { json: { mocked: 1 }, times: 1 })

// Fail the request: the reply's requests show error "net::ERR_FAILED".
mock('/api/save', { abort: 'Failed' })

// A slow response, to see a loading state (the action settles after it).
mock('/api/save', { json: { d: 1 }, delay: 300 })

// Mock the whole API except one endpoint (later rules win).
mock('/api/', { json: { all: true } }),
mock('/api/save', { passthrough: true }),

// Call the real backend and rewrite its JSON: a JSON merge patch, null deletes a key.
mock('/api/save', { patch: { json: { real: false, nested: null } } })
```

`patch.json` is applied as a JSON merge patch (RFC 7396) when the real body is a JSON object; otherwise it replaces the body. `patch.body` replaces the body, `patch.headers` sets headers, `patch.status` replaces the status.

### Holding a request

`hold` keeps a state on screen that would otherwise pass too quickly to check, such as a spinner or a disabled button while saving. With the app from [Your first test](/e2e/first-test):

```ts
await page.run([
  mock('/api/items', { method: 'POST', hold: 'save', json: { id: 99, title: 'held' } }),
  label('New todo').fill('X'),
  button('Add').click({ settle: false }),                        // its request will not finish yet
  check.request('POST', '/api/items').count(1).state('started'),
  check(text('Added X')).not.visible(),                            // the in-between state
  release('save'),
  check.request('POST', '/api/items').status(200),
  check(text('Added X')).visible(),
]);
```

In your app, the in-between check is where a spinner or a disabled Save button is asserted.

- Without `passthrough`, the request stays unsent until `release`, then gets the mock's response (when `status`, `body` or `json` is given) or goes on to the network unchanged.
- With `passthrough: true` or `patch`, the request reaches the backend and its **response** is held instead.
- With `delay` as well, a held request is released after the delay if no `release` came first.
- `release(name)` waits until something is held under that name, so it may come before the action's request starts. It releases everything held under the name; its result is `{ released: n }`. Holds outlive the run.
- Pass `{ settle: false }` to the action that sends a held request. Otherwise the action waits for the request to finish, gives up after 1 s of settling, and carries on with `settled: false` in its result.

## Removing mocks

```ts
unmock('/api/save')   // rules registered with exactly this url ('/api/save' string, or the same RegExp source)
unmock()              // every rule of the context
```

`unmock` removes rules whose pattern equals the one given, not rules that would match it. Its result is `{ removed: n }`.

## Regular expressions

A `RegExp` anywhere in the native API (locators, `mock`, `unmock`, `waitForUrl`, `check.request`, `.body`, `.headers`) is sent as its source and flags. The browser runs it with ECMAScript semantics: look-ahead `(?=...)`, negative look-ahead `(?!...)`, look-behind, backreferences and named groups work. Flags `i`, `m` and `s` are kept; the builders drop `g`, `y` and `u`.

## Mocks across the stack

Mocks sit in front of CDP `Fetch` interception on the same context, so a request no rule answers continues to Playwright-style routes (if any) and then to the network. Combined with [`--static`](/e2e/static), mocking the API of a static origin tests a front-end build with no server at all.

Mocks are scoped to the browser context of the page. To mock requests before the app's first request, the mock must run before the navigation: take a blank page and put the `mock` before a `goto`:

```ts
await using page = await pyro.page({ initScripts: ['localStorage.setItem("token", "t")'] });
await page.run([
  mock('http://app.test/api/items', { json: [{ id: 7, title: 'Mocked item' }] }),
  goto('http://app.test/'),
  check(text('Mocked item')).visible(),
]);
```

A page from a profile pool booted before the test, so its first requests have already happened. See [Network, routes and HAR](/playwright/network) for the Playwright layer's `page.route`.
