# Dedicated workers

`crates/pyro-webapi/src/worker.rs` (page side) and `worker_scope.rs` (worker side)
implement `new Worker(url)`. A worker runs its script off the page's thread.

## Design

- **Thread and VM.** One thread per worker, with its own JavaScriptCore VM
  (`pyro_js::Engine`), event loop and an empty, unexposed document at the worker's
  URL. Nothing is shared with the page: this is the same one-engine-per-thread
  model as a browser thread. A page without workers pays one empty-list check
  when disposed; `worker_wire.js` is compiled on a realm's first worker use.
- **Global.** `DedicatedWorkerGlobalScope` (`WorkerGlobalScope`, `EventTarget`):
  `self`, `location`, `navigator` (subset), `name`, `origin`, `isSecureContext`
  (the script URL is potentially trustworthy), `crossOriginIsolated` (false),
  `reportError`,
  `postMessage`, `close`, `onmessage`, `onmessageerror`, `onerror`,
  `importScripts`, `setTimeout`/`setInterval` and their clears, `queueMicrotask`,
  `atob`/`btoa`, `structuredClone`, `crypto` with `CryptoKey` (see
  [Web Crypto](CRYPTO.md)), `performance` (the worker's own timeline: User
  Timing, Resource Timing for its fetches, `PerformanceObserver`; see
  [Performance Timeline](PERFORMANCE_TIMELINE.md)), `console`,
  `fetch` with `Headers`/`Request`/`Response`/`FormData`/`Blob`/`File`, `FileReader`,
  `AbortController`, `URL`/`URLSearchParams`, `TextEncoder`/`TextDecoder`,
  `MessageChannel`, `Event` and its message, error, progress and rejection
  subclasses, `XMLHttpRequest` without `responseXML` (see [XHR](XHR.md)),
  `EventSource` (see [Fetch](FETCH.md)), `OffscreenCanvas` with its 2D
  context, `ImageBitmap`, `createImageBitmap` and the other canvas value types
  (made on first use; see [Canvas 2D](CANVAS.md)), `WebSocket`/`CloseEvent` (see
  [WebSocket](WEBSOCKET.md)), `indexedDB` with the `IDB*` interfaces, over the
  owner context's databases (see [IndexedDB](INDEXEDDB.md)), `caches`
  (`Cache`, `CacheStorage`) over the owner context's Cache Storage (see
  [Service workers](SERVICE_WORKER.md)),
  `navigator.storage` (`StorageManager`), and the [streams](STREAMS.md)
  (`ReadableStream`, `WritableStream`, `TransformStream`, the queuing
  strategies, `TextEncoderStream`/`TextDecoderStream`,
  `CompressionStream`/`DecompressionStream`, `response.body`), which, as on a
  window, are made on the realm's first use of one of their names. Everything
  else is undefined, including `document`, `FileReaderSync` and `Worker`.
- **Script fetch.** On the worker's thread, blocking only it, through the page's
  context `NetClient` (cookies, interception, Network events) with the page's
  user agent, extra headers and cache setting. The URL must be same-origin with
  the document (SecurityError otherwise), or `data:`, or `blob:` (bytes captured
  at construction). Non-2xx or a failed load fires a plain `error` Event at the
  `Worker`. A classic script's MIME type is not checked. Requests carry the owner's frame id and
  no loader id: they appear on the owner's page and do not delay its network idle.
- **Modules.** `new Worker(url, {type: 'module'})` evaluates the script as a module:
  static imports, top-level `await` and `import.meta.url`. Every worker's realm has
  a module fetcher, so `import()` works in classic workers too. Each module is
  fetched in a task of the worker's loop the way `importScripts` fetches (CORS mode,
  `blob:` URLs of the worker's own store). A module that fails to load or throws at
  its top level reports the error (`error` at the `Worker`). `importScripts` throws
  TypeError in a module worker. Import attributes work as in a window (patched
  JavaScriptCore): `with { type: "json" }` loads a JSON module, while a CSS module
  (`type: "css"`) fails with TypeError before any fetch, since workers have no
  `CSSStyleSheet`. Module responses (the module worker's own script included, over
  http) must have the MIME type of their module type, and decode as UTF-8.
- **Messages.** `postMessage` validates and copies with the realm's original
  `structuredClone`, then encodes the copy to text plus ArrayBuffer bytes
  (`worker_wire.js`), which cross as a task of the other side's event loop and
  are rebuilt there; failing to rebuild fires `messageerror`. Errors,
  DOMExceptions, Blobs, Files and FileLists cross too (Blob bytes are shared,
  not copied), and so do CryptoKeys and ImageBitmaps (pixels shared, not
  copied). An ImageBitmap or an OffscreenCanvas without a context in the
  transfer list moves and the sender's is detached; an OffscreenCanvas from
  `transferControlToOffscreen` keeps showing its frames on its `<canvas>`
  (see [Canvas 2D](CANVAS.md)). Transferring an ArrayBuffer copies it
  and detaches the sender's. MessagePorts transfer both ways and stay
  entangled across the threads (messages both ways, `start`/`onmessage`,
  `close`, `messageerror`; see [MessageChannel](MESSAGE_CHANNEL.md)), and
  arrive in `event.ports`; ReadableStream, WritableStream and TransformStream
  transfer as the Streams standard specifies, over such ports (see
  [Streams](STREAMS.md)). Messages to a worker wait until its script has
  run. A navigation drops messages on their way to the old document.
- **Errors.** An uncaught exception fires `ErrorEvent` at the global and, if not
  canceled, at the `Worker`, with message `Uncaught <text>`. It does not continue
  to the owner's window, and `error` is null on the page side.
- **End of life.** `terminate()`, `close()` and disposal of the document (navigation,
  page close) end the worker; a running script stops within about 50 ms, a blocked
  fetch is cancelled. `serve` waits up to 1 s for these threads when a connection ends.
- **Limits.** 16 live workers per document and 64 per connection; past that the
  worker fails like a failed load. 64 MiB of messages queued per direction per
  worker: a worker sending more waits for the page, a page sending more gets a
  DataCloneError. One message may exceed this alone if nothing else is queued.

## CDP

A page session with `Target.setAutoAttach` gets `Target.attachedToTarget` with
`type: "worker"` for current and later workers, and `detachedFromTarget` when one
ends (`Target.targetCreated`/`targetDestroyed` too after `setDiscoverTargets`). This
is what makes Playwright's `page.on('worker')` and `page.workers()` work.

The session accepts `Runtime.enable`, `disable`, `evaluate`, `callFunctionOn`,
`releaseObject`, `releaseObjectGroup` and sends `executionContextCreated`,
`consoleAPICalled`, `exceptionThrown`, so `worker.evaluate` and worker console
messages work. A worker does not wait for the debugger
(`waitingForDebugger: false`): up to 256 earlier console calls and exceptions are
replayed by the first `Runtime.enable`. `Network`, `Fetch`, `Security`, `Log`,
`Debugger`, `Profiler` and `Inspector` commands are accepted and do nothing.
`Runtime.getProperties` and `addBinding` are not implemented there, nor are workers
listed by `Target.getTargets`.

## `serve --fork`

A template forks only with no other thread alive, so it must not start workers:
`hold_for_fork` keeps a constructed worker held, queueing its messages and
inspector commands. The forked child that takes a connection starts it
(`release_after_fork`), before serving the connection. Derived templates and waiting
spares keep holding. Mach ports do not survive a fork, so a thread entering a VM in a
child would crash registering with JSC's exception port; a template therefore runs
with `JSC_useMachForExceptions=false` (POSIX signal handlers, inherited by a fork). A page that waits for a worker's reply while a template boots
it waits until a connection takes the snapshot, and a `setup` step that waits for it
times out.

## SharedWorker

`new SharedWorker(url, name | {name})` (`crates/pyro-webapi/src/shared_worker.rs`) runs
on the same machinery as a dedicated worker, with a `SharedWorkerGlobalScope`. One
worker serves every document of the browser thread (one CDP connection) that asks
for the same origin, script URL and name. Each construction makes a `MessageChannel`:
`port` is one end, and the other moves to the worker, whose `connect` event carries
it (`ports[0]`, `source`). The worker runs until it calls `close()`, its script
fails to load (`error` at every connected `SharedWorker`), or the last document that
connected to it goes away.

## Not supported

- Module workers have no import maps of their own, and `import.meta.resolve` follows
  the engine. Without the engine's module loader (system JavaScriptCore),
  `{type: 'module'}` throws NotSupportedError.
- Module shared workers (`new SharedWorker(url, {type: 'module'})`) need the
  engine's module loader in its two steps (`Scope::has_module_phases`, JSC
  patch 0037), so that a graph that fails to fetch, parse or link (`error` at
  every connected `SharedWorker`, no `connect`) is told from a throw of its
  evaluation (the worker's own exception); without them the constructor throws
  NotSupportedError. Their `credentials` is the credentials mode of the main
  script and every module they fetch. A `SharedWorker` whose type or
  `credentials` differ from the live worker it matches gets `error` and no
  connection. Module dedicated workers still ignore `credentials`. Uncaught
  errors inside a shared worker reach its console only, as in browsers, and it
  has no inspector target.
- Service workers reuse this machinery (threads, the cross-VM message format and
  cross-thread ports): see [Service workers](SERVICE_WORKER.md).
- Host objects other than Blob, File, FileList, CryptoKey, ImageBitmap and
  OffscreenCanvas do not cross (DataCloneError), ImageData included.
- Nested workers from shared workers: `Worker` is undefined inside a
  `SharedWorkerGlobalScope`. Dedicated workers start nested workers (see
  `crates/pyro-webapi/src/worker.rs`); they get no inspector target, and at most
  64 live in the process (past that, a nested worker fails like one whose script
  did not load). The place is held by the nested worker's thread until it exits,
  however the worker ended (close, load failure, terminate, its creator's end),
  even while script still holds its `Worker`. An ending worker waits at most 1 s
  for its nested workers' threads (once per level of nesting); one still running
  then, like any worker thread a `join_stopped` call outlasts, stays tracked and is
  joined by a later `join_stopped`. A nested worker's script request is sent with
  its creator's script response URL (after redirects) as the referrer, under the
  creator's policy. The creator's own `location` and base URL are still its
  request URL. An uncanceled error on the nested `Worker` does not propagate
  to its creator's global scope.
- `importScripts` of a page-created `blob:` URL (the store is the owner document's).
- A worker whose wrapper became unreachable runs until its document goes.
- Virtual time (`Emulation.setVirtualTimePolicy`) and the page clock do not reach a
  worker: its timers and `Date` run on real time.
- Playwright reports only workers that attach after it has announced the page. The
  worker of a prepared page attaches in that window: it runs and gets its session, but
  Playwright's `page.on('worker')` does not fire for it.
- A worker started on a page prebooted by `serve --prepare` outside fork mode runs,
  but has no CDP target: the connection did not exist to record it.

Tests: `crates/pyro-browser/src/tests.rs` (`worker_*`, `message_ports_cross_*`,
`a_port_from_a_worker_*`, `two_workers_talk_*`, `streams_transfer_*`,
`crypto_keys_cross_*`), `crates/pyro-webapi/tests/dom_js.rs`
(`worker_*`), `tests/e2e/about-blank.mjs`.
