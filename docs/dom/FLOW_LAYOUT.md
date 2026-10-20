# Flow layout (staged)

Status: Stages 1-4 (block/inline, flex, tables, grid), opt-in only. The default geometry is still the
upstream "faux layout" (`crates/lp-dom/src/layout.rs`).

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
- `Layout::set_geometry(Geometry::Flow)` (default `Geometry::Approximate`)
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
  margins centre a box with a definite width; adjacent sibling vertical
  margins collapse (max). Auto height = content height, clamped by min/max;
  `max-height` with overflow other than visible makes a scroll container
  whose scroll height is its content height.
- Inline formatting: runs of inline-level children and text form line boxes
  at the content width; text uses the existing fixed-advance measure;
  `inline-block` is atomic (shrink-to-fit). Line height = `line-height` or
  `font-size * 1.2`. An inline element's rect is the union of its fragments.
- Positioned: `relative` offsets the box and its subtree after layout;
  `absolute` uses the nearest positioned ancestor's padding box (else the
  initial containing block), auto width shrink-to-fit, auto insets keep the
  static position; `fixed` uses the viewport and does not move with window
  scroll. Out-of-flow boxes do not advance siblings.
- Stage 2 (flex): row/column, wrap, gaps, grow/shrink/basis (one pass), justify-content, align-items/align-self with stretch; text directly in a flex container is an anonymous item.
- Stage 3 (tables): rows in display order (header, body, footer groups), colspan, fixed and auto column widths with border-spacing / border-collapse and extra-width distribution, tables without a width shrinking to their columns, row heights from the tallest cell, cells stretched to the row with content vertically centred. No rowspan, captions or column elements yet.
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
  -self, justify-content distribution. Not yet: grid-template-areas, named
  lines, dense packing, subgrid, baseline alignment.
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
- Not yet (laid out as block containers): floats, sticky, margin collapsing through parents, writing modes,
  transforms other than inline translateX.

## Consumers switched in Stage 1 (Flow mode only)

`getBoundingClientRect`/`getClientRects`/CDP quads, `offset*`, `client*`,
`scrollWidth/Height` and scroll clamping, `elementFromPoint` (topmost in
paint order: in-flow by tree order, positioned boxes above), `scrollIntoView`,
document scroll size.

## Validation

Every Stage 1 test compares flow rects with numbers measured in headless
Chromium (Playwright 1.59.1) on the same markup, using explicit sizes where
text metrics would otherwise differ (glyph advances are approximate).
Old-vs-new tests keep the approximate geometry unchanged in default mode.
