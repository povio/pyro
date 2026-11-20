# html5ever 0.40.1, patched

The published `html5ever` 0.40.1 crate (crates.io; [servo/html5ever](https://github.com/servo/html5ever)), used through
`[patch.crates-io]` in the workspace `Cargo.toml`. License: MIT OR Apache-2.0, as upstream. `.cargo_vcs_info.json`,
`Cargo.toml.orig`, `Cargo.lock`, `benches/`, `data/`, `examples/` and `tests/` were left out of the copy, and the
matching example, test, bench and dev-dependency sections were removed from `Cargo.toml`.

## The change: processing instructions in HTML

`<?target data>` in HTML is a processing instruction (WPT html/syntax/parsing `processing-instructions.dat`, as
Chromium; upstream reads it as a bogus comment, the HTML Standard's older rule).

- `src/tokenizer/mod.rs`: a bogus comment that starts with `<?` is remembered (`comment_from_pi`) and its token's data
  is emitted with a leading `pi::MARK` (U+0000, which no comment's data holds: the tokenizer replaces NUL). When the
  input ends inside one that reads as a processing instruction so far, nothing is emitted (a bogus comment is).
- `src/pi.rs` (new): `parse` tells whether the data is a processing instruction and gives its target and data: a target
  of ASCII letters, digits, `-` and `_` that starts with a letter or `_` and not with "xml" (any case), followed by
  white space and the data, by a `?` that starts the data, or by nothing; one `?` at the end of the data is the
  instruction's own.
- The tree builder is untouched: the marked comment goes where a comment goes. The tree sinks (`pyro-dom`
  `parse.rs`, `blitz-html` `html_sink.rs`) strip the mark in `create_comment`; pyro-dom makes the processing instruction
  node when `pi::parse` gives one.

Tests: `src/pi.rs` (unit), `crates/pyro-dom/tests/html_processing_instructions.rs`.

Fragment sanitization supplies an optional element-removal predicate to the
adoption agency algorithm. Removed formatting elements and furthest blocks
participate in parser bookkeeping without being reinserted into the output.

The form token rules recognize both open templates and a template fragment
context as parsing template contents. In that context forms are inserted
without changing the form element pointer. Outside templates, a fragment's
nearest inclusive ancestor form still suppresses nested forms.
