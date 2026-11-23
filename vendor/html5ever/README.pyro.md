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

An optional tree-builder hook reports an explicit HTML template end tag to a sink implementing content patching. It identifies the last open template after implied end tags. EOF stack unwinding leaves markers in place.

## The change: options popped off the stack of open elements

The HTML element popped off the stack of open elements steps of `<option>` run "maybe clone an option into
selectedcontent" (WPT html/syntax/parsing `webkit02.dat`). Upstream calls the sink only after an explicit `</option>`
(servo/html5ever#712), so an option closed by another option or by the end of the input never filled its
`<selectedcontent>`.

- `src/tree_builder/mod.rs`: `popped_off_stack` calls `TreeSink::maybe_clone_an_option_into_selectedcontent` for an
  HTML option. `pop`, `remove_from_stack`, `pop_until`, `pop_until_current`, the EOF drain in `end` and every
  truncation of the stack (`truncate_open_elems`: "any other end tag", adoption agency step 4, `<frameset>` in body)
  report each element they take off, the current node first. The adoption agency's inner-loop removals (steps 13.5 and
  13.6) remove rather than pop and do not report.
- `src/tree_builder/rules.rs`: the `</option>` special case is gone; the end tag goes through "any other end tag" like
  the specification's, and the pop reports it once.
- The sink (`pyro-dom` `parse.rs` → `Dom::maybe_clone_option_into_selectedcontent`) does the cloning; `blitz-html`
  keeps the trait's no-op.

Tests: `crates/pyro-dom/tests/html_selectedcontent_parsing.rs`.

## The change: SVG script elements yield to the script runner

Upstream leaves "FIXME(#118)": an SVG `<script>` in HTML never reached the sink as a script, so no SVG script in an
HTML document ran (WPT html/semantics/scripting-1/the-script-element/execution-timing `131`–`146`).

- `src/tree_builder/rules.rs`: in foreign content, an end tag `script` whose current node is an SVG script element pops
  it and returns `ProcessResult::Script`, as the `</script>` of an HTML script does in the text insertion mode.
- `src/tree_builder/mod.rs`: a self-closing `<script/>` in SVG is inserted, pushed and popped and returns
  `ProcessResult::Script` (the HTML Standard: acknowledge the flag and act as for the end tag).
- The sinks see the same `TokenizerResult::Script` as for an HTML script; drivers that ignore scripts keep resuming.

Tests: `crates/pyro-dom/tests/svg_script_parsing.rs`.

## Empty DOCTYPE system identifiers

`src/tree_builder/data.rs` (`doctype_error_and_quirks`): an HTML 4.01 Frameset or Transitional public identifier with an
empty system identifier gives quirks mode, as a missing one does (whatwg/html#12023, Chromium; WPT html/syntax/parsing
`doctype-system-identifier-distinction.html`); a non-empty one still gives limited quirks.
