# Service workers

`crates/lp-webapi/src/service_worker.rs` (browser thread: registrations,
lifecycle, clients, request routing), `service_worker.js` (the page's
`navigator.serviceWorker`, `ServiceWorkerRegistration`, `ServiceWorker`),
`service_worker_scope.rs` and `service_worker_scope.js` (the worker's
`ServiceWorkerGlobalScope`). Enough of the Service Workers spec for real apps
and for MSW (Mock Service Worker) browser mode, whose unmodified
`mockServiceWorker.js` and `setupWorker()` run and mock `fetch()` and XHR.

## Cost when unused

- `navigator.serviceWorker`, `ServiceWorker`, `ServiceWorkerContainer`,
  `ServiceWorkerRegistration` and `NavigationPreloadManager` are lazy: the page
  module is compiled on a realm's first use of one of them.
- A request of a document that no worker controls pays one flag test
  (`PageDom::sw_controlled`) in `fetch()`/XHR.
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
  `no-cache` unless `updateViaCache: 'all'`. A non-2xx status is a TypeError
  naming the status (MSW looks for "(404)"), a non-JavaScript MIME type a
  SecurityError, and a scope outside the script's directory a SecurityError
  unless `Service-Worker-Allowed` allows it. A script that throws at its top
  level is a TypeError ("ServiceWorker script evaluation failed").
- Registering the same scope and script again resolves with the registration;
  a different script updates it. Jobs on one registration run one at a time
  (at most 16 waiting).
- `navigator.serviceWorker` exists only in secure contexts (`http://localhost`,
  `127.0.0.1`, https); elsewhere the getter returns `undefined`.
- Templates of `serve --fork` and prepared pages refuse registration
  (NotSupportedError): a template must not start threads.

## Lifecycle

Each version is a worker thread with its own VM (the dedicated worker
machinery). Its script runs, then: `installing` (`updatefound`; the `register()`
promise resolves), `install` with `waitUntil`, `installed` (`waiting`), then
`activating` once no client is controlled by the previous active worker or
`skipWaiting()` was called, `activate` with `waitUntil`, `activated`.
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
`fetch` events of its controller. The event's `request` has the request's
method, URL, headers, body, `mode`, `credentials`, `cache`, `redirect`,
`referrer`, `referrerPolicy` and `destination`; `clientId` is the document's.

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
`DOMContentLoaded`. Both serialize like dedicated worker messages, with
ArrayBuffers, MessagePorts and ReadableStreams in the transfer list (see
[Workers](WORKER.md)); at most 64 MiB wait for a worker.

## CDP

Each version is a `service_worker` target of its browser context
(`Target.targetCreated`, and `Target.attachedToTarget` on the browser
connection when the browser-level `Target.setAutoAttach` is on), with the
`Runtime` support of worker targets. This is what makes Playwright's
`context.on('serviceworker')`, `context.serviceWorkers()` and
`serviceWorker.evaluate()` work. Its target id starts with `SW`.

## Not supported

- Navigation requests and the document's subresources (scripts, stylesheets,
  images, iframes) do not go through the worker; nor do requests of dedicated
  workers a controlled page starts.
- Responses are read whole before the page sees them (no streaming from the
  worker); `FetchEvent.preloadResponse` is always `undefined`, and navigation
  preload does nothing.
- No Cache Storage (`caches`), Push, Background Sync, Notifications from the
  worker, or idle termination of a worker.
- A worker's `registration.update()` does nothing; `clients.matchAll` and
  `claim()` see the documents that are clients (above), not every document of
  the context; `frameType` is `top-level` or `nested`, never `auxiliary`.
- Events the worker's script receives are made by script: `isTrusted` is false.
- A page that touches `navigator.serviceWorker` in an isolated world gets its
  updates in the main world only.

Tests: `crates/lp-browser/tests/service_worker.rs` (lifecycle, interception,
messages and ports, registration errors, MSW's real worker script),
`tests/e2e/msw.mjs` (msw 2.15.0's `setupWorker` through Playwright).
