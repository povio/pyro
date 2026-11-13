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
interactive/complete ready states. Deferred modules currently wait for full evaluation, including
TLA, before DCL (a documented JSC integration deviation).

Linked stylesheets now load through the context client; see [stylesheet behavior and limits](STYLESHEETS.md).
This increment does not load images, iframes or script-inserted script resources,
and does not dispatch script-element load/error events. Inline style resource events are applied, and
`<style>` elements get `load`/`error` ([STYLESHEETS](STYLESHEETS.md#style-elements)).
History state is JSON rather than structured clone; popstate/hashchange remain absent. Form POST
navigates the frame; POST to a new window and named browsing targets fail explicitly. Native HTTPS
remains unsupported. See [parity](../PARITY.md)
and [runtime limits](../COMPATIBILITY.md). No TOS compatibility or matched TOS performance result is
claimed by these local HTTP and synthetic-loader tests.

`pyro fetch` loads a URL, executes scripts and optionally dumps the resulting DOM. Its deadline
interrupts running JavaScript during parsing and post-load timers. `--wait-ms` runs pending work until
idle or the requested interval/deadline; it is not a network-idle guarantee.

Tests: `pyro-browser` loading/bindings/fetch integration suites cover script order, lifecycle,
context/page storage partitions, shared-DOM isolated worlds, stale callbacks, fetch correlation and
navigation cleanup. `pyro` CLI tests check script-modified output and infinite-script timeout.
The merged CDP host enables this loader and supplies Network/Fetch/cookie protocol routing. This
increment changes no protocol source; DOM/Input protocol integration remains separate automation work.

Page-local user-agent, language and platform overrides persist across document
replacement; see [USER_AGENT.md](USER_AGENT.md) for loader/default semantics.
