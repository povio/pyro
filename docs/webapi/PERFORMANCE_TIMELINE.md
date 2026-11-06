# Performance Timeline

`window.performance` (crates/lp-browser/src/nav.rs) holds, per realm:

- User Timing: `mark`, `measure`, `clearMarks`, `clearMeasures`, the
  `PerformanceMark` constructor. `measure()` takes the `PerformanceTiming`
  attribute names as marks (`'domInteractive'`, `'loadEventEnd'`, ...); a
  step that has not run yet (or never does here, such as `unloadEventStart`)
  throws `InvalidAccessError`.
- Paint Timing: `first-paint` and `first-contentful-paint`, both at the
  document's first rendering opportunity (nothing is rasterized).
- Navigation Timing: one `PerformanceNavigationTiming` entry (`navigation`,
  named after the document URL) for HTTP(S) and file documents, with
  `type` (`navigate`, `reload`, `back_forward`), `domInteractive`,
  `domContentLoadedEventStart/End`, `domComplete` and `loadEventStart/End`
  recorded as the loader runs those steps (lp-webapi `lifecycle`,
  `PageDom::timing`). The legacy `performance.timing` (epoch milliseconds)
  and `performance.navigation` read the same values.
- `PerformanceObserver` / `PerformanceObserverEntryList`
  (crates/lp-browser/src/performance_observer.js), made on the realm's first
  use: `observe({type, buffered})` and `observe({entryTypes})` with Chrome's
  errors, `takeRecords`, `disconnect`, `supportedEntryTypes`
  (`mark`, `measure`, `navigation`, `paint`). Callbacks run in a task with
  `droppedEntriesCount: 0` on the first one after `observe()`.

## Cost

A realm that never touches `PerformanceObserver` has no module and no
registration: `mark`/`measure` check one empty list, the loader one empty
list at `load` and at first paint. The timing marks are six `Cell` writes
per document.

## Gaps

- The time origin is the commit of the document's response, not the start
  of the navigation, so the fetch phases (`fetchStart` .. `responseEnd`),
  redirects and unloading report 0 (the legacy attributes report
  `navigationStart`), and sizes, `nextHopProtocol` and `responseStatus` are
  empty.
- No Resource Timing: `getEntriesByType('resource')` is always empty, there
  is no `setResourceTimingBufferSize`/`clearResourceTimings`, and `resource`
  is not a supported entry type. Recording entries needs request-completion
  data from lp-net for every request of the document.
- No Event Timing, Largest Contentful Paint, Layout Shift or Long Tasks.
- Workers have `performance.now()` only.
