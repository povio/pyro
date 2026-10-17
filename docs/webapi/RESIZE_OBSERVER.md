# ResizeObserver

`ResizeObserver`, `ResizeObserverEntry` and `ResizeObserverSize` port upstream
Lightpanda's semantics (src/browser/webapi/ResizeObserver.zig @ 2ad38773) over
the native faux layout:

- An observation's size is the element's faux-layout width/height
  (`Layout::element_axis`, upstream `getElementAxis`) while it is connected and
  rendered, otherwise 0x0. An entry is delivered when the size differs from the
  last reported size, which starts at 0x0: observing a rendered element yields
  an initial entry; a hidden or detached one does not until it gets a size.
- `contentRect` is `(0, 0, width, height)`. `borderBoxSize`,
  `contentBoxSize` and `devicePixelContentBoxSize` are frozen one-item arrays
  with the same size (upstream ignores the `box` option; invalid `box` values
  throw TypeError).
- Only the latest size per target is queued between deliveries.

## Scheduling and ownership

The same checkpoint as IntersectionObserver: `PageDom.after_task` re-measures
only when the DOM/style/focus/hover/viewport/scroll stamp changed or a target
was observed, and delivers callbacks in a separate `PageHooks.post_task`
(after Promise microtasks). There is no polling and no inline fallback when no
task service is installed. Native observer references to the PageDom are weak;
an observer with targets or pending records pins its JS wrapper so it keeps
delivering when unreferenced. `lifecycle::realm_disposed` and `PageDom.dispose`
cancel observations and release pins.

## Limits

No rendering-frame cadence, no depth-ordered loop limit or "ResizeObserver loop
completed with undelivered notifications" error, no device-pixel rounding,
padding/border boxes are not distinguished (faux layout), and observing an
element of another PageDom throws NotSupportedError. Focused real-JSC tests
cover initial/task delivery, no-change suppression, style resizes, hidden 0x0,
unobserve/disconnect, `box` validation, GC keepalive and realm disposal. This
is not a WPT conformance claim.
