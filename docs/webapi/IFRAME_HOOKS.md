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

`PageHooks::frame_detached()` (default false) tells the bindings that the frame
showing the document was removed from its page; no JS runs in it. Script of such
a document can still be on the stack (`frameElement.remove()` returns to it), but
the document is no longer fully active: `PageDom::is_fully_active()` is false,
event-listener invocation is suppressed as for disabled scripting (HTML "check if
we can run script"), and `Observable`/`Subscriber`/`EventTarget.when()` do
nothing.

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

Nodes of a same-origin child are usable from the parent. A frame tree shares one
`Dom`, and a wrapper belongs to one realm and PageDom. `lp-js` runs a host member
or property hook in its receiver's realm: JSC calls property hooks in the caller's
realm and `Node.prototype.x.call(childNode)` in the callee's, so results, errors
and listeners stay with the receiver. This costs one realm-id comparison per call.
A node that moved between documents keeps its wrapper within a script world:
`PageHooks::sibling_realms` lists that world's realms in the other frames, searched
only when a realm has no wrapper yet, and empty for a page without child frames.
A node's wrapper is made in the realm of the document the node was created in
(DOM "create a node"), whichever realm asks for it first and whichever document
it has been adopted into: `Dom::creation_document` records that document when a
node moves to a document of another window, and a document without a browsing
context (`createHTMLDocument`, DOMParser, `new Document()`, template contents)
counts for the window whose script created it. A node whose creating window is
unknown or gone is wrapped in the realm that asks. A removed frame's realms stay
in `sibling_realms` until they are disposed, so its nodes keep one wrapper while
its script can still run.
An Event keeps its object the same way, and dispatch runs listeners in any realm of
the tree. After a child navigates, its old indexed and named properties, and direct
calls through its old prototypes, throw `Execution context was destroyed`.

Known gaps: a node adopted out of a child stays bound to the child's realm, so it
throws once that child navigates, although it lives on in the parent. Windows from
`window.open` have their own `Dom` and passing their nodes as arguments is not
supported. Observers on another document's nodes have no dedicated coverage.

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
