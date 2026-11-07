# stylo 0.23.0, patched

This is the published `stylo` 0.23.0 crate (crates.io, servo/stylo commit
`2dca34592ced37c6aa01733e94206a767194cf9d`, the one Blitz main pins), used through `[patch.crates-io]` in the
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

- `invalidation/element/invalidation_map.rs`: `AdditionalRelativeSelectorInvalidationMap::state_dependencies`
  is the union of the element states the relative selector map depends on (every `:has()` argument's
  pseudo-classes), kept as dependencies are added and reset by `clear`. Blitz reads it to tell with one
  flag test that a state change cannot change any `:has()` match before it runs the relative selector
  invalidation (`vendor/blitz-dom/src/has_invalidation.rs`).

- `servo/selector_parser.rs`: `:in-range` parses (upstream has the pseudo-class, its state flag and
  serialization, but left it out of the parser, so a rule using it was dropped).

## Container queries, typed values

- `stylesheets/rule_parser.rs`, `properties/shorthands.rs`, `properties/shorthands.toml`: `@container` rules
  and the `container` shorthand are parsed for servo too (upstream: gecko only). The shorthand's
  `servo_pref` is `layout.css.container-queries.enabled`: upstream names a pref that no longer exists in
  0.23 (`layout.container-queries.enabled`), which only compiles while the shorthand is gecko-only.
- `stylist.rs`: `Stylist::has_container_queries` tells blitz-dom whether a restyle has container
  conditions to evaluate.
- `values/generics/mod.rs`: `GreaterThanOrEqualToOne` derives `ToTyped` (for `orphans`/`widows`).

## Moving from 0.22 to 0.23

The patch applied to 0.23.0 with offsets only. Two adjustments: 0.23 names the by-value getters of
non-`Copy` values `slow_clone_*`, so the rule cache's inherited resets (`rule_cache.rs`,
`properties/helpers.mako.rs`) call `slow_clone_box_sizing` and friends; and the `container` shorthand's
pref name above.

## Updating or dropping the patch

- When upstream has the change: delete this directory and the `[patch.crates-io]` entry.
- To move to a newer stylo: copy the published crate over this directory and apply `UPSTREAM.patch`.
