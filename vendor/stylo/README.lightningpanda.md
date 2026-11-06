# stylo 0.22.0, patched

This is the published `stylo` 0.22.0 crate (crates.io), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: MPL-2.0, as upstream. Only `.cargo_vcs_info.json`, `.cargo-ok` and
`Cargo.toml.orig` were left out of the copy. `UPSTREAM.patch` is the whole diff against the published
crate (`diff -ruN`).

## The change

- `rule_cache.rs`, `properties/helpers.mako.rs`: a style that inherits a reset property explicitly
  (`inherit` on a non-inherited property) stays in the rule cache when the property is one of
  `box-sizing`, `baseline-shift`, `baseline-source` and `alignment-baseline`: the parent's value is
  recorded as a cache condition (`InheritedReset`), as font-size and writing-mode already are, and a
  cached style serves an element whose parent has the same value. Upstream made such styles
  uncacheable. `*, ::before, ::after { box-sizing: inherit }` (MUI's CssBaseline and many resets) is
  on every element, and the UA's `td, th, tr { vertical-align: inherit }` on every table cell, so the
  rule cache was nearly dead on such pages: 195 hits of 46218 lookups on a ToS invoice (80% after),
  5.5% on a long ToS CMR (71% after). The computed values are the same (44 ToS PDFs byte-identical).

- `properties/longhands.toml`, `properties/shorthands.toml`: `break-before`, `break-after` and
  `break-inside` (and the legacy `page-break-*` shorthands) are parsed and computed for servo too
  (upstream: gecko only). lp-render's PDF output reads them to place page cuts of paper-cut
  documents. Nothing else reads them; computed values of other properties are unchanged.

- `values/specified/box.rs`: `break-before` and `break-after` also take `avoid-page` and
  `avoid-column`, as the spec has them (upstream has only `avoid`, and drops the declaration).
  `page-break-*` still rejects them.

- `properties/longhands.toml`, `properties/counted_unknown_properties.py`: `orphans` and `widows`
  are added (upstream has neither; both were counted unknown properties), inherited, in
  `inherited_text`. lp-render's PDF output reads them to place page cuts of paper-cut documents.

- `stylist.rs`, `style_resolver.rs`: each origin's `CascadeData` also files its single-level
  `::before`/`::after` rules that declare `content`, `display` or `animation-name` (on the
  block's longhand set, so `all`, `var()` and CSS-wide keywords count) in a second pair of
  `SelectorMap`s, filled by `add_styles` next to `normal_rules` and cleared with it.
  `match_pseudo` (unvisited pass, not `PseudoElementResolution::Force`) matches only those first
  and returns `None` when none matches: every other rule leaves `content` at `normal` without
  setting `DISPLAY_OR_CONTENT_DEPEND_ON_INHERITED_STYLE`, so the matched and cascaded style was
  dropped by `eager_pseudo_is_definitely_not_generated` anyway. Universal resets
  (`*, ::before, ::after { box-sizing: ... }`, Tailwind's `--tw-*` block) made every element
  match and cascade both pseudo-elements. Elements that shadow-tree, `::slotted()`, `:host` or
  `::part()` rules for the pseudo-element can reach take the full path. A CSSOM declaration
  edit rebuilds the index with the rules (`RuleChangeKind::StyleRuleDeclarations` is
  `FullyInvalid`); debug builds check, on every skip, that no matched rule could generate a box.

## Updating or dropping the patch

- When upstream has the change: delete this directory and the `[patch.crates-io]` entry.
- To move to a newer stylo: copy the published crate over this directory and apply `UPSTREAM.patch`.
