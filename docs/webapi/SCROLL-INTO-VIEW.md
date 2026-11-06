# Nested scrollIntoView delivery

A fresh overflow container previously stayed at scrollTop zero when a descendant requested `scrollIntoView()`. Only containers with existing native scroll state were considered, and even when those moved the WebAPI queued only a document scroll event. The target could be visible in viewport coordinates while still clipped by its ancestors; a caller waiting for the changed container's event could never finish.

Layout now processes auto/scroll overflow ancestors inside-out, recalculating the target's relative rectangle after each move. Previously explicit scroll state retains its reveal behavior. `Layout::scroll_into_view_targets` returns the changed `ScrollTarget` values in order, followed by the viewport if it moved. The existing bool-returning method remains compatible. WebAPI drops its DOM/layout borrows before queuing one event for each changed target through the existing document and connected-target guards. Unchanged offsets queue no events.

The reduced Chrome147 and pinned5b baseline use the same nested-overflow HTML. Chrome moves both ancestors and delivers inner/outer/document events; baseline5b moves only the viewport and delivers only document. New native and binding regressions cover fresh and primed containers, geometry, event ordering, no-op repeats, focus after reveal and replaced-document task disposal.

This preserves the existing bounded vertical/start reveal policy and reveals horizontally clipped targets at the nearer edge. A horizontally visible target or oversized target spanning both edges retains its offset. This prevents a subsequent focus from creating an extra horizontal scroll after reveal. It does not implement complete scroll alignment options, smooth scrolling, hidden-versus-clip policy, full CSS geometry or child-frame scrolling. CDP consumption is a separate protocol increment. No full TOS/common-suite result is implied by the native tests or reduced control.

Validation: 302 affected release tests passed (one pre-existing ignored), strict release all-target Clippy and formatting passed. Retained red/green logs and Chrome/baseline probe are in `evidence/nested-scroll-reveal/`.

## Alignment options

`Element.scrollIntoView(arg)` reads its `(boolean or ScrollIntoViewOptions)` argument. No argument, `true`, `null` and `{}` keep the policy above (block start, inline nearest). `false` aligns block end. `{block, inline}` take `start`, `center`, `end` or `nearest` per axis (`Layout::scroll_into_view_aligned_script_targets`): each scrollable ancestor (auto, scroll or hidden overflow, inside-out) and then the viewport scrolls so the target's border box lands at that edge, centred, or by its nearest edge, clamped to the scroll range; a fixed-position target moves only its ancestors. An invalid `block`, `inline` or `behavior` value throws `TypeError`. `behavior: 'smooth'` is accepted and scrolls instantly. Scroll margins and `scroll-padding` are not applied.

`Element.checkVisibility(options)` reads `checkOpacity` / `opacityProperty` (an ancestor-or-self with `opacity: 0` is invisible) and `checkVisibilityCSS` / `visibilityProperty` (inherited `visibility: hidden`); `contentVisibilityAuto` is ignored.
