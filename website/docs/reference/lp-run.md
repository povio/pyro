# LP.run wire format

`LP.run` is a CDP command that sends a whole test scenario to the browser in one message: a list of steps (navigate, act, check, read, mock, ...). The browser runs them in order and replies once, when the last step holds or the first one fails. Locators are resolved against the DOM in Rust, and actionability checks and waiting happen in the browser, so a scenario costs one round trip. The SDK's `page.run()` sends this command. Any CDP client can send it too. Step semantics (waiting, settling, strictness) are described in [SDK: locators and steps](/reference/steps#semantics). This page is the JSON reference.

## The LP domain {#domain}

The `LP.*` commands are page-session commands: send them with the `sessionId` of an attached page (flat mode, `Target.attachToTarget {flatten: true}`). Without a session they fail with `-32000 LP.run is not supported by lightningpanda`.

| Command | Params | Result |
| --- | --- | --- |
| [`LP.run`](#lp-run) | `{steps, timeout?}` | `{results, reads, requests, ms}` |
| [`LP.act`](#lp-act) | one [`act`](#act) step body | the step's result |
| [`LP.expect`](#lp-expect) | one [`check`](#check) step body | the step's result |
| [`LP.query`](#lp-query) | one [`read`](#read) step body | `{value}` |
| [`LP.idle`](#lp-idle) | `{timeout?}` | the [`idle`](#idle) step's result |
| [`LP.dialogs`](#lp-dialogs) | `{block}` | `{}` |

## LP.run {#lp-run}

### Params {#params}

```json
{ "steps": [ { "goto": { "url": "http://app.test/" } }, { "check": { "locator": { "role": "heading" }, "assert": "visible" } } ],
  "timeout": 10000 }
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `steps` | array of [step objects](#steps) | required | At most 10000 steps. |
| `timeout` | integer (ms) | `5000` | Per-step timeout for steps without their own `timeout`. Values are capped at 600000. |

Each step is an object with exactly one key, the step kind, whose value is the step's body. Unknown keys are rejected everywhere: in steps, bodies and locators.

### Result {#result}

```json
{
  "results": [ {}, { "actual": true } ],
  "reads": { "title": "Hello" },
  "requests": [ { "method": "GET", "url": "http://app.test/", "status": 200, "error": null, "ms": 3.1, "requestHeaders": {}, "responseHeaders": {} } ],
  "ms": 104.26
}
```

| Field | Description |
| --- | --- |
| `results` | One entry per step, in order. See [step results](#step-results). |
| `reads` | Values of steps that have an `as` key (`read`, `eval`, `download`, `downloadCount`, `popup`, `held`, `matchScreenshot`), keyed by `as`. The SDK returns this as `values`. |
| `requests` | Requests the run's starting page made from the start of the run on. See [request log](#request-log). |
| `ms` | Time the run took in the browser, in ms (float). |

### Errors {#errors}

The run fails at the first failing step. The JSON-RPC error has:

| Code | When | Message |
| --- | --- | --- |
| `-32602` | A step does not parse or validate. Nothing has run. | `step <i>: <reason>`, e.g. `step 0: locator needs exactly one of css, xpath, testId, text, role, label, placeholder, ariaRef, of` |
| `-32602` | More than 10000 steps | `too many steps` |
| `-32000` | A step failed or timed out while running | `step <i> (<what>): <reason>` |
| `-32000` | Over 1024 commands are already waiting on this connection | `too many waiting LP commands` |

`<i>` is the zero-based index in `steps` and `<what>` describes the step. Typical reasons:

```text
step 0 (expect Visible {"css":"#nope"}): timed out after 100 ms: expect Visible {"css":"#nope"}: expected Visible null, got false
step 2 (Click {"name":"Save","role":"button"}): strict mode violation: {"name":"Save","role":"button"} matched 2 elements: <button> "Save", <button> "Save"
step 1 (goto http://app.test/): the navigation failed before its document loaded
step 3 (eval): rejected: TypeError: ...
step 4 (matchScreenshot home): matchScreenshot 'home': 812 pixels differ (1280x720); the actual image and the diff are in error.data
```

A failed `matchScreenshot` step carries `error.data`: its [result object](#match-screenshot-result) with `actual` and `diff` (base64 PNGs). Steps before the failing one have taken effect. Mocks they registered stay registered.

## Locator {#locator}

A locator names elements. It needs **exactly one** base kind, can be scoped and combined, and is narrowed by `filter` and `nth`.

```json
{ "role": "button", "name": "Close Position",
  "within": { "role": "dialog", "filter": { "hasText": "will be closed" } } }
```

| Field | Type | Description |
| --- | --- | --- |
| `css` | string | *Base.* CSS selector. |
| `xpath` | string | *Base.* XPath expression, relative to `within` (else the document). |
| `testId` | string | *Base.* Value of the test-id attribute. |
| `testIdAttribute` | string | Attribute for `testId` (default `data-testid`). |
| `text` | [pattern](#pattern) | *Base.* Elements by their text. |
| `role` | string | *Base.* ARIA role. |
| `name` | pattern | Accessible name for `role`. |
| `label` | pattern | *Base.* Form controls by their label. |
| `placeholder` | pattern | *Base.* Inputs by placeholder. |
| `ariaRef` | string | *Base.* Playwright's `aria-ref=e5`: the element that the document's last AI-mode ARIA snapshot gave this ref (`f1e5` for a child frame). |
| `of` | locator | *Base.* Another locator's matches as the base set, so a `filter` or `nth` can follow an `nth`. |
| `exact` | boolean | Whole-string, case-sensitive match for `text`, `name`, `label` and `placeholder` (after whitespace normalization). Otherwise matching is a case-insensitive substring. Ignored by regex patterns. |
| `within` | locator | Search inside each match of this locator. |
| `frame` | locator | Search inside the document of the `<iframe>` this locator matches (in the parent document; it can have its own `frame`). |
| `or` | locator[] | Add these locators' matches, in document order. |
| `and` | locator[] | Keep only elements that each of these locators matches too. |
| `filter` | object | `{visible?, hasText?, hasNotText?, has?, hasNot?}`: every given condition must hold. `hasText` is a case-insensitive substring (or regex) of the text. `has` and `hasNot` are locators matched inside. |
| `nth` | integer | Pick one match. Negative counts from the end. Applied after `filter`. |
| `checked`, `pressed` | `true` \| `false` \| `"mixed"` | Role state filters (need `role`). |
| `disabled`, `expanded`, `selected` | boolean | Role state filters (need `role`). |
| `level` | integer | Heading level (`aria-level` or `h1`-`h6`; needs `role`). |
| `includeHidden` | boolean | Let hidden elements match a `role`. |

### Pattern {#pattern}

A text pattern is either a string or a regex object:

```json
"Save"
{ "regex": "^Save( all)?$", "flags": "i" }
```

Regexes use ECMAScript semantics (look-around and backreferences work). Only the flags `i`, `m` and `s` are allowed. Any other flag, or an invalid regex, fails validation.

## Steps {#steps}

Every step kind with its body. `timeout` (ms) is accepted where listed and overrides the run's `timeout`.

### goto {#goto}

```json
{ "goto": { "url": "http://app.test/login", "waitUntil": "load", "timeout": 10000 } }
```

| Field | Type | Default |
| --- | --- | --- |
| `url` | string | required |
| `waitUntil` | `"load"` \| `"domcontentloaded"` | `"load"` |
| `timeout` | integer | run timeout |

Navigates as the address bar would and waits for the event. Result `{}`.

### loaded, reload {#loaded}

```json
{ "loaded": {} }
{ "reload": { "timeout": 10000 } }
```

`loaded` waits for the current document's `load`. On a page taken from a pool while it is still booting, it waits for the profile URL's load. `reload` reloads and waits for `load`. Both take only `timeout` and return `{}`.

### idle {#idle}

```json
{ "idle": { "timeout": 2000 } }
```

Waits until the page is quiet, up to its timeout, and never fails. Result: `{settleMs}`, plus `settled: false` and `pending` (`{tasks?, requests?, timers?, frames?, observers?, navigations?}`, nonzero counts only) if the timeout passed first.

### url {#url}

```json
{ "url": { "url": "/dashboard", "timeout": 5000 } }
{ "url": { "url": { "regex": "/orders/\\d+$" } } }
```

Waits until the page URL contains (or matches) the pattern. Result `{url}`.

### route {#route}

```json
{ "route": { "url": "/positions/42", "settle": true, "timeout": 5000 } }
```

Same-document navigation: `history.pushState` and a `popstate` event. Settles on the requests it starts unless `settle` is `false`. Result `{settleMs, …}`.

### act {#act}

```json
{ "act": { "locator": { "role": "button", "name": "Save" }, "action": "click" } }
{ "act": { "locator": { "label": "Email" }, "action": "fill", "value": "ann@example.com" } }
{ "act": { "locator": { "css": "#file" }, "action": "upload", "files": [ { "name": "a.csv", "text": "a,b\n1,2" } ] } }
```

| Field | Type | Default | Applies to |
| --- | --- | --- | --- |
| `locator` | [locator](#locator) | required | all |
| `action` | `click` \| `dblclick` \| `hover` \| `fill` \| `type` \| `press` \| `check` \| `uncheck` \| `focus` \| `blur` \| `upload` \| `dropFiles` \| `drag` \| `wheel` | required | |
| `value` | string | `""` | `fill`, `type` (text), `press` (key or chord such as `Shift+Tab`), `upload`/`dropFiles` (paths, one per line) |
| `files` | `[{name, mimeType?, text? \| base64?}]` | `[]` | `upload`, `dropFiles`: inline files, 64 MiB each at most. `mimeType` defaults from the extension. |
| `to` | locator | | `drag` (required) |
| `button` | `left` \| `right` \| `middle` | `left` | pointer actions |
| `position` | `{x, y}` | center of the visible part | `click`, `dblclick`, `hover`, `wheel`: offset from the element's top-left corner |
| `force` | boolean | `false` | `click`, `dblclick`, `hover`, `wheel`: skip the hit test and the enabled check |
| `deltaX`, `deltaY` | number | `0` | `wheel` |
| `modifiers` | `("Shift" \| "Control" \| "Alt" \| "Meta" \| "ControlOrMeta")[]` | `[]` | pointer actions (`ControlOrMeta` is Meta on macOS) |
| `settle` | boolean | `true` in `LP.run`, `false` in `LP.act` | wait for the page to go quiet afterwards |
| `timeout` | integer | run timeout | |

Result `{}`, or `{settleMs, settled?, pending?}` after a settle.

### mouse {#mouse}

```json
{ "mouse": { "action": "move", "x": 100, "y": 200, "steps": 5 } }
{ "mouse": { "action": "wheel", "deltaY": 400 } }
```

| Field | Type | Default |
| --- | --- | --- |
| `action` | `click` \| `dblclick` \| `move` \| `down` \| `up` \| `wheel` | required |
| `x`, `y` | number (viewport px) | current position. Required for `click`, `dblclick` and `move`. Give both or neither. |
| `button` | `left` \| `right` \| `middle` | `left` |
| `deltaX`, `deltaY` | number | `0` |
| `steps` | integer 1-1000 | `1` (intermediate `mousemove`s of a `move`) |
| `modifiers` | as for `act` | `[]` |
| `settle` | boolean | `true` |
| `timeout` | integer | run timeout |

### check {#check}

```json
{ "check": { "locator": { "css": ".row" }, "assert": "count", "expected": 3 } }
{ "check": { "locator": { "role": "dialog" }, "assert": "visible", "not": true } }
{ "check": { "locator": { "css": "a.docs" }, "assert": "attribute", "property": "href", "expected": { "regex": "/docs$" } } }
```

| Field | Type | Description |
| --- | --- | --- |
| `locator` | locator | required |
| `assert` | string | See the table below. |
| `expected` | JSON | Expected value. |
| `not` | boolean | Negate. |
| `property` | string | CSS property (`css`) or attribute name (`attribute`). Required for those two. |
| `timeout` | integer | |

| `assert` | `expected` | Holds when |
| --- | --- | --- |
| `visible` | | one match and it is visible |
| `hidden` | | no match, or the one match is not visible |
| `text` | string | text equals (both whitespace-normalized) |
| `containsText` | string | text contains (normalized) |
| `value` | string | form control value equals |
| `count` | integer | number of matches equals |
| `checked` | `true` (default) \| `false` \| `"mixed"` | checkbox/radio state equals |
| `css` | string | computed value of `property` equals |
| `attribute` | string (exact), pattern, or `null` (absent) | attribute `property` equals, matches, or is absent |
| `description` | pattern | accessible description (`aria-describedby`, else `aria-description`, else `title`) matches |
| `focused` | boolean (default `true`) | is the focused element |
| `name` | string | accessible name equals (normalized) |
| `enabled` | boolean (default `true`) | is not disabled |

Result `{actual}`: the value it saw last.

### read {#read}

```json
{ "read": { "locator": { "css": "h1" }, "read": "text", "as": "title" } }
{ "read": { "locator": { "css": ".btn" }, "read": "css", "property": "background-color", "as": "bg" } }
```

| Field | Type | Description |
| --- | --- | --- |
| `locator` | locator | required |
| `read` | `text` \| `value` \| `count` \| `visible` \| `box` \| `css` \| `name` \| `focused` \| `enabled` \| `aria` | What to read |
| `as` | string | Key in `reads` |
| `property` | string | `css`: property name (required) |
| `mode` | `"default"` \| `"ai"` | `aria` only: Playwright's AI snapshot with refs |
| `frameSeqs` | `{frameId: n}` | `aria` in AI mode: refs in a frame with seq `n > 0` are named `f<n>e…` |
| `timeout` | integer | |

Result: the value. `text` is a normalized string. `value`, `css` and `name` are strings. `count` is an integer (never waits). `visible`, `focused` and `enabled` are booleans. `box` is `{x, y, width, height}` or `null`. `aria` is the ARIA tree object that the Playwright drop-in uses for `ariaSnapshot()`.

### eval {#eval}

```json
{ "eval": { "expression": "document.title", "as": "title" } }
{ "eval": { "expression": "(el, cls) => el.classList.contains(cls)", "locator": { "css": "#menu" }, "arg": "open", "as": "open" } }
```

| Field | Type | Description |
| --- | --- | --- |
| `expression` | string | Evaluated in the page's main world |
| `as` | string | Key in `reads` |
| `call` | boolean | The expression is a function: call it (Playwright's `evaluate(fn, arg)`) |
| `locator` | locator | Call the function with the element this matches (waits for exactly one). Implies `call`. |
| `all` | boolean | With `locator`: call it with the array of all matches |
| `arg` | JSON | Last argument of the call. Implies `call`. |
| `timeout` | integer | How long to wait for a returned promise |

Result: the JSON value (a returned promise is awaited). A rejection fails the step with `rejected: <message>`.

### request {#request}

```json
{ "request": { "method": "POST", "path": "/api/orders", "status": [200, 201], "bodyKeys": ["sku", "qty"] } }
```

Waits until the run's request log has a match. All fields are optional.

| Field | Type | Description |
| --- | --- | --- |
| `method` | string | Case-insensitive |
| `path` | string | Exact path, no query |
| `url` | pattern | Substring of, or regex on, the full URL |
| `status` | integer or integer[] | One status, or any of a non-empty list |
| `count` | integer | Exactly this many matches (default: at least one) |
| `body` | pattern | Request body (as sent, UTF-8) contains or matches |
| `bodyJson` | JSON | Request body parsed as JSON equals this value |
| `bodyKeys` | string[] | Body is a JSON object with exactly these top-level keys |
| `headers` | `{name: pattern}` | Request headers present (names case-insensitive) whose values contain or match |
| `state` | `"finished"` (default) \| `"started"` | `finished`: with a response or an error. `started`: also in flight |
| `scope` | `"batch"` (default) \| `"context"` | `context` also includes earlier runs of this page (never sibling pages) |
| `timeout` | integer | |

Result: the matched [request log entry](#request-log), or `{count}` when `count` is given.

### mock {#mock}

```json
{ "mock": { "url": "/api/items", "json": [] } }
{ "mock": { "url": { "regex": "/api/slow" }, "delay": 2000, "status": 503, "body": "busy" } }
{ "mock": { "url": "/api/profile", "patch": { "json": { "plan": "pro" } } } }
```

Registers a mock on the page's browser context. It applies to every page of the context and stays registered across runs until [`unmock`](#unmock).

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `url` | pattern | required | Substring of, or regex on, the URL |
| `method` | string | any | |
| `status` | integer | `200` | |
| `body` | string | `""` | Content type `text/plain; charset=utf-8` |
| `json` | JSON | | Serialized body. Content type `application/json`. Not with `body`. |
| `contentType` | string | | Overrides the content type |
| `headers` | `{name: string}` | | Response headers. Values must be strings. |
| `delay` | integer (ms) | `0` | Before the response (capped at 600000) |
| `abort` | string | | `Network.ErrorReason` (`Failed`, `ConnectionRefused`, ...). Not with `status`, `body` or `json`. |
| `times` | integer ≥ 1 | unlimited | Uses |
| `passthrough` | boolean | `false` | The request reaches the real server. `patch`, `delay` and `hold` apply to its response. A response field (`status`, `body`, `json`, `abort`, `contentType`, `headers`) is not allowed. |
| `hold` | string | | Park the request (with `passthrough`, its response) under this name until a [`release`](#release) step or until `delay` elapses |
| `patch` | `{status?, headers?, json?, body?}` | | Rewrites the real response. `json` is a JSON merge patch (RFC 7396) when the real body is a JSON object, else the new body. Not both `json` and `body`. Implies `passthrough`, so it cannot be combined with `passthrough: false`. |

Result `{mockId, mocks}`: the new mock's id and the number of mocks on the context.

### unmock {#unmock}

```json
{ "unmock": { "url": "/api/items" } }
{ "unmock": {} }
```

Removes the mocks whose `url` pattern text equals the given one, or all mocks of the context. Result `{removed}`.

### release {#release}

```json
{ "release": { "hold": "save", "body": "{\"ok\":false}", "timeout": 5000 } }
```

Lets go of what a mock holds under `hold`. It waits until at least one request or response is held there. `body` replaces the response body (32 MiB at most). Result `{released: n}`.

### held {#held}

```json
{ "held": { "hold": "save", "count": 1, "status": 200, "as": "held" } }
```

Waits until exactly `count` responses are held under `hold`, all with an accepted `status` (an integer or a list). Result `{count, statuses, responses}`. This step has no SDK builder.

### advanceTime {#advance-time}

```json
{ "advanceTime": { "ms": 30000 } }
```

Advances the clock of the page's browser thread by `ms` (at most one day per step). `Date` follows the page clock from then on, and timers that come due fire. This does not need `LIGHTNINGPANDA_VIRTUAL_TIME`. Result `{advanced: ms}`. This step has no SDK builder.

### viewport {#viewport}

```json
{ "viewport": { "width": 390, "height": 844 } }
```

Resizes the viewport in CSS px and settles. Result `{width, height}`.

### media {#media}

```json
{ "media": { "colorScheme": "dark", "reducedMotion": "reduce", "media": "print" } }
```

`colorScheme` is `light`, `dark` or `no-preference`. `reducedMotion` is `reduce` or `no-preference`. `media` is `screen` or `print`. The step replaces the page's media emulation: omitted fields reset to the default (light, no-preference, screen). It settles. Result `{}`.

### download, downloadCount, popup, page {#download}

```json
{ "download": { "saveTo": "/tmp/downloads", "as": "file", "timeout": 10000 } }
{ "downloadCount": { "as": "n" } }
{ "popup": { "as": "report", "timeout": 5000 } }
{ "page": { "target": "report" } }
```

| Step | Result |
| --- | --- |
| `download` | Waits for the next download started during the run: `{url, filename, size?, text?, path?, error?}`. `text` is set for UTF-8 files of at most 64 KiB. `path` is set when the file was saved under its suggested name in `saveTo`. |
| `downloadCount` | The page's count of started downloads (an integer). Download records are not consumed. |
| `popup` | Waits for the next popup this page opened during the run: `{targetId, url}`. |
| `page` | The following steps drive another page of the context. `target` is a popup's `as` key, a target id, or `"main"` (the page the run started on). Result `{targetId, url}`. |

### matchScreenshot {#match-screenshot}

```json
{ "matchScreenshot": { "name": "home", "baseline": "<base64 PNG>", "sceneHash": "9f2c0a1b3d4e5f60", "threshold": 0.1, "fullPage": true } }
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `name` | string | required | Also the key under which this connection caches the baseline |
| `baseline` | base64 PNG | the baseline cached under `name` on this connection | Omitted with nothing cached: the render is returned (`recorded: true`) |
| `sceneHash` | hex string | | Display-list hash stored with the baseline. If it matches, the step passes without rasterizing (`hashHit`). |
| `threshold` | number 0-1 | `0` | Per-channel difference a pixel may have and still match |
| `maxDiffPixels` | integer | | Differing pixels allowed |
| `maxDiffPixelRatio` | number 0-1 | | Differing pixels allowed, as a fraction of the image |
| `clip` | `{x, y, width, height}` | | CSS px of the document. Width and height must be positive. |
| `fullPage` | boolean | `false` | Whole document |
| `locator` | locator | | This element's box |
| `update` | boolean | `false` | Ignore the baseline: render and return the image |
| `captureOnly` | boolean | `false` | Render and return the image without comparing |
| `as` | string | | Key in `reads` |
| `timeout` | integer | | Wait for the render |

Give at most one of `clip`, `fullPage` and `locator`.

#### Result {#match-screenshot-result}

`{name, pass, hashHit, recorded, diffPixels, sceneHash, width, height, ms, actual?, diff?}`. `sceneHash` is 16 hex digits. `ms` holds the timings of the stages. `actual` (base64 PNG) is included when the render is returned. On a mismatch the step fails, and the same object, with `actual` and `diff`, is the error's `data`.

## Step results {#step-results}

| Step | Result |
| --- | --- |
| `goto`, `loaded`, `reload`, `media` | `{}` |
| `act`, `route`, `mouse`, `viewport` (settling) | `{settleMs, settled?: false, pending?}` (`act`: `{}` with `settle: false`) |
| `idle` | `{settleMs, settled?: false, pending?}` |
| `url` | `{url}` |
| `check` | `{actual}` |
| `read`, `eval`, `downloadCount` | the value |
| `request` | a request log entry, or `{count}` |
| `mock` / `unmock` / `release` | `{mockId, mocks}` / `{removed}` / `{released}` |
| `held` | `{count, statuses, responses}` |
| `advanceTime` | `{advanced}` |
| `download` / `popup` / `page` | see [above](#download) |
| `matchScreenshot` | see [above](#match-screenshot-result) |

## Request log {#request-log}

From the first `LP.run` on, the page records its requests. `requests` in a run's result holds the entries made during that run, including requests still in flight.

```json
{
  "method": "POST",
  "url": "http://app.test/api/orders",
  "status": 201,
  "error": null,
  "ms": 12.4,
  "requestHeaders": { "content-type": "application/json", "authorization": "[REDACTED]" },
  "responseHeaders": { "content-type": "application/json" },
  "postData": "{\"sku\":\"A1\",\"qty\":2}"
}
```

| Field | Description |
| --- | --- |
| `status` | HTTP status, or `null` (failed or still in flight) |
| `error` | Network error text such as `net::ERR_CONNECTION_REFUSED`, or `null` |
| `ms` | Duration, or `null` while in flight |
| `requestHeaders`, `responseHeaders` | Header objects (`responseHeaders` is `null` without a response). Values of `authorization`, `proxy-authorization`, `cookie`, `cookie2`, `set-cookie`, `set-cookie2`, `x-api-key`, `x-auth-token`, `x-csrf-token` and `x-xsrf-token` are `[REDACTED]`. |
| `postData` | Request body: text, or base64 with `postDataBase64: true`. Bodies over 1 MiB are cut off and marked `postDataTruncated: true`. |

## LP.act, LP.expect, LP.query {#single-step}

### LP.act {#lp-act}

Params: one [`act`](#act) body. Runs it as a one-step run and replies with the step's result. `settle` defaults to `false`.

### LP.expect {#lp-expect}

Params: one [`check`](#check) body. Result: `{actual}`.

### LP.query {#lp-query}

Params: one [`read`](#read) body (`as` is ignored). It reads **at once, without waiting**: `{value}`. With no match it fails with `-32000 <locator> matched no element`, except for `count`.

Errors of these commands carry the bare reason, without the `step <i>` prefix.

## LP.idle {#lp-idle}

```json
{ "method": "LP.idle", "params": { "timeout": 2000 }, "sessionId": "…" }
```

Runs one [`idle`](#idle) step: it waits until the page is quiet, with no requests in flight, for up to `timeout` (default 5000). It never fails. Result `{settleMs}`, plus `settled: false` and `pending` if it timed out.

## LP.dialogs {#lp-dialogs}

```json
{ "method": "LP.dialogs", "params": { "block": true }, "sessionId": "…" }
```

With `block: true`, dialogs (`alert`, `confirm`, `prompt`) opened by this page's driver steps wait for `Page.handleJavaScriptDialog`, like other dialogs. By default they return what a dismissed dialog gives. Result `{}`.

## Raw CDP example {#example}

A complete client with nothing but a WebSocket (Bun or Node 22+), against `lightningpanda serve --port 9222`:

```ts
const version = await (await fetch('http://127.0.0.1:9222/json/version')).json();
const ws = new WebSocket(version.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));

let id = 0;
const pending = new Map<number, (m: any) => void>();
ws.onmessage = (e) => {
  const m = JSON.parse(String(e.data));
  if (m.id) pending.get(m.id)?.(m), pending.delete(m.id);
};
const send = (method: string, params = {}, sessionId?: string) =>
  new Promise<any>((resolve) => {
    const i = ++id;
    pending.set(i, resolve);
    ws.send(JSON.stringify({ id: i, method, params, ...(sessionId ? { sessionId } : {}) }));
  });

const { result: { browserContextId } } = await send('Target.createBrowserContext');
const { result: { targetId } } = await send('Target.createTarget', { url: 'about:blank', browserContextId });
const { result: { sessionId } } = await send('Target.attachToTarget', { targetId, flatten: true });

const reply = await send('LP.run', {
  timeout: 10_000,
  steps: [
    { mock: { url: '/api/items', json: [{ id: 1, name: 'Panda' }] } },
    { goto: { url: 'http://localhost:5173/' } },
    { act: { locator: { role: 'button', name: 'Load' }, action: 'click' } },
    { check: { locator: { css: 'li' }, assert: 'count', expected: 1 } },
    { read: { locator: { css: 'li' }, read: 'text', as: 'first' } },
    { request: { method: 'GET', path: '/api/items', status: 200 } },
  ],
}, sessionId);

if (reply.error) throw new Error(reply.error.message); // "step 3 (expect Count {...}): timed out after ..."
console.log(reply.result.reads.first, reply.result.ms);
ws.close();
```

For pages booted ahead (`Lightningpanda.acquirePreparedPage`), see [Pyro CDP domain](/reference/cdp-domain).
