# Iframe lifecycle hooks

The browser owns child browsing contexts. `lp-webapi` exposes these default-noop
`PageHooks` methods, all on the owner thread and outside DOM/hook borrows:

- `iframe_connected(NodeId)`: create the initial blank browsing context
  synchronously. An inserting native binding returns only after this callback.
- `iframe_disconnected(NodeId)`: tear down the owner's child context. Removing a
  subtree reports descendant owners before ancestor owners.
- `iframe_attribute_changed(NodeId, IframeAttribute)`: connected `src`, `srcdoc`,
  `name` or `sandbox` changed, including removal and reflected/token-list setters.
  The callback reads current attributes; detached changes are observed by the
  eventual connected callback. Other attributes do not trigger this hook.

`IframeAttribute` is exported from both `lp_dom::iframe` and `lp_webapi`.
`PageDom::flush_iframe_events()` is the explicit parser/native mutation boundary:
call it after releasing the DOM borrow and before entering page script. Installed
native callback hooks and `PageDom::after_task()` also flush. Reentrant mutations
are processed by the outer flush; attempts under a held DOM borrow defer until a
safe point. Initial parsed frames are queued when a document/host is registered.

The native queue is separate from loader resource events and routed by document,
so one frame's PageDom cannot consume another's notifications. Only registered
browsing-context documents produce events, not inert DOMParser documents.
Queued owners stay pinned through callbacks, including callbacks that collect
DOM garbage. Stale disconnected connection events are skipped. Replacement and
disposal drop pending notifications and release pins. The browser remains
responsible for tearing down already-created child contexts on parent navigation
or disposal; `set_document` does not call old-host teardown callbacks.

`PageDom::set_scripting_disabled(bool)` and `scripting_disabled()` hold the
browser's sandbox policy. Disabled WebAPI event-listener invocation is suppressed,
`on*` content-attribute compilation is skipped, and `javascript:` navigation is
not forwarded. Explicit inspector/Realm evaluation remains available. Parser
scripts, dynamic scripts and timers require browser-side checks. Existing gaps
in parser-created inline event handlers remain unchanged.

These hooks do not implement iframe navigation, origin isolation, WindowProxy,
child realms, child lifecycle/load events, layout, or input routing. Those are
browser/protocol integration work described in the browser frame design.

`PageHooks::frame_document_value(scope, iframe)` returns the actual child-world
Document wrapper, preserving the child's PageDom, network, security and observer
state. `Some(null)` explicitly denies access and does not fall back. `None` opts
into the legacy `frame_document` NodeId lookup, which wraps using parent binding
state and is only suitable for legacy embedders sharing that state. The browser
must enforce origin access before returning a child document. `frame_window`
continues to supply the browser-owned WindowProxy.

For srcdoc/initial blank documents, `Dom::set_document_base_url(doc, Some(base))`
sets a browser-validated inherited base independently of the document URL.
`document_base_url`, `Node.baseURI`, URL attributes, link/form resolution,
fetch/XHR and Response.redirect use that base; `None` resets it to the
current document URL. Empty form actions still use the document URL. The setter
does not change origin, cookie scope or storage scope. `<base href>` takes
precedence over it and over the document URL (`document_fallback_base_url` skips
it); the result is recomputed after a DOM mutation, and only in a DOM that has
created a `<base>` element. Browser resource loaders must use the same inherited
base and explicitly supply the inherited or opaque RealmOptions origin.

An explicit realm origin of `"null"` makes document.cookie reads/writes throw
SecurityError and document.domain return an empty string, including HTTP
sandbox documents. Existing storage and fetch/XHR origin handling already
honors this explicit value. Cookie inheritance for blank/srcdoc documents is
not implemented by the base URL setter.

Validation: 600 workspace tests; 121 DOM binding and 27 local-server fetch tests
in release; strict workspace/all-target Clippy and formatting. Executables were
explicitly signed with the repository's allow-jit entitlement. Coverage includes
synchronous connection, subtree/removal/reinsertion, inert documents, native
borrow deferral, queue teardown/GC, attributes, sandbox scripting, child-wrapper
ownership, inherited URL resolution and opaque HTTP origin isolation. These are
module tests, not child-frame browser integration or new TOS compatibility runs.
