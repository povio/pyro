# Linked stylesheet loading

The browser drains connected `<link rel=stylesheet>` resource events, resolves href against the document
URL, and uses the context ResourceLoader. A placeholder reserves the native style-engine cascade slot
before fetching, so response timing does not determine precedence. Successful 2xx `text/css` responses
supply decoded CSS; network, HTTP, MIME and limit failures dispatch link `error`. Completed links receive
`load` in an owner-thread task. Inline `<style>` processing remains unchanged.

Pending parser-created, matching-media sheets block parser classic scripts and deferred scripts. They
delay document load, but do not by themselves block DOMContentLoaded. A stylesheet inserted by a DCL
listener is included before window load. Parser scripts waiting for CSS currently start their external
script fetch after the CSS wait; parallel script prefetch remains future work. This follows the relevant
[HTML stylesheet/script lifecycle](https://html.spec.whatwg.org/multipage/semantics.html#the-link-element)
subset; it is not full HTML/CSS loading parity.

Requests carry the document frame/loader IDs. `crossorigin` selects CORS and anonymous/use-credentials
semantics with the document origin. Native HTTP tests verify allow/deny behavior. Integrity metadata is
explicitly unsupported and produces `error` without fetching. `<base>` resolution, CSS imports and
critical CSS subresources, alternate stylesheet selection, and browser-native HTTPS remain unsupported.
Media/disabled CSSOM behavior and fetched cssRules synchronization are separate binding work. The loader
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
