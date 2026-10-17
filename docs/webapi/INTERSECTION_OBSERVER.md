# IntersectionObserver

`IntersectionObserver` now supplies the constructor missing from eight cases in
[TOS coverage batch 1](../validation/TOS_COVERAGE_BATCH_1.md). This binding computes
intersections from lp-dom's existing faux layout; it does not report every target
as visible. It is not evidence of complete CSS layout or full TOS compatibility.

Supported options are an Element/Document/null root, rootMargin with one to four
absolute CSS lengths or percentages, and a scalar/iterable threshold. Thresholds
are finite, sorted values in [0,1], defaulting to [0], exposed as a stable frozen
array. Root margins expand the root rectangle; percentages use its width.
Observations include initial entries, threshold crossings and isIntersecting
changes. Edge contact can intersect at zero area; an intersecting zero-area
target has ratio one. Detached targets and targets outside the explicit root do
not intersect. Ancestor overflow clipping follows the native cascaded style.
Visibility/opacity are not treated as geometric occlusion.

`observe` deduplicates targets; `unobserve` and `disconnect` stop observation but
preserve entries already queued. `takeRecords` drains those entries without
calling the callback. Entries retain target identity and snapshot geometry even
if the target is subsequently removed. Callbacks receive the observer as both
this and the second argument; one callback's exception does not stop others.

## Scheduling and ownership

`PageDom.after_task` checks DOM/style/focus/hover/viewport and scroll revisions,
plus initial registrations. Unchanged checkpoints skip geometry work; changed
checkpoints visit registered targets, not a separate whole-document scan. The
underlying faux-layout operations still have their own tree traversal costs.
There is no polling timer and no measured CPU/memory improvement claim.

Delivery is coalesced into a separate `PageHooks.post_task`, after Promise
microtasks. The browser already calls the checkpoint at its owner-task, timer,
remote-completion and CDP boundaries. Direct native embedders must call it after
mutations and provide a genuinely queued task service. Without that service,
entries remain available through takeRecords; callbacks never run inline as a
fallback. Installing hooks later allows pending delivery at the next checkpoint.

Native observer references to PageDom are weak. Active observers and observers
with pending records pin their JS wrapper; callbacks are traced JS properties.
These active roots may retain callback-captured DOM objects until disconnected
and drained. Call `lifecycle::realm_disposed` before realm disposal, and
`PageDom.dispose` before document teardown. Both cancel records/observations and
release pins. Queued closures hold PageDom weakly and ignore disposed documents.
Native node references prevent slot reuse until observations/records are released.

## Element scroll geometry (native layout change)

Element `scrollTop`/`scrollLeft` offsets now shift the client rects of the
element's descendants (`Layout::ancestor_scroll_offset`), so scrolling an
overflow root changes observed geometry without a DOM mutation. Upstream's faux
layout subtracts only the window scroll (Element.zig:1734-1741); this is a
documented lightningpanda deviation. Hit testing (`elementFromPoint`, CDP mouse
input) applies the same offsets so a rect's interior hits its element, and
`scrollIntoView[IfNeeded]` first reveals the element inside ancestors that have
element scroll state (aligned to the container top). Containers that were
never scrolled keep upstream behavior, and hit testing still does not clip
descendants to overflow containers.

## Limits and validation

Nonzero delay/scrollMargin and trackVisibility throw NotSupportedError. Observing
another PageDom is explicitly unsupported. Cross-frame intersections, paint
occlusion, clip-path, complete containing-block/transform geometry, the public
IntersectionObserverEntry constructor, and a rendering-frame cadence are not
implemented. Geometry inherits the documented native layout subset, including
its positioning and sizing limitations. This is not a full WPT conformance claim.

Focused real-JSC regressions cover initial/task/Promise order, thresholds,
viewport changes, window and element scrolling, cascaded ancestor clipping, explicit roots,
root margins, edge and zero-area intersections, detached nodes, queued-record
retention, takeRecords, option conversion/reentry, GC keepalive/release, callback
exceptions, isolated realms and stale task disposal. The original missing-binding
failure is retained separately. The unchanged nine-case TOS batch must be rerun
on a new pinned candidate; the separate public Response constructor gap remains.

The contract follows the supported subset of the
[Intersection Observer algorithms](https://w3c.github.io/IntersectionObserver/).
