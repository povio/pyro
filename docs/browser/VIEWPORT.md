# Persistent viewport metrics

`Browser::set_viewport(PageId, pyro_dom::media::Viewport)` applies the full native
viewport to the current PageDom and keeps it for document replacements. New
main and isolated worlds see these values before init scripts. Sibling pages
are independent. `Page::viewport()` returns the persistent state so protocol
callers can clone it and update dimensions/DPR/screen fields without resetting
unrelated media, color-scheme or input preferences.

DPR must be finite and positive; invalid input returns JsError::Range without
changing current or future state. A missing page returns JsError::Type. CDP
integer bounds and zero-as-disable normalization are protocol responsibilities.
Calling the browser setter with Viewport::default() resets the entire viewport;
a metrics-only reset should preserve unrelated fields via the getter.

A changed CSS width/height queues a trusted, nonbubbling, noncancelable window
resize event in the owner loop. Changes before delivery coalesce; restoring the
original size before that task suppresses the event. DPR/screen-only changes do
not fire resize. Event delivery holds no DOM/layout borrow, verifies the current
document generation, and does not retain an old PageDom. Pending tasks become
harmless on navigation or page closure. Media changes also recheck parser scripts
waiting for matching linked stylesheets; they need not wait for an irrelevant
stylesheet response after the viewport changes.

Window dimensions, screen size, devicePixelRatio, native media queries and
computed-style invalidation use the existing PageDom/layout implementation.

## Mobile emulation

`Viewport::mobile` (CDP `Emulation.setDeviceMetricsOverride` `mobile: true`,
Playwright's `isMobile`) lays the main frame's document out as Chrome's mobile
emulation does; `width`/`height` are then the device's. `pyro_dom::viewport_meta`
ports Blink's parsing of `<meta name="viewport">` (and the legacy
`HandheldFriendly`/`MobileOptimized`), `ViewportDescription::Resolve` and the
page scale constraints, in Blink's `f32` arithmetic, so every size rounds as
Chrome's (checked against Chrome 147 on 38 tag, device and content-width cases):

- the layout viewport (`Layout::viewport`, what layout, media queries, viewport
  units and the root's `clientWidth`/`clientHeight` see) is the tag's `width`
  (`device-width`, 1 to 10000 px), widened to fit `initial-scale` and the
  limits, else 980 px without a tag or width, rounded down; its height follows
  the device's aspect ratio unless the tag sets `height`;
- the page scale has DevTools' limits (0.25 to 5) under the tag's own
  (`minimum-scale`/`maximum-scale`, 0.25 and 5 by default, `user-scalable=no`
  locks it), and a minimum that fits the document's width into the device. It
  is the tag's `initial-scale` within them, else that minimum;
- the window (`innerWidth`/`innerHeight`, `Page.getLayoutMetrics`'
  `cssLayoutViewport`, scrolling, hit testing, the implicit intersection root)
  is the device at the minimum scale, rounded up: wider than the layout
  viewport when content overflows it (a `width=device-width, initial-scale=1`
  page with a 1200 px element is 1200 px in `innerWidth`, 390 in
  `clientWidth` on a 390 px phone);
- `visualViewport` (and `cssVisualViewport`) is the device at the page scale;
  `outerWidth`/`outerHeight` and `screen` stay the device's.

The Dom records a document's tag when it is processed (inserted into the
document tree, or its `name`/`content` changed while there): the latest one
wins, a lower legacy kind never replaces a higher one, and removal changes
nothing, as in Blink. `PageDom::layout` takes a new one before answering, so a
tag a script adds is in effect at the next read (Chrome lays out at once too).
Changes to the window or layout viewport fire window `resize`, changes to the
visual viewport's size or scale its `resize`, then media query lists their
`change`, in Chrome's order; tags the parser meets before the first layout
fire nothing (resize events wait for Blink's first layout). The window and
the page scale follow the document's width as of the last layout; the
`innerWidth`, `innerHeight` and `visualViewport` getters lay out first, as
Chrome does. `Area::Viewport` screenshots become the visual viewport at its
scale, in the device's size (CDP `captureScreenshot` without a clip).

Not emulated: pinch zoom (the visual viewport never moves inside the window,
`offsetLeft`/`offsetTop` stay 0 and scrolling moves the window, so a page
zoomed in past its minimum scale cannot pan as in Chrome); `position: fixed`
boxes sit against the layout viewport, where Chrome puts them against the
window (they differ when content is wider than the layout viewport, and by a
pixel of rounding without a tag); Chrome's visual-viewport `resize` at the
first layout of a page without a tag; `resize` when only the document's width
moves the minimum scale; the deprecated `device-width`/`device-height` media
features, which report the layout viewport (as `width`/`height` do in pyro)
where Chrome reports the screen. Child frames ignore `mobile`.
