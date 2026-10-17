# Persistent viewport metrics

`Browser::set_viewport(PageId, lp_dom::media::Viewport)` applies the full native
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
Explicit screen orientation and MediaQueryList change notifications remain
binding gaps; this change does not claim mobile emulation, visualViewport, full
layout or a TOS pass. CDP metrics mapping remains W3's separate increment.
