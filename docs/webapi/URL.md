# URL

URL resolves relative references, parses and serializes components, exposes origin,
and implements canParse and parse using the existing url crate's browser quirks
API. Construction and href assignment reject invalid URLs; invalid component
assignments follow the component parser's no-op behavior. String conversion occurs
before native borrows. IDN, IPv6, opaque URLs, default ports, subclass construction
and JSON serialization have focused coverage.

searchParams returns the same wrapper across calls and worlds. href/search
assignment replaces its list in place; parameter mutations update the URL query
with form encoding. Existing iterators observe the current list. Native state is
shared through Rc, while URL/params wrapper identity uses traced JS links rather
than persistent self-roots. Retaining params keeps its URL wrapper alive; dropping
both permits collection. There is no polling or document scan. Serialization and
query replacement allocate proportional to the affected query; no performance or
full WPT conformance claim is made.

URL objects work with fetch's string conversion. Associated params retain the
existing native fetch/XHR BodyInit snapshot and Content-Type semantics.

`URL.createObjectURL(blob)` registers the Blob's bytes and type under
`blob:<origin>/<uuid>`. Opaque origins use `null` as the origin, as upstream does.
- The registry belongs to the creating document. `PageDom::blob_url` exposes it
  to embedders. Replacing the document through `PageDom::set_document` or
  disposing it drops the registry and its byte references.
- `revokeObjectURL` removes a URL of the caller's origin. Other strings are
  ignored silently.
- A non-Blob argument throws TypeError.
- Hyperlinks with a `download` attribute that target a `blob:`, `data:` or
  same-origin URL do not navigate. Cross-origin download links navigate, since
  browsers ignore the attribute there.

Hosts: ASCII hosts with invalid `xn--` labels pass through as in browsers, through
the patched `vendor/idna`. `<a>`/`<area>` read and write URL components with the
same setters as `URL`, against the `<base>`-aware document base; an unparseable
`href` reads as itself with protocol `:` and ignores component writes.
`navigator.sendBeacon` validates the URL and sends nothing.

Gaps:
- The download itself is not performed, and no CDP download events are emitted.
- `fetch`, XHR and navigation of `blob:` URLs are not implemented (upstream
  resolves them).

The reproduced application failure was Vite's preload helper calling new URL in
the [unchanged TOS query-parameters run](../validation/TOS_QUERY_PARAMS_SMOKE.md).
That failed run and its binary remain preserved. A fresh exact-candidate smoke
is required before claiming progress beyond this binding's tests.
