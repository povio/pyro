# Asynchronous XMLHttpRequest

`lp-webapi::register` registers `XMLHttpRequest`; installed realms expose the
native EventTarget subclass. Request configuration and lifecycle state remain
on the browser thread. Native network callbacks retain bounded byte buffers and
queue page/request IDs to that thread. No browser/runtime source is changed by
this module.

Use the same `page.net`, `PageHooks::remote_poster`, and exact
`PageDom::set_request_context` frame/loader IDs as [fetch](FETCH.md).
The destination is `Destination::Xhr`, with the native request ID preserved.
Install browser `setTimeout` and `clearTimeout` **before** `lp_webapi::install`:
the binding captures those functions before page scripts can replace them.
Nonzero timeouts require those timer functions. Changing `timeout` during a
request reschedules it relative to the original `send` time; zero disables it.
There is no polling timer or new network thread per XHR.

Supported behavior includes asynchronous `open`/`send`/`abort`, request headers,
`withCredentials`, timeout, response headers/status/URL, MIME override, text,
JSON, ArrayBuffer and Blob response types. GET/HEAD ignore send bodies. Other
methods support strings, Blob and BufferSource (including views). HTTP errors
such as 404 complete normally; native network/CORS/limit failures fire `error`.
Invalid JSON yields a null response. Non-text response objects are cached for
repeated access. Text uses a persistent incremental decoder, including charset
selection and incomplete multibyte characters, without decoding earlier chunks
again on each `responseText` read.

The object dispatches `readystatechange`, `loadstart`, download `progress`,
`load`, `error`, `abort`, `timeout`, and `loadend`, with matching `on...`
properties. Replacing an existing non-null handler preserves its listener
position. Handlers live in traced JS properties, so closures referencing their
XHR remain collectable. Pending requests root their wrapper through final event
delivery. Reentrant `open`, `send` and `abort` use request identities/generations
to stop an old request from completing a replacement request. Header callbacks
run before body completion, and abort resets an active request to UNSENT after
its final events.

The first body data notifies immediately; later data callbacks notify at roughly
50 ms intervals when more data arrives;
headers and completion also notify. Pending notifications coalesce, rather than
posting for every chunk. There is no periodic progress wakeup when data is idle.
No CPU, throughput, or process-memory improvement is claimed from this design.

`PageDom::set_xhr_limits(XhrLimits)` configures independent XHR limits before
requests/buffers exist. The shape/defaults match FetchLimits: 64 pending jobs,
16 MiB decoded body bytes per request, 64 MiB retained logical bytes per
PageDom, and 8192 chunks per request. `xhr_usage()` reports pending delivery jobs
and retained logical response bytes. Reopening, abort, network failure, disposal
and GC release buffers. Reading a response does not discard it because XHR
allows repeated access. These are logical native-buffer limits, not RSS or JS
heap limits; uploads, decoded text caches, materialized JS responses and the
separate network-observer queues are outside that byte counter.

Before realm teardown, call `page.dispose()` for the document or
`lifecycle::realm_disposed(page, id)` for one world. This cancels only its XHR
request IDs, clears timers/buffers and active roots, and suppresses stale queued
events. A shared context NetClient remains usable by other pages. Call at a
browser-thread safe point without holding a DOM borrow.

Upload observation: `xhr.upload` is an `XMLHttpRequestUpload` (one per request,
illegal constructor, the seven `on*` handlers). It follows the XHR flags.
- The upload listener flag is captured at `send()`, so listeners added later
  do not count.
- With no request body the upload complete flag is set at once: no upload
  events.
- Upload `loadstart` (0, body length) fires after the request's `loadstart`.
- When the response headers arrive, upload `progress`/`load`/`loadend` fire
  with (length, length) before readyState 2.
- Error, abort and timeout fire the upload event and `loadend` with (0, 0)
  after `readystatechange` and before the request's own events.
- The native transport sends a body in one piece, so there is one upload
  `progress` event rather than intermediate ones.

Explicit gaps: synchronous XHR, URL username/password authentication,
responseXML/document responses, and Document request serialization (FormData
bodies are sent as multipart/form-data). The first three are rejected with
NotSupportedError.
Full MIME/XML sniffing, compressed transfer progress accounting and complete
XMLHttpRequest compatibility are not claimed. Native HTTPS and the existing
URL/CORS/string-model limitations still apply. Ordinary response readers are
implemented; this is not full TOS compatibility evidence.

Focused validation uses real JavaScriptCore and isolated loopback HTTP servers:
event/handler order, native request correlation, POST bodies, 404/CORS behavior,
header/body abort, reentrant reopen/conversion, timer capture/update/cancel,
split UTF-8/charset decoding, response caching, caps/refunds, GC cycles and
page/world disposal with shared networking.
