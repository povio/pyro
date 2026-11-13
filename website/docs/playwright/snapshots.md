# Screenshots and snapshots

`toHaveScreenshot`, `toMatchSnapshot` and `page.screenshot()` work with Playwright's API and file paths. The pixels are Pyro's: pages are rendered by its own CPU renderer, not by Chromium. Baselines from a Playwright suite will not match, so re-baseline once when you migrate.

## toHaveScreenshot

```ts
test('home page', async ({ page }) => {
  await page.goto('/');
  await expect(page).toHaveScreenshot('home.png');
  await expect(page).toHaveScreenshot('home-full.png', { fullPage: true });
  await expect(page.getByRole('navigation')).toHaveScreenshot('nav.png', { maxDiffPixels: 20 });
});
```

The page (or the locator's element) is rendered and compared inside the browser. The test process sends the baseline once per connection; later comparisons against the same baseline do not resend it.

### Options

| Option | Default | Meaning |
| --- | --- | --- |
| `threshold` | `0` | How far each color channel of a pixel may differ (0 to 1) and still match. 0 means exact. |
| `maxDiffPixels` | `0` | Differing pixels allowed. |
| `maxDiffPixelRatio` | none | Differing pixels allowed, as a fraction of the image. With `maxDiffPixels` too, the stricter limit applies. |
| `clip` | none | `{ x, y, width, height }` in CSS pixels. |
| `fullPage` | `false` | The whole document, not only the viewport. |
| `omitBackground` | `false` | The page's default white background is left transparent for this capture. |
| `timeout` | `expect` timeout | |

Defaults for `threshold`, `maxDiffPixels` and `maxDiffPixelRatio` can be set in the config:

```ts
export default defineConfig({
  expect: { toHaveScreenshot: { maxDiffPixelRatio: 0.01 } },
});
```

`threshold` here is a per-channel tolerance. Playwright's `threshold` (default 0.2) is a perceptual color distance. Both are 0 to 1, but the same number does not mean the same thing.

::: warning Other options are errors
`animations`, `caret`, `mask`, `maskColor`, `scale` and `stylePath` are in the types but the browser rejects them:

```text
Pyro.run: step 0: unknown field `animations`, expected one of `name`, `baseline`, `sceneHash`, `threshold`, ...
```

Remove them when you migrate. Pyro renders a still frame, so there is no animation or caret to disable.
:::

`expect(...).not.toHaveScreenshot()` is not supported.

The image is rendered at the context's `deviceScaleFactor`: with `deviceScaleFactor: 2`, a 1280x720 viewport gives a 2560x1440 baseline. Playwright's default for `toHaveScreenshot` is `scale: 'css'`, one image pixel per CSS pixel, and there is no `scale` option here. A project that spreads a HiDPI device descriptor therefore gets larger baselines than it had in Playwright.

### Names and paths

A name must end in `.png` (Playwright requires this too); otherwise the assertion throws. A list of strings (`['nav', 'open.png']`) makes sub-directories. Without a name, the name is the test's title path plus a counter, as in Playwright.

The baseline path follows Playwright's default template:

```text
{snapshotDir}/{testFileDir}/{testFileName}-snapshots/{arg}{-projectName}{-snapshotSuffix}{ext}
```

So `expect(page).toHaveScreenshot('home.png')` in `e2e/home.spec.ts`, project `chromium`, on Linux, uses `e2e/home.spec.ts-snapshots/home-chromium-linux.png`. Without a project name the `-chromium` part is left out. The suffix is the platform (`darwin`, `linux`); set `testInfo.snapshotSuffix = ''` to drop it.

`snapshotPathTemplate` (config or project) and `expect.toHaveScreenshot.pathTemplate` change the template, with Playwright's tokens: `{testDir}`, `{snapshotDir}`, `{testFileDir}`, `{testFileName}`, `{testFilePath}`, `{testName}`, `{arg}`, `{ext}`, `{platform}`, `{projectName}`, `{snapshotSuffix}`. Paths resolve against the config file's directory.

Beside each baseline `home-chromium-linux.png` the runner writes `home-chromium-linux.json`. It holds the hash of the render's display list. When a later render has the same hash, the assertion passes without rasterizing or comparing pixels. Commit the `.json` files with the `.png` files. Without one, the comparison runs on pixels.

### Failures

On a mismatch the assertion fails with the number of differing pixels and writes three images into the test's output directory (`test-results/<test>/` by default):

```text
test-results/e2e-home.spec.ts-home-page/home-actual.png
test-results/e2e-home.spec.ts-home-page/home-expected.png
test-results/e2e-home.spec.ts-home-page/home-diff.png
```

A missing baseline is written, and the test fails at its end with `A snapshot doesn't exist at <path>, writing actual.` That test is not retried, as in Playwright. The next run compares against the new file.

## Update modes

`-u` / `--update-snapshots [mode]` on `pyro run`, the config's `updateSnapshots`, or `$PYRO_UPDATE_SNAPSHOTS` choose what is written:

| Mode | Missing baseline | Mismatch |
| --- | --- | --- |
| `missing` (default without `-u`) | Written; the test fails | Fails |
| `changed` (default with `-u`) | Written; passes | Rewritten; passes |
| `all` | Written; passes | Rewritten; passes. Matching files are rewritten too. |
| `none` | Fails: `A snapshot doesn't exist at <path>.` | Fails |

```sh
bunx pyro run -u            # changed
bunx pyro run -u all        # rewrite every baseline
bunx pyro run -u none       # CI: never write
```

With plain `bun test`, `-u` means `changed`. These modes also apply to `toMatchSnapshot` and `toMatchAriaSnapshot` files.

## Re-baselining on migration

Chromium and Pyro draw text, anti-aliasing, form controls and some layout details differently. The renderer is described in [Rendering: screenshots and PDF](/guide/rendering). Expect every Chromium baseline to fail.

1. Remove the old baseline images, or keep them on a branch for reference.
2. Run `bunx pyro run -u all` on the platform where CI runs. Fonts come from the operating system, and the file name carries the platform, so macOS and Linux need their own baselines.
3. Look at the new images. A rendering problem in Pyro would otherwise become the new truth.
4. Commit the `.png` and `.json` files.
5. In CI, run with `-u none` or without `-u`, so a missing baseline fails.

While Chromium and Pyro both run the suite, keep their baselines apart: give Pyro's config a `snapshotPathTemplate` such as `'{testDir}/{testFilePath}-snapshots/{arg}-pyro-{platform}{ext}'`. [Run the same suite on Chromium and Pyro](/recipes/playwright-side-by-side) uses this.

If a test's screenshot depends on text wrapping at hyphens next to digits (IDs, dates), `use: { pyro: { args: ['--hyphen-digit-breaks', 'on'] } }` wraps like Chrome does. It is off by default.

## toMatchSnapshot

```ts
test('export', async ({ page }) => {
  await page.goto('/report');
  const csv = await page.getByTestId('csv').textContent();
  expect(csv).toMatchSnapshot('report.csv');
  expect(await page.screenshot()).toMatchSnapshot('report.png', { maxDiffPixels: 10 });
});
```

`expect(value).toMatchSnapshot(name?, options?)` compares a string or a Buffer with a file at Playwright's snapshot path. It is synchronous, as in Playwright.

- PNG files compare by pixel, with `threshold` (default 0.2, Playwright's perceptual distance), `maxDiffPixels` and `maxDiffPixelRatio`. Other files compare byte for byte; a text mismatch prints a line diff.
- Without a name, the extension comes from the content: `.txt` for strings, `.png` or `.jpg` from the bytes, else `.dat`.
- On a mismatch, `-actual` and `-expected` files are written to the test's output directory.
- Defaults come from `expect.toMatchSnapshot` in the config.
- `.not` works against an existing file; a missing file with `.not` fails.

`testInfo.snapshotPath(name, { kind: 'snapshot' | 'screenshot' | 'aria' })` returns the path for a name, as in Playwright.

## page.screenshot and locator.screenshot

```ts
const png = await page.screenshot({ fullPage: true });
await page.locator('#chart').screenshot({ path: 'out/chart.png' });
```

`page.screenshot()` takes `path`, `fullPage`, `clip`, `type` (`png` or `jpeg`, also from the path's extension), `quality` and `omitBackground` (PNG only). Other options (`mask`, `animations`, `scale`, `caret`, `style`) are ignored. The image is in device pixels: the viewport times the context's `deviceScaleFactor`. Missing directories of `path` are made. `locator.screenshot()` clips the page screenshot to the element's bounding box and takes `path`, `type`, `quality` and `omitBackground`.

To capture the pages of failed tests without code, set the runner's `screenshot` option: see [Screenshots of failed tests](/playwright/runner#screenshots-of-failed-tests). For a video of each test, see [Video](/playwright/runner#video).

### Raw frames

`raw` is Pyro's own option. It returns the pixels as they are, with nothing encoded:

```ts
const frame = await page.screenshot({ raw: { format: 'rgba' } });   // width * height * 4 bytes
await page.screenshot({ raw: { format: 'yuv420p', path: '/tmp/frames.yuv' } });
```

- `format` is `'rgba'`, `'rgb24'` or `'yuv420p'`, named as ffmpeg's `-pix_fmt` values.
- With `raw.path` (an absolute path), the browser appends each frame to that file and the returned buffer is empty. The path can be a FIFO that ffmpeg reads, which turns a page stepped frame by frame into a video without an image encoder in between.

Use it to feed frames to ffmpeg or to an image library. [Screenshots and PDFs](/scraping/pdf#raw-frames) covers the same capture from the SDK, with `FrameFifo`.

Both need the browser's rendering support, which the npm package's builds include.

## page.pdf

`page.pdf(options)` passes its options to CDP `Page.printToPDF` unchanged and writes `path` if given. Use CDP's option names: `paperWidth` and `paperHeight` in inches, `marginTop` / `marginBottom` / `marginLeft` / `marginRight` in inches, `landscape`, `scale`, `printBackground`, `pageRanges`. Playwright's `format: 'A4'` and `margin: { top: '1cm' }` are not translated and have no effect. See [Screenshots and PDFs](/scraping/pdf).

## Next

- The first red run, screenshots included: [Triage the first red run](/recipes/playwright-triage-first-failures).
- Writing Linux baselines for CI: [Sharding in CI](/recipes/playwright-ci-without-sharding#screenshot-baselines-in-ci).
- How pages are drawn: [Rendering](/guide/rendering).
