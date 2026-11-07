# Service workers

`crates/lp-webapi/src/service_worker.rs` (browser thread: registrations,
lifecycle, clients, request routing), `service_worker.js` (the page's
`navigator.serviceWorker`, `ServiceWorkerRegistration`, `ServiceWorker`),
`service_worker_scope.rs` and `service_worker_scope.js` (the worker's
`ServiceWorkerGlobalScope`), `cache_storage.rs` and `cache_storage.js` (Cache
Storage). Enough of the Service Workers spec for real apps
and for MSW (Mock Service Worker) browser mode, whose unmodified
`mockServiceWorker.js` and `setupWorker()` run and mock `fetch()` and XHR.

## Cost when unused

- `navigator.serviceWorker`, `ServiceWorker`, `ServiceWorkerContainer`,
  `ServiceWorkerRegistration` and `NavigationPreloadManager` are lazy: the page
  module is compiled on a realm's first use of one of them.
- A request of a document that no worker controls pays one flag test
  (`PageDom::sw_controlled`) in `fetch()`/XHR.
- A loader fetch (navigation or subresource) in a browser context without
  registrations pays one emptiness test (`ContextServices::may_route_loads`);
  with registrations, a subresource pays the requesting document's flag test
  and a navigation the scope match of its URL.
- `caches`, `Cache` and `CacheStorage` are lazy too: `cache_storage.js` is
  compiled on a realm's first use of one of them.
- A new document pays one emptiness test of its context's registrations;
  disposal one `Option` test.

## Registrations

- Per browser context, in memory (`ContextServices`), shared by its pages; at
  most 32 per context and 64 live worker versions per browser thread. Nothing
  persists across contexts or runs.
- `register(scriptURL, {scope, type, updateViaCache})`: URLs resolve against
  the document's base; script and scope must be http(s), same origin as the
  document (SecurityError) and free of `%2f`/`%5c` (TypeError). The default
  scope is the script's directory. The script is fetched on the page's network
  (CDP and Playwright routing see it) with `Service-Worker: script`, cache mode
  `no-cache` unless `updateViaCache: 'all'`, and redirect mode `error`: a
  redirected script is a TypeError (the max scope is the requested URL's). The
  job outlives the page: a navigation during the fetch or install does not
  wedge the registration. A non-2xx status is a TypeError
  naming the status (MSW looks for "(404)"), a non-JavaScript MIME type a
  SecurityError, and a scope outside the script's directory a SecurityError
  unless `Service-Worker-Allowed` allows it. A script that throws at its top
  level is a TypeError ("ServiceWorker script evaluation failed").
- Registering the same scope and script again resolves with the registration;
  a different script updates it. Jobs on one registration run one at a time
  (at most 16 waiting).
- `navigator.serviceWorker` exists only in secure contexts (`http://localhost`,
  `127.0.0.1`, https); elsewhere the getter returns `undefined`. In a document
  with an opaque origin (a frame sandboxed without `allow-same-origin`) the
  getter throws a SecurityError (as Chromium), and such a document is never a
  client nor controlled.
- Templates of `serve --fork` and prepared pages refuse registration
  (NotSupportedError): a template must not start threads.

## Lifecycle

Each version is a worker thread with its own VM (the dedicated worker
machinery). Its script runs, then: `installing` (`updatefound`; the `register()`
promise resolves), `install` with `waitUntil`, `installed` (`waiting`), then
`activating` once no client is controlled by the previous active worker (also
checked when such a client goes away) or `skipWaiting()` was called, `activate` with `waitUntil`, `activated`.
`statechange` fires on every `ServiceWorker` object of each client page; the
same version is the same object in a realm. A replaced or failed version is
`redundant` and its thread stops. `registration.update()` fetches the script
again and starts a new version only if its bytes changed. `unregister()` removes
the registration at once (new documents are no longer controlled); its versions
stop when no client is controlled by them.

## Clients and control

A document becomes a client when it touches the API or is created in the scope
of a registration with an active worker; it is then controlled by that worker
(`navigator.serviceWorker.controller`). `clients.claim()` controls the in-scope
clients and fires `controllerchange`. `ready` resolves with the registration of
the document's URL once it has an active worker. `clients.get(id)`,
`clients.matchAll({includeUncontrolled, type})` (window clients of the worker's
origin) and `Client`/`WindowClient` (`id`, `url`, `type`, `frameType`,
`visibilityState`, `focused`, `postMessage`) are there; `focus()`, `navigate()`
and `clients.openWindow()` reject.

## Requests

A controlled document's `fetch()` and asynchronous XHR to http(s) URLs become
`fetch` events of its controller, and so do the requests the loader makes for
it: classic and module scripts (`script`, module ones `cors`), stylesheets
(`style`), images, fonts, audio and video, preloads and downloads. The
event's `request` has the request's method, URL, headers, body, `mode`,
`credentials`, `cache`, `redirect`, `referrer`, `referrerPolicy` and
`destination`; `clientId` is the document's.

Navigations: a top-level or iframe navigation (form POSTs included) to a URL
in the scope of a registration with an active worker is a `fetch` event of
that worker, whether or not the navigating frame was controlled: `mode` is
`navigate`, `destination` `document` or `iframe`, `clientId` empty and
`resultingClientId` the id of the client the navigation creates, which is the
new document's (`clients.get(resultingClientId)` finds it once it exists).
The response of `respondWith` becomes the document (its URL is the
response's, or the request's for a script-made one); without `respondWith` the
navigation goes to the network. The new document is controlled by the worker
of its URL's registration, as before. `fetch(event.request)` in the worker
fetches a navigation request as one (no CORS, a `basic` response).

- `respondWith(response | promise)`: the response's body is read whole and
  answers the request with its status, status text, headers and type (an
  opaque response answers only a `no-cors` request, a CORS one not a
  `same-origin` request). A rejected promise, a non-Response or `Response.error()`
  is a network error (a TypeError in `fetch()`).
- No `respondWith`: the request goes on to the network as it would have, so
  CDP `Network`/`Fetch` and Playwright's `page.route()` see it.
- The worker's own `fetch()` uses the context's network with the frame id of
  the page that registered it: its requests appear on that page, where
  `page.route()` sees them (MSW's passthrough requests included).
- Synchronous XHR bypasses the worker (its answer may need the blocked thread).

## Messages

`ServiceWorker.postMessage` fires an `ExtendableMessageEvent` at the worker
(`source` the sending `Client`, `origin` the client's), and `Client.postMessage`
a `MessageEvent` at `navigator.serviceWorker` (`source` the `ServiceWorker`),
queued until `startMessages()`, `onmessage` or the document's
`DOMContentLoaded`. Both use the cross-VM format of dedicated worker
messages ([Structured cloning](STRUCTURED_CLONE.md)): Blobs, Files and
CryptoKeys cross, and the transfer list takes ArrayBuffers, MessagePorts
(entangled across threads by the shared `port_link.rs` pipe, see
[MessageChannel](MESSAGE_CHANNEL.md)) and Readable, Writable and Transform
streams (over such ports, see [Streams](STREAMS.md)); at most 64 MiB wait for
a worker (more is a DataCloneError), and a worker's `Client.postMessage` past
64 MiB waiting for the browser thread blocks until it catches up, as a
dedicated worker's does. A worker has at most 4096 unanswered `clients`/
`skipWaiting`/`update` requests (more reject with QuotaExceededError).
Messages and requests keep flowing after the page that registered the worker
navigated or closed. MSW's `MOCK_RESPONSE` sends its body stream to the worker this way.

## CDP

Each version is a `service_worker` target of its browser context
(`Target.targetCreated`, and `Target.attachedToTarget` on the browser
connection when the browser-level `Target.setAutoAttach` is on), with the
`Runtime` support of worker targets. This is what makes Playwright's
`context.on('serviceworker')`, `context.serviceWorkers()` and
`serviceWorker.evaluate()` work. Its target id starts with `SW`.

`Network.setBypassServiceWorker` on a page session makes that page's
navigations, subresources, fetch() and XHR (all frames, across navigation)
skip the worker, as Playwright's
`serviceWorkers: 'block'` and its storage-state page expect. Promises of
`navigator.serviceWorker` settle in the world that made the call, so
isolated worlds (Playwright's utility world) can use it too.

## Cache Storage

`caches` (on windows, dedicated workers and service workers), `CacheStorage`
(`open`, `has`, `delete`, `keys`, `match` with `cacheName`) and `Cache`
(`match`, `matchAll`, `add`, `addAll`, `put`, `delete`, `keys`, with
`ignoreSearch`, `ignoreMethod` and `ignoreVary`):

- In memory, per browser context and origin, in the context's local storage
  partition like [IndexedDB](INDEXEDDB.md): the context's documents, frames,
  worlds, dedicated workers and service workers of one origin share the
  caches; they survive navigation; another context starts empty; nothing is
  written to disk. Cache names keep their creation order.
- Entries keep insertion order; a `put` replaces the entries its request
  matches (Vary included) and appends. `put`, `add` and `addAll` read the
  response body whole; `addAll` stores all or nothing (a duplicate request is
  an InvalidStateError). The checks and errors are the spec's: GET and
  http(s) only, no 206, no `Vary: *`, no used body, and for `add`/`addAll`
  an ok response.
- A matched response is a new `Response` with the stored type (`default`,
  `basic`, `cors`, `opaque`, ...), URL, status, status text, headers and
  body; `keys()` gives new `Request`s with the stored URL, method and headers.
  A `Cache` object keeps working after `caches.delete()` dropped its name.
- `caches` is `undefined` outside secure contexts and throws a SecurityError
  in a document with an opaque origin (a frame sandboxed without
  `allow-same-origin`), as in Chromium.
- `add`/`addAll` fetch with the realm's `fetch` as it was when the module was
  made. No quota, no persistence, no CDP `CacheStorage` domain.

## Not supported

- Requests of dedicated workers a controlled page starts (their scripts and
  their own fetches) do not go through the worker. A redirect the worker
  answers a navigation with (`Response.redirect()`) commits as a document
  instead of being followed.
- Responses are read whole before the page sees them (no streaming from the
  worker); `FetchEvent.preloadResponse` is always `undefined`, and navigation
  preload does nothing.
- No Push, Background Sync, Notifications from the
  worker, or idle termination of a worker.
- A worker's `registration.update()` does nothing; `clients.matchAll` and
  `claim()` see the documents that are clients (above), not every document of
  the context; `frameType` is `top-level` or `nested`, never `auxiliary`.
- Events the worker's script receives are made by script: `isTrusted` is false.
- A page that touches `navigator.serviceWorker` in an isolated world gets its
  updates in the main world only.

Tests: `crates/lp-browser/tests/service_worker.rs` (lifecycle, interception of
fetch/XHR, navigations and subresources, bypass, Cache Storage in pages,
workers and service workers, messages and ports, registration errors, MSW's
real worker script),
`tests/e2e/msw.mjs` (msw 2.15.0's `setupWorker` through Playwright).
