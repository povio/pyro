# DOMParser

`new DOMParser().parseFromString(string, type)` returns a detached, inert document
with the calling window's document URL. `text/html` uses the existing HTML
parser. `text/xml`, `application/xml`, `application/xhtml+xml` and `image/svg+xml`
use the bounded XML 1.0 parser and preserve the requested contentType. Other
MIME strings throw TypeError.

XML parsing preserves qualified names, namespace declarations and namespaced
attributes, case, text, predefined/numeric character references, CDATA, comments,
and processing instructions. Default namespace resets and attribute whitespace
normalization are supported. The DOM's namespace-aware tag queries now filter
namespace and local name, including wildcards; qualified tag queries distinguish
prefixes and preserve XML case. Namespace lookup follows ancestor declarations.

Malformed input returns an XMLDocument whose root is `parsererror` in
`http://www.mozilla.org/newlayout/xml/parsererror.xml`. The parser validates into
a bounded temporary representation before allocating DOM nodes, so failures do
not leave partial arena trees. No fetch, script execution, resource registration,
or active browsing context occurs during parsing. Parsed scripts remain marked
already-started when adopted. Existing import/adoption APIs apply to the result.

Limits per parse:

- Input: 8 MiB of UTF-8 after JS string conversion.
- Nodes: 100,000; element depth: 256.
- Attributes per element: 256; in-scope namespace declarations: 256.
- Qualified names: 1,024 bytes.

Limit violations, DTD declarations (including external/internal entity
processing), and XML 1.1 declarations produce parsererror documents. The parser
never performs I/O. DOMParser receives Unicode strings, so an encoding declaration
does not reinterpret their bytes. Lone JS UTF-16 surrogates retain the existing
Rust-string conversion limitation.

This increment targets the DOMParser path used by docx-preview 0.3.7. It does not
add XML document navigation, XHR responseXML or XML innerHTML parsing; the existing
HTML-oriented serialization path is unchanged. `XMLSerializer` (which Playwright's
`page.content()` calls for the doctype) writes declarations where the in-scope ones do
not cover a name but does not generate prefixes (`ns1`) or check well-formedness. It does not establish a passing DOCX preview or new TOS coverage.

Implementation: `lp-dom/src/xml.rs` uses the
[quick-xml event reader](https://docs.rs/quick-xml/0.42.0/quick_xml/reader/struct.Reader.html)
with explicit document, namespace, declaration, character and resource-limit
validation. Only `lp-dom/Cargo.toml` and the corresponding lockfile entry change;
no root dependency policy or existing dependency versions change.

Tests: native `crates/lp-dom/tests/xml.rs` covers namespace/entity normalization,
malformed inputs, limits and failed-parse collection. The DOMParser binding tests
in `crates/lp-webapi/tests/dom_js.rs` cover Office-shaped qualified/namespaced
queries, live collections, MIME types, CDATA, inert adoption, and argument
conversion reentry. Existing HTML parser tests remain in place.

Validation: 261 owned DOM/WebAPI tests; signed release XML parser 6/6 and DOM
bindings 114/114; strict workspace/all-target Clippy, formatting and diff checks.
Release debug information was disabled; thin LTO remained enabled. No full
workspace runtime suite or new TOS pass is claimed for this separate increment.
