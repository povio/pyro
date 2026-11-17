# Steps and checks

A run is a list of steps. Each step waits in the browser until it can complete, then the next one starts. This page lists every step the native builders produce, with its options and its result, plus the few wire-only steps that have no builder. Network steps (`check.request`, `mock`, `unmock`, `release`) are on [Requests and mocks](/e2e/network); `matchScreenshot` is on [Visual tests](/e2e/screenshots).

```ts
import {
  button, check, label, text, css, goto, route, reload, waitForUrl, loaded, idle,
  evaluate, viewport, media, popup, switchTo, download, downloadCount, mouse,
} from '@povio/pyro';
```

## Timeouts

Every step has its own timeout: **5,000 ms** unless you set one. A step waits only for its own condition, so the timeout is how long that condition may take, not how long the whole test may take.

```ts
await page.run(steps, { timeout: 10_000 });     // default for every step of this run
button('Export').click({ timeout: 20_000 });    // one action
check(text('Done')).visible({ timeout: 20_000 });
check.request('POST', '/api/export').status(200).timeout(20_000);
```

The browser caps any timeout at 600,000 ms. When a step's timeout passes, the run fails with a `RunError` that says why the step was still waiting (see [Debugging failures](/e2e/debugging)). Raising a timeout fixes only a condition that is slow, never one that cannot hold.

## Settling

After an action, the next step waits until the page is quiet: no task queued, no request the action started still in flight, no `setTimeout` due within 50 ms, no animation frame pending, no `scroll` event or ResizeObserver callback owed by the next rendering update, and no navigation short of `DOMContentLoaded`. Intervals, timers due later, animation loops and requests that were in flight before the action do not count. The step's result records `settleMs`.

Settling gives up after **1,000 ms** and carries on. The result then has `settled: false` and `pending`, which says what was still busy:

```json
{ "pending": { "requests": 1 }, "settleMs": 1002.07, "settled": false }
```

These steps settle: every locator action except `focus()` and `blur()`, `mouse(...)`, `route(...)`, `viewport(...)` and `media(...)`. Pass `{ settle: false }` to an action whose requests never finish (polling, streaming, a request a mock holds) or that you do not want to wait for. An action that does not settle, an `evaluate` and an `advanceTime` still give the page one event-loop turn before the next step, so work they queued with `setTimeout(fn, 0)` runs first. Newer than 0.0.33, an `evaluate` that left the page nothing to run (no task, timer, request, DOM change or observer work) is followed by the next step directly. See [Settling and auto-waiting](/guide/settling).

## Actions

Every action first resolves its locator to exactly one element ([strict mode](/e2e/locators#strict-mode)), then waits until the element is attached and visible (a box with non-zero width and height). `click`, `dblclick`, `fill`, `check` and `uncheck` also need it enabled; `press` and `type` do not (as in Playwright, they focus the element and send the keys, so an `aria-disabled` button still gets its keydown). Pointer actions scroll it into view and need it not covered by another element.

All actions take `{ settle?: boolean; timeout?: number }`.

| Step | What it does | Result |
|---|---|---|
| `loc.click(opts?)` | Moves the pointer to the element and clicks with a real press and release | `{ settleMs }` |
| `loc.dblclick(opts?)` | Two clicks at the same point, the second with click count 2 | `{ settleMs }` |
| `loc.hover(opts?)` | Moves the pointer to the element (tooltips, hover toolbars). Does not need an enabled element | `{ settleMs }` |
| `loc.fill(value, opts?)` | Focuses, selects the current content and replaces it with `value` (input, textarea or contenteditable element). An empty string clears it | `{ settleMs }` |
| `loc.type(text, opts?)` | Focuses, then one key press per character (Playwright's `pressSequentially`) | `{ settleMs }` |
| `loc.press(key, opts?)` | Focuses, then presses a key or a chord (see [Keys](#keys)) | `{ settleMs }` |
| `loc.check(opts?)` | Clicks a checkbox or radio button unless it is already checked | `{ settleMs }` |
| `loc.uncheck(opts?)` | Clicks a checkbox unless it is already unchecked | `{ settleMs }` |
| `loc.focus()` | Focuses the element. Does not settle | `{}` |
| `loc.blur()` | Moves focus away (`blur`, `focusout`). Does not settle | `{}` |
| `loc.upload(files, opts?)` | Sets the files of an `<input type=file>` and fires `input` and `change` | `{ files, settleMs }` |
| `loc.dropFiles(files, opts?)` | Drags external files onto a visible drop target: real `dragenter`, `dragover` and `drop` with `File` data | `{ files, settleMs }` |
| `loc.drag(target, opts?)` | Drags the element onto `target`: native drag and drop when the element is a drag source, else a pointer drag with the button held | `{ native, settleMs }` |
| `loc.wheel(deltaX, deltaY, opts?)` | Hovers the element, fires `wheel` there and scrolls the nearest scrollable container unless the event is cancelled | `{ settleMs }` |

`check()` and `uncheck()` on a native checkbox that is visually hidden (a custom-styled control) click its visible `<label>` instead. On an element that is not a checkbox or radio button they wait with `element is not a checkbox or radio button`.

`fill()` on an element that is not editable waits with `element is not editable`. On `<input type=time>` it sets the value directly and rejects a malformed time.

### Pointer options

`click`, `dblclick` and `hover` take these options as well:

| Option | Default | Meaning |
|---|---|---|
| `button` | `'left'` | `'left'`, `'right'` or `'middle'` (`click` and `dblclick`). A right click fires `contextmenu` after the press and `auxclick` after the release, never `click`, as Chromium does |
| `position` | center of the element's visible part | `{ x, y }` offset from the element's top-left corner |
| `force` | `false` | Skip the covered check and the enabled check; act at the point whatever is there |

The default point is the center of the part of the element that is visible inside the viewport and its scroll containers, as in Playwright, so an element wider than its container is clicked where it can be seen.

```ts
css('#row-3').click({ button: 'right' });
css('canvas').click({ position: { x: 10, y: 20 } });
button('Save').click({ force: true });
```

Modifier keys held during a pointer action (`Shift`, `Control`, `Alt`, `Meta`, `ControlOrMeta`) are a wire field, `modifiers`, with no builder option. See [Wire-only steps](#wire-only-steps).

### Files

`upload` and `dropFiles` take either inline files or a path:

```ts
label('Attachment').upload([
  { name: 'a.csv', text: 'x,y\n' },                        // text contents
  { name: 'b.png', base64: 'iVBORw0KGgo=', mimeType: 'image/png' },
]);
label('Attachment').upload('/fixtures/report.pdf');         // a path on the browser's machine
label('Attachment').upload('/fixtures/a.pdf\n/fixtures/b.pdf'); // several: one per line
```

A file takes `text` or `base64`, not both. Without `mimeType` the type comes from the file extension. A path is read by the browser process, so it must exist where the browser runs. Each file can be at most 64 MiB.

### Keys

`press` takes a key name or a chord joined with `+`:

| Kind | Names |
|---|---|
| Named keys | `Enter`, `Tab`, `Escape`, `Backspace`, `Delete`, `Space`, `ArrowLeft`, `ArrowUp`, `ArrowRight`, `ArrowDown`, `Home`, `End`, `PageUp`, `PageDown`, `F1` to `F12` |
| Modifiers | `Shift`, `Control`, `Alt`, `Meta`, `ControlOrMeta` |
| Characters | Any single character: `a`, `A`, `1`, `/` |

```ts
label('Search').press('Enter');
label('Keys').press('Shift+Tab');
label('Notes').press('ControlOrMeta+A');   // Meta+A on a macOS browser, Control+A on Linux
```

A chord sends the modifiers down, the key down and up, then the modifiers up. With `Shift`, a single letter is sent upper-case. With `Control`, `Meta` or `Alt` held the key inserts no text. `ControlOrMeta` resolves on the machine the browser runs on. An unknown name waits with `unknown key "..."`.

`type` sends each character as its own key press; a character it cannot send waits with `cannot type '...'`.

## Checks

`check(loc)` returns a builder. Each check waits until it holds, trying again whenever the page's DOM changes. Every check takes `{ timeout }`.

| Check | Holds when | Result |
|---|---|---|
| `.visible()` | The one match has a visible box | `{ actual: true }` |
| `.hidden()` | Nothing matches, or the one match has no visible box | `{ actual: false }` |
| `.text(expected)` | The element's text equals `expected` after whitespace normalization (case-sensitive) | `{ actual }` |
| `.containsText(expected)` | The normalized text contains `expected` (case-sensitive) | `{ actual }` |
| `.value(expected)` | The form control's value equals `expected` exactly | `{ actual }` |
| `.count(n)` | The locator matches exactly `n` elements (no strict mode) | `{ actual }` |
| `.checked(expected = true)` | A checkbox or radio button is (`true`) or is not (`false`) checked | `{ actual }` |
| `.css(property, expected)` | The computed style property equals `expected`: `.css('background-color', 'rgb(0, 0, 0)')` | `{ actual }` |
| `.focused(expected = true)` | The element is (or is not) the document's focused element | `{ actual }` |
| `.name(expected)` | The accessible name equals `expected` after whitespace normalization | `{ actual }` |
| `.enabled(expected = true)` | The element is (or is not) enabled | `{ actual }` |
| `.disabled()` | Same as `.enabled(false)` | `{ actual }` |

`check(loc).not` negates any check: `check(text('Saving…')).not.visible()`. All checks except `count` follow strict mode: several matches fail the step at once.

Computed colors read back in the `rgb(r, g, b)` form, so compare with that form.

## Reads

`loc.read(as, what = 'text', property?)` stores a value in `values[as]` and in the step's result. It waits until the locator matches one element and follows strict mode. `'count'` is the exception: it reads the number of matches at once, zero included, without waiting.

| `what` | Value |
|---|---|
| `'text'` | The element's text, whitespace-normalized |
| `'value'` | The form control's value |
| `'count'` | How many elements match |
| `'visible'` | `true` or `false` |
| `'box'` | `{ x, y, width, height }` in CSS px, or `null` when it has no visible box |
| `'css'` | A computed style property: `read('color', 'css', 'color')` (the property is required) |
| `'name'` | The accessible name |
| `'focused'` | Whether it is the focused element |
| `'enabled'` | Whether it is enabled |

```ts
const r = await page.run([text(/^Updated by /).read('updatedBy'), button('Save').read('box', 'box')]);
expect(r.values.updatedBy).toContain('Ada');
```

Use reads for values the test computes with; assert everything else with `check(...)`, which waits.

## Navigation and waiting

| Step | What it does | Options |
|---|---|---|
| `goto(url, waitUntil = 'load')` | Navigates the page and waits for `load` (or `'domcontentloaded'`) | Step timeout comes from the run |
| `route(url, opts?)` | Same-document navigation: `history.pushState` then a `popstate` event, as a client-side router sees a link. Settles on the requests it starts | `{ settle?, timeout? }` |
| `reload(opts?)` | Reloads the current document and waits for `load` | `{ timeout? }` |
| `waitForUrl(url, opts?)` | Waits until the page URL contains the string, or matches the `RegExp` | `{ timeout? }` |
| `loaded(opts?)` | Waits for the current document's `load` | `{ timeout? }` |
| `idle(opts?)` | Waits until the page is quiet, with no requests in flight. Never fails | `{ timeout? }` |

`route` needs a document: on a page still at `about:blank` with no navigation under way it fails with `the page is on about:blank: route needs a document`. A pooled page that is still booting gets a `loaded()` step in front of your steps automatically, and `route` waits for its navigation.

`idle` waits up to its timeout. When the timeout passes first, its result has `settled: false` and `pending` instead of failing the run.

`goto` fails with `the navigation failed before its document loaded` when the document cannot load, for example a refused connection.

## Evaluating JavaScript

```ts
evaluate(expression: string, as?: string, opts?: { timeout?: number })
```

Evaluates an expression in the page. Its value, converted to JSON, lands in `values[as]` when `as` is given, and in the step's result. A returned promise is awaited within the step's timeout. A rejection fails the run with `rejected: <message>`.

```ts
const r = await page.run([
  evaluate('document.title', 'title'),
  evaluate('fetch("/api/me").then((r) => r.json())', 'me'),
]);
```

It is an expression string, not a function: nothing from the test's scope is captured. Use it to read state the locators cannot reach, and as an escape hatch for setup. Prefer actions and checks for anything a user does or sees.

## Viewport and media

| Step | What it does |
|---|---|
| `viewport(width, height)` | Resizes the viewport mid-run. `innerWidth`/`innerHeight` and layout follow at once, `resize` and `matchMedia` `change` listeners fire, and the step settles like an action. Result: `{ width, height, settleMs }` |
| `media({ colorScheme, reducedMotion?, media? })` | Emulates media features. `colorScheme`: `'light'`, `'dark'`, `'no-preference'`; `reducedMotion`: `'reduce'`, `'no-preference'`; `media`: `'screen'`, `'print'`. Settles like an action. Result: `{ settleMs }` |

`media` replaces the whole emulation: a field you omit resets to its default (`light`, `no-preference`, `screen`). `@media` rules, `matchMedia(...).matches` and its `change` listeners follow.

```ts
await page.run([media({ colorScheme: 'dark' }), check(css('body')).css('background-color', 'rgb(0, 0, 0)')]);
```

For the starting viewport of a page, use the profile's `viewport` option (see [Profiles](/e2e/profiles)).

## Popups

| Step | What it does |
|---|---|
| `popup(as?, opts?)` | Waits for the next popup the page opens during the run (`window.open`, `target=_blank`). `values[as]` is `{ targetId, url }` |
| `switchTo(target)` | The following steps drive another page of the same browser context: the `as` name of an earlier `popup`, a target id, or `'main'` for the page the run started on |

```ts
await page.run([
  link('Open report').click(),
  popup('report'),
  switchTo('report'),
  loaded(),                               // a fresh popup may still be at about:blank
  check(heading('Report')).visible(),
  switchTo('main'),
  check(button('Save')).visible(),
]);
```

After switching, `check.request` and `download` see the other page's activity. The reply's `requests` stay those of the page the run started on.

## Downloads

| Step | What it does |
|---|---|
| `download({ as?, saveTo?, timeout? })` | Waits for the next download the page starts during the run. `values[as]` is `{ url, filename, size, text? }`; `text` is set for UTF-8 files up to 64 KiB. With `saveTo` (a directory), the file is written there under its suggested name and the value has `path` |
| `downloadCount(as)` | Stores how many downloads this page has started so far, without consuming any. The count persists across runs on the same page |

```ts
const r = await page.run([link('Export CSV').click(), download({ as: 'csv', saveTo: '/tmp/exports' })]);
expect(r.values.csv).toMatchObject({ filename: 'report.csv' });
```

To assert that an action starts no download, read `downloadCount` before and after it.

## Raw mouse

```ts
mouse(action, { x?, y?, button?, deltaX?, deltaY?, steps?, settle?, timeout? })
```

Playwright's `page.mouse` at viewport coordinates, not tied to an element:

| `action` | Notes |
|---|---|
| `'click'`, `'dblclick'` | Need `x` and `y`; `button` is `'left'` by default |
| `'move'` | Needs `x` and `y`; `steps` sends intermediate `mousemove` events (1 to 1,000) |
| `'down'`, `'up'` | At the current pointer position unless `x` and `y` are given; a button stays held across `move` steps until `up` |
| `'wheel'` | `deltaX`, `deltaY` at the current position (or `x`, `y`) |

`x` and `y` go together. Raw mouse steps skip the actionability checks and settle by default.

## Wire-only steps

The engine accepts a few steps and fields the builders do not produce. A run takes any object with a `toStep()` method, so you can send them directly. Field names are those of [Pyro.run wire format](/reference/pyro-run).

```ts
const raw = (step: Record<string, unknown>) => ({ toStep: () => step });

await page.run([
  button('Retry').click(),
  raw({ advanceTime: { ms: 3000 } }),                    // fire timers due in the next 3 s
  check(text('Retried')).visible(),
]);
```

| Wire step or field | What it does |
|---|---|
| `{ advanceTime: { ms } }` | Advances the page clock: `Date` jumps forward and timers due within `ms` fire. At most one day per step. Result `{ advanced }`. Use it for timeouts and debounces you would otherwise wait out; a mock's `delay` is a real network delay and is not affected |
| `act` field `modifiers: ['Shift', ...]` | Modifier keys held during a pointer action: `raw({ act: { locator: button('Mod').spec, action: 'click', modifiers: ['Shift'] } })` |
| `check` with `assert: 'attribute'`, `property`, `expected` | An attribute equals `expected` (a string), matches a `{ regex }`, or is absent (`expected: null`) |
| `check` with `assert: 'description'` | The accessible description (from `aria-describedby`, `aria-description` or `title`) matches `expected` |
| `eval` fields `call`, `locator`, `all`, `arg` | Call a function expression, optionally with the matched element (or all matches) and a JSON argument; this is what the Playwright layer's `evaluate(fn, arg)` uses |
| `{ held: { hold, count, status, as? } }` | Waits until `count` responses held under `hold` have these statuses; their bodies land in `values[as]` |
| `release` field `body` | Replaces the body of a held passthrough response when it is released |

These are stable parts of the wire format, but they have no TypeScript types in the native SDK.

## Step results at a glance

`r.results[i]` is the result of your step `i`. Actions give `{ settleMs }` (plus `settled: false` and `pending` when settling gave up), checks `{ actual }`, reads and `evaluate` their value, `goto`/`reload`/`loaded` `{}`, `idle` and `media` `{ settleMs }`, `waitForUrl` `{ url }`, `viewport` `{ width, height, settleMs }`, `downloadCount` the number, `request` checks the matched request (or `{ count }` with `.count(n)`), `mock` `{ mockId, mocks }`, `unmock` `{ removed }`, `release` `{ released }`. The steps that `pyro.page({ route, url })` added in front of yours are not included.

## Next

- [Requests and mocks](/e2e/network): `check.request`, `mock`, `hold` and `release`.
- [Profiles and page pools](/e2e/profiles): where the page a run starts on comes from.
