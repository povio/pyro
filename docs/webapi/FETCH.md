# Fetch binding and embedder contract

`pyro-webapi::install` installs `fetch(url, init)` and the network-created `Response`
interface. This is an implemented subset, not full Fetch or TOS compatibility.
The binding uses the existing `pyro-net` HTTP/CORS/cookie/interception path and
JavaScriptCore promises. No browser/runtime wiring is added by this change.

Over HTTP/2 `Response.statusText` is empty, as the protocol has no reason phrase.

Each HTTP(S) fetch (and `navigator.sendBeacon`) adds a Resource Timing entry
to its document's timeline when it ends (`resource_timing::timed_sink`; see
`PERFORMANCE_TIMELINE.md`).

Before fetching, the embedder must provide:

- `page.net`: the context's `NetClient`.
- `PageHooks::remote_poster()`: posts a `Send` closure to the owning browser thread.
  It must enqueue work, not execute the closure on the network thread.
- `page.set_request_context(RequestContext { frame_id, loader_id })`: exact page
  frame ID and current document loader ID for CDP correlation. The default has
  neither ID. Requests retain `Destination::Fetch` and their native `RequestId`.
- Normal `page.after_task()` checkpoints and native finalizer draining.

`PageDom::set_fetch_limits(FetchLimits)` returns false while any job or response
body remains. Defaults are 64 pending jobs, 16 MiB decoded bytes per response,
64 MiB retained logical body bytes per PageDom, and 8192 chunks per response.
Clones share one retained buffer and count those bytes once; consuming the last
clone or collecting all unread wrappers releases it. Each materialized JS body
result is a separate allocation. These limits do not bound JS heap usage,
request uploads, response metadata, transient reader copies, or the separate
network-observer queues. `fetch_usage()` reports pending browser delivery jobs
and retained logical bytes, not process RSS.

Headers resolve the fetch promise before body completion. Data callbacks append
to the bounded buffer without posting browser tasks. Headers and termination
coalesce wakeups; a request posts at most two delivery tasks. Overflow clears the
retained buffer, rejects body consumption, and cancels the request when its
browser task runs. This architecture has no polling timer. Tests verify wake
counts and retention limits; no measured CPU or throughput improvement is claimed.

Call `page.dispose()` at a browser-thread safe point before disposing document
realms, without holding a DOM borrow. It cancels only that page's fetch IDs,
closes delivery buffers, releases abort registrations and pending JS roots,
and removes task routing. It does not shut down a shared context NetClient.
`lifecycle::realm_disposed(page, id)` performs the corresponding per-world cleanup.
Pending promises in the disposed realm are discarded without invoking JS.
Already queued delivery tasks contain only page/request IDs and become harmless.
Abort before headers rejects fetch; abort after headers rejects unread/pending
body readers with the original signal reason. Null bodies remain readable as
empty bytes. Native abort registrations are removed once completed bodies have no remaining
consumers, or when their body is dropped.

Supported behavior:

- String URL input, relative to document URL; HTTP and native data URLs.
- Method, HeadersInit, credentials, CORS/same-origin/no-cors, redirect modes,
  AbortSignal, and string/Blob/BufferSource request bodies including subviews.
- Forbidden request-header filtering and combined duplicate request values.
- Status, URL, type, redirect flag, immutable same-object response headers;
  cookie-header filtering, CORS exposed headers, and opaque response filtering.
- `text`, `json`, `arrayBuffer`, `bytes`, `blob`, `clone`, and `bodyUsed`.
  Body consumption is single-use except for null bodies. HTTP errors such as
  404 resolve normally; network errors reject.

Script-created responses: `new Response(body, init)` (null, string,
Blob, BufferSource or URLSearchParams body with the matching Content-Type;
status 200-599 else RangeError; statusText reason-phrase check; null-body
statuses 101/103/204/205/304 reject a body), `Response.json(data, init)`,
`Response.redirect(url, status)` (301/302/303/307/308, absolute Location
resolved against the document URL) and `Response.error()` (type "error",
status 0, immutable headers). They have type "default", empty URL, mutable
headers (Set-Cookie/Set-Cookie2 dropped from the init only), complete
in-memory bodies with the same single-use readers, and clones with copied
headers. `response.body` is `null` for null bodies and otherwise throws
TypeError (no streams).

URL parsing for fetch inputs uses the [URL binding](URL.md).

FormData request/response bodies are serialized as multipart/form-data
(see [FormData](FORM_DATA.md)).

Bodies as streams are described in [Streams](STREAMS.md).

Not implemented: the Request constructor,
the "response" header guard after construction (later
`headers.set('set-cookie')` is not filtered), multipart `Response.formData()`
(urlencoded bodies are parsed; other types reject with TypeError), full MIME parsing for
response Blob types and service workers.

Referrer, referrer policy, integrity and keepalive (fetch init or a `Request`
input):
- `referrer` (`""` is no referrer; `about:client` or another origin's URL is the
  document) is sent as `Referer` under the request's `referrerPolicy`, else the
  document's policy: its last `<head>` `<meta name=referrer>`, else its
  `Referrer-Policy` response header, else strict-origin-when-cross-origin. The
  client applies the policy per hop (including the https-to-http downgrade
  rule), and a redirect's `Referrer-Policy` header changes the next hop's. XHR
  sends the document URL under the document's policy. Element `referrerpolicy`
  attributes and `rel=noreferrer` are not applied yet.
- `integrity` (SRI sha256/384/512, base64 or base64url): the response is held
  until the whole body has arrived, then delivered if it matches the strongest
  algorithm's digests, else the fetch rejects with TypeError. A no-cors request
  with valid metadata rejects (an opaque response cannot match).
- `keepalive`: the request is not canceled when its document is disposed
  (navigation, frame removal); its page-side state goes. Keepalive fetches and
  beacons of a document share a 64 KiB in-flight body quota; over it fetch
  rejects with TypeError.

`navigator.sendBeacon(url, data)` POSTs with credentials and keepalive
semantics (no document owns it, so it outlives the page), `no-cors` unless the
body's type is not CORS-safelisted. CDP reports it as resource type `Ping`. It
returns false over the keepalive quota or without page networking; a
ReadableStream body throws TypeError.

Request headers like Chrome: `Sec-Fetch-Site`/`-Mode`/`-Dest` (and
`Sec-Fetch-User: ?1` on browser-initiated top-level navigations) go to
potentially trustworthy URLs (https, localhost, loopback), keeping the most
cross-site value across redirects. A navigation without a referrer counts as
browser-initiated (`none`), so a CDP `Page.navigate` with a `referrer` reports
a site relation to it; link and form navigations do not send `Sec-Fetch-User`
(no user-activation tracking).

`EventSource` (server-sent events) exists in windows and dedicated workers and
is made on a realm's first use of the name. It streams a `text/event-stream`
GET (`Accept: text/event-stream`, `Cache-Control: no-cache`, credentials per
`withCredentials`, CORS for cross-origin URLs) through the context client, and
parses it per HTML (BOM, CR/LF/CRLF, comments, `event`/`data`/`id`/`retry`).
`open`, `message`/named events (MessageEvent with `origin` and `lastEventId`)
and `error` fire in order; the end of the stream or a network error
reconnects after the reconnection time (default 3 s, `retry`) with
`Last-Event-ID`; a non-200 response, another MIME type or a CORS failure
closes it for good. Its events are dispatched by script, so `isTrusted` is
false. Document disposal or realm teardown stops its connection.

Content-Encoding is decoded like Chrome: lists (`gzip, br`, or several header
lines) decode in reverse order, `deflate` accepts zlib-wrapped and raw streams,
and a list naming `identity` or an unsupported coding (`zstd`, bogus values)
passes the body through undecoded instead of failing.

HTTP cache: each browser context has a private, in-memory, byte-bounded cache in
pyro-net (`crates/pyro-net/src/cache.rs`, an RFC 9111 subset). By default contexts
never share entries, and a context's entries are freed with it.
- The one exception is an explicit opt-in process-wide shared cache
  (`PYRO_SHARED_ASSET_CACHE_MB` or `NetConfig.shared_asset_cache`).
- It shares credential-free GET 200 responses that are explicitly `public` and
  fresh, with no `Set-Cookie`/`Vary`/`private`/`no-store`/`no-cache`, across
  every context of the process that goes out as the browser does: a context with
  a proxy or user agent of its own never uses it (its requests are its
  identity's). The contract is header-based, not limited to a MIME type or path;
  see `SharedAssetCache`.
- `init.cache` maps to it: `default`, `no-store`, `reload`, `no-cache`,
  `force-cache` and `only-if-cached` follow the Fetch cache-mode rules, including
  the `Pragma`/`Cache-Control` request headers that no-store, reload and no-cache add.
- `only-if-cached` without `mode: 'same-origin'`, and any other value, reject
  with TypeError. A miss under `only-if-cached` is a network error.
- A request carrying its own conditional headers bypasses the cache, so the page
  sees its 304.
- Stored: GET responses with explicit freshness (`max-age`/`Expires`) or with
  validators (`ETag`/`Last-Modified`, revalidated with a 304 merge). `Vary` is
  honoured.
- Never stored: `no-store`, `Vary: *`, `Set-Cookie` responses, `Range`/206 and
  redirects.
- Not implemented: heuristic freshness, stale serving, and partitioning by
  top-level site within one context. HTTPS remains unsupported by the native transport.
CORS and URL handling inherit the documented native subset. Lone UTF-16
surrogates share the existing UTF-8 string-model limitation.

Validation uses real JavaScriptCore and isolated loopback HTTP servers. It covers
headers-before-body delivery, method/body conversion, redirects/HTTP errors,
CORS/opaque filtering, cross-realm borrowed methods, abort reason identity,
limits and refunds, disposal/late tasks, and GC release. It does not exercise
browser navigation wiring, Playwright/TOS flows, or upstream performance parity.

The separate [XHR binding](XHR.md) now implements an asynchronous subset using
the same network client and request-context IDs, with its own buffer limits.
