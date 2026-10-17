# DOMParser

`new DOMParser().parseFromString(string, "text/html")` returns a new HTML
`Document` parsed by the HTML parser.

- Scripting is disabled while parsing, so `<script>` elements are inserted but
  never run.
- The document's URL is the URL of the window's document, and its `contentType`
  is `text/html`.
- Its nodes can be imported or adopted into the window's document.

Rich-text editors such as tiptap/ProseMirror depend on this path. TOS's
`TextEditor` chunk calls it when creating an editor from HTML content.

Not implemented: the XML types (`text/xml`, `application/xml`,
`application/xhtml+xml`, `image/svg+xml`). There is no XML parser yet, so these
throw NotSupportedError; upstream parses them, and malformed input yields a
`parsererror` document. Any other type string throws TypeError, as for a Web IDL
enum mismatch.

Tests: `dom_parser_parses_inert_html_documents` in
`crates/lp-webapi/tests/dom_js.rs`.
