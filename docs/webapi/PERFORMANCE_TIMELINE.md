# Performance Timeline

`window.performance` (crates/pyro-browser/src/nav.rs) holds, per realm:

- User Timing: `mark`, `measure`, `clearMarks`, `clearMeasures`, the
  `PerformanceMark` constructor. `measure()` takes the `PerformanceTiming`
  attribute names as marks (`'domInteractive'`, `'requestStart'`, ...); a
  step that has not run yet (or never does here, such as `unloadEventStart`)
  throws `InvalidAccessError`.
- Paint Timing: `first-paint` and `first-contentful-paint`, both at the
  document's first rendering opportunity (nothing is rasterized).
- Resource Timing: a `PerformanceResourceTiming` entry (`resource`) for
  each HTTP(S) fetch of the document, named after the request URL, with
  `initiatorType` `fetch`, `xmlhttprequest`, `beacon`, `other`
  (EventSource), `script` (classic and module scripts, module imports),
  `link` (stylesheets, preloads), `img` or `iframe` (the container's
  navigation of the frame, from its `src`, in the parent's timeline; one
  the frame's document starts, a reload or a traversal has none). See below.
- Navigation Timing: one `PerformanceNavigationTiming` entry (`navigation`,
  named after the document URL) for HTTP(S) and file documents, with
  `type` (`navigate`, `reload`, `back_forward`), the document fetch's phases
  (`fetchStart` .. `responseEnd`, `redirectCount`, sizes,
  `nextHopProtocol`, `responseStatus`, `contentType`), then
  `domInteractive`, `domContentLoadedEventStart/End`, `domComplete` and
  `loadEventStart/End` recorded as the loader runs those steps (pyro-webapi
  `lifecycle`, `PageDom::timing`). The time origin is the start of the
  document's fetch, so `startTime` and (without redirects) `fetchStart` are
  0. The legacy `performance.timing` (epoch milliseconds) and
  `performance.navigation` read the same values.
- `PerformanceObserver` / `PerformanceObserverEntryList`
  (crates/pyro-webapi/src/performance_observer.js), made on the realm's first
  use: `observe({type, buffered})` and `observe({entryTypes})` with Chrome's
  errors, `takeRecords`, `disconnect`, `supportedEntryTypes`
  (`mark`, `measure`, `navigation`, `paint`, `resource`). Callbacks run in a
  task; the first one after `observe()` gets `droppedEntriesCount` (the
  resource entries the buffer dropped, when `resource` is observed, else 0).

The entry interfaces and User Timing live in crates/pyro-webapi/src/performance.rs,
shared by a Window's `Performance` (pyro-browser `nav`) and a worker's.

## Workers

A dedicated or service worker's `performance` (made on the realm's first
read of it) is its own timeline, as in Chrome: the time origin is the
worker's start (`timeOrigin`, `now()`, and the worker's `Event.timeStamp`),
User Timing (`mark`, `measure`, `clearMarks`, `clearMeasures`; a
`PerformanceTiming` attribute name is a TypeError as a mark to measure from),
and a `resource` entry for each HTTP(S) fetch the worker makes (`fetch`,
`XMLHttpRequest`, `EventSource`, `importScripts` and module imports), with the buffer, `setResourceTimingBufferSize`,
`clearResourceTimings` and `resourcetimingbufferfull` as on a window.
Recording is the window's (`timed_sink` over the worker's own `PageDom`), and
Timing-Allow-Origin is checked against the worker's origin. A worker's fetches
are not in its document's timeline. `PerformanceObserver` and
`PerformanceObserverEntryList` are the window's script, made on first use;
`supportedEntryTypes` is `mark`, `measure`, `resource`. The interfaces
`Performance`, `PerformanceEntry`, `PerformanceResourceTiming`,
`PerformanceMark` and `PerformanceMeasure` are exposed; `timing`,
`navigation` and the navigation and paint entries are not (Window only).

## Resource Timing

Where the times come from (crates/pyro-net/src/timing.rs): every response
head carries a `NetTiming` of `Instant`s. `NetClient::fetch` stamps the
start; each hop stamps its `fetchStart`, and a redirect its end; the request
goes out when a pooled connection is free (or the new one is ready) and the
response starts when its head arrives. A new connection reports its own
phases: the connector (`ProxyConnector`) times TCP (and a proxy tunnel), the
resolver finds the connection it resolves for in a task-local and times the
lookup, and hyper copies the times into every response over that
connection (`Connected::extra`); a response over a connection opened before
its request reused it, so its lookup and connect times are its
`fetchStart`. TLS runs from the end of TCP (`secureConnectionStart`) to the
connection being handed to hyper (`connectEnd`). A cache hit, a static file
or an intercepted response has no connection and starts its request and
response when it is answered.

Timing-Allow-Origin (Fetch "TAO check") runs per hop in pyro-net, a stored
response served from the HTTP cache included, against the requesting
document's origin (an iframe's navigation: its parent's): a
cross-origin hop passes when the header lists `*` or that origin. A failure
keeps `startTime`, `fetchStart`, `responseEnd` and `duration`, and zeroes
the redirect, lookup, connect, request and response times, the sizes and
`nextHopProtocol` ("opaque timing info"). `responseStatus` and
`contentType` are 0 / empty for opaque (`no-cors` cross-origin) responses,
and in an iframe's entry for a frame document of another origin (or reached
through one).

Sizes: `encodedBodySize` and `decodedBodySize` as received;
`transferSize` is `encodedBodySize + 300` (Resource Timing's header
stand-in), 0 for a cache hit (`deliveryType` `cache`).
`renderBlockingStatus` is `blocking` for stylesheets, `non-blocking`
otherwise.

Recording (crates/pyro-webapi/src/resource_timing.rs): a consumer wraps its
pyro-net sink with `timed_sink` (fetch, XHR, beacon, EventSource) or, in
pyro-browser's `NetLoader`, by the `initiator` of its `FetchRequest`. When the
fetch ends (an aborted fetch has no entry; a network error has one with
only its start and end) the I/O thread pushes a plain `FetchReport` into
the document's inbox and, if it was empty, posts one task that turns the
inbox into entries (`ResourceEntry`: milliseconds since the time origin,
floored to 100 microseconds as `performance.now()` is). Every read of the
timeline drains the inbox first, so once a fetch's consumer has seen its end
the entry is there. No JS object exists until a script reads the timeline:
each entry's wrapper is made then (one per entry and realm while held, by
the entry's sequence number) and shares the entry's data.

The buffer follows Resource Timing: 250 entries until
`setResourceTimingBufferSize(n)` (entries over a smaller size stay);
`clearResourceTimings()` empties it. While it is full, new entries wait in a
secondary buffer (at most 1024; more are dropped at once) and one
`resourcetimingbufferfull` event is queued at every `Performance` object of
the document (`onresourcetimingbufferfull` too); what the listeners make
room for moves into the buffer, and the rest is dropped and counted in
`droppedEntriesCount`. Observers get every entry, buffered or not.

## Cost

A worker that never reads `performance` makes no object for it; its
fetches cost what a window's do. A realm that never touches `PerformanceObserver` has no module and no
registration: `mark`/`measure` check one empty list, the loader one empty
list at `load` and at first paint. The timing marks are six `Cell` writes
per document. Each fetch costs a few `Instant::now()` calls in pyro-net, one
small allocation per new connection, and at its end one `FetchReport` (its
URL string) pushed under a mutex plus, per batch, one posted task that
pushes the entries; nothing runs JS unless an observer watches, in which
case each new entry makes its object for the observing realms.

## Gaps

- Unloading the previous document is not timed (`unloadEventStart/End` are
  0), and the time origin is the document fetch's start rather than the
  start of the navigation (before `beforeunload`).
- Initiator types not distinguished: `css` (`@import`, fonts and
  backgrounds are fetched only by the renderer, which reports no entries),
  `input`, `video`, `audio`, `track`, `object`, `embed`, `body`; workers'
  scripts report `script`. Parser-blocking scripts are `non-blocking`.
- `serverTiming` is always empty; `workerStart` and
  `firstInterimResponseStart` are 0; `finalResponseHeadersStart` equals
  `responseStart`; HTTP/3 is never used.
- A 304 revalidation reports `deliveryType` "" and its full transfer size.
- Navigation Timing hides redirects through another origin
  (`redirectCount` 0), but not the cross-origin redirect hops' own
  Timing-Allow-Origin.
- A worker's `importScripts` and module imports are in its timeline with
  initiator type `other` (Chrome's); its WebSocket handshake has no entry.
- No Event Timing, Largest Contentful Paint, Layout Shift or Long Tasks.
