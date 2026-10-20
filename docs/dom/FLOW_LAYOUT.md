# Flow layout (staged)

Status: Stage 1 in progress, opt-in only. The default geometry is still the
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
- Not yet (laid out as block containers): grid inner layout,
  tables, floats, sticky, margin collapsing through parents, writing modes,
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
