# Dedicated workers

`crates/lp-webapi/src/worker.rs` (page side) and `worker_scope.rs` (worker side)
implement `new Worker(url)`. A worker runs its script off the page's thread.

## Design

- **Thread and VM.** One thread per worker, with its own JavaScriptCore VM
  (`lp_js::Engine`), event loop and an empty, unexposed document at the worker's
  URL. Nothing is shared with the page: this is the same one-engine-per-thread
  model as a browser thread. A page without workers pays one empty-list check
  when disposed; `worker_wire.js` is compiled on a realm's first worker use.
- **Global.** `DedicatedWorkerGlobalScope` (`WorkerGlobalScope`, `EventTarget`):
  `self`, `location`, `navigator` (subset), `name`, `origin`, `isSecureContext`,
  `postMessage`, `close`, `onmessage`, `onmessageerror`, `onerror`,
  `importScripts`, `setTimeout`/`setInterval` and their clears, `queueMicrotask`,
  `atob`/`btoa`, `structuredClone`, `crypto` with `CryptoKey` (see
  [Web Crypto](CRYPTO.md)), `performance.now()`, `console`,
  `fetch` with `Headers`/`Response`/`FormData`/`Blob`/`File`, `FileReader`,
  `AbortController`, `URL`/`URLSearchParams`, `TextEncoder`/`TextDecoder`,
  `MessageChannel`, `Event` and its message, error, progress and rejection
  subclasses, `XMLHttpRequest` without `responseXML` (see [XHR](XHR.md)),
  `navigator.storage` (`StorageManager`), and the [streams](STREAMS.md)
  (`ReadableStream`, `WritableStream`, `TransformStream`, the queuing
  strategies, `TextEncoderStream`/`TextDecoderStream`,
  `CompressionStream`/`DecompressionStream`, `response.body`), which, as on a
  window, are made on the realm's first use of one of their names. Everything
  else is undefined, including `document`, `indexedDB`, `FileReaderSync` and
  `Worker`.
  `atob`/`btoa`, `structuredClone`, `crypto`, `performance.now()`, `console`,
  `fetch` with `Headers`/`Response`/`FormData`/`Blob`/`File`, `AbortController`,
  `URL`/`URLSearchParams`, `TextEncoder`/`TextDecoder`, `MessageChannel`, `Event`
  and its message, error, progress and rejection subclasses, `XMLHttpRequest`
  without `responseXML` (see [XHR](XHR.md)), `WebSocket`/`CloseEvent` (see
  [WebSocket](WEBSOCKET.md)). Everything else is undefined,
  including `document`, `indexedDB` and `Worker`.
- **Script fetch.** On the worker's thread, blocking only it, through the page's
  context `NetClient` (cookies, interception, Network events) with the page's
  user agent, extra headers and cache setting. The URL must be same-origin with
  the document (SecurityError otherwise), or `data:`, or `blob:` (bytes captured
  at construction). Non-2xx or a failed load fires a plain `error` Event at the
  `Worker`. The MIME type is not checked. Requests carry the owner's frame id and
  no loader id: they appear on the owner's page and do not delay its network idle.
- **Messages.** `postMessage` validates and copies with the realm's original
  `structuredClone`, then encodes the copy to text plus ArrayBuffer bytes
  (`worker_wire.js`), which cross as a task of the other side's event loop and
  are rebuilt there; failing to rebuild fires `messageerror`. Transferring an
  ArrayBuffer copies it and detaches the sender's. Messages to a worker wait until
  its script has run. A navigation drops messages on their way to the old document.
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

## Not supported

- `new Worker(url, {type: 'module'})` throws NotSupportedError. `import()` in a worker
  rejects, since no module loader is installed there.
- `SharedWorker` and `ServiceWorker` stay undefined, as does `navigator.serviceWorker`:
  `typeof` feature detection is what sites use, and a constructor that throws would
  send them down a path that then fails.
- Transferring anything but an ArrayBuffer (MessagePort) throws NotSupportedError,
  as does cloning what `structuredClone` rejects (Blob, File and other host objects).
- Nested workers. `Worker` is undefined inside a worker.
- `importScripts` of a page-created `blob:` URL (the store is the owner document's).
- A worker whose wrapper became unreachable runs until its document goes.
- Virtual time (`Emulation.setVirtualTimePolicy`) and the page clock do not reach a
  worker: its timers and `Date` run on real time.
- Playwright reports only workers that attach after it has announced the page. The
  worker of a prepared page attaches in that window: it runs and gets its session, but
  Playwright's `page.on('worker')` does not fire for it.
- A worker started on a page prebooted by `serve --prepare` outside fork mode runs,
  but has no CDP target: the connection did not exist to record it.

Tests: `crates/lp-browser/src/tests.rs` (`worker_*`), `crates/lp-webapi/tests/dom_js.rs`
(`worker_*`), `tests/e2e/about-blank.mjs`.
