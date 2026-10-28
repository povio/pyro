# Flow layout (staged)

Status: Stages 1-4 (block/inline with floats, flex, tables, grid), with incremental relayout. The flow layout is the default geometry of
new documents; `LIGHTNINGPANDA_GEOMETRY=approximate` selects the upstream "faux layout"
(`crates/lp-dom/src/layout.rs`) for comparisons. A `Layout` built directly still defaults to
`Geometry::Approximate`, so lp-dom's own tests name the geometry they exercise.

## Why

The faux layout places every element 5px per preceding tree node vertically
and to the right of its preceding visible siblings horizontally. With real
declared sizes from the computed-style cascade this puts block siblings side by
side (a react-aria listbox option 11 at x=2870 in a 1440px viewport), makes
boxes with real heights overlap (32px boxes 15px apart), and ignores
`top/left` of positioned popovers. Playwright then reports "element is outside
of the viewport" or hits the wrong element, and boundingBox assertions fail.
Chromium, for the same page, stacks the options at one x, 32px apart, inside
the popover at its `top/left`.

## Module and switch

- `crates/lp-dom/src/flow.rs`: computes border-box rects (document
  coordinates) for every rendered element and scroll extents per box, from a
  [`FlowStyleSource`] and the viewport. One pass, cached on the layout
  observation stamp (DOM version, style generation, focus/hover) and viewport.
- `Layout::set_geometry(Geometry::Flow)` (pages take `Geometry::from_env()`, flow by default)
  routes the geometry getters through the flow result. Nothing changes unless
  a caller opts in. The browser exposes the selector as an internal flag only
  after Stage 1 parity is reviewed.

## Style input contract (owned by the style engine)

`flow.rs` never reads stylesheets. It asks a `FlowStyleSource` for used values
of one element. `StyleEngine` implements it in `style_flow.rs` (owned by the
style engine), including the UA defaults (body margin, p/h*/list margins,
display per tag).

```rust
pub trait FlowStyleSource {
    /// None when the element is not rendered (display:none in the flat tree).
    fn flow_style(&self, dom: &Dom, el: NodeId) -> Option<FlowStyle>;
}

pub struct FlowStyle {
    pub outer: OuterDisplay,          // Block | Inline | Contents
    pub inner: InnerDisplay,          // Flow | FlowRoot | Flex | Grid | Table...
    pub position: Position,           // Static | Relative | Absolute | Fixed | Sticky
    pub inset: Sides<Dim>,            // top right bottom left (Dim::Auto allowed)
    pub width: Dim, pub height: Dim,  // Auto | len (px + % of containing block)
    pub min_width: Dim, pub max_width: Dim, pub min_height: Dim, pub max_height: Dim,
    pub margin: Sides<Dim>,           // Auto allowed
    pub padding: Sides<Len>, pub border: Sides<f64>,  // used border widths
    pub border_box: bool,             // box-sizing
    pub overflow_x: Overflow, pub overflow_y: Overflow,
    pub float: Float,
    pub font_size: f64,               // px
    pub line_height: Option<f64>,     // px; None = normal
    pub nowrap: bool,                 // white-space nowrap/pre
    // Stage 2: flex direction/wrap/gaps/grow/shrink/basis/align/justify.
}
// Len = px + percent (resolves calc(100% - 8px)); Dim = Auto | Len.
```

Percentages, `em` and `vw/vh` reach `flow.rs` as px + percent of the
containing block (em/rem/vw/vh resolved by the style engine).

## Stage 1 algorithm

- Initial containing block = viewport. `html` is a block; `body` gets UA margin
  8px unless set.
- Block formatting: block-level in-flow children stack vertically at the
  content-box x; auto width fills the containing block minus margins; auto
  margins centre a box with a definite width. Vertical margins collapse as in
  CSS 2.1 8.3.1: between siblings, between a box and its first/last in-flow
  child when no border, padding, line content or clearance separates them,
  and through empty blocks (largest positive plus most negative). Roots of a
  block formatting context (floats, overflow other than visible, flex, grid,
  table, flow-root, inline-block, out of flow) do not collapse with their
  children. Auto height = content height, clamped by min/max;
  `max-height` with overflow other than visible makes a scroll container
  whose scroll height is its content height.
- Floats (CSS 2.1 9.5): floated boxes are blockified, shrink-to-fit when
  their width is auto, and placed in their block formatting context at the
  current line's top when they fit beside the line's content (else below
  the line), not above earlier floats, as high and then as far left/right as
  they fit. Line boxes shorten beside floats (measured at the strut height)
  and move below a float when a word or atomic box does not fit. `clear` on
  a block and on a block-level `::after` (clearfix) puts its border edge
  below the cleared floats. Formatting context roots sit beside floats
  (lower when their declared width does not fit) and their auto height
  contains their floats; the root element is one. Floats count as inline
  boxes for intrinsic widths.
- Inline formatting: runs of inline-level children and text form line boxes
  at the content width; text uses the existing fixed-advance measure;
  `inline-block` is atomic (shrink-to-fit). Line height = `line-height` or
  `font-size * 1.2`. An inline element's rect is the union of its fragments.
  Line boxes are as tall as their items need: a line-height below the
  content area gives negative half-leading, and a line holding only empty
  inline boxes without inline-axis margin, border or padding makes no line
  box (CSS 2.1 9.4.2). Headings and `small`/`big`/`sub`/`sup` take their UA
  font sizes.
- Positioned: `relative` offsets the box and its subtree after layout;
  `absolute` uses the nearest positioned ancestor's padding box (else the
  initial containing block), auto width shrink-to-fit, auto insets keep the
  static position; `fixed` uses the viewport and does not move with window
  scroll. Out-of-flow boxes do not advance siblings. `sticky` boxes are laid
  out in flow; `Layout::sticky_shift` offsets them against their scroll
  container's scroll position.
- Stage 2 (flex): row/column, wrap, gaps, grow/shrink/basis (one pass), justify-content, align-items/align-self with stretch; text directly in a flex container is an anonymous item.
- Stage 3 (tables): rows in display order (header, body, footer groups), colspan, fixed and auto column widths with border-spacing / border-collapse and extra-width distribution, tables without a width shrinking to their columns, row heights from the tallest cell, cells stretched to the row with content vertically centred. In automatic layout a cell's explicit width is a preferred width, not a clamp: a column is at least its cells' min-content width, and the table is at least the sum of its columns, past its declared width and its container, as in Chromium. `rowspan` takes slots in the following rows of its group (0 = the rest of the group; it stops at the group's end); a spanning cell taller than its rows grows them in proportion to their heights (all into the last when they are empty). No captions or column elements yet.
- Form controls (`input` except hidden/image, `textarea`, `select`) are atomic
  replaced boxes with Chromium's intrinsic size when width/height are auto:
  a text input is `size` (20) x 0.525em + 0.4375em wide and one line tall;
  textarea `cols` x 0.525em + 16px by `rows` lines; select the widest option
  + 24px by the normal line height + 2px; checkbox/radio 13x13; range 129x16.
  Text-bearing controls sit on their text baseline in a line. Measured in
  Chromium at 16px with a Tailwind-like preflight and a sans font (glyph
  advances differ per font, so widths are approximate).
- Percentage heights (and min/max heights) resolve against the containing
  block's definite height: the viewport for the root, declared heights,
  flexed/stretched flex items, and absolutely positioned boxes with both
  insets; otherwise they behave as auto.
- One pass memoizes used styles, max-content widths and measured heights
  (element, width), so nested flex/table measurement is linear rather than
  exponential in nesting depth, and records descendants by order instead of
  scanning every box.
- Flex (refined): hypothetical sizes are clamped by min/max; flexible
  lengths are resolved with the spec's freeze loop from the flex base sizes;
  `min-width/height: auto` items that do not clip overflow keep at least
  their min-content width / content height; an auto-height column container
  flexes within its own min/max height and a row container's line is at
  least its min-height. Max-content of a flex row is the sum of its items and
  gaps (min-content: longest word / widest atomic box).
  Auto margins on flex items absorb positive free space before
  justify-content, and auto cross-axis margins override align-self (no
  stretch).
- Stage 4 (grid): explicit tracks (px, %, fr, auto, min/max-content,
  minmax(), fit-content() approximated, repeat(n) and auto-fill/auto-fit with
  collapsed empty auto-fit tracks), line-based and span placement, sparse
  row/column auto-placement, implicit tracks from grid-auto-rows/-columns,
  gaps, track sizing (fixed, intrinsic from item contributions, maximize,
  fr expansion with inflexible tracks, auto-track stretch), auto-height grids
  clamped by min/max height (fr rows then fill), justify/align-items and
  -self, justify-content distribution. `grid-template-areas` extends the
  explicit grid and names each area's edges `<area>-start` / `<area>-end`;
  `[name]` lines in track lists (also inside `repeat(n)`); placement by
  `grid-area`, by name, by the nth name and from the end; a name no line has
  is the first implicit line. Percentage widths of items count as auto in
  their tracks' intrinsic contributions (a `width:100%` item in `1fr auto`
  leaves the auto track its content width), and a compressible replaced
  item (an input with a percentage width) contributes no min-content width.
  Not yet: dense packing, subgrid, baseline
  alignment, names inside auto repeats, `span <name>`.
- `text-align` (FlowStyle::text_align, inherited, LTR): each line box of a
  block container is offset by its free space (center: half, end/right:
  all); trailing collapsible spaces do not count and an overflowing line is
  start-aligned. Anonymous text items in flex/grid containers inherit it.
  `justify` is laid out as start.
- Lines with only collapsible whitespace have no height. Replaced elements
  without a CSS size use their width/height attributes (lucide icons), else
  300x150 (svg, canvas, video, iframe). Table row groups paint below their
  rows and cells (hit testing a sticky th inside a relative thead).
- Diagnostics: `tests/flow_snapshot.rs` (ignored) prints every body
  element's flow box for a static page snapshot, to diff against Chromium's
  getBoundingClientRect.
- Not yet: writing modes and right-to-left, transforms other than inline
  translateX, `float` line wrapping measured at the line's own height (the
  strut is used), float placement inside flex/grid items' anonymous text.

## Relayout

A page asks for geometry after almost every script step, so the flow
result is computed incrementally (`MeasureCache` in `flow.rs`, subtree
reuse in `flow_reuse.rs`, invalidation in `Layout::flow`):

- Invalidation reads the DOM change journal and the style engine's
  restyled set. An attribute the layout does not read (class changes that
  restyle nothing layout-relevant, data-*, aria-*), a focus/hover change
  that changes no used style, or a restyle whose used `FlowStyle` and
  generated content are unchanged dirties nothing, and the previous result
  is returned as is.
- Otherwise a new pass runs, and each block container whose subtree is
  clean and whose constraints (width, definite height, containing-block
  height, fixed, relative shift, margin collapsing) are unchanged replays
  its previous boxes, texts and text runs translated to its new position
  instead of being laid out again. Table rows are reuse units too (a clean
  row at the same column widths and height), and a table's row/cell
  structure is kept until a change reaches it (not one inside a cell).
  Subtrees that share a formatting context with floats are always laid out
  afresh.
- A result keeps boxes in an array by layout order and texts in the order
  laid out, each with an index by node (`flow_store.rs`), so a replay is a
  slice copy; only a subtree that moved (new position or renumbered
  orders) gets a linear fix-up sweep. A pass lays out into the buffers of
  the result before last when nothing else holds it.
- A replayed subtree does not renumber the reuse entries of the elements
  inside it. Each pass records what it replayed and how far its orders,
  texts and runs moved; an older entry is carried through those records
  when it is next used (up to 64 passes back), and dropped when some pass
  did not copy it.
- Invalidation is by node: the measurement caches and reuse entries are
  grouped per element, so a change drops the dirty path's entries without
  scanning the caches. A table rebuilds a changed structure (a row
  inserted) from the cached cells of its clean rows.
- Scroll sizes come from extents folded upward as boxes finish (each box
  adds what its unclipped contents reach to its parent), not from walking
  every descendant at every ancestor; debug builds check each against the
  walk. A stretched item in a single-line flex container of definite cross
  size is not measured (the line decides its size).
- `LIGHTNINGPANDA_FLOW_VERIFY=1` compares every incremental result with a
  fresh layout and panics on the first difference (for test runs);
  `long_mutation_sequences_relayout_like_a_fresh_pass` does the same over
  160 random edits, inserts and removals.
- `tests/flow_bench.rs` (ignored) times the first pass and the relayout
  after typical single-element changes on an app-shaped page (sticky header,
  sidebar, a table of N rows). On an Apple Silicon laptop at 1,000 rows
  (20k boxes): a class toggle about 0.13 ms, hover 0.3 ms, a text or value
  edit about 0.9 ms, a row insert about 1.4 ms, against a full pass of
  about 145 ms (mostly the style cascade). At 200 rows every change is
  under 0.35 ms. What is left grows with the rows of the changed table
  (each row's reuse check, column widths, row heights), not with its boxes.

## Consumers switched in Stage 1 (Flow mode only)

`getBoundingClientRect`/`getClientRects`/CDP quads, `offset*`, `client*`,
`scrollWidth/Height` and scroll clamping, `elementFromPoint` (topmost in
paint order: in-flow by tree order, positioned boxes above), `scrollIntoView`,
document scroll size.

- Overflow clipping (hit testing and text geometry) follows the containing
  block chain: a `fixed` box is clipped by no ancestor, an `absolute` box only
  by the clipping ancestors of its positioned containing block, and content
  inside an out-of-flow box escapes the clips that box escapes.
- SVG shapes, text and containers inside an `<svg>` have no CSS box; their
  client rect is the `getBBox()` geometry with their own and ancestor
  `transform`s, mapped by the svg's `viewBox`/`preserveAspectRatio` into the
  svg box's content box.
- Script-visible `scroll*`, `client*` and `offset*` metrics
  (`Layout::metric`) are whole pixels like Chromium: offsets round, sizes
  snap at the box's offset (`round(left + size) - round(left)`, client and
  scroll sizes at the padding edge). Rects and internal users keep the
  fractional values.
- `ResizeObserver` entries carry the content box (`contentRect` at the
  padding offset), the border box and the device-pixel content box; the
  observed `box` decides what is a resize, and a disconnected target gets no
  entry. Every completed scroll queues `scroll` and then `scrollend` for the
  next rendering update (layout clamps queue `scroll` only).

## Validation

Every Stage 1 test compares flow rects with numbers measured in headless
Chromium (Playwright 1.59.1) on the same markup, using explicit sizes where
text metrics would otherwise differ (glyph advances are approximate).
Old-vs-new tests keep the approximate geometry unchanged in default mode.
