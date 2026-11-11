# stylo 0.24.0, patched

This is the published `stylo` 0.24.0 crate (crates.io, servo/stylo commit
`32359dc3f17780ee269980cb7a3a7f8597a1dc0e`; Blitz main still pins 0.23.0), used through `[patch.crates-io]` in the
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
  block's longhand set, so `all`, `var()` and CSS-wide keywords count; `content: normal | none`
  and `animation-name: none` do not, so lightningpanda's UA
  `*::before, *::after { animation: none !important }` stays out) in a second pair of
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

- `invalidation/element/invalidation_map.rs`: `AdditionalRelativeSelectorInvalidationMap::state_dependencies`
  is the union of the element states the relative selector map depends on (every `:has()` argument's
  pseudo-classes), kept as dependencies are added and reset by `clear`. Blitz reads it to tell with one
  flag test that a state change cannot change any `:has()` match before it runs the relative selector
  invalidation (`vendor/blitz-dom/src/has_invalidation.rs`).

- `properties/data.py`: a longhand without an explicit `servo_restyle_damage` gets the damage its `affects` implies
  (`layout` → `rebuild_box`, `overflow` → `recalculate_overflow`, `paint` and `""` → `repaint`). Upstream gave every
  such property `rebuild_box`, so a change of `border-top-color`, `visibility`, `box-shadow`, a border radius, a mask
  or `scrollbar-color` was relayout damage (`RELAYOUT`), and Blitz relaid out and rebuilt boxes for it. Explicit
  annotations are unchanged. `servo/restyle_damage.rs`: `backdrop-filter` (now `recalculate_overflow`) makes a
  containing block for fixed boxes as `filter` does, so a change between none and some stays `RELAYOUT`
  (`augmented_restyle_damage_rebuild_box`). A `mask-image` (now `repaint`) makes a stacking context in Blitz's paint tree
  (`applies_atomic_paint_effect`), so a change between none and some is `REBUILD_STACKING_CONTEXT`
  (`augmented_restyle_damage_rebuild_stacking_context`, `has_mask_image`). Blitz's `compute_layout_damage` runs only on `RELAYOUT`, so this is what
  lets a paint-only restyle of a table cell run no layout (`vendor/blitz-dom/README.lightningpanda.md`, "Paint-only
  restyles run no layout and shape no text").

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
pref name above. The `::before`/`::after` index (`stylist.rs`, `style_resolver.rs`) was written
on 0.22 and ported onto 0.23's `stylist.rs` as is.

## Moving from 0.23 to 0.24

The patch applied to 0.24.0 but for one hunk: `:in-range` parses upstream now, so that change is gone.
0.24 lends applicable declarations instead of cloning them (`ApplicableDeclarationBlock<'a>`,
`ApplicableDeclarationList<'a>`), which the `::before`/`::after` index follows:
`Stylist::may_generate_box_for_pseudo` borrows the stylist for the scratch list's lifetime, and the
debug check in `style_resolver.rs` passes its `AnimationDeclarations` by reference. blitz-dom's side of
the move (presentational hints the element owns) is in `vendor/blitz-dom/README.lightningpanda.md`.

## Updating or dropping the patch

- When upstream has the change: delete this directory and the `[patch.crates-io]` entry.
- To move to a newer stylo: copy the published crate over this directory and apply `UPSTREAM.patch`.
