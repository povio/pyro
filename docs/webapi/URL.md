# URL

URL resolves relative references, parses and serializes components, exposes origin,
and implements canParse and parse using the url crate's browser quirks API.
Construction and href assignment reject invalid URLs; invalid component
assignments follow the component parser's no-op behavior. String conversion occurs
before native borrows. IDN, IPv6, opaque URLs, default ports, subclass construction
and JSON serialization have focused coverage.

## Conformance

The url crate is `vendor/url`: url 2.5.8 (the latest release) patched to the current
WHATWG URL Standard; [its README](../../vendor/url/README.lightningpanda.md) lists the
changes. Every URL in the engine goes through it (`URL`, `<a>`/`<area>`, `Location`,
navigation, fetch, the network stack), so they all agree:
- `file:` URLs keep a host next to a Windows drive letter (`file://host/C:/`), take the
  base's host for relative and drive-letter inputs, keep empty leading path segments
  (`file:////x`), and `..` stops only at a lone drive letter of a `file:` URL.
- In special URLs every slash and backslash before the authority is skipped
  (`///x` against an http base is `http://x/`). In other URLs a backslash is a path
  character.
- The path percent-encode set includes `^`. A space before `?` or `#` in an opaque path
  is `%20`, and removing the query or fragment no longer strips trailing spaces.
- The port setter ignores values that are empty only after tab/newline removal; the
  pathname setter keeps an empty path under an empty host (`foo://`) and maintains the
  `/.` prefix of host-less URLs whose path starts with an empty segment.
- `blob:` URLs have a tuple origin only for an `http(s):` inner URL.

`crates/lp-webapi/tests/url_conformance.rs` runs WPT's `urltestdata.json`,
`setters_tests.json` and `toascii.json` without a WPT run: a subset committed under
`crates/lp-webapi/tests/data/url/`, and the full upstream files when a WPT checkout is
found (`WPT_DIR`). It checks the Rust calls the bindings make, and the subset again through
the JavaScript `URL`, `URL.canParse` and `<a>` (parsing against `<base>`, setters,
`toascii`). Results with url 2.5.8 before the patch, and after:

| Data (upstream) | Before | After |
| --- | --- | --- |
| `urltestdata.json` | 835/896 | 896/896 |
| `setters_tests.json` | 258/278 | 278/278 |
| `toascii.json` (URL, host and hostname setters) | 261/261 | 261/261 |

The committed subset: 410/470, 229/249 and 261/261 before; all pass after. Through the
JavaScript bindings, 2433 checks pass. `toascii` already passed through the patched
`vendor/idna`.

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
`navigator.sendBeacon` validates the URL and sends the beacon (see [Fetch](FETCH.md)).

Gaps:
- `IdnaTestV2.json` and `percent-encoding.json` (legacy-encoding queries) are not in the
  Rust conformance test; CI's WPT run covers them.
- `Location` (in `lp-browser`) parses with the same crate and its setters call
  `url::quirks`, but since its setters navigate it is not run against
  `setters_tests.json`; `lp-browser/tests/file_urls.rs` covers a `file:` URL with an
  empty leading segment through navigation and `location`.
- The download itself is not performed, and no CDP download events are emitted.
- Top-level navigation to a `blob:` URL is not covered by tests. The page's
  resource loader serves `blob:` URLs to scripts, `import()`, stylesheets and
  iframes from the store of a same-origin document of the page (a parent's
  blob as a child frame's `src`); fetch and XHR resolve them in the
  document's own store.

The reproduced application failure was Vite's preload helper calling new URL in
the [unchanged TOS query-parameters run](../validation/TOS_QUERY_PARAMS_SMOKE.md).
That failed run and its binary remain preserved. A fresh exact-candidate smoke
is required before claiming progress beyond this binding's tests.
