# blitz-html, Blitz main (c27b4ca), unchanged

`packages/blitz-html` of [DioxusLabs/blitz](https://github.com/dioxuslabs/blitz) main at commit
`c27b4ca14158dc457f35c78eb32b8c4fec906b7e`, as `vendor/blitz-dom` (see its README.pyro.md), which
needs this version. No local changes: `src/` is upstream's, and `Cargo.toml` is the package manifest with the
workspace's dependency versions written in. Drop it (and its `[patch.crates-io]` entry) once a Blitz release
carries these commits.
- `html_sink.rs` (`HtmlProvider::parse_svg_document`): parses an SVG image's source as XML (blitz-dom
  `svg_image_documents`).

- Round 5, `src/html_sink.rs` (`create_comment`): the data of a comment that started with `<?` carries the mark of the
  vendored html5ever (`pi::MARK`, vendor/html5ever/README.pyro.md); it is stripped. The render document keeps such
  markup as a comment.

- Round 6, `src/html_sink.rs` (`create_element`, `elem_name`): a render snapshot writes an HTML `<xmp>` or
  `<plaintext>` as `<pre data-pyro-tag="xmp|plaintext">` (pyro-dom `snapshot::TAG_ATTRIBUTE`), so the reparse ends it
  where the DOM does (a `<plaintext>` would make the rest of the markup its text). The sink creates the element under
  its own name, without the attribute, and answers the tree builder with `pre` for it (its end tag, scope checks).
