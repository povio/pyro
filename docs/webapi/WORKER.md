# Worker (stub)

`crates/lp-webapi/src/worker.rs` defines `Worker` so pages that construct one at
startup do not die on `ReferenceError`. It is a stub: **no worker ever runs**.
Lightningpanda has no worker threads, and no script is fetched or executed. The
object behaves like a worker whose script failed to load, which pages already
have to handle.

- **Construction.** `new Worker(url, options)` resolves `url` against the
  document base and throws SyntaxError if it cannot be parsed. `options` is
  ignored.
- **Failure.** A later task fires a trusted, non-bubbling, non-cancelable plain
  `Event` named `error` at the object, once, the same event shape as a failed
  script load. It does not reach `window.onerror`.
- **`postMessage`.** Requires one argument, then drops it. The message is not
  structured-cloned, so a value Chrome rejects with DataCloneError is silently
  accepted, and transfer lists are ignored (buffers are not detached).
- **`terminate()`.** Cancels a pending `error` event. Calling it again, or after
  the event, has no effect.
- **Handlers.** `onmessage`, `onmessageerror` and `onerror` work as attributes.
  `message` and `messageerror` never fire on their own.
- **Lifetime.** The wrapper is pinned until the error fires or `terminate()` is
  called, so `new Worker(url).onerror = f` reports with no JS reference. After
  that it is collectable.

Consequences:
- Code that waits for a worker reply and has no `error` path waits forever.
- `typeof Worker` is now `"function"`, so sites that feature-detect will try a
  worker and get the `error` event rather than taking their no-worker path.
- Playwright sees no `worker` event and `page.workers()` stays empty.

Not implemented: `SharedWorker` is undefined (its `port` MessagePort and
connect model do not fit this class), as are `ServiceWorker` and
`navigator.serviceWorker`.

Tests (`crates/lp-webapi/tests/dom_js.rs`):
- `worker_stub_validates_its_url_and_reports_a_load_failure_without_running`
- `worker_stub_reports_without_a_reference_and_is_then_collectable`
