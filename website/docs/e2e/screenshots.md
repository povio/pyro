# Visual tests

A `matchScreenshot` step renders the page in the browser and compares it there with a baseline PNG, in the same run as the steps that led to the state. Rendering runs on the CPU and is deterministic, so baselines compare exactly by default. When the page's display list is unchanged since the baseline was recorded, the step passes without rasterizing at all.

The browsers bundled in `@povio/lightning-panda` are built with rendering enabled. A browser you build yourself needs the `render` feature (see [Rendering: screenshots and PDF](/guide/rendering)).

## The three forms

```ts
import { css, label, matchScreenshot, screenshotMatchers } from '@povio/lightning-panda';

// 1. Steps inside a run
await page.run([
  label('New todo').fill('Buy milk'),
  matchScreenshot('home-filled'),                        // the viewport
  css('form').matchScreenshot('todo-form'),              // one element's box
  matchScreenshot('home-full', { fullPage: true }),      // the whole document
]);

// 2. One step on its own
const result = await page.matchScreenshot('dashboard', { maxDiffPixels: 20 });

// 3. A bun:test matcher
expect.extend(screenshotMatchers);
await expect(page).toMatchScreenshot('home');
```

`page.matchScreenshot(name, opts)` is one run with one step; it returns the step's result. `toMatchScreenshot(name, opts)` wraps it: it passes when the screenshot matches and fails with the mismatch message otherwise.

Names may use letters, digits, `.`, `-`, `_`, `@` and `/` (for subdirectories), and must not contain `..`. Anything else throws when the step is built.

## Options

| Option | Default | Meaning |
|---|---|---|
| `threshold` | `0` | 0 to 1: how far a pixel's channels may differ and still count as matching |
| `maxDiffPixels` | `0` | Differing pixels allowed |
| `maxDiffPixelRatio` | | Differing pixels allowed, as a fraction (0 to 1) of the image |
| `clip` | | `{ x, y, width, height }`: a rectangle of the document in CSS px |
| `fullPage` | `false` | The whole document, not only the viewport |
| `dir` | `__screenshots__` beside the test file | Where the baselines live |
| `timeout` | step default | How long the render may take |

`clip`, `fullPage` and an element (`loc.matchScreenshot`) exclude each other. The test file's directory is found from the call stack where the step is built.

## Baselines on disk

For `matchScreenshot('home')` in `e2e/todos.test.ts`:

| File | Contents |
|---|---|
| `e2e/__screenshots__/home.png` | The baseline image. Commit it |
| `e2e/__screenshots__/home.json` | `{ "sceneHash", "width", "height" }`: the display-list hash of the baseline's render. Commit it with the PNG |
| `e2e/__screenshots__/home.actual.png` | Written on a mismatch: what the page looks like now |
| `e2e/__screenshots__/home.diff.png` | Written on a mismatch: differing pixels in red |

The `actual` and `diff` files are removed again when the step next passes. Add `*.actual.png` and `*.diff.png` to `.gitignore`.

The SDK sends a baseline to the browser once per connection and remembers its hash, so repeated comparisons against the same baseline do not resend the PNG.

### The display-list hash

The browser builds the page's display list before it rasterizes. When its hash equals the `sceneHash` stored beside the baseline, the step passes at once with `hashHit: true` and no pixels are drawn or compared. A changed display list is rasterized and compared pixel by pixel, so a change that does not alter any pixel still passes.

## First run and updating

A step with no baseline renders the page, writes the PNG and its JSON, and **fails** the test:

```text
screenshot 'home': the baseline did not exist; wrote /path/to/e2e/__screenshots__/home.png
```

The run reports the first missing baseline it meets and stops writing there, so a run with several new screenshots writes only the first one. Write them all at once in update mode, which records every baseline the run reaches instead of comparing:

| How | Where |
|---|---|
| `LP_UPDATE_SNAPSHOTS=1` (also `true`, `all`, `changed`) | Any runner |
| `bun test -u` or `bun test --update-snapshots` | `bun test` |
| `lightningpanda e2e -u` | The suite runner |

```sh
LP_UPDATE_SNAPSHOTS=1 bun test e2e/visual.test.ts
```

Update mode rewrites every baseline the run reaches, matching or not. Run it for the tests you mean to update, look at the changed PNGs in your diff tool, then commit.

## When a screenshot does not match

A mismatch throws a `ScreenshotMismatchError` (a subclass of `Error`, exported from the package):

```text
step 1 (matchScreenshot /path/to/__screenshots__/home.png): matchScreenshot '/path/to/__screenshots__/home.png': 474981 pixels differ (800x600); the actual image and the diff are in error.data
  actual: /path/to/__screenshots__/home.actual.png
  diff:   /path/to/__screenshots__/home.diff.png
```

```ts
import { ScreenshotMismatchError } from '@povio/lightning-panda';

try {
  await page.run([matchScreenshot('home')]);
} catch (e) {
  if (e instanceof ScreenshotMismatchError) console.log(e.result.diffPixels, e.result.files);
  throw e;
}
```

`e.result` is a `ScreenshotResult`:

| Field | Meaning |
|---|---|
| `name` | The step's name |
| `pass` | Whether it matched |
| `hashHit` | Passed on the display-list hash, without rasterizing |
| `recorded` | No baseline was compared: the render was written as the baseline |
| `diffPixels` | Pixels that differ beyond `threshold` |
| `sceneHash`, `width`, `height` | Of the new render |
| `ms` | Browser-side timings of the render and compare |
| `files` | `{ baseline, actual, diff }` paths written |

A successful run returns the same result per step in `r.screenshots[name]`.

Look at the diff before updating a baseline. Use `threshold` or `maxDiffPixels` only for noise you understand, such as antialiasing on one edge.

## What is captured

Captured:

- the live DOM after scripts ran, and every stylesheet, including ones added or edited by script
- the state of inputs, textareas and selects
- the window's scroll position, the viewport and the device scale factor
- images and fonts loaded through the page's network (mocks apply)

Not captured:

- canvas and video pixels
- iframe contents
- scroll offsets of elements other than the window
- `:hover` and `:focus` styles, the text caret
- running animations

Pixels come from Pyro's own renderer (Blitz, rasterized on the CPU). They never match Chromium screenshots pixel for pixel: record baselines with Pyro.

## Baselines per operating system

The renderer uses the system's fonts for any text whose fonts the page does not load itself. macOS and Linux have different fonts, so their renders differ. Keep one set of baselines per OS, or load every font the page uses as a web font. A per-OS directory:

```ts
import { join } from 'node:path';

const dir = join(import.meta.dir, '__screenshots__', process.platform);
await page.run([matchScreenshot('home', { dir })]);
```

Record the Linux baselines on the same image your CI uses (for example `ubuntu-24.04`); see [CI recipes](/e2e/ci#screenshot-baselines).

Text that should wrap as Chrome wraps it between a hyphen and a digit needs `args: ['--hyphen-digit-breaks', 'on']` on the server. It is off by default.

## Playwright's toHaveScreenshot

The Playwright layer implements `expect(page).toHaveScreenshot()` with Playwright's own paths (`<test file>-snapshots/<name>-<platform>.png`). See [Screenshots and snapshots](/playwright/snapshots).
