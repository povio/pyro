# SDK: locators and steps

Locators and steps are plain data. Builders such as `button('Save')`, `dialog().hasText('…').button('OK')` and `check(locator).visible()` only describe work. `page.run([...])` sends the whole list to the browser in one [`Pyro.run`](/reference/pyro-run) command, and the browser resolves locators, waits and acts in Rust. Everything on this page is exported from `@povio/pyro`.

```ts
import { button, check, css, dialog, goto, mock, textbox } from '@povio/pyro';

await page.run([
  mock('/api/items', { json: [] }),
  goto('http://app.test/items'),
  textbox('Search').fill('panda'),
  button('Add').click(),
  dialog().hasText('Saved').button('OK').click(),
  check(css('.item')).count(1),
]);
```

Guides: [Locators](/e2e/locators), [Steps and checks](/e2e/steps), [Requests and mocks](/e2e/network).

## How steps run {#semantics}

- **Order.** Steps run in order. The run replies when the last one holds or fails at the first that cannot finish.
- **Waiting.** A step that cannot finish yet (no match, not visible, check not holding, request not seen) is retried whenever the page's DOM changes or its document is replaced. Nothing polls. It fails once its timeout passes.
- **Timeouts.** Each step's timeout is its own `timeout` option, else the `timeout` of `page.run(steps, { timeout })`, else 5000 ms. The maximum is 600000 ms.
- **Strictness.** A step that needs one element (actions, `read` other than `count`, and most checks) fails at once with `strict mode violation: <locator> matched N elements: …` when several elements match. Narrow the locator, or pick one with `nth`, `first` or `last`.
- **Actionability.** An action needs exactly one match (unless you pick one with `nth`, `first` or `last`) that is visible, not covered at the point it acts on, and editable for `fill` and `type`. `click`, `dblclick`, `fill`, `check`, `uncheck`, `press` and `type` also need it enabled. `hover`, `focus`, `blur`, `wheel` and `drag` do not.
- **Settle.** After an action (`settle` defaults to `true`), and after `route`, `viewport`, `media` and `mouse`, the next step waits until the page is quiet. Quiet means no queued task, no timer due within 50 ms, no pending animation frame, no `scroll` event or ResizeObserver callback owed by the next rendering update, no navigation short of `DOMContentLoaded`, and no request the action started still in flight. That wait is capped at 1 s. The run then goes on, and the step's result records `settled: false` and what was `pending`. See [Settling and auto-waiting](/guide/settling).
- **Input timing.** Each input event of a pointer action is dispatched in its own task, with an event-loop turn between events, as Chromium delivers input.

## Text matching {#text-match}

```ts
type TextMatch = string | RegExp;
```

| Given | Matches |
| --- | --- |
| `'Save'` | Case-insensitive substring, after whitespace normalization |
| `'Save', { exact: true }` | Whole string, case-sensitive, after whitespace normalization |
| `/^Save( all)?$/i` | ECMAScript regular expression. Flags `i`, `m` and `s` are sent; `g`, `y` and `u` are dropped. Any other flag is rejected by the browser. `exact` is ignored |

`exact` applies to `text`, `name` (role locators), `label` and `placeholder`. For `text` it is Playwright's `getByText(…, { exact: true })`: the element's whole text, and only where no child element's text matches too.

## Top-level locators {#locators}

Each returns a [`Locator`](#locator).

| Builder | Matches |
| --- | --- |
| `css(selector: string)` | CSS selector |
| `testId(id: string, attribute?: string)` | `[data-testid="id"]`, or the given attribute |
| `text(t: TextMatch, opts?: { exact?: boolean })` | Elements by their text |
| `label(t: TextMatch, opts?)` | Form controls by label (`<label>`, `aria-label`, `aria-labelledby`) |
| `placeholder(t: TextMatch, opts?)` | Inputs by placeholder |
| `role(role: string, name?: TextMatch, opts?)` | ARIA role (implicit or explicit), optionally with an accessible name |
| `button(name?, opts?)` | `role('button', name, opts)` |
| `link(name?, opts?)` | `role('link', …)` |
| `dialog(name?, opts?)` | `role('dialog', …)` |
| `heading(name?, opts?)` | `role('heading', …)` |
| `textbox(name?, opts?)` | `role('textbox', …)` |
| `checkbox(name?, opts?)` | `role('checkbox', …)` |
| `combobox(name?, opts?)` | `role('combobox', …)` |
| `option(name?, opts?)` | `role('option', …)` |
| `row(name?, opts?)` | `role('row', …)` |
| `cell(name?, opts?)` | `role('cell', …)` |
| `tab(name?, opts?)` | `role('tab', …)` |
| `menuitem(name?, opts?)` | `role('menuitem', …)` |
| `inFrame(frame: Locator)` | A [`FrameScope`](#frame-scope): locators inside that `<iframe>`'s document |

`opts` is `{ exact?: boolean }` everywhere.

For locator kinds without a builder (`xpath`, `or`, `and`, `of`, role states such as `checked` or `level`, `ariaRef`), construct one from its [wire spec](/reference/pyro-run#locator): `new Locator({ role: 'checkbox', checked: true })`.

## Locator {#locator}

```ts
class Locator {
  constructor(readonly spec: LocatorSpec);
}
```

Every method returns a new locator or a step. The original is unchanged.

### Narrowing {#narrowing}

| Method | Effect |
| --- | --- |
| `visible(visible = true)` | Only visible (or, with `false`, hidden) matches |
| `hasText(t: TextMatch)` | Matches whose text contains (or matches) `t`, case-insensitive |
| `hasNotText(t: TextMatch)` | Matches whose text does not |
| `has(inner: Locator)` | Matches with at least one `inner` match inside |
| `hasNot(inner: Locator)` | Matches with none inside |
| `nth(index: number)` | One match by index. Negative indexes count from the end |
| `first()` | `nth(0)` |
| `last()` | `nth(-1)` |

Filters apply before `nth`. Calling a filter on a locator that already has `nth` throws `filter before first() / last() / nth()`. To filter after picking, use the wire `of` field.

### Locators inside this one {#inner}

`locator(css)`, `role(role, name?, opts?)`, `button(name?, opts?)`, `link(name?, opts?)`, `textbox(name?, opts?)`, `text(t, opts?)`, `label(t, opts?)`, `placeholder(t, opts?)` and `testId(id)` search inside each match of this locator (the wire `within`). If this locator is inside a frame, the frame stays on the outermost locator.

```ts
dialog('Delete item').button('Delete').click();
row('Invoice 42').locator('td.amount').read('amount');
```

### Frames {#frames}

| Method | Effect |
| --- | --- |
| `frame()` | A [`FrameScope`](#frame-scope) for the document of the `<iframe>` this locator matches (Playwright's `frameLocator`) |
| `inFrame(frame: Locator)` | This locator, searched in the document of the `<iframe>` that `frame` matches |

```ts
css('#editor').frame().button('Bold').click();
```

### Actions {#actions}

Each returns a step.

```ts
type ActOptions = { settle?: boolean; timeout?: number };
type PointerOptions = ActOptions & {
  button?: 'left' | 'right' | 'middle';
  position?: { x: number; y: number };
  force?: boolean;
};
type UploadFile = { name: string; text?: string; base64?: string; mimeType?: string };
```

| Method | Description |
| --- | --- |
| `click(opts?: PointerOptions)` | Pointer click. `button` defaults to `left`. `position` is relative to the element's top-left corner (default: the center of its visible part). `force` skips the hit test and the enabled check |
| `dblclick(opts?: PointerOptions)` | Two clicks; the second has click count 2 |
| `hover(opts?: PointerOptions)` | Moves the pointer to the element (tooltips, hover toolbars) |
| `fill(value: string, opts?)` | Focuses an editable input, textarea or contenteditable element, selects its content and inserts `value` as text input (`Input.insertText` semantics, so `input` events fire). An empty `value` clears it |
| `type(text: string, opts?)` | Focuses the element, then presses one key per character (Playwright's `pressSequentially`) |
| `press(key: string, opts?)` | Focuses the element, then presses a key (`Enter`, `Tab`) or a chord (`Shift+Tab`, `Control+a`) |
| `check(opts?)` | Clicks a checkbox or radio unless it is already checked. A visually hidden native control is clicked through its visible label |
| `uncheck(opts?)` | Clicks a checkbox unless it is already unchecked |
| `focus()` | Focuses the element. Does not settle |
| `blur()` | Removes focus. Does not settle |
| `upload(files: UploadFile[] \| string, opts?)` | Sets the files of an `<input type=file>`, then fires `input` and `change`. Pass inline files (`text` or `base64`; `mimeType` defaults from the extension), or a path string (several paths: one per line) on the machine running the browser. 64 MiB per file at most |
| `dropFiles(files: UploadFile[] \| string, opts?)` | Drops files on the element (drag-and-drop file events) |
| `drag(to: Locator, opts?)` | Drags the element onto `to`: native drag and drop when the element is a drag source, else pointer moves with the button held |
| `wheel(deltaX: number, deltaY: number, opts?)` | Moves the pointer to the element, then turns the wheel there (scrolls the nearest scrollable container) |
| `matchScreenshot(name: string, opts?: ScreenshotOptions)` | Compares this element's box with a baseline; see [`matchScreenshot`](#matchscreenshot) |

The step result of an action is `{}` or `{ settleMs, settled?, pending? }`. Pointer actions also accept `modifiers` on the wire (`Shift`, `Control`, `Alt`, `Meta`, `ControlOrMeta`); see [act](/reference/pyro-run#act).

### read() {#read}

```ts
read(as: string, what: ReadWhat = 'text', property?: string): StepLike
type ReadWhat = 'text' | 'value' | 'count' | 'visible' | 'box' | 'css' | 'name' | 'focused' | 'enabled';
```

Reads a value into `RunResult.values[as]`. Waits for exactly one match, except `count`.

| `what` | Value |
| --- | --- |
| `text` | Text content, whitespace-normalized |
| `value` | Form control value |
| `count` | Number of matches (`0` included; never waits) |
| `visible` | `true` / `false` |
| `box` | `{ x, y, width, height }` in CSS px, or `null` when not visible |
| `css` | Computed value of `property` (required: `read('c', 'css', 'color')`) |
| `name` | Accessible name |
| `focused` | Whether it is the document's focused element |
| `enabled` | Whether it is not disabled |

## FrameScope {#frame-scope}

Locators inside an `<iframe>`'s document, from `locator.frame()` or `inFrame(locator)`. Methods: `locator(css)`, `role(role, name?, opts?)`, `button(…)`, `link(…)`, `textbox(…)`, `text(t, opts?)`, `label(t, opts?)`, `placeholder(t, opts?)`, `testId(id)`, and `frame(inner: Locator)` for an `<iframe>` nested inside this one.

```ts
inFrame(css('iframe#pay')).frame(css('iframe#card')).textbox('Card number').fill('4242 4242 4242 4242');
```

## check {#check}

`check(locator)` returns assertions. Each one waits in the browser until it holds, and fails at its timeout.

```ts
check(button('Save')).visible();
check(css('.row')).count(3, { timeout: 10_000 });
check(dialog()).not.visible();
```

| Assertion | Holds when | Result |
| --- | --- | --- |
| `.visible(opts?)` | One match, and it is visible | `{ actual: true }` |
| `.hidden(opts?)` | No match, or the one match is not visible | `{ actual: false }` |
| `.text(expected: string, opts?)` | Text equals `expected` (both whitespace-normalized) | `{ actual: "<text>" }` |
| `.containsText(expected: string, opts?)` | Text contains `expected` (normalized) | `{ actual }` |
| `.value(expected: string, opts?)` | Form control value equals `expected` | `{ actual }` |
| `.count(expected: number, opts?)` | Exactly `expected` matches | `{ actual }` |
| `.checked(expected = true, opts?)` | Checked state of a checkbox or radio equals `expected` (the wire also takes `"mixed"`) | `{ actual }` |
| `.css(property: string, expected: string, opts?)` | Computed `property` equals `expected` (for example `'rgb(1, 2, 3)'`) | `{ actual }` |
| `.focused(expected = true, opts?)` | Is (or is not) the focused element | `{ actual }` |
| `.name(expected: string, opts?)` | Accessible name equals `expected` (normalized) | `{ actual }` |
| `.enabled(expected = true, opts?)` | Is (or is not) enabled | `{ actual }` |
| `.disabled(opts?)` | `enabled(false)` | `{ actual }` |
| `.not` | Negates the next assertion: `check(x).not.text('…')` | |

`opts` is `{ timeout?: number }`. The wire also has `attribute` and `description` assertions with no builder; see [check](/reference/pyro-run#check).

### check.request() {#check-request}

```ts
check.request(method: string, path: string | RegExp): RequestCheck
```

Checks the run's request log. The method compares case-insensitively. A string `path` must equal the URL's path exactly (no query). A RegExp is tested against the full URL. The check waits until a matching request has finished, meaning it has a response or failed. Chain to narrow it:

| Method | Description |
| --- | --- |
| `.status(status: number \| number[])` | One status, or any of several (`[200, 304]`) |
| `.count(n: number)` | Exactly `n` matching requests (default: at least one) |
| `.body(t: TextMatch)` | Request body (as sent, UTF-8) contains (or matches) `t` |
| `.bodyKeys(keys: string[])` | The body is a JSON object with exactly these top-level keys |
| `.headers(h: Record<string, TextMatch>)` | Request headers (names case-insensitive) whose values contain (or match) these |
| `.state('started' \| 'finished')` | `finished` (default) counts requests with a response or an error. `started` also counts requests still in flight |
| `.scope('batch' \| 'context')` | `batch` (default): this run only. `context`: also earlier runs of this page (never sibling pages) |
| `.timeout(ms: number)` | Step timeout |

```ts
check.request('POST', '/api/orders').status(201).bodyKeys(['sku', 'qty']);
check.request('GET', /\/api\/items\?page=2/).count(1);
```

Result: the matched request (see [request log](/reference/pyro-run#request-log)), or `{ count }` with `.count()`. The wire also takes `bodyJson` (the body parsed as JSON equals a value).

## Navigation and scripts {#navigation}

### goto() {#goto}

```ts
goto(url: string, waitUntil: 'load' | 'domcontentloaded' = 'load'): StepLike
```

Full navigation, waiting for `load` (or `DOMContentLoaded`). Fails with `the navigation failed before its document loaded` if the navigation fails first. Result: `{}`.

### route() {#route}

```ts
route(url: string, opts?: ActOptions): StepLike
```

Same-document navigation: `history.pushState` plus a `popstate` event, as a client-side router sees a link click. Settles on the requests it starts unless `settle: false`.

### reload() {#reload}

```ts
reload(opts?: { timeout?: number }): StepLike
```

Reloads the current document and waits for `load`.

### loaded() {#loaded}

```ts
loaded(opts?: { timeout?: number }): StepLike
```

Waits for the current document's `load`. On a pooled page taken while it is still booting, it waits for the profile URL's navigation to commit and load, never for the `about:blank` before it. `pyro.page()` adds this step when needed.

### waitForUrl() {#waitforurl}

```ts
waitForUrl(url: TextMatch, opts?: { timeout?: number }): StepLike
```

Waits until the page URL contains (or matches) `url`, like Playwright's `waitForURL`. Result: `{ url }`.

### idle() {#idle}

```ts
idle(opts?: { timeout?: number }): StepLike
```

Waits until the page is quiet: no queued task, no request in flight, no timer due within 50 ms, no pending animation frame and no unfinished navigation. It never fails. When `timeout` passes first, its result has `settled: false` and `pending` (counts of `tasks`, `requests`, `timers`, `frames`, `observers`, `scrolls`, `navigations`; `scrolls` is newer than 0.0.33).

### evaluate() {#evaluate}

```ts
evaluate(expression: string, as?: string, opts?: { timeout?: number }): StepLike
```

Evaluates an expression in the page's main world. Its JSON value lands in `values[as]`. A returned promise is awaited within `timeout`, and a rejection fails the step (`rejected: <message>`).

The wire `eval` step can also call a function with an element, all matches, or an argument (`call`, `locator`, `all`, `arg`); see [eval](/reference/pyro-run#eval).

## Network mocks {#mocks}

### mock() {#mock}

```ts
mock(url: TextMatch, opts?: MockOptions): StepLike
```

Answers requests of this page's browser context whose URL contains `url` (or matches the RegExp), like Playwright's `page.route` with `fulfill`. Mocks stay in place for the rest of the context's life, across runs, until `unmock`.

| Option | Type | Description |
| --- | --- | --- |
| `method` | `string` | Only this method |
| `status` | `number` | Response status (default `200`) |
| `json` | `unknown` | Serialized as the body. Content type `application/json` |
| `body` | `string` | Body text. Content type `text/plain; charset=utf-8`. Not together with `json` |
| `contentType` | `string` | Overrides the content type |
| `headers` | `Record<string, string>` | Response headers |
| `delay` | `number` | Milliseconds before the response (loading states) |
| `abort` | `string` | Fail the request with a `Network.ErrorReason` such as `Failed` or `ConnectionRefused`. No response fields allowed |
| `times` | `number` | Answer only the first `n` matching requests (at least 1) |
| `passthrough` | `boolean` | Let matching requests reach the network unchanged (an exception to a broader mock). Takes only `patch`, `delay` and `hold` |
| `hold` | `string` | Park matching requests under this name until a [`release(name)`](#release) step (with `passthrough`, park the response) |
| `patch` | `{ status?, headers?, json?, body? }` | Let the request reach the server, then rewrite the given fields of its response. `json` is a JSON merge patch (RFC 7396) when the real body is a JSON object, else the new body. Implies `passthrough` |

Result: `{ mockId, mocks }`.

```ts
mock('/api/items', { json: [{ id: 1 }] });
mock(/\/api\/slow/, { delay: 2000, json: {} });
mock('/api/save', { method: 'POST', hold: 'save', json: { ok: true } }); // answered on release('save')
mock('/api/profile', { patch: { json: { plan: 'pro' } } });
```

### unmock() {#unmock}

```ts
unmock(url?: TextMatch): StepLike
```

Removes the mocks whose pattern equals `url`, or all of them when `url` is omitted. Result: `{ removed }`.

### release() {#release}

```ts
release(hold: string): StepLike
```

Lets the requests (or responses) held by `mock(url, { hold })` go on. Waits until at least one is held. Result: `{ released: n }`. The wire step also takes `body`, a replacement response body.

## Emulation {#emulation}

| Builder | Description |
| --- | --- |
| `viewport(width: number, height: number)` | Resizes the viewport mid-test (CSS px). Layout, media queries and `resize` listeners see the new size. Settles. Result `{ width, height }` |
| `media(opts: MediaOptions)` | Emulates media features. Replaces the page's media emulation: omitted fields reset to the default (`light`, `no-preference`, `screen`). Settles |

```ts
type MediaOptions = {
  colorScheme: 'light' | 'dark' | 'no-preference';
  reducedMotion?: 'reduce' | 'no-preference';
  media?: string; // 'screen' | 'print'
};
```

## Downloads, popups and the mouse {#downloads-popups-mouse}

### download() {#download}

```ts
download(opts?: { saveTo?: string; as?: string; timeout?: number }): StepLike
```

Waits for the next download the page starts during the run. `values[as]` gets `{ url, filename, size, text?, path?, error? }`. `text` is included when the file is UTF-8 and at most 64 KiB. With `saveTo`, a directory on the browser's machine, the file is saved there under its suggested name and `path` is set.

### downloadCount() {#downloadcount}

```ts
downloadCount(as: string): StepLike
```

Reads the page's count of started downloads into `values[as]` without consuming download records.

### popup() {#popup}

```ts
popup(as?: string, opts?: { timeout?: number }): StepLike
```

Waits for the next popup the page opens during the run. `values[as]` gets `{ targetId, url }`.

### switchTo() {#switchto}

```ts
switchTo(target: string): StepLike
```

The following steps drive another page of the context. `target` is the `as` name of an earlier `popup(as)`, a target id, or `'main'` for the page the run started on. After switching to a fresh popup, add `loaded()` or `waitForUrl()`.

### mouse() {#mouse}

```ts
mouse(action: MouseAction, opts?: MouseOptions): StepLike
wheel(loc: Locator, deltaX: number, deltaY: number, opts?: ActOptions): StepLike // = loc.wheel(...)
```

A raw mouse event at viewport coordinates, not tied to an element, with no actionability checks.

```ts
type MouseAction = 'click' | 'dblclick' | 'move' | 'down' | 'up' | 'wheel';
type MouseOptions = {
  x?: number;
  y?: number;
  button?: 'left' | 'right' | 'middle';
  deltaX?: number;
  deltaY?: number;
  steps?: number;
  settle?: boolean;
  timeout?: number;
};
```

`click`, `dblclick` and `move` need `x` and `y`. `down`, `up` and `wheel` act at the current pointer position unless both are given; one coordinate alone is an error. `steps` (1-1000, default 1) is the number of intermediate `mousemove` events of a `move`.

```ts
await page.run([
  link('Open report').click(),
  popup('report'),
  switchTo('report'),
  loaded(),
  download({ as: 'csv', saveTo: './downloads' }),
  switchTo('main'),
]);
```

## Screenshots {#screenshots}

### matchScreenshot() {#matchscreenshot}

```ts
matchScreenshot(name: string, opts?: ScreenshotOptions): ScreenshotStep
locator.matchScreenshot(name: string, opts?: ScreenshotOptions): ScreenshotStep
```

Renders the viewport (or `clip`, `fullPage`, or the locator's element box) in the browser and compares it there with the baseline `<dir>/<name>.png`. `page.run` reads the baseline from disk and sends it once per connection, together with the display-list hash from `<name>.json`. A matching hash skips the raster entirely. It writes the baseline when it is missing or in [update mode](/reference/sdk#update-mode). On a mismatch it writes `<name>.actual.png` and `<name>.diff.png` and throws [`ScreenshotMismatchError`](/reference/sdk#screenshot-mismatch-error).

```ts
type ScreenshotOptions = {
  threshold?: number;
  maxDiffPixels?: number;
  maxDiffPixelRatio?: number;
  clip?: { x: number; y: number; width: number; height: number };
  fullPage?: boolean;
  dir?: string;
  timeout?: number;
};
```

| Option | Default | Description |
| --- | --- | --- |
| `threshold` | `0` | 0-1: how far a pixel's channels may differ and still match |
| `maxDiffPixels` | `0` | Differing pixels allowed |
| `maxDiffPixelRatio` | | Differing pixels allowed as a fraction of the image (0-1) |
| `clip` | | A rectangle of the document in CSS px (positive size) |
| `fullPage` | `false` | The whole document, not only the viewport |
| `dir` | `__screenshots__` next to the test file | Where baselines live (resolved on the client) |
| `timeout` | step timeout | Wait for the render |

Only one of `clip`, `fullPage` and a locator may be given. Names may use letters, digits, `.`, `-`, `_`, `@` and `/` (subdirectories), and must not contain `..`. The result appears in `RunResult.screenshots[name]` ([`ScreenshotResult`](/reference/sdk#screenshot-result)). Guide: [Visual tests](/e2e/screenshots).

### ScreenshotStep {#screenshot-step}

The class `matchScreenshot` returns. It has `name`, `opts` and `testDir` (the directory of the test file that built it, found from the call stack), and `wire` (fields `page.run` adds: `baseline`, `sceneHash`, `update`).

## Types {#types}

```ts
interface StepLike { toStep(): Step }
type Step = Record<string, unknown>; // one wire step, e.g. { act: {...} }
type LocatorSpec = { /* see the wire format */ };
```

Any object with `toStep()` is a step. Use this for wire steps that have no builder, such as `held` and `advanceTime`:

```ts
const advance = (ms: number): StepLike => ({ toStep: () => ({ advanceTime: { ms } }) });
await page.run([button('Save').click(), advance(30_000), check(text('Saved')).visible()]);
```

The full wire format of every step and of `LocatorSpec` is in [Pyro.run wire format](/reference/pyro-run).
