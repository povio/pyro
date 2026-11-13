# Stylesheet loading

The browser drains connected `<link rel=stylesheet>` resource events, resolves href against the document
URL, and uses the context ResourceLoader. A placeholder reserves the native style-engine cascade slot
before fetching, so response timing does not determine precedence. Successful 2xx `text/css` responses
supply decoded CSS; network, HTTP, MIME and limit failures dispatch link `error`. Completed links receive
`load` in an owner-thread task. `<style>` elements are [below](#style-elements).

Pending parser-created, matching-media sheets block parser classic scripts and deferred scripts. They
delay document load, but do not by themselves block DOMContentLoaded. A stylesheet inserted by a DCL
listener is included before window load. Parser scripts waiting for CSS currently start their external
script fetch after the CSS wait; parallel script prefetch remains future work. This follows the relevant
[HTML stylesheet/script lifecycle](https://html.spec.whatwg.org/multipage/semantics.html#the-link-element)
subset; it is not full HTML/CSS loading parity.

Requests carry the document frame/loader IDs. `crossorigin` selects CORS and anonymous/use-credentials
semantics with the document origin. Native HTTP tests verify allow/deny behavior. Integrity metadata is
explicitly unsupported and produces `error` without fetching. `<base>` resolution, the `@import`s of a
linked sheet (its `load` does not wait for them; the layout engine fetches them itself) and its other
critical CSS subresources, alternate stylesheet selection, and browser-native HTTPS remain unsupported.
`cssRules` parses the sheet's text on first access and reparses when the text changes: style rules, `@media`/`@supports` groups and other known at-rules, with invalid rules dropped as browsers do. Style matching does not use this parse. Missing: `CSSStyleRule.style`, the `media` and `disabled` CSSOM APIs, and a real `CSSRuleList` (`cssRules` is a live array). The loader
honors the disabled HTML attribute at resource boundaries; it does not claim a complete disabled IDL API.

Href/rel/type/disabled changes and removal are inspected using the final DOM state of a task. Replacing,
removing or navigating away from a link drops its cancellation handle. NetLoader sends a native cancel;
custom loaders may opt into `ResourceLoader::fetch_cancellable` or leave transport cancellation unsupported.
A late completion checks document and resource generations, and only holds a weak PageDom reference.
The retained native node pin and CSS accounting are released on removal/navigation; old completions cannot
update the replacement document or fire its events. Custom loader bodies are checked on delivery.

Limits per document: 128 retained stylesheet entries, 4 MiB decoded source per response, 16 MiB retained
CSS source total. Native requests also respect any smaller NetConfig body limit. Failed entries consume
an entry slot until changed or removed; source bytes are recovered on replacement/removal. These are
logical source/entry limits, not an RSS ceiling: native in-flight buffers, parsed rules and binding caches
have additional costs. `Page::stylesheet_usage()` reports pending requests and retained source bytes.

`Browser::checkpoint()` is the owner-thread safe point after synchronous host/inspector JavaScript,
with no outstanding DOM borrows. Event-loop tasks and BrowserThread run helpers call it automatically.
Protocol owners must call it after commands executed outside those tasks; otherwise dynamically inserted
links might wait for unrelated work. It is not a microtask checkpoint or MutationObserver implementation.

Tests cover delayed responses/order, parser/deferred/DCL/load timing, dynamic replacement, MIME/errors,
source and entry bounds/recovery, native CORS/correlation/cancellation, and old-document release during
pending work. No full TOS result or matched performance comparison is claimed here.

## `<style>` elements

HTML's "update a style block", as Chromium (147) runs it: a `<style>` is processed when it is inserted
into a document with a browsing context (shadow trees and frames included), when its text changes, when
a `type` change makes it CSS, and, for one a parser created, when the parser pops it (not before: the
`STYLE_PARSING` element flag marks it until then). A processed element whose `type` is absent, empty or
`text/css` gets `load` in a later task (DOM manipulation source; bubbles and cancelable false) once its
sheet applies. A `media` that does not match and the `disabled` attribute (not one of `<style>`'s) do not
matter, and a `media` change processes nothing; another `type` fires nothing. The event fires even if the
element was removed meanwhile. DOMContentLoaded waits for the events queued before it (HTML queues it as a
task), the document's `load` for all of them.

The `@import` rules the sheet starts with (`pyro_dom::cssom::leading_imports`: only before every other
rule but `@charset` and `@layer` statements; one whose `supports()` fails imports nothing; media do not
matter) are fetched through the context loader first (CDP sees them as stylesheets, Resource Timing as
`css`), then the imports of those sheets, 16 levels and 64 sheets at most per element. `load` comes once
all of them have arrived; `error` when an import of the element's own sheet failed (a network error or a
status outside 2xx; a wrong MIME type still loads, and a failed import of an imported sheet does not fail
the element, both as in Chromium). Each `text/css` body also goes to the bodies the layouts of the page's
network identity load (`SubresourceBodies::provide`), so the sheet applies with it when the event fires
instead of the layout fetching it again. While its imports load, a parser-inserted element whose `media` matches is a
script-blocking style sheet (the parser's next script waits, and sees the imported rules) and blocks
rendering (`blocking=render` does for any element). New text, a non-CSS `type` or removal drops the
pending imports and their event.

Differences from Chromium: several text changes in one task fire one event (Chromium fires one per
change); an element inserted and removed in the same task fires none (Chromium: `load`); elements of
documents without a browsing context (`createHTMLDocument`) fire none.

Tests: `crates/pyro-browser/tests/style_load.rs` (orders as Chromium gives them for the same markup) and
`packages/pyro/test/wright/style-load.pw.ts` (pyro/wright, and stock Playwright's `addStyleTag` over CDP).
