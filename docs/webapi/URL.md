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
existing native fetch/XHR BodyInit snapshot and Content-Type semantics. Object URL
creation/revocation throw NotSupportedError; Blob URL registration is unsupported.

The reproduced application failure was Vite's preload helper calling new URL in
the [unchanged TOS query-parameters run](../validation/TOS_QUERY_PARAMS_SMOKE.md).
That failed run and its binary remain preserved. A fresh exact-candidate smoke
is required before claiming progress beyond this binding's tests.
