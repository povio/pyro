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
  (upstream: gecko only). pyro-render's PDF output reads them to place page cuts of paper-cut
  documents. Nothing else reads them; computed values of other properties are unchanged.

- `properties/longhands.toml`: `margin-trim` is parsed and computed for servo too (upstream: gecko
  only), with `rebuild_box` restyle damage. Taffy's block layout reads it through stylo_taffy
  (vendor/taffy/README.pyro.md).

- `properties/longhands.toml`, `properties/shorthands.toml`: `contain-intrinsic-width`, `-height`,
  `-inline-size`, `-block-size` and the `contain-intrinsic-size` shorthand are parsed and computed for servo
  too (upstream: gecko only), with `rebuild_box` restyle damage. blitz-dom's layout reads them for boxes
  with size containment (vendor/blitz-dom/README.pyro.md).

- `values/specified/box.rs`: `break-before` and `break-after` also take `avoid-page` and
  `avoid-column`, as the spec has them (upstream has only `avoid`, and drops the declaration).
  `page-break-*` still rejects them.

- `properties/longhands.toml`, `properties/counted_unknown_properties.py`: `orphans` and `widows`
  are added (upstream has neither; both were counted unknown properties), inherited, in
  `inherited_text`. pyro-render's PDF output reads them to place page cuts of paper-cut documents.

- `stylist.rs`, `style_resolver.rs`: each origin's `CascadeData` also files its single-level
  `::before`/`::after` rules that declare `content`, `display` or `animation-name` (on the
  block's longhand set, so `all`, `var()` and CSS-wide keywords count; `content: normal | none`
  and `animation-name: none` do not, so pyro's UA
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
  lets a paint-only restyle of a table cell run no layout (`vendor/blitz-dom/README.pyro.md`, "Paint-only
  restyles run no layout and shape no text").

- `values/specified/effects.rs`, `values/computed/effects.rs`, `values/animated/effects.rs`: `filter` and
  `backdrop-filter` take `url()` for servo too (upstream: gecko only; servo's value type was `Impossible`, so a
  `filter: url(#f)` declaration was dropped). blitz-paint resolves it to an SVG `<filter>` of the document.

- `properties/longhands.toml`, `values/specified/transform.rs`: `transform-box` is parsed and computed for servo too
  (upstream: gecko only), with all five boxes (upstream's servo parser took `border-box`, `fill-box` and
  `view-box`). blitz-dom resolves SVG elements' transforms in it (`vendor/blitz-dom/README.pyro.md`, "The page's
  CSS on inline SVG"), and a CSS box's transform, `transform-origin` and `perspective-origin` against the box it
  names (`Node::transform_reference_box`: the content box for `content-box` and `fill-box`, the border box
  otherwise; "3D transforms flattened").

## Multi-column layout

- `properties/longhands.toml`, `properties/shorthands.toml`, `properties/shorthands.rs`: `column-rule-color`,
  `column-rule-style`, `column-rule-width`, the `column-rule` shorthand and `column-fill` are parsed and computed for
  servo too (upstream: gecko only, so the declarations were dropped). blitz-dom balances columns or fills them by
  `column-fill` (`layout::multicol`), and blitz-paint paints the column rules. `column-rule-width` is not zeroed for a
  `none` or `hidden` style (Gecko does that outside Stylo); blitz-paint draws no rule then.
- `properties/longhands.toml`, `properties/counted_unknown_properties.py`: `-webkit-box-decoration-break` is an alias
  of `box-decoration-break` (`extra_prefixes = ["webkit"]`; it was a counted unknown property, so the declaration was
  dropped), as in Chromium, which supports both. blitz-paint reads the value for `text-decoration-inset` and for the
  shadows and backgrounds of inline boxes broken across lines (`vendor/blitz-paint/README.pyro.md`, "Inline
  fragments and text painting").

## Text decorations and emphasis

- `properties/longhands.toml`: `text-decoration-skip-ink` is parsed and computed for Servo too (upstream: Gecko only);
  blitz-paint skips ink by it. `text-emphasis-style` gives `rebuild_box` damage under Servo (its `affects` is
  `overflow`): Blitz lays its marks out with the text, and they take room in their lines.

## The `filter()` image function

- `values/generics/image.rs` (`GenericImage::Filter`, `GenericFilterImage`, a sixth type parameter for the filter
  function type), `values/specified/image.rs` (`Image::parse_filter_args`), `values/computed/image.rs`:
  `filter(<image> | <string>, <filter-value-list>)` is an `<image>` (Filter Effects 1, "The filter() function";
  upstream has no such image, so the declaration was dropped). It computes its image and filters as each computes
  elsewhere, and serializes as `filter(<image>, <filters>)`. blitz-dom loads the image it filters and blitz-paint
  draws it with the filters. It is accepted in `background-image`, `border-image-source` (and their shorthands) and
  in registered custom properties of `<image>` syntax. `parse_with_cors_anonymous` (`mask-image`, `shape-outside`),
  the new `parse_without_filter` (`list-style-image`, given as its `parse_method` in `properties/longhands.toml`, so
  `list-style` too) and the new `parse_for_content` (`content`; `parse_forbid_none` is unchanged) still reject it, as
  Chromium and Gecko do (neither has `filter()`): blitz-paint draws it in background and border image layers only. A
  marker draws its image as it is, so a `filter()` marker would draw the image unfiltered; a mask of a smoothly scaled
  filtered image seams where the image's edges meet its shadow, so WPT `filter-function-008` (which WebKit, the one
  engine with `filter()`, fails) would stop passing.

## Container queries, typed values

- `stylesheets/rule_parser.rs`, `properties/shorthands.rs`, `properties/shorthands.toml`: `@container` rules
  and the `container` shorthand are parsed for servo too (upstream: gecko only). The shorthand's
  `servo_pref` is `layout.css.container-queries.enabled`: upstream names a pref that no longer exists in
  0.23 (`layout.container-queries.enabled`), which only compiles while the shorthand is gecko-only.
- `stylist.rs`: `Stylist::has_container_queries` tells blitz-dom whether a restyle has container
  conditions to evaluate.
- `values/generics/mod.rs`: `GreaterThanOrEqualToOne` derives `ToTyped` (for `orphans`/`widows`).
- `values/specified/background.rs`: `background-attachment: local` is parsed and computed for servo too
  (upstream: gecko only, so the declaration was dropped). blitz-paint positions and clips such a layer against a
  scroll container's scrolled content.
- `values/specified/background.rs`, `properties/shorthands.rs`: `mask-clip` takes `fill-box`, `stroke-box`,
  `view-box` and `no-clip`, and `mask-origin` the three SVG boxes, for servo too (upstream: gecko only, so the
  declaration was dropped); `background-clip` and `background-origin` still reject them. The `mask` shorthand
  serializes them as Gecko's does. blitz-paint clips and positions mask layers with them.
- `device/servo.rs` (`Device::system_color`): the deprecated system colours `ThreeDHighlight` and
  `ThreeDLightShadow` are `ButtonBorder`, as CSS Color 4 §6.2 maps them (upstream: `ButtonFace`; WPT
  `css-color/deprecated-sameas-018`/`-019`). Test: `deprecated_three_d_colours_are_button_border` in
  `crates/pyro-render/tests/background_images_and_edges.rs`.
- `servo/selector_parser.rs`: `:dir()` is parsed for servo too (`NonTSPseudoClass::Dir`, its identifier
  ASCII-lowercased; upstream servo had no `:dir()`, so a rule using it was dropped). It has no element state flag
  (as `:lang()`), so style sharing revalidates it. blitz-dom matches `ltr` and `rtl` by the element's
  directionality (`vendor/blitz-dom/src/direction.rs`), pyro-dom by its own; any other identifier matches nothing.
  Test: `dir_matches_the_directionality_of_elements` in `crates/pyro-render/tests/direction_text.rs`.

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
the move (presentational hints the element owns) is in `vendor/blitz-dom/README.pyro.md`.

`invalidation/viewport_units.rs`: the walk that invalidates styles using viewport units skips an element
that has no primary style yet (blitz-dom gives every element its data up front, so one inserted since the
last cascade, or below `display: none`, has data and no style). The 0.23 copy here had this check;
0.24.0 as published unwraps the primary style (`ElementStyles::viewport_unit_usage`) and panics on a
viewport resize. Test: `a_viewport_resize_with_elements_not_styled_yet` in pyro-render.

`values/specified/box.rs` (`DisplayKeyword::Math`): `display` accepts the `math` inside keyword (`math`,
`inline math`, `block math`; CSS Display 4). On an element that is not MathML it computes to `flow`, which is
how it is kept (pyro lays no MathML out); alone it is inline-level. The declaration was dropped as invalid, so
`display: block math` left a `::before` inline (WPT `display-math-on-pseudo-elements-002`, whose reference a
block-level `::before` in an inline box now renders as a block). Test: `display_math_computes_to_flow` in
`crates/pyro-render/tests/inline_leftovers.rs`.

`values/specified/box.rs` (`DisplayInside::WebkitBox`, `Display::WebkitBox`, `WebkitInlineBox`, `FlowRoot`),
`properties/longhands.toml` (`-webkit-box-orient`, servo only), `properties/shorthands.toml`
(`-webkit-line-clamp`), `style_adjuster.rs` (`adjust_for_webkit_box`), `properties/computed_value_flags.rs`
(`WEBKIT_LINE_CLAMP_APPLIES`): the legacy line clamp in the servo engine. `display: -webkit-box` and
`-webkit-inline-box`, `-webkit-box-orient` and the `-webkit-line-clamp` shorthand were gecko-only, so their
declarations were dropped and `-webkit-line-clamp` never applied. The servo engine has no legacy box layout:
the adjuster turns the display, before any other fixup, into `flow-root`/`inline-block` for a vertical box (as
Gecko and Blink do for one that clamps) and `flex`/`inline-flex` for a horizontal one, and flags the style when
the box is vertical and has a line clamp; blitz-dom's `layout::line_clamp` clamps the lines of flagged boxes
only. `-webkit-box` is never a computed value. Tests: `crates/pyro-render/tests/line_clamp.rs`, WPT
`css/css-overflow/line-clamp/webkit-line-clamp-*`.

`values/specified/box.rs`, `values/computed/box.rs` (`OverflowClipMargin`): the length of `overflow-clip-margin`
may be negative, as Blink takes it (the clip edge moves inwards; WPT `css-overflow/overflow-clip-margin-016`,
`-017`), where the specification and upstream have a non-negative length. Test:
`crates/pyro-render/tests/overflow_clip.rs`.
- `values/specified/text.rs` (`WordBreak::Manual`): `word-break: manual` (CSS Text 4) is parsed for the servo engine.
  blitz-dom lays it out as `normal` without dictionary breaks in Southeast Asian scripts (its README.pyro.md).
`values/specified/image.rs` (`Color::has_modern_syntax`): a colour with a missing component
(`rgb(none 255 0)`) is modern syntax, so a gradient with one interpolates in Oklab by default, as in
Chromium (WPT `gradient-eval-004`, `gradient-analogous-missing-components-*`). Stylo's legacy flag says
how the colour serializes. Test: `a_colour_with_a_missing_component_makes_the_gradient_oklab` in
`crates/pyro-render/tests/gradient_stops_image_set.rs`.

`properties/longhands.toml`, `values/specified/basic_shape.rs` (`ClipPath::parse_object_view_box`): the
`object-view-box` longhand (CSS Images 5; `none | inset() | rect() | xywh()`), kept as a `clip-path` value of
`none` or one rectangle, animated discretely. blitz-dom reads it (`util::object_view_box`). Tests:
`crates/pyro-render/tests/object_view_box_embed.rs`.

## `::before::marker`, `::after::marker` (WPT round 3, lane I)

`servo/selector_parser.rs`: `PseudoElement::valid_after_before_or_after` (`::marker`) and `is_before_or_after`,
so the selectors parse (CSS Pseudo-Elements 4 § 4.1); the stylist already keeps such rules by pseudo-element
chain. blitz-dom answers `implemented_pseudo_element` for the boxes (its README).

## `initial-letter: <number> drop | raise` (WPT round 3, lane I)

`values/generics/text.rs` (`GenericInitialLetter::keyword`, `InitialLetterKeyword`), `values/specified/text.rs`:
`initial-letter` takes `drop` or `raise` before or after its size (CSS Inline 3 § 7.2, as Blink); the keyword is
kept for serialization (`1.23 drop`) and `sink` stays 0 then. pyro turns the property on with its pref
(`layout.css.initial-letter.enabled`); blitz-dom lays initial letters out (its README).
## `font-palette`, `palette-mix()`, `@font-palette-values` (WPT css-fonts, round 3 lane F)

- `properties/longhands.toml`: `font-palette` is parsed and computed for servo too, behind
  `layout.css.font-palette.enabled` (upstream: gecko only). blitz-paint draws COLR glyphs in the palette it selects.
- `values/specified/font.rs`: `FontPalette` is an enum: a name (`normal`, `light`, `dark`, `<dashed-ident>`) or
  `palette-mix(<color-interpolation-method>, <palette> <percentage>?, <palette> <percentage>?)` (CSS Fonts 5,
  nested mixes too; `FontPaletteMix::weights` normalizes the percentages). Its computed and resolved values are
  the specified one, written out (deriving them recurses without end). `color/mix.rs`: `ColorInterpolationMethod`
  and `HueInterpolationMethod` derive serde, as servo's font style struct needs.
- `values/specified/number.rs` (`Integer::resolve_with_font_size`), `stylesheets/font_palette_values_rule.rs`:
  an `<integer>` of `base-palette` or `override-colors` whose calc() has font-relative lengths
  (`calc(2 + sign(100em - 1px))`) resolves at the initial font size, 16px (CSS Values 4 §6.1.1: a descriptor has
  no element); upstream dropped the declaration. `FontPaletteOverrideColor`'s fields are public, with
  `resolved_index` there and on `FontPaletteBase`.

## Container units for servo (WPT css-images, round 3 lane F)

`values/specified/length.rs` (`LengthUnit::parse_dimension`): `cqw`, `cqh`, `cqi`, `cqb`, `cqmin` and `cqmax` parse
for servo too (upstream: gecko only, so any declaration using them was dropped). They compute against the nearest
size container through `TElement::query_container_size`, as `@container` conditions do (Blitz answers it with
the container's laid-out content box, see blitz-dom's README); without one, against the small viewport.

## `text-spacing-trim` (WPT round 3, lane TS)

`properties/longhands.toml`, `properties/data.py`, `values/specified/text.rs` (`TextSpacingTrim`), `values/specified/mod.rs`,
`values/computed/{mod,text}.rs`: the `text-spacing-trim` longhand (CSS Text 4 § 9.2.1; `normal | space-all |
space-first | trim-start | trim-both | trim-all | auto`), inherited, for the servo engine. blitz-dom passes it to parley,
which trims fullwidth punctuation while shaping and at line edges (their READMEs). `text-autospace` was already there;
pyro turns it on with its pref (`layout.css.text-autospace.enabled`); its initial value stays `no-autospace`, as in
Chromium (whose default is not the spec's `normal`).
## `border-shape` (WPT round 3, lane CB)

`properties/longhands.toml`, `values/generics/basic_shape.rs` (`BorderShapeBox`, `GenericBorderShapeItem`,
`GenericBorderShape`), `values/specified/basic_shape.rs`, `values/computed/basic_shape.rs`: `border-shape: none |
[ <basic-shape> <geometry-box>? ]{1,2}` (CSS Borders 4 § 11) is parsed and computed for servo (upstream has no such
property), boxed, not animated, in the border struct. A shape's box may also be `half-border-box`; an omitted box is
kept as `Default` (serialized as nothing) for blitz-paint to resolve: `half-border-box` for a single shape, the border
and padding boxes for two.

`stylesheets/rule_parser.rs`: `@counter-style` rules are parsed for servo too (upstream: gecko only). blitz-dom reads
their `system: extends` for list markers (its README).
## Pseudo-elements after `::slotted()` (WPT round 3, lane D)

`servo/selector_parser.rs`: `PseudoElement::valid_after_slotted`: `::slotted()` can be followed by `::before`,
`::after`, `::marker` (tree-abiding, CSS Scoping 1 § 3.2.2) and, as in Blink, `::placeholder`,
`::file-selector-button` and `::details-content`; the trait's default (servo's until now) dropped such rules
(WPT css-shadow `slotted-with-pseudo-element`, `slotted-placeholder`). The rule collector already collects the
slotted rules of a pseudo-element. blitz-dom keeps shadow trees' styles (its README).
## Scroll markers and scroll buttons, `content-visibility: auto` (WPT round 3, lane SM)

- `properties/longhands.toml`, `values/specified/box.rs` (`ScrollMarkerGroup`, `ScrollTargetGroup`),
  `properties/data.py`: `scroll-marker-group` (`none | [ before | after ] || [ links | tabs ]`, rebuilds boxes) and
  `scroll-target-group` (`none | auto`) for servo (CSS Overflow 5 § 3.1).
- `servo/selector_parser.rs`: the lazy pseudo-elements `::scroll-marker-group`, `::scroll-marker`, `::column` and
  `::scroll-button(<direction> | *)` (functional: one variant per argument). `::column::scroll-marker` parses
  (`::column` lets `::scroll-marker` follow it as `::before` lets `::marker`). `:target-current` is a state
  pseudo-class (`TARGET_CURRENT`, the Gecko-only `STYLEEDITOR_TRANSITIONING` bit, never set by servo otherwise), so
  are `:target-before` and `:target-after` (`TARGET_BEFORE`, `TARGET_AFTER`: the Gecko-only `DEVTOOLS_HIGHLIGHTED`
  and `REVEALED` bits); `::scroll-marker` and `::scroll-button()` accept state pseudo-classes after them
  (`:target-current`, `:target-before`, `:target-after`, `:enabled`, `:disabled` count as user action states for
  that). A button is element-backed (the UA's `button` rules apply to it).
  `PSEUDO_COUNT` passed 32: `pseudo_none_array` builds its array with `array::from_fn`.
- `stylist.rs`: every `::scroll-button()` rule is filed under `::scroll-button(*)` (`PseudoElement::rule_map_key`)
  and its bucket never matches alone (`has_argument`): the button's node matches the argument against its direction.
  `Stylist::compute_pseudo_node_style` styles a node that is a pseudo-element's box in normal matching mode, so state
  pseudo-classes after the pseudo-element match the node's own state (lazy matching on the originating element
  rejects every selector with one).
- `style_adjuster.rs`: a `::scroll-marker-group` always has layout containment, and size containment unless it is
  absolutely positioned (CSS Overflow 5's recommended `contain: size !important`; Chromium contains its layout too).
  `content-visibility` is parsed for servo; `auto` adds layout, style and paint containment to the computed `contain`
  (every element is taken as relevant to the user: no size containment), `hidden` does nothing yet.
- `style_adjuster.rs` (`adjust_for_author_styled_button`): a button (`button`, `input` of type `button`, `submit`
  or `reset`, `::scroll-button()`) whose border or background an author or user styles has square corners unless
  one rounds them: pyro's UA sheet rounds buttons by 1px for Chromium's native look, which Chromium drops then
  (appearance devolves).
- `values/specified/align.rs` (`AlignFlags::FLOW_START`/`FLOW_END`): the `flow-start` and `flow-end` keywords
  (css-align-3) in `<content-position>` and `<self-position>`, serialized, flipped and listed like the others;
  stylo_taffy maps them to `flex-start`/`flex-end` (WPT `css/css-flexbox/alignment/flow-start-flow-end-*`).

blitz-dom generates and lays out the boxes (its README).

## `flow-root list-item` for servo (WPT round 3, lane ID)

`values/specified/box.rs` (`DisplayInside::is_valid_for_list_item`): `flow-root` with `list-item` parses in servo too
(upstream only with the gecko feature), as in Blink: `display: flow-root list-item` is a list item that establishes a
block formatting context (WPT css-display `display-flow-root-list-item-001`).
## `text-fit` (WPT round 3, lane TS)

`properties/longhands.toml`, `values/generics/text.rs` (`GenericTextFit`, `TextFitKind`, `TextFitTarget`),
`values/specified/text.rs`, `values/computed/text.rs`: CSS Text 5 `text-fit`
(`[none | grow | shrink] [consistent | per-line | per-line-all]? <percentage>?`, inherited as in Blink) for servo.
blitz-dom passes it to parley, which scales the lines (their READMEs).

## `::highlight()` and `::target-text` (WPT round 3, lane PX)

- `servo/selector_parser.rs`: the lazy pseudo-elements `::target-text` and `::highlight(<custom-ident>)`. A
  highlight's name is a process-wide id (`highlight_name_id`, `highlight_name`), so `PseudoElement` stays `Copy`:
  `Highlight(u16)` makes the enum carry data, so `index()`/`eager_index()` read the `#[repr(u8)]` tag
  (`discriminant()`) and `from_eager_index` matches instead of transmuting. Every `::highlight()` rule is filed under
  `Highlight(ANY_HIGHLIGHT)` (`rule_map_key`, `has_argument()`); a style for one name is computed lazily with a
  `matching_fn` that compares the name (blitz-dom `BaseDocument::highlight_pseudo_style`). Both apply
  `APPLIES_TO_HIGHLIGHT` properties only.

## `overflow: clip` beside `hidden` (WPT round 3, lane FO)

`StyleAdjuster::adjust_for_overflow` keeps `clip` when the other axis is `hidden` (Blink): that axis neither scrolls
nor keeps a scroll offset. Other mixes still turn `visible` into `auto` and `clip` into `hidden` (WPT css-overflow
`single-axis-overflow-scroll-to-clip`).
## `calc-size()` (WPT round 3, lane LT)

- `values/generics/length.rs` (`GenericSize::CalcSizeAuto`, `CalcSizeFitContent`, `CalcSizeMinContent`,
  `CalcSizeMaxContent`), `values/specified/length.rs` (`Size::parse_calc_size`), `values/specified/calc.rs`
  (`AdditionalFunctions::CALC_SIZE`, `CalcNode::parse_calc_size_calculation`), `values/computed/length.rs`:
  `calc-size(auto | fit-content | min-content | max-content, <calculation>)` in the `Size` properties (`width`,
  `height`, their logical and min forms). The calculation is kept as a length-percentage whose `size` keyword is
  `100%` (other percentages are rejected), so its percentage basis is the size of the basis: layout resolves it
  against that size (stylo_taffy, blitz-dom). A `<length-percentage>` or `calc-size()` basis, `any`, `max-*` and
  interpolation are not supported; the value serializes as `calc-size(<calculation>)` with `size` as `100%`.

## Legacy `-webkit-box` alignment and flexing for servo (WPT round 3, lane FO)

`-webkit-box-align`, `-webkit-box-pack`, `-webkit-box-direction` and `-webkit-box-flex` parse for servo.
`adjust_for_webkit_box` makes a horizontal box a single-line flex row (`flex-wrap: nowrap`) aligned by them and sets
`ComputedValueFlags::IS_WEBKIT_BOX`; `adjust_for_webkit_box_child` gives its children `-webkit-box-flex` as flex-grow
and flex-shrink and `align-self: auto` (Blink's `LayoutDeprecatedFlexibleBox`; WPT css-flexbox
`flexbox-safe-overflow-position-006`, `balance/webkit-box-no-balance`).

## Updating or dropping the patch

- When upstream has the change: delete this directory and the `[patch.crates-io]` entry.
- To move to a newer stylo: copy the published crate over this directory and apply `UPSTREAM.patch`.
