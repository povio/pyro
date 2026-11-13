# Browser loading and binding installation

`BrowserThread` registers the WebAPI classes and the Window host. A committed document owns one
`PageDom`; its main and isolated realms share the native DOM with separate JavaScript globals and
wrappers. Installation and the module fetcher precede init scripts, so those scripts can use the DOM,
Web Storage and fetch. DOM callbacks enter the owner browser thread and check the document loader ID.

`enable_network` supplies a NetClient per context. Document/script loads use the context loader;
WebAPI fetch uses that same client with the main frame and current document loader IDs. A context owns
its localStorage namespace, while a top-level page owns sessionStorage across navigation. Both are
in-memory and partitioned by origin. Opaque-origin documents (e.g. a blank page without an inherited
origin) get per-document ephemeral storage instead, as upstream does (Chrome throws); see PARITY.

Navigation calls `PageDom::dispose` before disposing old realms, cancels realm timers, marks the old
document inactive and installs fresh bindings. Closing a page performs the same binding/realm cleanup.
Fetch jobs, buffered bytes and retained JS roots are released by the WebAPI lifecycle; replacing one
page does not shut down the context's network client. Queued old-document binding tasks are discarded. Location/history tasks and
same-document protocol notifications also check their originating document generation. A queued history
traversal whose target entry was replaced is discarded, and out-of-range history deltas are no-ops.
Every new navigation invalidates the previous document fetch, including synchronous `about:blank`
navigation; late success/error completions cannot replace the new document or emit stale navigation
events. History replacement intent travels with its own request, so failed or superseded replacements
cannot overwrite the next navigation's history entry. This invalidates delivery; it does not yet cancel
document transport (unlike cancellable linked stylesheet requests).

The document loader parses a fetched body incrementally around script pauses. Classic scripts receive
`document.currentScript`, including deferred elements removed from the live tree. Module scripts see
null. Parser-blocking, deferred and async scripts are supported; async module dependency evaluation
finishes before load. `DOMContentLoaded` and load dispatch to the document/window bindings with
interactive/complete ready states. As HTML says, a document's `load`, `pageshow`, `pagehide`, `unload`
and `beforeunload` are fired at its Window with the legacy target override flag: the Window's
listeners run at the target phase (it is the whole path) while `event.target` is the document, as in
Chromium; `pageshow` and `pagehide` bubble and are cancelable. Deferred modules currently wait for full evaluation, including
TLA, before DCL (a documented JSC integration deviation).

Linked stylesheets now load through the context client; see [stylesheet behavior and limits](STYLESHEETS.md).
This increment does not load images, iframes or script-inserted script resources,
and does not dispatch script-element load/error events. Inline style resource events are applied, and
`<style>` elements get `load`/`error` ([STYLESHEETS](STYLESHEETS.md#style-elements)).
History state is JSON rather than structured clone; popstate/hashchange remain absent. Form POST
navigates the frame; POST to a new window and named browsing targets fail explicitly.

A navigation whose document request fails on the network reports `FrameNavigateFailed` (CDP
`errorText`). In a new window, a top-level frame still showing its first about:blank document (a
popup, a new page), any network error but an aborted navigation, an unknown scheme or an invalid URL
then commits Chrome's error document under the same loader: an empty document at
`chrome-error://chromewebdata/` (`ERROR_DOCUMENT_URL`) with an opaque origin, whose frame keeps the
URL it could not reach (`Frame::unreachable`, CDP `unreachableUrl`) and whose history entry has that
URL, so a reload fetches it again. A frame with a document of its own keeps it, and a child frame's
failure leaves its initial document (Chrome commits error pages in both). Native HTTPS
remains unsupported. See [parity](../PARITY.md)
and [runtime limits](../COMPATIBILITY.md). No TOS compatibility or matched TOS performance result is
claimed by these local HTTP and synthetic-loader tests.

`pyro fetch` loads a URL, executes scripts and optionally dumps the resulting DOM. Its deadline
(`--timeout-ms`, 30 s by default) interrupts running JavaScript during parsing and post-load timers.
Without `--wait-ms` the DOM is dumped at `load`. `--wait-ms N` keeps the page running after `load`
until nothing is left that can still change it, for N ms at most: no keep-alive timer (a
`setTimeout`, `setInterval` or animation frame) and the page quiet as `Pyro.idle` waits for it
(`Browser::page_work`): no queued task, no request in flight (one that a response's callback starts
counts from its start), no navigation short of DOMContentLoaded, no ResizeObserver delivery. A page
that polls never gets there and is dumped after N ms. `pyro eval` runs its page the same way until it
is quiet, within its `--timeout-ms`; that page is about:blank, an opaque origin, so a cross-origin
response it fetches needs CORS headers. Both end quietly with status 0 when the reader of their
output goes away (EPIPE, `pyro fetch URL | head -1`).

Tests: `pyro-browser` loading/bindings/fetch integration suites cover script order, lifecycle,
context/page storage partitions, shared-DOM isolated worlds, stale callbacks, fetch correlation and
navigation cleanup. `pyro` CLI tests check script-modified output, infinite-script timeout,
`--wait-ms` against data fetched after `load` (also by a chain of requests, a later timer and a page
that polls), `eval` waiting for its fetch, and a closed stdout.
The merged CDP host enables this loader and supplies Network/Fetch/cookie protocol routing. This
increment changes no protocol source; DOM/Input protocol integration remains separate automation work.

Page-local user-agent, language and platform overrides persist across document
replacement; see [USER_AGENT.md](USER_AGENT.md) for loader/default semantics.
