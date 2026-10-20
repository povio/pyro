//! Flow layout, Stage 1 (docs/dom/FLOW_LAYOUT.md): CSS block and inline
//! formatting with relative/absolute/fixed positioning, computed from used
//! style values supplied by a [`FlowStyleSource`]. Opt-in through
//! `Layout::set_geometry(Geometry::Flow)`; the default geometry remains the
//! upstream approximation in `layout.rs`.
//!
//! Used values come from `StyleEngine`'s [`FlowStyleSource`] implementation
//! (`style_flow.rs`, owned by the style engine).
//!
//! Stage 1 lays out flex, grid and table containers as block containers and
//! ignores floats, sticky positioning, margin collapsing through parents and
//! baseline alignment. Text uses the engine's fixed-advance measure, so text
//! widths are approximate; boxes with explicit sizes match Chromium.

use rustc_hash::FxHashMap;

use crate::layout::{text_width, Rect};
use crate::node::{NodeId, NodeType};
use crate::Dom;

// ----- style input contract -------------------------------------------------

/// A length: `px` plus `percent` of the containing block's matching axis
/// (so `calc(100% - 8px)` is `{ px: -8, percent: 100 }`).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Len {
    pub px: f64,
    pub percent: f64,
}

impl Len {
    pub const ZERO: Len = Len { px: 0.0, percent: 0.0 };
    pub fn px(px: f64) -> Len {
        Len { px, percent: 0.0 }
    }
    pub fn percent(percent: f64) -> Len {
        Len { px: 0.0, percent }
    }
    pub fn resolve(self, basis: f64) -> f64 {
        self.px + self.percent * basis / 100.0
    }
}

/// A size or offset that may be `auto` (or `none` for max sizes).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum Dim {
    #[default]
    Auto,
    Len(Len),
}

impl Dim {
    pub fn px(px: f64) -> Dim {
        Dim::Len(Len::px(px))
    }
    pub fn resolve(self, basis: Option<f64>) -> Option<f64> {
        match self {
            Dim::Auto => None,
            Dim::Len(l) if l.percent != 0.0 => basis.map(|b| l.resolve(b)),
            Dim::Len(l) => Some(l.px),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum OuterDisplay {
    #[default]
    Block,
    Inline,
    /// The element generates no box; its children are laid out in its place.
    Contents,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum InnerDisplay {
    #[default]
    Flow,
    /// Establishes a new formatting context (inline-block, flow-root).
    FlowRoot,
    Flex,
    Grid,
    Table,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Position {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

/// top, right, bottom, left.
pub type Sides<T> = [T; 4];
pub const TOP: usize = 0;
pub const RIGHT: usize = 1;
pub const BOTTOM: usize = 2;
pub const LEFT: usize = 3;

/// Used style values of one rendered element.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowStyle {
    pub outer: OuterDisplay,
    pub inner: InnerDisplay,
    pub position: Position,
    pub inset: Sides<Dim>,
    pub width: Dim,
    pub height: Dim,
    pub min_width: Dim,
    pub max_width: Dim,
    pub min_height: Dim,
    pub max_height: Dim,
    pub margin: Sides<Dim>,
    pub padding: Sides<Len>,
    pub border: Sides<f64>,
    pub border_box: bool,
    pub clips_x: bool,
    pub clips_y: bool,
    pub font_size: f64,
    /// px; None = `normal`.
    pub line_height: Option<f64>,
    pub nowrap: bool,
    /// `<img>`/`<iframe>`-like replaced element: atomic, sized by its own
    /// width/height only.
    pub replaced: bool,
    /// Non-standard CSS `zoom` (1 = none). Carried for Stage 1; not yet
    /// applied to used sizes.
    pub zoom: f64,
}

impl Default for FlowStyle {
    fn default() -> Self {
        FlowStyle {
            outer: OuterDisplay::Block,
            inner: InnerDisplay::Flow,
            position: Position::Static,
            inset: [Dim::Auto; 4],
            width: Dim::Auto,
            height: Dim::Auto,
            min_width: Dim::Auto,
            max_width: Dim::Auto,
            min_height: Dim::Auto,
            max_height: Dim::Auto,
            margin: [Dim::px(0.0); 4],
            padding: [Len::ZERO; 4],
            border: [0.0; 4],
            border_box: false,
            clips_x: false,
            clips_y: false,
            font_size: 16.0,
            line_height: None,
            nowrap: false,
            replaced: false,
            zoom: 1.0,
        }
    }
}

/// Supplies used style values. `None`: the element is not rendered.
pub trait FlowStyleSource {
    fn flow_style(&self, dom: &Dom, el: NodeId) -> Option<FlowStyle>;
}

// ----- output ---------------------------------------------------------------

/// One element's geometry. Rects are border boxes in document coordinates,
/// except boxes inside `position: fixed`, which are in viewport coordinates.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct FlowBox {
    pub rect: Rect,
    /// Inside (or itself) a `position: fixed` box.
    pub fixed: bool,
    /// Padding box size (clientWidth/Height before scrollbars).
    pub client_width: f64,
    pub client_height: f64,
    /// Padding box plus overflowing descendants (scrollWidth/Height).
    pub scroll_width: f64,
    pub scroll_height: f64,
    /// Positioned (relative/absolute/fixed/sticky): painted above in-flow boxes.
    pub positioned: bool,
    /// Tree order, for paint order among equals.
    pub order: u32,
    /// `order` of the nearest ancestor clipping this box's overflow (0: none).
    pub clipped_by: u32,
}

#[derive(Default, Debug)]
pub struct FlowResult {
    pub boxes: FxHashMap<NodeId, FlowBox>,
    /// Document scroll size (at least the viewport).
    pub document_width: f64,
    pub document_height: f64,
}

impl FlowResult {
    pub fn get(&self, el: NodeId) -> Option<&FlowBox> {
        self.boxes.get(&el)
    }
}

// ----- layout ---------------------------------------------------------------

/// Lays out the document rooted at `doc` for a `vw`×`vh` viewport.
pub fn layout(dom: &Dom, doc: NodeId, src: &dyn FlowStyleSource, vw: f64, vh: f64) -> FlowResult {
    let mut cx = Ctx { dom, src, vw, vh, out: FlowResult::default(), order: 0, pending: Vec::new() };
    let icb = Rect { x: 0.0, y: 0.0, width: vw, height: vh };
    cx.pending.push(Vec::new());
    let mut cursor = 0.0;
    if let Ok(children) = dom.children(doc) {
        let kids: Vec<NodeId> = children.collect();
        for c in kids {
            if dom.node_type(c).ok() == Some(NodeType::Element) {
                if let Some(style) = src.flow_style(dom, c) {
                    let h = cx.block_child(c, &style, 0.0, cursor, vw, false, (0.0, 0.0));
                    cursor += h;
                }
            }
        }
    }
    let pending = cx.pending.pop().unwrap_or_default();
    cx.place_absolute(pending, icb, false);
    let (mut w, mut h) = (vw, vh);
    for b in cx.out.boxes.values().filter(|b| !b.fixed && b.clipped_by == 0) {
        w = w.max(b.rect.x + b.rect.width);
        h = h.max(b.rect.y + b.rect.height);
    }
    cx.out.document_width = w.ceil();
    cx.out.document_height = h.ceil();
    cx.out
}

struct Ctx<'a> {
    dom: &'a Dom,
    src: &'a dyn FlowStyleSource,
    vw: f64,
    vh: f64,
    out: FlowResult,
    order: u32,
    /// Absolutely positioned descendants waiting for their containing block:
    /// one list per open positioned ancestor (bottom = initial containing
    /// block). Each entry: element, style, static position, fixed context.
    pending: Vec<Vec<Pending>>,
}

struct Pending {
    el: NodeId,
    style: FlowStyle,
    static_x: f64,
    static_y: f64,
}

/// A laid-out box's used edges.
struct Edges {
    margin: Sides<f64>,
    padding: Sides<f64>,
    border: Sides<f64>,
}

impl Edges {
    fn horizontal(&self) -> f64 {
        self.padding[LEFT] + self.padding[RIGHT] + self.border[LEFT] + self.border[RIGHT]
    }
    fn vertical(&self) -> f64 {
        self.padding[TOP] + self.padding[BOTTOM] + self.border[TOP] + self.border[BOTTOM]
    }
}

/// An item of an inline formatting context.
enum InlineItem {
    Word {
        width: f64,
        height: f64,
        space_before: f64,
        owners: Vec<NodeId>,
        ascent: f64,
    },
    Break,
    Atomic {
        el: NodeId,
        style: Box<FlowStyle>,
        owners: Vec<NodeId>,
    },
    /// An inline element that starts or ends (for empty inline boxes).
    Empty {
        owners: Vec<NodeId>,
        height: f64,
        ascent: f64,
    },
}

impl<'a> Ctx<'a> {
    fn edges(&self, style: &FlowStyle, cb_width: f64) -> Edges {
        let m = |d: Dim| d.resolve(Some(cb_width)).unwrap_or(0.0);
        Edges {
            margin: [m(style.margin[0]), m(style.margin[1]), m(style.margin[2]), m(style.margin[3])],
            padding: style.padding.map(|p| p.resolve(cb_width).max(0.0)),
            border: style.border.map(|b| b.max(0.0)),
        }
    }

    fn clamp(v: f64, min: Option<f64>, max: Option<f64>) -> f64 {
        let v = max.map_or(v, |m| v.min(m));
        min.map_or(v, |m| v.max(m)).max(0.0)
    }

    /// Content-box width from a declared width (box-sizing aware).
    fn declared_content(style: &FlowStyle, declared: f64, edges_h: f64) -> f64 {
        if style.border_box {
            (declared - edges_h).max(0.0)
        } else {
            declared
        }
    }

    fn relative_offset(&self, style: &FlowStyle, cb_width: f64, cb_height: Option<f64>) -> (f64, f64) {
        // Stage 1 treats sticky as relative (no scroll-dependent stickiness).
        if !matches!(style.position, Position::Relative | Position::Sticky) {
            return (0.0, 0.0);
        }
        let dx = match (style.inset[LEFT].resolve(Some(cb_width)), style.inset[RIGHT].resolve(Some(cb_width))) {
            (Some(l), _) => l,
            (None, Some(r)) => -r,
            _ => 0.0,
        };
        let dy = match (style.inset[TOP].resolve(cb_height), style.inset[BOTTOM].resolve(cb_height)) {
            (Some(t), _) => t,
            (None, Some(b)) => -b,
            _ => 0.0,
        };
        (dx, dy)
    }

    /// Lays out an in-flow block-level element at (x, y) (its margin edge)
    /// inside a containing block of width `cb_width`. Returns the vertical
    /// space it takes in its parent (margin box height, before collapsing).
    #[allow(clippy::too_many_arguments)]
    fn block_child(
        &mut self,
        el: NodeId,
        style: &FlowStyle,
        x: f64,
        y: f64,
        cb_width: f64,
        fixed: bool,
        shift: (f64, f64),
    ) -> f64 {
        let e = self.edges(style, cb_width);
        let declared = style.width.resolve(Some(cb_width));
        let auto_width = (cb_width - e.margin[LEFT] - e.margin[RIGHT] - e.horizontal()).max(0.0);
        let mut content_w = declared.map_or(auto_width, |w| Self::declared_content(style, w, e.horizontal()));
        let min_w = style.min_width.resolve(Some(cb_width)).map(|w| Self::declared_content(style, w, e.horizontal()));
        let max_w = style.max_width.resolve(Some(cb_width)).map(|w| Self::declared_content(style, w, e.horizontal()));
        content_w = Self::clamp(content_w, min_w, max_w);
        // Auto horizontal margins centre a box narrower than its containing block.
        let mut margin_left = e.margin[LEFT];
        let used = content_w + e.horizontal();
        let free = cb_width - used - e.margin[LEFT] - e.margin[RIGHT];
        match (style.margin[LEFT], style.margin[RIGHT]) {
            (Dim::Auto, Dim::Auto) if free > 0.0 => margin_left = free / 2.0,
            (Dim::Auto, _) if free > 0.0 => margin_left = free + e.margin[LEFT],
            _ => {}
        }
        let (dx, dy) = self.relative_offset(style, cb_width, None);
        let shift = (shift.0 + dx, shift.1 + dy);
        let bx = x + margin_left;
        let by = y + e.margin[TOP];
        let content_x = bx + e.border[LEFT] + e.padding[LEFT];
        let content_y = by + e.border[TOP] + e.padding[TOP];
        let order = self.next_order();
        let positioned = style.position != Position::Static;
        if positioned {
            self.pending.push(Vec::new());
        }
        let content_h = self.contents(el, style, content_x, content_y, content_w, fixed, shift);
        let declared_h =
            style.height.resolve(None).map(|h| if style.border_box { (h - e.vertical()).max(0.0) } else { h });
        let min_h = style.min_height.resolve(None).map(|h| if style.border_box { h - e.vertical() } else { h });
        let max_h = style.max_height.resolve(None).map(|h| if style.border_box { h - e.vertical() } else { h });
        let height = Self::clamp(declared_h.unwrap_or(content_h), min_h, max_h);
        let rect =
            Rect { x: bx + shift.0, y: by + shift.1, width: content_w + e.horizontal(), height: height + e.vertical() };
        self.record(
            el,
            rect,
            &e,
            fixed,
            positioned,
            style.clips_x || style.clips_y,
            order,
            content_w,
            height,
            content_h,
        );
        if positioned {
            let pending = self.pending.pop().unwrap_or_default();
            let padding_box = Rect {
                x: rect.x + e.border[LEFT],
                y: rect.y + e.border[TOP],
                width: rect.width - e.border[LEFT] - e.border[RIGHT],
                height: rect.height - e.border[TOP] - e.border[BOTTOM],
            };
            self.place_absolute(pending, padding_box, fixed);
        }
        e.margin[TOP] + rect.height + e.margin[BOTTOM]
    }

    fn next_order(&mut self) -> u32 {
        self.order += 1;
        self.order
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        el: NodeId,
        rect: Rect,
        e: &Edges,
        fixed: bool,
        positioned: bool,
        clips: bool,
        order: u32,
        content_w: f64,
        content_h: f64,
        flow_h: f64,
    ) {
        let client_width = content_w + e.padding[LEFT] + e.padding[RIGHT];
        let client_height = content_h + e.padding[TOP] + e.padding[BOTTOM];
        // Overflowing descendants extend the scroll size, except those clipped
        // by a box inside this one (and fixed boxes escaping a non-fixed one).
        let inner_x = rect.x + e.border[LEFT];
        let inner_y = rect.y + e.border[TOP];
        let mut sw = client_width;
        let mut sh = client_height.max(flow_h + e.padding[TOP] + e.padding[BOTTOM]);
        for b in self.out.boxes.values_mut().filter(|b| b.order > order) {
            let clipped_inside = b.clipped_by > order;
            if !clipped_inside && (fixed || !b.fixed) {
                sw = sw.max(b.rect.x + b.rect.width - inner_x);
                sh = sh.max(b.rect.y + b.rect.height - inner_y);
            }
            if clips && b.clipped_by == 0 {
                b.clipped_by = order;
            }
            // Descendants of a positioned box paint in its layer.
            if positioned {
                b.positioned = true;
            }
        }
        self.out.boxes.insert(
            el,
            FlowBox {
                rect,
                fixed,
                client_width,
                client_height,
                scroll_width: sw,
                scroll_height: sh,
                positioned,
                order,
                clipped_by: 0,
            },
        );
    }

    /// Lays out the children of a block container in its content box and
    /// returns the content height.
    #[allow(clippy::too_many_arguments)]
    fn contents(
        &mut self,
        el: NodeId,
        style: &FlowStyle,
        x: f64,
        y: f64,
        width: f64,
        fixed: bool,
        shift: (f64, f64),
    ) -> f64 {
        if style.replaced {
            return 0.0;
        }
        let kids = self.flatten_children(el);
        let mut cursor = y;
        let mut prev_margin_bottom: Option<f64> = None;
        let mut inline_run: Vec<NodeId> = Vec::new();
        for (child, child_style) in kids {
            let is_text = child_style.is_none();
            let out_of_flow =
                child_style.as_ref().is_some_and(|s| matches!(s.position, Position::Absolute | Position::Fixed));
            if out_of_flow {
                let s = child_style.expect("styled");
                let static_y = cursor + prev_margin_bottom.unwrap_or(0.0).min(0.0);
                self.defer_absolute(child, s, x - shift.0, static_y);
                continue;
            }
            let inline = is_text || child_style.as_ref().is_some_and(|s| s.outer == OuterDisplay::Inline);
            if inline {
                inline_run.push(child);
                continue;
            }
            if !inline_run.is_empty() {
                let run = std::mem::take(&mut inline_run);
                let h = self.inline_run(
                    &run,
                    style,
                    x,
                    cursor + prev_margin_bottom.take().unwrap_or(0.0),
                    width,
                    fixed,
                    shift,
                );
                cursor += h;
            }
            let s = child_style.expect("styled");
            let e = self.edges(&s, width);
            // Adjacent sibling margins collapse to the larger one.
            let collapse = prev_margin_bottom.map_or(0.0, |prev| prev.max(e.margin[TOP]) - e.margin[TOP]);
            let top = cursor + collapse;
            let taken = self.block_child(child, &s, x, top, width, fixed, shift);
            cursor = top + taken - e.margin[BOTTOM];
            prev_margin_bottom = Some(e.margin[BOTTOM]);
        }
        if !inline_run.is_empty() {
            let h = self.inline_run(
                &inline_run,
                style,
                x,
                cursor + prev_margin_bottom.take().unwrap_or(0.0),
                width,
                fixed,
                shift,
            );
            cursor += h;
        }
        cursor += prev_margin_bottom.unwrap_or(0.0);
        cursor - y
    }

    /// Children of `el` with their styles (None for text), with
    /// `display: contents` elements replaced by their children and
    /// unrendered elements dropped.
    fn flatten_children(&self, el: NodeId) -> Vec<(NodeId, Option<FlowStyle>)> {
        let mut out = Vec::new();
        let Ok(children) = self.dom.children(el) else { return out };
        for c in children {
            match self.dom.node_type(c) {
                Ok(NodeType::Text) => {
                    if self.dom.char_data(c).is_ok_and(|t| !t.is_empty()) {
                        out.push((c, None));
                    }
                }
                Ok(NodeType::Element) => {
                    if let Some(s) = self.src.flow_style(self.dom, c) {
                        if s.outer == OuterDisplay::Contents {
                            out.extend(self.flatten_children(c));
                        } else {
                            out.push((c, Some(s)));
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn defer_absolute(&mut self, el: NodeId, style: FlowStyle, static_x: f64, static_y: f64) {
        let entry = Pending { el, style, static_x, static_y };
        if entry.style.position == Position::Fixed {
            // The viewport is the containing block.
            self.pending.first_mut().expect("initial containing block").push(entry);
        } else {
            self.pending.last_mut().expect("containing block").push(entry);
        }
    }

    /// Places absolutely positioned boxes against `cb` (a padding box; the
    /// viewport for fixed ones).
    fn place_absolute(&mut self, pending: Vec<Pending>, cb: Rect, fixed_cb: bool) {
        for p in pending {
            let fixed = p.style.position == Position::Fixed || fixed_cb;
            let cb = if p.style.position == Position::Fixed {
                Rect { x: 0.0, y: 0.0, width: self.vw, height: self.vh }
            } else {
                cb
            };
            let s = &p.style;
            let e = self.edges(s, cb.width);
            let left = s.inset[LEFT].resolve(Some(cb.width));
            let right = s.inset[RIGHT].resolve(Some(cb.width));
            let top = s.inset[TOP].resolve(Some(cb.height));
            let bottom = s.inset[BOTTOM].resolve(Some(cb.height));
            let declared_w = s.width.resolve(Some(cb.width)).map(|w| Self::declared_content(s, w, e.horizontal()));
            let available = (cb.width
                - e.margin[LEFT]
                - e.margin[RIGHT]
                - e.horizontal()
                - left.unwrap_or(0.0)
                - right.unwrap_or(0.0))
            .max(0.0);
            let mut content_w = match (declared_w, left, right) {
                (Some(w), _, _) => w,
                (None, Some(_), Some(_)) => available,
                _ => self.shrink_to_fit(p.el, s, available),
            };
            let min_w = s.min_width.resolve(Some(cb.width)).map(|w| Self::declared_content(s, w, e.horizontal()));
            let max_w = s.max_width.resolve(Some(cb.width)).map(|w| Self::declared_content(s, w, e.horizontal()));
            content_w = Self::clamp(content_w, min_w, max_w);
            let border_w = content_w + e.horizontal();
            let bx = match (left, right) {
                (Some(l), _) => cb.x + l + e.margin[LEFT],
                (None, Some(r)) => cb.x + cb.width - r - e.margin[RIGHT] - border_w,
                (None, None) => p.static_x + e.margin[LEFT],
            };
            let declared_h =
                s.height.resolve(Some(cb.height)).map(|h| if s.border_box { (h - e.vertical()).max(0.0) } else { h });
            let order = self.next_order();
            self.pending.push(Vec::new());
            // Lay out content at a provisional y, then move once the height is known.
            let provisional_y = match top {
                Some(t) => cb.y + t + e.margin[TOP],
                None => p.static_y + e.margin[TOP],
            };
            let content_x = bx + e.border[LEFT] + e.padding[LEFT];
            let content_y = provisional_y + e.border[TOP] + e.padding[TOP];
            let first_new = self.order;
            let content_h = self.contents(p.el, s, content_x, content_y, content_w, fixed, (0.0, 0.0));
            let min_h = s.min_height.resolve(Some(cb.height));
            let max_h = s.max_height.resolve(Some(cb.height));
            let mut height = Self::clamp(declared_h.unwrap_or(content_h), min_h, max_h);
            if declared_h.is_none() && top.is_some() && bottom.is_some() {
                height = (cb.height
                    - top.unwrap_or(0.0)
                    - bottom.unwrap_or(0.0)
                    - e.margin[TOP]
                    - e.margin[BOTTOM]
                    - e.vertical())
                .max(0.0);
            }
            let border_h = height + e.vertical();
            let by = match (top, bottom) {
                (Some(_), _) => provisional_y,
                (None, Some(b)) => cb.y + cb.height - b - e.margin[BOTTOM] - border_h,
                (None, None) => provisional_y,
            };
            let delta = by - provisional_y;
            if delta != 0.0 {
                for b in self.out.boxes.values_mut().filter(|b| b.order > first_new) {
                    b.rect.y += delta;
                }
            }
            let rect = Rect { x: bx, y: by, width: border_w, height: border_h };
            self.record(p.el, rect, &e, fixed, true, s.clips_x || s.clips_y, order, content_w, height, content_h);
            let nested = self.pending.pop().unwrap_or_default();
            let padding_box = Rect {
                x: rect.x + e.border[LEFT],
                y: rect.y + e.border[TOP],
                width: rect.width - e.border[LEFT] - e.border[RIGHT],
                height: rect.height - e.border[TOP] - e.border[BOTTOM],
            };
            self.place_absolute(nested, padding_box, fixed);
        }
    }

    /// Shrink-to-fit content width: min(max-content, available).
    fn shrink_to_fit(&self, el: NodeId, style: &FlowStyle, available: f64) -> f64 {
        self.max_content(el, style).min(available)
    }

    /// Max-content width of an element's contents (no wrapping).
    fn max_content(&self, el: NodeId, style: &FlowStyle) -> f64 {
        if style.replaced {
            return style.width.resolve(None).unwrap_or(0.0);
        }
        let mut best: f64 = 0.0;
        let mut line: f64 = 0.0;
        for (child, s) in self.flatten_children(el) {
            match s {
                None => {
                    let text = self.dom.char_data(child).unwrap_or("");
                    line += collapsed_width(text, style.font_size);
                }
                Some(s) if matches!(s.position, Position::Absolute | Position::Fixed) => {}
                Some(s) => {
                    let e = self.edges(&s, 0.0);
                    let inner = match s.width.resolve(None) {
                        Some(w) => Self::declared_content(&s, w, e.horizontal()),
                        None => self.max_content(child, &s),
                    };
                    let w = inner + e.horizontal() + e.margin[LEFT] + e.margin[RIGHT];
                    if s.outer == OuterDisplay::Inline {
                        line += w;
                    } else {
                        best = best.max(line).max(w);
                        line = 0.0;
                    }
                }
            }
        }
        best.max(line)
    }

    /// Lays out a run of inline-level children as line boxes in
    /// [x, x+width); returns the run's height. Items sit on a common
    /// baseline: text by its font ascent (plus half-leading), atomic boxes by
    /// their bottom margin edge, and every line includes the parent's strut.
    #[allow(clippy::too_many_arguments)]
    fn inline_run(
        &mut self,
        run: &[NodeId],
        parent: &FlowStyle,
        x: f64,
        y: f64,
        width: f64,
        fixed: bool,
        shift: (f64, f64),
    ) -> f64 {
        let mut items = Vec::new();
        for &n in run {
            self.collect_inline(n, parent, &[], &mut items);
        }
        let strut = text_extents(parent);
        let mut frags: FxHashMap<NodeId, Rect> = FxHashMap::default();
        let mut order_of: Vec<NodeId> = Vec::new();
        let mut line = Line::default();
        let mut line_y = y;
        for item in items {
            match item {
                InlineItem::Break => {
                    line.extents(strut);
                    line_y += self.flush_line(&mut line, x, line_y, width, fixed, shift, &mut frags, &mut order_of);
                }
                InlineItem::Word { width: w, height: h, space_before, owners, ascent } => {
                    let gap = if line.x > 0.0 { space_before } else { 0.0 };
                    if line.x > 0.0 && line.x + gap + w > width && !parent.nowrap {
                        line_y += self.flush_line(&mut line, x, line_y, width, fixed, shift, &mut frags, &mut order_of);
                        line.push(0.0, w, h, ascent, owners, None);
                    } else {
                        let at = line.x + gap;
                        line.push(at, w, h, ascent, owners, None);
                    }
                    line.extents(strut);
                }
                InlineItem::Atomic { el, style, owners } => {
                    let e = self.edges(&style, width);
                    let content_w = match style.width.resolve(Some(width)) {
                        Some(w) => Self::declared_content(&style, w, e.horizontal()),
                        None => self.shrink_to_fit(el, &style, (width - e.horizontal()).max(0.0)),
                    };
                    let outer_w = content_w + e.horizontal() + e.margin[LEFT] + e.margin[RIGHT];
                    let outer_h = self.atomic_height(el, &style, content_w, width) + e.margin[TOP] + e.margin[BOTTOM];
                    if line.x > 0.0 && line.x + outer_w > width && !parent.nowrap {
                        line_y += self.flush_line(&mut line, x, line_y, width, fixed, shift, &mut frags, &mut order_of);
                    }
                    let at = line.x;
                    line.push(at, outer_w, outer_h, outer_h, owners, Some((el, *style)));
                    line.extents(strut);
                }
                InlineItem::Empty { owners, height, ascent } => {
                    let at = line.x;
                    line.push(at, 0.0, height, ascent, owners, None);
                    line.extents(strut);
                }
            }
        }
        line_y += self.flush_line(&mut line, x, line_y, width, fixed, shift, &mut frags, &mut order_of);
        // Inline element boxes: union of their fragments.
        for el in order_of {
            if let Some(r) = frags.get(&el).copied() {
                let order = self.next_order();
                let positioned = self.src.flow_style(self.dom, el).is_some_and(|s| s.position != Position::Static);
                self.out.boxes.insert(
                    el,
                    FlowBox {
                        rect: r,
                        fixed,
                        client_width: 0.0,
                        client_height: 0.0,
                        scroll_width: 0.0,
                        scroll_height: 0.0,
                        positioned,
                        order,
                        clipped_by: 0,
                    },
                );
            }
        }
        line_y - y
    }

    /// Places a finished line's items at `line_y` and returns its height
    /// (0 for an empty line).
    #[allow(clippy::too_many_arguments)]
    fn flush_line(
        &mut self,
        line: &mut Line,
        x: f64,
        line_y: f64,
        width: f64,
        fixed: bool,
        shift: (f64, f64),
        frags: &mut FxHashMap<NodeId, Rect>,
        order_of: &mut Vec<NodeId>,
    ) -> f64 {
        if line.items.is_empty() {
            *line = Line::default();
            return 0.0;
        }
        let baseline = line_y + line.ascent;
        for it in line.items.drain(..) {
            let left = x + it.x;
            let top = baseline - it.ascent;
            if let Some((el, style)) = it.atomic {
                self.atomic(el, &style, left, top, width, fixed, shift);
                if let Some(r) = self.out.boxes.get(&el).map(|b| b.rect) {
                    for o in &it.owners {
                        union(frags, order_of, *o, r);
                    }
                }
                continue;
            }
            let r = Rect { x: left + shift.0, y: top + shift.1, width: it.width, height: it.height };
            for o in &it.owners {
                union(frags, order_of, *o, r);
            }
        }
        let height = line.ascent + line.descent;
        *line = Line::default();
        height
    }

    fn atomic_height(&self, el: NodeId, style: &FlowStyle, content_w: f64, cb_width: f64) -> f64 {
        let e = self.edges(style, cb_width);
        let content_h = match style.height.resolve(None) {
            Some(h) => {
                if style.border_box {
                    (h - e.vertical()).max(0.0)
                } else {
                    h
                }
            }
            None if style.replaced => 0.0,
            None => self.measure_height(el, style, content_w),
        };
        content_h + e.vertical()
    }

    /// Content height of `el` laid out at `content_w`, without recording.
    fn measure_height(&self, el: NodeId, style: &FlowStyle, content_w: f64) -> f64 {
        let mut probe = Ctx {
            dom: self.dom,
            src: self.src,
            vw: self.vw,
            vh: self.vh,
            out: FlowResult::default(),
            order: 0,
            pending: vec![Vec::new()],
        };
        probe.contents(el, style, 0.0, 0.0, content_w, false, (0.0, 0.0))
    }

    /// Lays out an atomic inline (inline-block / replaced) at (x, y), its
    /// margin edge.
    #[allow(clippy::too_many_arguments)]
    fn atomic(&mut self, el: NodeId, style: &FlowStyle, x: f64, y: f64, cb_width: f64, fixed: bool, shift: (f64, f64)) {
        let e = self.edges(style, cb_width);
        let content_w = match style.width.resolve(Some(cb_width)) {
            Some(w) => Self::declared_content(style, w, e.horizontal()),
            None => self.shrink_to_fit(el, style, (cb_width - e.horizontal()).max(0.0)),
        };
        let mut s = style.clone();
        s.width = Dim::px(if style.border_box { content_w + e.horizontal() } else { content_w });
        s.margin[LEFT] = Dim::px(e.margin[LEFT]);
        s.margin[RIGHT] = Dim::px(e.margin[RIGHT]);
        self.block_child(el, &s, x, y, content_w + e.horizontal() + e.margin[LEFT] + e.margin[RIGHT], fixed, shift);
    }

    /// Flattens an inline-level node into items. `owners`: enclosing inline
    /// elements whose boxes include this content.
    fn collect_inline(&mut self, n: NodeId, parent: &FlowStyle, owners: &[NodeId], items: &mut Vec<InlineItem>) {
        match self.dom.node_type(n) {
            Ok(NodeType::Text) => {
                let text = self.dom.char_data(n).unwrap_or("").to_owned();
                let fs = parent.font_size;
                let (ascent, descent) = text_extents(parent);
                let lh = ascent + descent;
                let space = text_width(" ", fs);
                // Collapsible whitespace becomes one space before the next word
                // (also across element boundaries via a zero-width word).
                let mut pending_space = false;
                for (i, part) in text.split(|c: char| c.is_ascii_whitespace()).enumerate() {
                    if i > 0 {
                        pending_space = true;
                    }
                    if part.is_empty() {
                        continue;
                    }
                    items.push(InlineItem::Word {
                        width: text_width(part, fs),
                        height: lh,
                        space_before: if pending_space { space } else { 0.0 },
                        owners: owners.to_vec(),
                        ascent,
                    });
                    pending_space = false;
                }
                if pending_space {
                    items.push(InlineItem::Word {
                        width: 0.0,
                        height: 0.0,
                        space_before: space,
                        owners: Vec::new(),
                        ascent: 0.0,
                    });
                }
            }
            Ok(NodeType::Element) => {
                let Some(style) = self.src.flow_style(self.dom, n) else { return };
                if self.dom.element(n).is_ok_and(|e| e.is_html() && *e.local_name() == *"br") {
                    items.push(InlineItem::Break);
                    return;
                }
                let mut with = owners.to_vec();
                with.push(n);
                if style.replaced || style.inner != InnerDisplay::Flow {
                    items.push(InlineItem::Atomic { el: n, style: Box::new(style), owners: owners.to_vec() });
                    return;
                }
                let before = items.len();
                for (c, _) in self.flatten_children(n) {
                    self.collect_inline(c, &style, &with, items);
                }
                if items.len() == before {
                    let (ascent, descent) = text_extents(&style);
                    items.push(InlineItem::Empty { owners: with, height: ascent + descent, ascent });
                }
            }
            _ => {}
        }
    }
}

/// Default-font metrics (Chromium's default Times on macOS, measured at
/// 10–32px): ascent ≈ round(0.9em), descent = round(0.25em).
fn font_metrics(font_size: f64) -> (f64, f64) {
    ((0.9 * font_size + 0.5).floor(), (0.25 * font_size + 0.5).floor())
}

/// A text run's (ascent, descent) above/below the baseline, including
/// half-leading: the content area plus half the difference to `line-height`
/// on each side (`normal` = the content area).
fn text_extents(style: &FlowStyle) -> (f64, f64) {
    let (a, d) = font_metrics(style.font_size);
    match style.line_height {
        None => (a, d),
        Some(lh) => {
            let half = (lh - a - d) / 2.0;
            (a + half, d + half)
        }
    }
}

/// A line box being filled.
#[derive(Default)]
struct Line {
    x: f64,
    ascent: f64,
    descent: f64,
    items: Vec<LineItem>,
}

struct LineItem {
    x: f64,
    width: f64,
    height: f64,
    ascent: f64,
    owners: Vec<NodeId>,
    atomic: Option<(NodeId, FlowStyle)>,
}

impl Line {
    fn push(
        &mut self,
        x: f64,
        width: f64,
        height: f64,
        ascent: f64,
        owners: Vec<NodeId>,
        atomic: Option<(NodeId, FlowStyle)>,
    ) {
        self.ascent = self.ascent.max(ascent);
        self.descent = self.descent.max(height - ascent);
        self.x = self.x.max(x + width);
        self.items.push(LineItem { x, width, height, ascent, owners, atomic });
    }

    /// The strut of the containing block's font.
    fn extents(&mut self, (ascent, descent): (f64, f64)) {
        self.ascent = self.ascent.max(ascent);
        self.descent = self.descent.max(descent);
    }
}

fn collapsed_width(text: &str, font_size: f64) -> f64 {
    let words: Vec<&str> = text.split_ascii_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let space = text_width(" ", font_size);
    words.iter().map(|w| text_width(w, font_size)).sum::<f64>() + space * (words.len() - 1) as f64
}

fn union(frags: &mut FxHashMap<NodeId, Rect>, order: &mut Vec<NodeId>, el: NodeId, r: Rect) {
    match frags.get_mut(&el) {
        Some(u) => {
            let x0 = u.x.min(r.x);
            let y0 = u.y.min(r.y);
            let x1 = (u.x + u.width).max(r.x + r.width);
            let y1 = (u.y + u.height).max(r.y + r.height);
            *u = Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
        }
        None => {
            frags.insert(el, r);
            order.push(el);
        }
    }
}

// ----- hit testing ------------------------------------------------------------

impl FlowResult {
    /// Topmost element at document point (x, y) for which `hit` allows hit
    /// testing: positioned boxes paint above in-flow ones, later boxes above
    /// earlier. `clip` reports whether an ancestor clipping overflow excludes
    /// the point.
    pub fn element_at(
        &self,
        x: f64,
        y: f64,
        scroll: (f64, f64),
        hit: impl Fn(NodeId) -> bool,
        clipped: impl Fn(NodeId, f64, f64) -> bool,
    ) -> Option<NodeId> {
        let mut best: Option<(bool, u32, NodeId)> = None;
        for (&el, b) in &self.boxes {
            let (px, py) = if b.fixed { (x - scroll.0, y - scroll.1) } else { (x, y) };
            let r = b.rect;
            if px < r.x || py < r.y || px > r.x + r.width || py > r.y + r.height || r.width <= 0.0 || r.height <= 0.0 {
                continue;
            }
            if !hit(el) || clipped(el, x, y) {
                continue;
            }
            let key = (b.positioned, b.order, el);
            if best.as_ref().is_none_or(|k| (k.0, k.1) < (key.0, key.1)) {
                best = Some(key);
            }
        }
        best.map(|k| k.2)
    }
}
