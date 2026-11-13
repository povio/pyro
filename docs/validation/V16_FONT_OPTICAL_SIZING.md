# v0.0.16 optical sizing correction

The first v0.0.16 visual run differed from twelve existing TOS PNGs; the thirteenth was not reached because a separate DocuBox setup hook timed out. No baseline, mask or tolerance was changed during this investigation.

The v0.0.16 fork-safe font scan remains enabled. A static native/Chrome comparison isolated the missing behavior to optical sizing, rather than font directory enumeration:

| system-ui, 13px, weight500, “TOS Platform” | Width |
| --- | ---: |
| v16 default before correction |73.296875px|
| Chrome default |82.6875px|
| v16 explicit `opsz:13` |82.6875px|
| Chrome explicit `opsz:13` |82.6875px|
| v16/Chrome explicit `opsz:28` |73.296875px|

SFNS.ttf has an optical axis whose minimum is17 and default is28. Chrome also matches explicit CSS-pixel optical values at18/24/32px; converting the size to points does not match. Explicit Arial and Helvetica Neue measurements already matched Chrome and were left unchanged.

`vendor/blitz-dom/src/stylo_to_parley.rs` now supplies an optical variation equal to the used CSS font size when `font-optical-sizing:auto` applies and no explicit optical variation exists. `none` retains the font default; explicit `opsz` wins over either setting. Fonts without an optical axis ignore that variation normally.

The isolated macOS renderer regression `crates/pyro-render/tests/font_optical_sizing.rs` failed before the change (1passed/1failed) and passes afterward (2passed/0failed). It compares exact pixels for auto versus explicit size, distinguishes auto from none, and checks explicit override behavior. The new test is formatted; the changed vendor hunk follows rustfmt output, while unrelated pre-existing vendor formatting differences were preserved. Diff whitespace checks pass.

Evidence:

- `/tmp/v16-optical-sizing-before.log`
- `/tmp/v16-optical-sizing-after.log`
- `/tmp/v16-font-probe-metrics.log`
- `/tmp/v16-font-probe-chrome-metrics.log`
- `/tmp/v16-font-axis-native.log`
- `/tmp/v16-font-axis-chrome.log`
- `/tmp/v16-font-axis-sizes-chrome.log`

Old v14 DOM text widths were approximate and therefore were not used as font attribution. The complete PNG set must be rerun after the parent rebuild; this correction is not a claim that all twelve differences have disappeared. Owned static server64089 and Chrome tab21 were closed; the shared Chrome bridge was left running.

## Exact TOS stack and old-renderer attribution

After rebuilding, all 87 behavior tests across six focused TOS files passed, while all 13 existing PNG comparisons differed (`/tmp/tos-v16-optical-visual-verification.log`). Increased changed-pixel counts alone do not establish a regression: the old PNGs also encode the missing auto optical sizing.

An isolated static page uses the exact `--font-body` stack from TOS `apps/fe/src/styles/tailwind.css:977`, 13px/500, and the text “TOS Platform”. Rendered PNGs distinguish the font identity without relying on v14's approximate DOM measurements:

| Renderer and setting | Ink bounds relative to a 120×32 text crop |
| --- | --- |
| v14 auto | x0–72, y4–12 |
| v14 explicit opsz28 | x0–72, y4–12 |
| v14 explicit opsz13 | x0–81, y3–12 |
| v16 auto | x0–81, y3–12 |
| v16 none/default | x0–72, y4–12 |
| Chrome auto | x0–81, y3–12 |

The v14 auto crop is **pixel-identical** to v16 none/default and v14 explicit opsz28. v16 auto is pixel-identical to v16 explicit opsz13. Chrome and v16 auto both measure 82.6875px; both explicit opsz28 measure 73.296875px. Chrome has its own raster antialiasing, so its pixels are not expected to match the native renderer exactly. This proves the old default is the same system font at its default optical axis, rather than a Helvetica fallback.

One separate platform behavior remains: Chrome `font-optical-sizing:none` with `system-ui` still measures 82.6875px, whereas the native renderer's none uses the raw font default (73.296875px). This does not affect the TOS default-auto comparison; matching that platform-specific none behavior is not claimed here. Actual TOS Chrome references below subsequently confirm the representative drawer and resize layout. Individual PNG state differences still require inspection before baseline migration.

Evidence:

- `/tmp/v16-exact-stack-probe.ts` (isolated page and native captures; old v14 captured without fork because its CoreText scan is not fork-safe)
- `/tmp/v16-exact-stack-native.log`
- `/tmp/v16-exact-stack-chrome.log`
- `/tmp/v16-exact-stack-pixels.log`
- `/tmp/font-stack-0.0.14.png`
- `/tmp/font-stack-0.0.16.png`
- `/tmp/font-stack-chrome.png`

## Actual TOS Chrome reference

The parent supplied a seeded manual inspection runtime on43100/43101. A separate Chrome tab at1440×900 opened the same formatting-remark drawer and text-color palette. This was a manual reference, not a qualifying E2E test. The owned tab23 was closed afterward; the parent retains runtime ownership.

Chrome's drawer text, toolbar wrapping and palette location agree with corrected v16: bullets occupy the second toolbar row and the outer palette starts at approximately x1194. Old baseline text is narrower and bullets remain on the first row. Computed styles confirm the exact TOS font stack and auto optical sizing: navigation/action text spans13px/600, drawer title16px/600, input14px/400. Screenshot selection/focus was not identical, so focus-border differences are not renderer findings.

The initial fresh1440px Chrome reference placed Used For text at x429, whereas native placed it at x509. A later **matched resize comparison** explains this difference as current app behavior, not an engine regression. TOS observes a `w-fit min-w-full` table parent that can retain its child's previous explicit width after the outer scroller shrinks.

Chrome initially at1920px has table and observed parent1698px wide, six283px columns, Used For header x505 (text x509). After resizing to1440×900 **without reloading**, Chrome keeps the table and observed parent1698px, while the overflow scroller becomes1218px. This matches v16's screenshot. Fresh1440px initialization instead produces1218px and203px columns. Baseline14's narrower columns therefore encode different resize behavior from current Chrome/native; the initial fresh reference was not a matched transition and cannot support an engine-regression claim.
Evidence:

- `/tmp/tos-chrome-remark-textstyle-1440-palette.png`
- `/tmp/tos-chrome-font-geometry.json`
- `/tmp/tos-chrome-text-table-geometry.json`

A direct-CDP-only static resize probe initially left the table width stale before its subsequent evaluation. This **does not reproduce the TOS path**: repeating with the actual SDK `viewport(1440,900)` step correctly delivers ResizeObserver notifications and updates the table to1218px. The earlier direct-CDP result must not be used to attribute the TOS screenshot difference to an engine regression. The TOS table observes a `w-fit min-w-full` parent that may remain1698px because its own child table has that width; a matched Chrome1920→1440 transition without reloading is required before drawing a conclusion.

- `/tmp/v16-resize-table-probe.ts` (direct CDP, insufficient as TOS evidence)
- `/tmp/v16-resize-table-native.log`
- `/tmp/v16-resize-table-viewport-probe.ts` (actual viewport step, passes)
- `/tmp/v16-resize-table-viewport-native.log`

## Matched resize evidence

- `/tmp/tos-chrome-resize-1920.json`
- `/tmp/tos-chrome-resize-1440.json`
- `/tmp/tos-chrome-resize-measure.js`
- `/tmp/tos-chrome-remark-resized-1440-palette.png`

Both the native viewport step and the matched actual Chrome resize are now consistent. No table/ResizeObserver engine fix is justified by this comparison. Correct optical sizing plus Chrome-equivalent retained table width explain the representative1440px palette screenshot's broad text and column differences. Baseline updates remain unperformed.

## Document-template empty select confirmation

A focused actual TOS Chrome1920×1080 reference confirms that the empty Default role select is28px high, matching v16 rather than the old18px baseline. The value span is24px high with24px line-height; its empty `::before` is an inline-block with zero height, preserving the otherwise-empty line box. Source `ReceiverBlock.tsx:41` uses the default Povio Select, whose installed `Select-CfsyVxsw.js:641` supplies the empty pseudo-element classes. Evidence: `/tmp/tos-chrome-document-default-role.json`, `/tmp/tos-chrome-document-title-block.png`. Owned Chrome tab26 was closed, runtime remains parent-owned.

The complete13-image visual review and39 image hashes are recorded in TOS restoration `v16-visual-baseline-review.md` and `.json`. No baseline was changed.
