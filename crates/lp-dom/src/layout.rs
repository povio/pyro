//! Layout queries (CSSOM View: `getBoundingClientRect`, `offset*`,
//! `client*`, `scroll*`, `elementFromPoint`, scrolling) over the geometry an
//! external engine laid the document out as ([`GeometrySource`]: Blitz, in
//! lp-render). lp-dom lays nothing out itself; without a source (builds
//! without `render`) every element reports an empty box.
//!
//! What lp-dom adds on top of the engine's boxes: the window and element
//! scroll offsets (owned here, applied to client rects and hit testing),
//! `position: sticky` offsets, CSS scroll anchoring, hit testing in paint
//! order with overflow clipping, and Chromium's pixel snapping of the
//! integer metrics. Visibility (`isVisible`, `checkVisibility`) comes from
//! the cascade in [`crate::style`].
//!
//! Not ported (honest omissions):
//! - `scroll` / `scrollend` event scheduling and throttling (Element.zig:
//!   2277-2357, Window.zig:796-845): scroll writers return whether anything
//!   moved and the caller dispatches. Likewise the `wheel` DOM event and
//!   MediaQueryList `change` events.
//! - Multiple frames: one [`Layout`] per document; elements whose node
//!   document is another one (template contents, DOMParser documents) have no
//!   owner frame and report "not rendered", as upstream does without a frame.
//! - CSSOM value normalization of inline values (`0` -> `0px`, leading
//!   zeros); values are returned as written.

use markup5ever::{local_name, ns};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::rc::Rc;

use crate::dom::Dom;
pub use crate::geometry::{ExternalBox, ExternalGeometry, ExternalQuads, ExternalSticky, ExternalWord, GeometrySource};
use crate::geometry::{LayoutBox, LayoutResult, PaintChain, PaintEntry, BOTTOM, LEFT, RIGHT, TOP};
/// Overflow clips an out-of-flow box escapes while walking up to its
/// containing block (`Layout::clip_escape`).
#[derive(Copy, Clone, PartialEq, Eq)]
enum ClipEscape {
    None,
    /// `position: fixed`: no ancestor clips (the viewport contains it).
    Viewport,
    /// `position: absolute`: non-positioned ancestors do not clip, up to the
    /// nearest positioned one.
    Positioned,
}
/// Geometry memoization is confined to one synchronous hit test. Input
/// handlers can mutate the DOM before the next pointer event.
#[derive(Default)]
struct HitTestGeometry {
    rects: FxHashMap<NodeId, Rect>,
    scroll_offsets: FxHashMap<NodeId, (f64, f64)>,
    has_element_scroll: bool,
    has_sticky: bool,
}

use crate::media::Viewport;
use crate::node::{DocumentKind, NodeId, NodeKind, NIL};
use crate::style::{CheckVisibilityOptions, ComputedStyle, Display, Position, ScrollAxes, StyleEngine, UserSelect};
use style::values::specified::box_::PositionProperty;

/// A DOMRect's values.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn left(&self) -> f64 {
        self.x.min(self.x + self.width)
    }
    pub fn top(&self) -> f64 {
        self.y.min(self.y + self.height)
    }
    pub fn right(&self) -> f64 {
        self.x.max(self.x + self.width)
    }
    pub fn bottom(&self) -> f64 {
        self.y.max(self.y + self.height)
    }
}

impl Rect {
    /// The rect in layout units, as Chromium reports untransformed layout
    /// geometry: position and size each truncated toward zero to 1/64 px
    /// (measured: 100.7 -> 100.6875, -10.7 -> -10.6875, 33.33 -> 33.328125).
    /// Edges computed from it are then exact (a quad's right edge minus its
    /// left edge is the width again). Layout itself keeps full precision.
    pub fn layout_units(self) -> Rect {
        Rect {
            x: layout_unit(self.x),
            y: layout_unit(self.y),
            width: layout_unit(self.width),
            height: layout_unit(self.height),
        }
    }
}

/// A length in layout units (1/64 px), truncated toward zero like Chromium's
/// LayoutUnit conversion.
pub fn layout_unit(v: f64) -> f64 {
    if v.is_finite() {
        (v * 64.0).trunc() / 64.0
    } else {
        v
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Axis {
    Width,
    Height,
}

/// The integer CSSOM View element metrics (`Layout::metric`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Metric {
    ScrollWidth,
    ScrollHeight,
    ClientWidth,
    ClientHeight,
    OffsetTop,
    OffsetLeft,
    OffsetWidth,
    OffsetHeight,
}

/// Chromium's `LayoutUnit::Round` (half up) of a length in layout units.
fn round_layout_unit(v: f64) -> f64 {
    (layout_unit(v) + 0.5).floor()
}

/// Chromium's `SnapSizeToPixel`: the pixel-snapped size of a box `size`
/// long starting at `location`, so adjacent snapped boxes tile.
fn snap_size_to_pixel(size: f64, location: f64) -> f64 {
    round_layout_unit(location + size) - round_layout_unit(location)
}

/// An axis' value, and whether it's explicit or the default
/// (Element.zig:1591-1600).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AxisState {
    pub value: f64,
    pub explicit: bool,
}

/// What a scroll along one axis lands on (Element.zig:1781-1784).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScrollTarget {
    Viewport,
    Container(NodeId),
}

/// `ScrollWrite` (Element.zig:2213-2231): absolute (`to`) or relative (`by`)
/// per-axis offsets; `None` leaves that axis alone.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScrollWrite {
    To { left: Option<i32>, top: Option<i32> },
    By { left: Option<i32>, top: Option<i32> },
}

impl ScrollWrite {
    fn target(self, axis: Axis, current: u32) -> Option<i64> {
        let (ScrollWrite::To { left, top } | ScrollWrite::By { left, top }) = self;
        let value = match axis {
            Axis::Width => left,
            Axis::Height => top,
        }? as i64;
        Some(match self {
            ScrollWrite::To { .. } => value,
            ScrollWrite::By { .. } => current as i64 + value,
        })
    }
}

/// Layout state of one document: its style engine (which owns the
/// viewport), its geometry source, the window scroll offset and
/// per-element scroll offsets.
pub struct Layout {
    style: StyleEngine,
    window_scroll: (u32, u32),
    scroll_generation: u64,
    element_scroll: FxHashMap<NodeId, (u32, u32)>,
    /// CSS Scroll Anchoring: the layout each vertically scrolled element
    /// was last settled in, to select its anchor in when the layout next
    /// changes.
    scroll_anchors: FxHashMap<NodeId, Rc<LayoutResult>>,
    /// The engine laying the document out.
    external: Option<Rc<dyn GeometrySource>>,
    /// The observation stamp and viewport the engine was last synced at.
    external_synced: std::cell::Cell<Option<SyncedAt>>,
    /// Syncs so far (the whole document's geometry is read again after one).
    syncs: std::cell::Cell<u64>,
    /// The whole document's geometry, with the sync it was read at.
    result: RefCell<Option<(u64, Rc<LayoutResult>)>>,
}

/// The observation stamp and viewport a geometry source was synced at.
type SyncedAt = ((u64, u64, u32, u32), (u32, u32));

/// Makes the geometry source of a new document.
pub type GeometrySourceFactory = Rc<dyn Fn() -> Rc<dyn GeometrySource>>;

thread_local! {
    static GEOMETRY_SOURCE: RefCell<Option<GeometrySourceFactory>> = const { RefCell::new(None) };
}

/// Installs the factory of geometry sources for this thread's new documents.
pub fn set_geometry_source_factory(factory: Option<GeometrySourceFactory>) {
    GEOMETRY_SOURCE.with(|f| *f.borrow_mut() = factory);
}

impl Layout {
    pub fn new(doc: NodeId) -> Self {
        Self::with_style(StyleEngine::new(doc))
    }

    pub fn with_viewport(doc: NodeId, viewport: Viewport) -> Self {
        Self::with_style(StyleEngine::with_viewport(doc, viewport))
    }

    /// A layout of the style's document, with this thread's geometry source
    /// (see [`set_geometry_source_factory`]).
    pub fn with_style(style: StyleEngine) -> Self {
        let external = GEOMETRY_SOURCE.with(|f| f.borrow().as_ref().map(|make| make()));
        style.set_style_engine(external.clone());
        Layout {
            style,
            window_scroll: (0, 0),
            scroll_generation: 0,
            element_scroll: FxHashMap::default(),
            scroll_anchors: FxHashMap::default(),
            external,
            external_synced: std::cell::Cell::new(None),
            syncs: std::cell::Cell::new(0),
            result: RefCell::new(None),
        }
    }

    /// Takes the geometry from `source` (see [`GeometrySource`]); `None`:
    /// nothing is laid out.
    pub fn set_geometry_source(&mut self, source: Option<Rc<dyn GeometrySource>>) {
        self.style.set_style_engine(source.clone());
        self.external = source;
        self.external_synced.set(None);
        self.result.borrow_mut().take();
    }

    /// The geometry source, brought up to the document (synced when
    /// anything it lays out changed since the last query).
    pub fn synced_geometry_source(&self, dom: &Dom) -> Option<&Rc<dyn GeometrySource>> {
        let source = self.external.as_ref().filter(|s| s.active())?;
        let key = self.sync_key(dom);
        if self.external_synced.get() != Some(key) || source.stale() {
            self.style.before_recalc(dom);
            source.sync(dom, self);
            self.external_synced.set(Some(key));
            self.syncs.set(self.syncs.get() + 1);
            self.style.engine_synced(dom);
        }
        Some(source)
    }

    fn sync_key(&self, dom: &Dom) -> ((u64, u64, u32, u32), (u32, u32)) {
        let v = self.viewport();
        (self.style.observation_stamp(dom), (v.width, v.height))
    }

    /// The document's URL, for the geometry source.
    pub fn set_base_url(&self, url: &str) {
        if let Some(source) = &self.external {
            source.set_base_url(url);
        }
    }

    /// The whole document's geometry, read again when the DOM, styles,
    /// focus/hover or viewport changed.
    pub fn layout_result(&self, dom: &Dom) -> Rc<LayoutResult> {
        let Some(source) = self.synced_geometry_source(dom) else { return Rc::default() };
        let key = self.syncs.get();
        if let Some((k, r)) = self.result.borrow().as_ref() {
            if *k == key {
                return r.clone();
            }
        }
        let started = flow_stats::enabled().then(std::time::Instant::now);
        let result = Rc::new(source.geometry().map(|g| LayoutResult::from_external(dom, g)).unwrap_or_default());
        if let Some(t) = started {
            flow_stats::record(None, self.sync_key(dom), t.elapsed());
        }
        *self.result.borrow_mut() = Some((key, result.clone()));
        result
    }

    /// One element's box, synced only when something the engine lays out
    /// changed: no whole-document work per query.
    fn layout_box(&self, dom: &Dom, el: NodeId) -> Option<LayoutBox> {
        let source = self.synced_geometry_source(dom)?;
        source.element_box(crate::snapshot::render_key(el)).map(|b| LayoutBox::from_external(&b, 0))
    }

    /// Changes when DOM/style/viewport/focus/hover or a scroll offset changes.
    pub fn observation_stamp(&self, dom: &Dom) -> ((u64, u64, u32, u32), u64) {
        (self.style.observation_stamp(dom), self.scroll_generation)
    }

    pub fn style(&self) -> &StyleEngine {
        &self.style
    }

    pub fn style_mut(&mut self) -> &mut StyleEngine {
        &mut self.style
    }

    pub fn document(&self) -> NodeId {
        self.style.document()
    }

    pub fn viewport(&self) -> &Viewport {
        self.style.viewport()
    }

    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.style.set_viewport(viewport);
    }

    /// `window.innerWidth` (Window.zig:766-768).
    pub fn inner_width(&self) -> u32 {
        self.viewport().width
    }

    /// `window.innerHeight` (Window.zig:772-774).
    pub fn inner_height(&self) -> u32 {
        self.viewport().height
    }

    /// `window.devicePixelRatio` (Window.zig:776-778).
    pub fn device_pixel_ratio(&self) -> f64 {
        self.viewport().device_scale_factor
    }

    fn owned(&self, dom: &Dom, el: NodeId) -> Option<u32> {
        let idx = dom.check(el).ok()?;
        let n = dom.n(idx);
        (n.is_element() && n.owner_doc == self.document().idx).then_some(idx)
    }

    fn html_local<'a>(&self, dom: &'a Dom, idx: u32) -> Option<&'a str> {
        dom.n(idx).element().filter(|e| e.name.ns == ns!(html)).map(|e| &*e.name.local)
    }

    // ----- visibility --------------------------------------------------------

    /// `Element.isVisible` (Element.zig:1570-1573): not in a display:none
    /// subtree.
    pub fn is_visible(&self, dom: &Dom, el: NodeId) -> bool {
        !self.style.is_hidden(dom, el, CheckVisibilityOptions::default())
    }

    /// `Element.checkVisibility(opts)` (Element.zig:1582-1589).
    pub fn check_visibility(&self, dom: &Dom, el: NodeId, opts: CheckVisibilityOptions) -> bool {
        !self.style.is_hidden(dom, el, opts)
    }

    // ----- boxes -------------------------------------------------------------

    /// Element.zig:1639-1649: the document's `<html>`, or a `<body>` under it.
    pub fn is_root_container(&self, dom: &Dom, el: NodeId) -> bool {
        dom.check(el).is_ok_and(|i| self.is_root_idx(dom, i))
    }

    fn is_root_idx(&self, dom: &Dom, idx: u32) -> bool {
        let parent = dom.n(idx).parent;
        if parent == NIL {
            return false;
        }
        match self.html_local(dom, idx) {
            Some("html") => matches!(dom.n(parent).kind, NodeKind::Document(_)),
            Some("body") => self.html_local(dom, parent) == Some("html") && self.is_root_idx(dom, parent),
            _ => false,
        }
    }

    /// `Document.isQuirksMode` (Document.zig:160-171): an HTML document
    /// without a doctype child.
    fn is_quirks(&self, dom: &Dom) -> bool {
        let Ok(d) = dom.check(self.document()) else { return false };
        match &dom.n(d).kind {
            NodeKind::Document(dd) if dd.kind == DocumentKind::Html => {
                !dom.child_idx_iter(d).any(|c| matches!(dom.n(c).kind, NodeKind::Doctype(_)))
            }
            _ => false,
        }
    }

    /// `clientWidth` / `clientHeight` (Element.zig:1655-1668).
    pub fn client_width(&self, dom: &Dom, el: NodeId) -> f64 {
        self.client_axis(dom, el, Axis::Width)
    }

    pub fn client_height(&self, dom: &Dom, el: NodeId) -> f64 {
        self.client_axis(dom, el, Axis::Height)
    }

    fn client_axis(&self, dom: &Dom, el: NodeId, axis: Axis) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        if let Some(v) = self.viewport_axis(dom, el, axis) {
            return v;
        }
        self.layout_box(dom, el).map_or(0.0, |b| match axis {
            Axis::Width => b.client_width,
            Axis::Height => b.client_height,
        })
    }

    /// The content box size of `el` (an iframe's: its child document's
    /// viewport); `None` when it has no box.
    pub fn content_size(&self, dom: &Dom, el: NodeId) -> Option<(f64, f64)> {
        let b = self.layout_box(dom, el)?;
        let edges = |a: usize, b2: usize| b.border[a] + b.border[b2] + b.padding[a] + b.padding[b2];
        Some(((b.rect.width - edges(LEFT, RIGHT)).max(0.0), (b.rect.height - edges(TOP, BOTTOM)).max(0.0)))
    }

    /// The content box's offset inside `el`'s border box (left and top
    /// border plus padding): where an iframe's child viewport starts.
    pub fn content_offset(&self, dom: &Dom, el: NodeId) -> Option<(f64, f64)> {
        let b = self.layout_box(dom, el)?;
        Some((b.border[LEFT] + b.padding[LEFT], b.border[TOP] + b.padding[TOP]))
    }

    /// Element.zig:1677-1692: the root scroller (html in standards mode, body
    /// in quirks mode) reports the viewport for clientWidth/Height.
    fn viewport_axis(&self, dom: &Dom, el: NodeId, axis: Axis) -> Option<f64> {
        let idx = dom.check(el).ok()?;
        if !self.is_root_idx(dom, idx) {
            return None;
        }
        if (self.html_local(dom, idx) == Some("body")) != self.is_quirks(dom) {
            return None;
        }
        let v = self.viewport();
        Some(match axis {
            Axis::Width => v.width as f64,
            Axis::Height => v.height as f64,
        })
    }

    /// Element.zig:1671-1675.
    fn root_scroll_size(&self, dom: &Dom, el: NodeId, axis: Axis) -> Option<f64> {
        self.viewport_axis(dom, el, axis)?;
        let (w, h) = self.document_scroll_size(dom);
        Some(if axis == Axis::Width { w } else { h })
    }

    /// `scrollHeight` (Element.zig:1811-1824).
    pub fn scroll_height(&self, dom: &Dom, el: NodeId) -> f64 {
        self.scroll_axis_size(dom, el, Axis::Height)
    }

    /// `scrollWidth` (Element.zig:1826-1840).
    pub fn scroll_width(&self, dom: &Dom, el: NodeId) -> f64 {
        self.scroll_axis_size(dom, el, Axis::Width)
    }

    fn scroll_axis_size(&self, dom: &Dom, el: NodeId, axis: Axis) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        if let Some(size) = self.root_scroll_size(dom, el, axis) {
            return size;
        }
        let text = if axis == Axis::Width { self.text_control_scroll_width(dom, el) } else { None };
        self.layout_box(dom, el).map_or(0.0, |b| match axis {
            Axis::Width => b.scroll_width.max(b.client_width).max(text.unwrap_or(0.0)),
            Axis::Height => b.scroll_height.max(b.client_height),
        })
    }

    /// The scrollable width of a single-line text `<input>`: its value's
    /// width plus the padding. The value sits in an inner editor that
    /// scrolls horizontally whatever the input's `overflow` is, and the
    /// input's `scrollLeft` is that editor's (as in Chrome). `None` for
    /// other elements and an empty value.
    fn text_control_scroll_width(&self, dom: &Dom, el: NodeId) -> Option<f64> {
        use crate::forms::InputType as T;
        let e = dom.element(el).ok()?;
        if !(e.is_html() && &*e.name.local == "input") {
            return None;
        }
        if !matches!(dom.input_type(el).ok()?, T::Text | T::Search | T::Tel | T::Url | T::Email | T::Password) {
            return None;
        }
        let value = dom.value(el).ok().filter(|v| !v.is_empty())?;
        let b = self.layout_box(dom, el)?;
        let size = self.style.computed_font_size(dom, Some(el));
        // A fractional weight (`350.5`) reads as the default, as its
        // serialization did not parse as an integer.
        let weight = self
            .style
            .stylo_style(dom, el)
            .map(|s| s.get_font().font_weight.value())
            .filter(|w| w.fract() == 0.0)
            .map_or(400, |w| w as u16);
        Some(crate::text_metrics::text_width(&value, size, weight) + b.padding[LEFT] + b.padding[RIGHT])
    }

    /// `scrollExtent` (Element.zig:1845-1858): how far `el` scrolls on
    /// `axis`; `None` for the root containers (the window scrolls).
    fn scroll_extent(&self, dom: &Dom, el: NodeId, axis: Axis) -> Option<f64> {
        if self.is_root_container(dom, el) {
            return None;
        }
        // Only scroll containers scroll; others stay at 0 (as in Chrome).
        let clips = self.style.overflow_clip_axes(dom, el);
        let text_control = axis == Axis::Width && self.text_control_scroll_width(dom, el).is_some();
        if !(if axis == Axis::Width { clips.x } else { clips.y }) && !text_control {
            return Some(0.0);
        }
        let client = self.client_axis(dom, el, axis);
        Some((self.scroll_axis_size(dom, el, axis) - client).max(0.0))
    }

    /// `offsetWidth` / `offsetHeight` (Element.zig:1915-1927).
    pub fn offset_width(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        self.layout_box(dom, el).map_or(0.0, |b| b.rect.width)
    }

    pub fn offset_height(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        self.layout_box(dom, el).map_or(0.0, |b| b.rect.height)
    }

    /// `offsetTop`/`offsetLeft`: border-box offset from the offset parent's
    /// border box (document coordinates for the body/none).
    fn box_offset(&self, dom: &Dom, el: NodeId) -> (f64, f64) {
        let Some(b) = self.layout_box(dom, el) else { return (0.0, 0.0) };
        let parent = self
            .offset_parent(dom, el)
            .filter(|&p| !self.is_root_container(dom, p))
            .and_then(|p| self.layout_box(dom, p));
        match parent {
            Some(p) => (b.rect.x - p.rect.x, b.rect.y - p.rect.y),
            None => (b.rect.x, b.rect.y),
        }
    }

    /// `offsetTop` (Element.zig:1929-1934).
    pub fn offset_top(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        self.box_offset(dom, el).1
    }

    /// `offsetLeft` (Element.zig:1936-1941).
    pub fn offset_left(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        self.box_offset(dom, el).0
    }

    /// An element metric as script reads it, pixel-snapped to whole pixels
    /// like Chromium (Gap 36: `scrollWidth > clientWidth` must not come from
    /// a fractional width). Offsets round; sizes snap at the box's offset
    /// (client and scroll sizes at its padding edge) so that equal
    /// fractional sizes snap alike.
    pub fn metric(&self, dom: &Dom, el: NodeId, metric: Metric) -> f64 {
        let raw = match metric {
            Metric::ScrollWidth => self.scroll_width(dom, el),
            Metric::ScrollHeight => self.scroll_height(dom, el),
            Metric::ClientWidth => self.client_width(dom, el),
            Metric::ClientHeight => self.client_height(dom, el),
            Metric::OffsetTop => self.offset_top(dom, el),
            Metric::OffsetLeft => self.offset_left(dom, el),
            Metric::OffsetWidth => self.offset_width(dom, el),
            Metric::OffsetHeight => self.offset_height(dom, el),
        };
        if raw == 0.0 {
            return raw;
        }
        let border = |side: usize| self.layout_box(dom, el).map_or(0.0, |b| b.border[side]);
        match metric {
            Metric::OffsetTop | Metric::OffsetLeft => round_layout_unit(raw),
            Metric::OffsetWidth => snap_size_to_pixel(raw, self.offset_left(dom, el)),
            Metric::OffsetHeight => snap_size_to_pixel(raw, self.offset_top(dom, el)),
            Metric::ScrollWidth | Metric::ClientWidth => {
                snap_size_to_pixel(raw, self.offset_left(dom, el) + border(LEFT))
            }
            Metric::ScrollHeight | Metric::ClientHeight => {
                snap_size_to_pixel(raw, self.offset_top(dom, el) + border(TOP))
            }
        }
    }

    /// `clientTop` / `clientLeft`: the top and left border widths.
    pub fn client_top(&self, dom: &Dom, el: NodeId) -> f64 {
        self.client_border(dom, el, TOP)
    }

    pub fn client_left(&self, dom: &Dom, el: NodeId) -> f64 {
        self.client_border(dom, el, LEFT)
    }

    fn client_border(&self, dom: &Dom, el: NodeId, side: usize) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        self.layout_box(dom, el).map_or(0.0, |b| round_layout_unit(b.border[side]))
    }

    /// `offsetParent`, including positioned ancestors styled through stylesheets.
    pub fn offset_parent(&self, dom: &Dom, el: NodeId) -> Option<NodeId> {
        if !dom.is_connected(el).ok()? || !self.is_visible(dom, el) {
            return None;
        }
        let idx = dom.check(el).ok()?;
        if matches!(self.html_local(dom, idx), Some("html" | "body")) {
            return None;
        }
        let self_position = self.position_of(dom, el);
        if self_position == Position::Fixed {
            return None;
        }
        let self_static = !self_position.is_positioned();
        let mut node = dom.n(idx).parent;
        while node != NIL {
            match &dom.n(node).kind {
                // this always pokes through the shadow dom
                NodeKind::Fragment(f) if f.shadow.is_some() => {
                    node = f.host;
                    continue;
                }
                NodeKind::Element(_) => {}
                _ => break,
            }
            let tag = self.html_local(dom, node);
            if tag == Some("body") {
                return Some(dom.id_of(node));
            }
            if self.position_of(dom, dom.id_of(node)).is_positioned() {
                return Some(dom.id_of(node));
            }
            if self_static && matches!(tag, Some("td" | "th" | "table")) {
                return Some(dom.id_of(node));
            }
            node = dom.n(node).parent;
        }
        None
    }

    /// `el`'s computed `position` (`Static` without a style).
    fn position_of(&self, dom: &Dom, el: NodeId) -> Position {
        self.computed(dom, el).position
    }

    /// `el`'s memoized computed style (the default without one: static,
    /// not `display: contents`, ...).
    fn computed(&self, dom: &Dom, el: NodeId) -> ComputedStyle {
        self.style.computed(dom, el).unwrap_or_default()
    }

    // ----- rects -------------------------------------------------------------

    /// `getBoundingClientRect` values (Element.zig:1724-1742): all zero when
    /// the element isn't rendered.
    pub fn bounding_client_rect(&self, dom: &Dom, el: NodeId) -> Rect {
        if !self.is_visible(dom, el) {
            return Rect::default();
        }
        self.bounding_client_rect_for_visible(dom, el)
    }

    /// A text node's line-box rect (the union of its words) in client
    /// coordinates; `None` when it is not laid out. Range geometry over text
    /// uses it (Chromium reports text boxes, even inside a `display:
    /// contents` parent that has no box).
    pub fn text_rect(&self, dom: &Dom, text: NodeId) -> Option<Rect> {
        let result = self.layout_result(dom);
        let &(r, fixed) = result.texts.get(&text)?;
        let (dx, dy) = self.ancestor_scroll_offset(dom, text);
        let (kx, ky) = self.sticky_shift(dom, &result, text);
        let (sx, sy) = if fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
        Some(Rect { x: r.x - sx - dx + kx, y: r.y - sy - dy + ky, width: r.width, height: r.height })
    }

    /// The caret position (text node, UTF-16 offset) under a viewport
    /// point within `within` (the element under the point, as Chromium
    /// resolves positions in the hit element): the character boundary
    /// nearest to the point in the visible word nearest to it on its line
    /// (else on the nearest line), and whether the point is on that word.
    /// Words clipped away by an overflow ancestor are not candidates;
    /// `selectable` filters the text nodes considered.
    pub fn caret_from_point(
        &self,
        dom: &Dom,
        within: NodeId,
        x: f64,
        y: f64,
        selectable: &dyn Fn(NodeId) -> bool,
    ) -> Option<(NodeId, u32, bool)> {
        let result = self.layout_result(dom);
        let (wx, wy) = (self.window_scroll.0 as f64, self.window_scroll.1 as f64);
        let mut best: Option<((f64, f64), u32, NodeId, f64)> = None;
        for (i, run) in result.text_runs.iter().enumerate() {
            if !dom.is_connected(run.node).unwrap_or(false)
                || !dom.contains(within, run.node).unwrap_or(false)
                || !selectable(run.node)
            {
                continue;
            }
            if dom.parent_element(run.node).ok().flatten().is_some_and(|p| !self.is_visible(dom, p)) {
                continue;
            }
            let (dx, dy) = self.ancestor_scroll_offset(dom, run.node);
            let (kx, ky) = self.sticky_shift(dom, &result, run.node);
            let (sx, sy) = if run.fixed { (0.0, 0.0) } else { (wx, wy) };
            let (left, top) = (run.rect.x - sx - dx + kx, run.rect.y - sy - dy + ky);
            if self.text_clipped(dom, run.node, left, top, run.rect.width, run.rect.height) {
                continue;
            }
            let gap = |p: f64, lo: f64, len: f64| {
                if p < lo {
                    lo - p
                } else if p > lo + len {
                    p - lo - len
                } else {
                    0.0
                }
            };
            let key = (gap(y, top, run.rect.height), gap(x, left, run.rect.width));
            if best.is_none_or(|b| key < b.0) {
                best = Some((key, i as u32, run.node, left));
            }
        }
        // The flag: the point is on the word itself.
        best.map(|(key, i, node, left)| (node, result.text_runs[i as usize].offset_at(x - left), key == (0.0, 0.0)))
    }

    /// Whether a text box (viewport coordinates) lies entirely outside the
    /// visible area of an overflow-clipping ancestor of `node`.
    fn text_clipped(&self, dom: &Dom, node: NodeId, x: f64, y: f64, w: f64, h: f64) -> bool {
        self.clipped_by_ancestor(dom, node, |a, clips| {
            let r = self.bounding_client_rect_for_visible(dom, a);
            (clips.x && (x + w <= r.x || x >= r.x + r.width)) || (clips.y && (y + h <= r.y || y >= r.y + r.height))
        })
    }

    /// Whether an overflow-clipping ancestor of `node` clips it, as `out`
    /// decides for each one that applies (its clipped axes given). An
    /// out-of-flow box escapes the clips between it and its containing
    /// block (CSS 2.1 10.1): every ancestor for `position: fixed` (the
    /// viewport; transforms are not laid out), the non-positioned ones below
    /// the nearest positioned ancestor for `position: absolute`. In-flow
    /// content escapes with its out-of-flow ancestors.
    fn clipped_by_ancestor(&self, dom: &Dom, node: NodeId, mut out: impl FnMut(NodeId, ScrollAxes) -> bool) -> bool {
        self.for_each_clip(dom, node, &mut out)
    }

    /// [`Self::clipped_by_ancestor`]'s clips of `node` as hit testing reads
    /// them: computed once per element of `result`.
    fn hit_test_clips(&self, dom: &Dom, result: &LayoutResult, node: NodeId) -> Rc<[(NodeId, ScrollAxes)]> {
        if let Some(clips) = result.hit.borrow().clips.get(&node) {
            return clips.clone();
        }
        let mut clips = Vec::new();
        self.for_each_clip(dom, node, &mut |a, axes| {
            clips.push((a, axes));
            false
        });
        let clips: Rc<[(NodeId, ScrollAxes)]> = clips.into();
        result.hit.borrow_mut().clips.insert(node, clips.clone());
        clips
    }

    /// Calls `out` with each overflow clip that applies to `node`, nearest
    /// first, until it returns true (then: true).
    fn for_each_clip(&self, dom: &Dom, node: NodeId, out: &mut dyn FnMut(NodeId, ScrollAxes) -> bool) -> bool {
        let mut escape = self.clip_escape(dom, node);
        let mut cur = dom.parent_element(node).ok().flatten();
        while let Some(a) = cur {
            if self.is_root_container(dom, a) {
                break;
            }
            let c = self.computed(dom, a);
            if c.display_contents {
                cur = dom.parent_element(a).ok().flatten();
                continue;
            }
            let positioned = c.position.is_positioned();
            let applies = match escape {
                ClipEscape::None => true,
                ClipEscape::Viewport => false,
                ClipEscape::Positioned => positioned,
            };
            if applies {
                let clips = ScrollAxes { x: c.overflow_x_clips, y: c.overflow_y_clips };
                if (clips.x || clips.y) && out(a, clips) {
                    return true;
                }
            }
            if escape == ClipEscape::None || (escape == ClipEscape::Positioned && positioned) {
                escape = self.clip_escape(dom, a);
            }
            cur = dom.parent_element(a).ok().flatten();
        }
        false
    }

    /// Which overflow clips above `el` do not apply to it, from its
    /// `position`.
    fn clip_escape(&self, dom: &Dom, el: NodeId) -> ClipEscape {
        if dom.node_type(el).ok() != Some(crate::node::NodeType::Element) {
            return ClipEscape::None;
        }
        match self.position_of(dom, el) {
            Position::Fixed => ClipEscape::Viewport,
            Position::Absolute => ClipEscape::Positioned,
            _ => ClipEscape::None,
        }
    }

    /// `el`'s `position` as laid out (`Static` when it has no box).

    /// Whether `node` (or a text node's parent) cannot be selected: the
    /// nearest element up the tree with a `user-select` other than `auto`
    /// says `none` (`text`, `all` or `contain` inside a `none` ancestor
    /// select again).
    pub fn user_select_none(&self, dom: &Dom, node: NodeId) -> bool {
        let mut cur = if dom.node_type(node).ok() == Some(crate::node::NodeType::Text) {
            dom.parent_element(node).ok().flatten()
        } else {
            Some(node)
        };
        while let Some(e) = cur {
            match self.computed(dom, e).user_select {
                UserSelect::None => return true,
                UserSelect::Text | UserSelect::All | UserSelect::Contain => return false,
                UserSelect::Auto => {}
            }
            cur = dom.parent_element(e).ok().flatten();
        }
        false
    }

    /// The bounding rect, or `None` when the element isn't rendered.
    pub fn client_rect(&self, dom: &Dom, el: NodeId) -> Option<Rect> {
        self.is_visible(dom, el).then(|| self.bounding_client_rect_for_visible(dom, el))
    }

    /// `el`'s CSS boxes as quads in client coordinates (CDP
    /// `DOM.getBoxModel`/`getContentQuads`): the engine's quads moved by the
    /// same scroll and sticky offsets as its client rect. `None` when it is
    /// not rendered, has no CSS box (SVG content) or the engine reports none.
    pub fn box_quads(&self, dom: &Dom, el: NodeId) -> Option<ExternalQuads> {
        if !self.is_visible(dom, el) || self.svg_client_rect(dom, el).is_some() {
            return None;
        }
        let source = self.synced_geometry_source(dom)?;
        let mut q = source.element_quads(crate::snapshot::render_key(el))?;
        let (dx, dy) = self.ancestor_scroll_offset(dom, el);
        let (kx, ky) = match source.has_sticky() {
            true => self.sticky_shift(dom, &self.layout_result(dom), el),
            false => (0.0, 0.0),
        };
        let (sx, sy) = if q.fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
        let (ox, oy) = (kx - sx - dx, ky - sy - dy);
        for quad in std::iter::once(&mut q.border).chain(q.fragments.iter_mut()) {
            for i in 0..4 {
                quad[2 * i] += ox;
                quad[2 * i + 1] += oy;
            }
        }
        Some(q)
    }

    /// `getClientRects` (Element.zig:1744-1751): one rect, or none when not
    /// rendered.
    pub fn client_rects(&self, dom: &Dom, el: NodeId) -> Vec<Rect> {
        self.client_rect(dom, el).into_iter().collect()
    }

    /// Sum of the scroll offsets of `el`'s ancestor elements (x, y).
    ///
    /// Deviation from upstream (which only subtracts the window scroll,
    /// Element.zig:1734-1741): scrolling an overflow container moves its
    /// descendants' client rects, so `scrollTop` writes are observable through
    /// geometry (IntersectionObserver, getBoundingClientRect) and hit testing
    /// stays consistent with the rects.
    pub fn ancestor_scroll_offset(&self, dom: &Dom, el: NodeId) -> (f64, f64) {
        if self.element_scroll.values().all(|&(x, y)| x == 0 && y == 0) {
            return (0.0, 0.0);
        }
        let Ok(mut cur) = dom.check(el) else { return (0.0, 0.0) };
        let (mut dx, mut dy) = (0.0, 0.0);
        loop {
            let parent = dom.n(cur).parent;
            if parent == NIL {
                return (dx, dy);
            }
            if let Some(&(x, y)) = self.element_scroll.get(&dom.id_of(parent)) {
                dx += x as f64;
                dy += y as f64;
            }
            cur = parent;
        }
    }

    fn bounding_client_rect_for_visible(&self, dom: &Dom, el: NodeId) -> Rect {
        let (dx, dy) = self.ancestor_scroll_offset(dom, el);
        // SVG content (anything in an `<svg>` but a nested `<svg>`'s own box
        // aside) has no CSS box: its geometry is its bbox.
        if let Some(r) = self.svg_client_rect(dom, el) {
            return r;
        }
        let Some(b) = self.layout_box(dom, el) else { return Rect::default() };
        let (kx, ky) = match self.external.as_ref().is_some_and(|s| s.has_sticky()) {
            true => self.sticky_shift(dom, &self.layout_result(dom), el),
            false => (0.0, 0.0),
        };
        let (sx, sy) = if b.fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
        Rect { x: b.rect.x - sx - dx + kx, y: b.rect.y - sy - dy + ky, width: b.rect.width, height: b.rect.height }
    }

    fn hit_test_client_rect(&self, dom: &Dom, result: &LayoutResult, el: NodeId, cache: &mut HitTestGeometry) -> Rect {
        if let Some(rect) = cache.rects.get(&el) {
            return *rect;
        }
        // SVG shapes retain their bbox mapping, rather than using the empty
        // CSS box returned for inline SVG content by the external engine.
        let rect = if let Some(rect) = self.svg_client_rect_with(dom, el, |svg| {
            let bounds = *result.get(svg)?;
            Some((bounds, self.hit_test_client_rect(dom, result, svg, cache)))
        }) {
            rect
        } else if let Some(b) = result.get(el) {
            let (dx, dy) = if cache.has_element_scroll {
                self.hit_test_scroll_offset(dom, el, &mut cache.scroll_offsets)
            } else {
                (0.0, 0.0)
            };
            let (kx, ky) = if cache.has_sticky { self.sticky_shift(dom, result, el) } else { (0.0, 0.0) };
            let (sx, sy) =
                if b.fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
            Rect { x: b.rect.x - sx - dx + kx, y: b.rect.y - sy - dy + ky, width: b.rect.width, height: b.rect.height }
        } else {
            Rect::default()
        };
        cache.rects.insert(el, rect);
        rect
    }

    fn hit_test_scroll_offset(&self, dom: &Dom, el: NodeId, cache: &mut FxHashMap<NodeId, (f64, f64)>) -> (f64, f64) {
        let Ok(mut current) = dom.check(el) else { return (0.0, 0.0) };
        let mut path = Vec::new();
        let mut offset = loop {
            let node = dom.id_of(current);
            if let Some(&offset) = cache.get(&node) {
                break offset;
            }
            path.push(node);
            let parent = dom.n(current).parent;
            if parent == NIL {
                break (0.0, 0.0);
            }
            current = parent;
        };
        for node in path.into_iter().rev() {
            let parent = dom.n(dom.check(node).expect("node in the current hit-test tree")).parent;
            if parent != NIL {
                if let Some(&(x, y)) = self.element_scroll.get(&dom.id_of(parent)) {
                    offset.0 += x as f64;
                    offset.1 += y as f64;
                }
            }
            cache.insert(node, offset);
        }
        offset
    }

    /// The client rect of an SVG element without a CSS box (a shape, text
    /// or container inside an `<svg>`): its bbox mapped into the content
    /// box of the nearest `<svg>` ancestor laid out as a box (Gap 34:
    /// Chromium reports the painted geometry; visibility checks rely on
    /// it). None for other elements or without geometry.
    fn svg_client_rect(&self, dom: &Dom, el: NodeId) -> Option<Rect> {
        self.svg_client_rect_with(dom, el, |svg| {
            Some((self.layout_box(dom, svg)?, self.bounding_client_rect_for_visible(dom, svg)))
        })
    }

    fn svg_client_rect_with(
        &self,
        dom: &Dom,
        el: NodeId,
        mut geometry: impl FnMut(NodeId) -> Option<(LayoutBox, Rect)>,
    ) -> Option<Rect> {
        if dom.element(el).ok()?.name.ns != markup5ever::ns!(svg) {
            return None;
        }
        if dom.element(el).ok()?.name.local == local_name!("svg") {
            return None;
        }
        // The nearest `<svg>` laid out as a box.
        let mut svg = dom.parent_element(el).ok().flatten()?;
        let (b, origin) = loop {
            let e = dom.element(svg).ok()?;
            if e.name.ns != markup5ever::ns!(svg) {
                return None;
            }
            if e.name.local == local_name!("svg") {
                if let Some((bounds, origin)) = geometry(svg).filter(|(b, _)| b.rect.width > 0.0 || b.rect.height > 0.0)
                {
                    break (bounds, origin);
                }
            }
            svg = dom.parent_element(svg).ok().flatten()?;
        };
        let edges = |i: usize| b.border[i] + b.padding[i];
        let (left, top) = (edges(LEFT), edges(TOP));
        let viewport = (
            (b.client_width - b.padding[LEFT] - b.padding[RIGHT]).max(0.0),
            (b.client_height - b.padding[TOP] - b.padding[BOTTOM]).max(0.0),
        );
        let inner = crate::svg_geometry::client_bounds(dom, &self.style, el, svg, viewport)?;
        Some(Rect {
            x: origin.x + left + inner.x,
            y: origin.y + top + inner.y,
            width: inner.width,
            height: inner.height,
        })
    }

    /// The scroll-dependent offset of `node`'s sticky ancestors (and itself):
    /// each sticky box moves with its descendants.
    fn sticky_shift(&self, dom: &Dom, result: &LayoutResult, node: NodeId) -> (f64, f64) {
        if result.sticky.is_empty() {
            return (0.0, 0.0);
        }
        let (mut x, mut y) = (0.0, 0.0);
        let mut cur = Some(node);
        while let Some(n) = cur {
            if result.sticky.contains_key(&n) {
                let (dx, dy) = self.sticky_offset(dom, result, n);
                x += dx;
                y += dy;
            }
            cur = dom.parent_element(n).ok().flatten();
        }
        (x, y)
    }

    /// CSS Positioned Layout 3 sticky positioning for one box laid out in
    /// place: the offset that keeps its border box within its nearest scroll
    /// container's scrollport (the viewport when none) inset by its
    /// `top`/`bottom`/`left`/`right`, without leaving its containing block
    /// (the table for table parts). `top` wins over `bottom`, `left` over
    /// `right`. Inside `position: fixed` the window scroll moves nothing, so
    /// with no scroll container in between the scrollport is the viewport at
    /// the origin.
    fn sticky_offset(&self, dom: &Dom, result: &LayoutResult, el: NodeId) -> (f64, f64) {
        let (Some(b), Some(sticky)) = (result.get(el), result.sticky.get(&el)) else { return (0.0, 0.0) };
        // Scrollport in document coordinates of the scrolled content.
        let mut port = None;
        let mut a = dom.parent_element(el).ok().flatten();
        while let Some(p) = a {
            if self.is_root_container(dom, p) {
                break;
            }
            let axes = self.style.overflow_clip_axes(dom, p);
            if axes.x || axes.y {
                if let Some(pb) = result.get(p) {
                    port = Some(Rect {
                        x: pb.rect.x + pb.border[LEFT] + pb.padding[LEFT] + self.scroll_left(p) as f64,
                        y: pb.rect.y + pb.border[TOP] + pb.padding[TOP] + self.scroll_top(p) as f64,
                        width: (pb.client_width - pb.padding[LEFT] - pb.padding[RIGHT]).max(0.0),
                        height: (pb.client_height - pb.padding[TOP] - pb.padding[BOTTOM]).max(0.0),
                    });
                }
                break;
            }
            a = dom.parent_element(p).ok().flatten();
        }
        let (wx, wy) = if b.fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
        let port =
            port.unwrap_or(Rect { x: wx, y: wy, width: self.inner_width() as f64, height: self.inner_height() as f64 });
        let limit = sticky.limit.map(|(x, y, width, height)| Rect { x, y, width, height });
        let inset = |side: usize, basis: f64| sticky.insets[side].map(|(px, fraction)| px + fraction * basis);
        let r = b.rect;
        // One axis: start inset pulls forward, end inset pulls back, both
        // bounded by the containing block (`lo`..`hi` along the axis).
        let axis = |start: Option<f64>,
                    end: Option<f64>,
                    pos: f64,
                    size: f64,
                    port0: f64,
                    port_size: f64,
                    lo: f64,
                    hi: f64| {
            if let Some(s) = start {
                let need = port0 + s - pos;
                if need > 0.0 {
                    return need.min(hi - (pos + size)).max(0.0);
                }
            }
            if let Some(e) = end {
                let need = port0 + port_size - e - (pos + size);
                if need < 0.0 {
                    return need.max(lo - pos).min(0.0);
                }
            }
            0.0
        };
        let (lo_x, hi_x, lo_y, hi_y) = match limit {
            Some(l) => (l.x, l.x + l.width, l.y, l.y + l.height),
            None => (f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY),
        };
        let dy =
            axis(inset(TOP, port.height), inset(BOTTOM, port.height), r.y, r.height, port.y, port.height, lo_y, hi_y);
        let dx = axis(inset(LEFT, port.width), inset(RIGHT, port.width), r.x, r.width, port.x, port.width, lo_x, hi_x);
        (dx, dy)
    }

    // ----- document ----------------------------------------------------------

    /// The document's laid-out size (width, height).
    pub fn document_extent(&self, dom: &Dom) -> (f64, f64) {
        self.synced_geometry_source(dom).and_then(|s| s.document_size()).unwrap_or((0.0, 0.0))
    }

    /// `Document.scrollSize` (Document.zig:548-556): the document, at least
    /// viewport-sized.
    pub fn document_scroll_size(&self, dom: &Dom) -> (f64, f64) {
        let (w, h) = self.document_extent(dom);
        let v = self.viewport();
        (w.max(v.width as f64), h.max(v.height as f64))
    }

    // ----- hit testing -------------------------------------------------------

    /// CSS stacking-context order, with source order breaking ties inside a
    /// layer. Computed once per box of `result` (see [`PaintChain`]).
    fn paint_key(&self, dom: &Dom, result: &LayoutResult, el: NodeId) -> Rc<[PaintEntry]> {
        if let Some(key) = result.hit.borrow().keys.get(&el) {
            return key.clone();
        }
        let chain = self.paint_chain(dom, result, el);
        // Context backgrounds precede negative contexts, normal flow, positioned
        // zero-level content, then positive contexts. Nested contexts stay atomic.
        let mut key: Vec<PaintEntry> = chain.contexts.to_vec();
        if let Some(layer) = chain.positioned_layer {
            key.push(layer);
        }
        let own = chain.own_layer;
        key.push((if own { 0 } else { 2 }, 0, if own { 0 } else { result.get(el).map_or(0, |b| b.order) }));
        let key: Rc<[PaintEntry]> = key.into();
        result.hit.borrow_mut().keys.insert(el, key.clone());
        key
    }

    /// The paint-order state after `el`'s ancestor chain (up
    /// `parentElement`, itself included): each box of the chain enters a
    /// stacking context, joins a z-auto positioned layer or paints in flow.
    /// Derived from the parent's state, so each element's is computed once
    /// per layout result.
    fn paint_chain(&self, dom: &Dom, result: &LayoutResult, el: NodeId) -> PaintChain {
        // The ancestors without a state yet, nearest first.
        let mut pending = Vec::new();
        let mut cur = Some(el);
        let mut state = loop {
            let Some(node) = cur else { break PaintChain::default() };
            if let Some(chain) = result.hit.borrow().chains.get(&node) {
                break chain.clone();
            }
            pending.push(node);
            cur = dom.parent_element(node).ok().flatten();
        };
        let reached_root = cur.is_none();
        for (i, &node) in pending.iter().enumerate().rev() {
            // The chain's first element (no parent element) always starts a context.
            let is_root = reached_root && i == pending.len() - 1;
            if let Some(bounds) = result.get(node) {
                state = self.paint_step(dom, node, bounds.order, is_root, state);
            }
            result.hit.borrow_mut().chains.insert(node, state.clone());
        }
        state
    }

    /// [`Self::paint_chain`]'s step for one box.
    fn paint_step(&self, dom: &Dom, node: NodeId, order: u32, is_root: bool, mut state: PaintChain) -> PaintChain {
        let c = self.computed(dom, node);
        let positioned = c.position.is_positioned();
        let flex_or_grid_item = dom
            .parent_element(node)
            .ok()
            .flatten()
            .is_some_and(|parent| matches!(self.computed(dom, parent).display, Display::Flex | Display::Grid));
        let z = if positioned || flex_or_grid_item { c.z_index } else { None };
        let context = is_root
            || matches!(c.position, Position::Fixed | Position::Sticky)
            || z.is_some()
            || c.transformed
            || c.translucent
            || c.isolated;
        if context {
            if !is_root {
                let z = z.unwrap_or(0);
                let layer = match z {
                    z if z < 0 => 1,
                    z if z > 0 => 4,
                    _ => 3,
                };
                let mut contexts = state.contexts.to_vec();
                contexts.push((layer, z, order));
                state.contexts = contexts.into();
            }
            state.positioned_layer = None;
            state.own_layer = true;
        } else if positioned {
            // z-auto positioned boxes group their normal content, but their
            // positioned descendants belong to the enclosing real context.
            state.positioned_layer = Some((3, 0, order));
            state.own_layer = true;
        } else {
            state.own_layer = false;
        }
        state
    }

    /// An inset clip-path clips the element and its descendants, including
    /// positioned descendants that escape an ancestor's overflow clip.
    fn outside_inset_clip_path(
        &self,
        dom: &Dom,
        el: NodeId,
        x: f64,
        y: f64,
        ignore_x: bool,
        mut client_rect: impl FnMut(NodeId) -> Rect,
    ) -> bool {
        use style::values::computed::basic_shape::{BasicShape, ClipPath};
        use style::values::computed::CSSPixelLength;
        use style::values::generics::basic_shape::{ShapeBox, ShapeGeometryBox};

        let mut current = Some(el);
        while let Some(node) = current {
            if let Some(style) = self.style.stylo_style(dom, node) {
                if let ClipPath::Shape(shape, reference) = style.clone_clip_path() {
                    // The default reference box for inset() is the border box.
                    if matches!(
                        reference,
                        ShapeGeometryBox::ElementDependent | ShapeGeometryBox::ShapeBox(ShapeBox::BorderBox)
                    ) {
                        if let BasicShape::Rect(inset) = &*shape {
                            let rect = client_rect(node);
                            let horizontal = CSSPixelLength::new(rect.width as f32);
                            let vertical = CSSPixelLength::new(rect.height as f32);
                            let left = rect.x + inset.rect.3.resolve(horizontal).px() as f64;
                            let right = rect.x + rect.width - inset.rect.1.resolve(horizontal).px() as f64;
                            let top = rect.y + inset.rect.0.resolve(vertical).px() as f64;
                            let bottom = rect.y + rect.height - inset.rect.2.resolve(vertical).px() as f64;
                            if right <= left
                                || bottom <= top
                                || y < top
                                || y > bottom
                                || (!ignore_x && (x < left || x > right))
                            {
                                return true;
                            }
                        }
                    }
                }
            }
            current = dom.parent_element(node).ok().flatten();
        }
        false
    }

    fn element_from_point_impl(&self, dom: &Dom, x: f64, y: f64, ignore_x: bool) -> Option<NodeId> {
        let result = self.layout_result(dom);
        let mut geometry = HitTestGeometry {
            has_element_scroll: self.element_scroll.values().any(|&(x, y)| x != 0 || y != 0),
            has_sticky: self.external.as_ref().is_some_and(|source| source.has_sticky()),
            ..Default::default()
        };
        let inside = |r: &Rect| (ignore_x || (x >= r.x && x <= r.x + r.width)) && y >= r.y && y <= r.y + r.height;
        let mut best: Option<(Rc<[PaintEntry]>, NodeId)> = None;
        for (&el, b) in &result.boxes {
            if b.rect.width <= 0.0 && b.rect.height <= 0.0 {
                continue;
            }
            let r = self.hit_test_client_rect(dom, &result, el, &mut geometry);
            if !inside(&r) || !self.is_visible(dom, el) || !self.style.is_hit_testable(dom, el) {
                continue;
            }
            if self.is_table_row_box(dom, el) {
                continue;
            }
            if self.outside_inset_clip_path(dom, el, x, y, ignore_x, |node| {
                self.hit_test_client_rect(dom, &result, node, &mut geometry)
            }) {
                continue;
            }
            let key = self.paint_key(dom, &result, el);
            if let Some((top_key, top_node)) = &best {
                let same_layer = key.len() == top_key.len()
                    && key[..key.len() - 1] == top_key[..top_key.len() - 1]
                    && key.last().map(|value| (value.0, value.1)) == top_key.last().map(|value| (value.0, value.1));
                if same_layer && dom.contains(el, *top_node).unwrap_or(false) {
                    continue;
                }
                if *top_key > key && !(same_layer && dom.contains(*top_node, el).unwrap_or(false)) {
                    continue;
                }
            }
            let clipped = self.hit_test_clips(dom, &result, el).iter().any(|&(a, clips)| {
                if result.get(a).is_none() {
                    return false;
                }
                let ar = self.hit_test_client_rect(dom, &result, a, &mut geometry);
                let out_x = clips.x && !ignore_x && (x < ar.x || x > ar.x + ar.width);
                let out_y = clips.y && (y < ar.y || y > ar.y + ar.height);
                out_x || out_y
            });
            if !clipped {
                best = Some((key, el));
            }
        }
        best.map(|(_, node)| node)
    }

    /// A table row or row group, which Chromium hits only through its
    /// cells: a later row's box overlaps a cell spanning into it from an
    /// earlier row, whose content must still be hit (the tag is checked
    /// before the computed display).
    fn is_table_row_box(&self, dom: &Dom, el: NodeId) -> bool {
        let tag = dom.element(el).map(|e| e.local_name().clone());
        if !tag.is_ok_and(|t| matches!(&*t, "tr" | "tbody" | "thead" | "tfoot")) {
            return false;
        }
        self.computed(dom, el).table_row_or_group
    }

    /// Visible viewport rectangle, using the same containing-block clips as hit testing.
    pub fn visible_client_rect(&self, dom: &Dom, node: NodeId, rect: Rect) -> Option<Rect> {
        let (mut left, mut top) = (rect.left().max(0.0), rect.top().max(0.0));
        let (mut right, mut bottom) =
            (rect.right().min(self.inner_width() as f64), rect.bottom().min(self.inner_height() as f64));
        self.clipped_by_ancestor(dom, node, |ancestor, clips| {
            if self.computed(dom, ancestor).display_contents || self.layout_box(dom, ancestor).is_none() {
                return false;
            }
            if let Some(bounds) = self.client_rect(dom, ancestor) {
                if clips.x {
                    left = left.max(bounds.left());
                    right = right.min(bounds.right());
                }
                if clips.y {
                    top = top.max(bounds.top());
                    bottom = bottom.min(bounds.bottom());
                }
            }
            right <= left || bottom <= top
        });
        (right > left && bottom > top).then_some(Rect { x: left, y: top, width: right - left, height: bottom - top })
    }

    /// `document.elementFromPoint(x, y)` (Document.zig:953-1032), viewport
    /// coordinates.
    pub fn element_from_point(&self, dom: &Dom, x: f64, y: f64) -> Option<NodeId> {
        self.element_from_point_impl(dom, x, y, false)
    }

    /// Hit test on the vertical axis only (Document.zig:960-962), for
    /// WebDriver scroll actions.
    pub fn element_from_vertical_point(&self, dom: &Dom, y: f64) -> Option<NodeId> {
        self.element_from_point_impl(dom, 0.0, y, true)
    }

    /// `document.elementsFromPoint(x, y)` (Document.zig:1034-1047): the hit
    /// and its hit-testable ancestors, always ending with the root.
    pub fn elements_from_point(&self, dom: &Dom, x: f64, y: f64) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = self.element_from_point(dom, x, y);
        while let Some(el) = cur {
            let parent = dom.parent_element(el).ok().flatten();
            if parent.is_none() || self.style.is_hit_testable(dom, el) {
                out.push(el);
            }
            cur = parent;
        }
        out
    }

    // ----- window scroll -----------------------------------------------------

    /// `window.scrollX` / `scrollY` (Window.zig:758-764).
    pub fn scroll_x(&self) -> u32 {
        self.window_scroll.0
    }

    pub fn scroll_y(&self) -> u32 {
        self.window_scroll.1
    }

    /// `window.scrollTo` (Window.zig:780-794): each given axis is clamped to
    /// [0, scrollSize - viewport]. Returns whether the offset moved (the
    /// caller then schedules `scroll`/`scrollend` on the document).
    ///
    /// Only a positive coordinate needs the limit: zero or less lands at the
    /// origin whatever the document's size, so `scrollTo(0, 0)` (a router's
    /// scroll restoration on every navigation) lays nothing out.
    pub fn window_scroll_to(&mut self, dom: &Dom, left: Option<i32>, top: Option<i32>) -> bool {
        let (max_x, max_y) = if left.is_some_and(|l| l > 0) || top.is_some_and(|t| t > 0) {
            let (w, h) = self.document_scroll_size(dom);
            (scroll_limit(w, self.inner_width()), scroll_limit(h, self.inner_height()))
        } else {
            (0, 0)
        };
        let new_x = left.map_or(self.window_scroll.0, |l| (l.max(0) as u32).min(max_x));
        let new_y = top.map_or(self.window_scroll.1, |t| (t.max(0) as u32).min(max_y));
        if (new_x, new_y) == self.window_scroll {
            return false;
        }
        self.window_scroll = (new_x, new_y);
        self.scroll_generation = self.scroll_generation.wrapping_add(1);
        true
    }

    /// `window.scrollBy` (Window.zig:853-860): relative, both axes written.
    pub fn window_scroll_by(&mut self, dom: &Dom, left: Option<i32>, top: Option<i32>) -> bool {
        let abs = |cur: u32, d: Option<i32>| (cur as i64 + d.unwrap_or(0) as i64).min(i32::MAX as i64) as i32;
        let x = abs(self.window_scroll.0, left);
        let y = abs(self.window_scroll.1, top);
        self.window_scroll_to(dom, Some(x), Some(y))
    }

    // ----- element scroll ----------------------------------------------------

    /// `scrollTop` (Element.zig:1757-1761).
    pub fn scroll_top(&self, el: NodeId) -> u32 {
        self.element_scroll.get(&el).map_or(0, |p| p.1)
    }

    /// `scrollLeft` (Element.zig:1767-1771).
    pub fn scroll_left(&self, el: NodeId) -> u32 {
        self.element_scroll.get(&el).map_or(0, |p| p.0)
    }

    /// `scrollTop = value` (Element.zig:1763-1765).
    pub fn set_scroll_top(&mut self, dom: &Dom, el: NodeId, value: i32) -> bool {
        self.write_scroll(dom, el, ScrollWrite::To { left: None, top: Some(value) })
    }

    /// `scrollLeft = value` (Element.zig:1773-1775).
    pub fn set_scroll_left(&mut self, dom: &Dom, el: NodeId, value: i32) -> bool {
        self.write_scroll(dom, el, ScrollWrite::To { left: Some(value), top: None })
    }

    /// `Element.scrollTo` (Element.zig:2194-2197).
    pub fn element_scroll_to(&mut self, dom: &Dom, el: NodeId, left: Option<i32>, top: Option<i32>) -> bool {
        self.write_scroll(dom, el, ScrollWrite::To { left, top })
    }

    /// `Element.scrollBy` (Element.zig:2200-2203).
    pub fn element_scroll_by(&mut self, dom: &Dom, el: NodeId, left: Option<i32>, top: Option<i32>) -> bool {
        self.write_scroll(dom, el, ScrollWrite::By { left, top })
    }

    /// `scrollByAxis` (Element.zig:2206-2211): whether the container moved.
    pub fn scroll_by_axis(&mut self, dom: &Dom, el: NodeId, axis: Axis, delta: i32) -> bool {
        let w = match axis {
            Axis::Width => ScrollWrite::By { left: Some(delta), top: None },
            Axis::Height => ScrollWrite::By { left: None, top: Some(delta) },
        };
        self.write_scroll(dom, el, w)
    }

    /// `writeScroll` (Element.zig:2235-2259): every element scroll write.
    /// Returns whether anything moved (the caller schedules `scroll` /
    /// `scrollend`, Element.zig:2277-2294).
    pub fn write_scroll(&mut self, dom: &Dom, el: NodeId, write: ScrollWrite) -> bool {
        if self.owned(dom, el).is_none() {
            return false;
        }
        let current = self.element_scroll.get(&el).copied().unwrap_or((0, 0));
        let mut x = current.0;
        let mut y = current.1;
        if let Some(t) = write.target(Axis::Width, current.0) {
            x = self.clamp_scroll(dom, el, Axis::Width, t);
        }
        if let Some(t) = write.target(Axis::Height, current.1) {
            y = self.clamp_scroll(dom, el, Axis::Height, t);
        }
        if (x, y) == current {
            return false;
        }
        self.element_scroll.insert(el, (x, y));
        self.scroll_generation = self.scroll_generation.wrapping_add(1);
        self.capture_scroll_anchor(dom, el);
        true
    }

    /// Whether any element or the window is scrolled away from the origin.
    /// Records the layout `scroller` is settled in (nothing at the top).
    fn capture_scroll_anchor(&mut self, dom: &Dom, scroller: NodeId) {
        if self.scroll_top(scroller) == 0 {
            self.scroll_anchors.remove(&scroller);
            return;
        }
        let result = self.layout_result(dom);
        self.scroll_anchors.insert(scroller, result);
    }

    /// The scroll anchor of a vertically scrolled `scroller` (CSS Scroll
    /// Anchoring 2.2, as Chromium selects it): in tree order, the first
    /// element entirely inside the scrollport, descending into a partly
    /// visible one, which is the anchor when none of its descendants is.
    /// `overflow-anchor: none` excludes an element and its subtree;
    /// out-of-flow boxes are skipped and nested scrollers not entered. Like
    /// Chromium, which selects before layout, the geometry is `flow` (the
    /// previous layout) with the current tree and styles; `keep` rejects
    /// elements the new layout no longer boxes.
    fn scroll_anchor(
        &self,
        dom: &Dom,
        flow: &LayoutResult,
        scroller: NodeId,
        keep: &dyn Fn(NodeId) -> bool,
    ) -> Option<NodeId> {
        if self.overflow_anchor_none(dom, scroller) {
            return None;
        }
        let sb = flow.get(scroller)?;
        let top = sb.rect.y + self.scroll_top(scroller) as f64;
        self.anchor_in(dom, flow, scroller, top, top + sb.client_height, keep)
    }

    #[allow(clippy::too_many_arguments)]
    fn anchor_in(
        &self,
        dom: &Dom,
        flow: &LayoutResult,
        parent: NodeId,
        top: f64,
        bottom: f64,
        keep: &dyn Fn(NodeId) -> bool,
    ) -> Option<NodeId> {
        for child in dom.children(parent).ok()? {
            if !dom.n(dom.check(child).ok()?).is_element() || self.overflow_anchor_none(dom, child) {
                continue;
            }
            let Some(b) = flow.get(child) else {
                // No box then (display: contents, or added since): its
                // children may have one.
                if let Some(a) = self.anchor_in(dom, flow, child, top, bottom, keep) {
                    return Some(a);
                }
                continue;
            };
            // Absolute and fixed boxes are skipped themselves; `b.fixed` also
            // marks every box inside a fixed ancestor, which anchors normally
            // when the scroller is inside it too (a scroller in a fixed drawer).
            if !keep(child) || self.style.computed(dom, child).is_some_and(|c| c.absolutely_positioned) {
                continue;
            }
            let (y0, y1) = (b.rect.y, b.rect.y + b.rect.height);
            if y1 <= top || y0 >= bottom {
                continue;
            }
            if y0 >= top && y1 <= bottom {
                return Some(child);
            }
            let clips = self.style.overflow_clip_axes(dom, child);
            if !(clips.x || clips.y) {
                if let Some(a) = self.anchor_in(dom, flow, child, top, bottom, keep) {
                    return Some(a);
                }
            }
            return Some(child);
        }
        None
    }

    fn overflow_anchor_none(&self, dom: &Dom, el: NodeId) -> bool {
        self.style.computed_value(dom, el, "overflow-anchor").is_some_and(|v| v.trim().eq_ignore_ascii_case("none"))
    }

    /// Scroll anchoring after a layout change: each scroller selects its
    /// anchor in the layout it was settled in and moves by however far that
    /// anchor moved within it, as Chromium adjusts before painting (and
    /// fires `scroll`).
    fn apply_scroll_anchors(&mut self, dom: &Dom, moved: &mut Vec<ScrollTarget>) {
        if self.scroll_anchors.is_empty() {
            return;
        }
        let flow = self.layout_result(dom);
        let mut anchors: Vec<(NodeId, Rc<LayoutResult>)> =
            self.scroll_anchors.iter().map(|(&k, v)| (k, v.clone())).collect();
        anchors.sort_by_key(|(k, _)| (k.idx, k.gen));
        for (el, old) in anchors {
            if Rc::ptr_eq(&old, &flow) {
                continue;
            }
            let Some(&(x, y)) = self.element_scroll.get(&el) else { continue };
            if y == 0 || !dom.is_connected(el).unwrap_or(false) {
                continue;
            }
            let keep = |n: NodeId| flow.get(n).is_some();
            let Some(anchor) = self.scroll_anchor(dom, &old, el, &keep) else { continue };
            let (Some(sb), Some(ab), Some(osb), Some(oab)) =
                (flow.get(el), flow.get(anchor), old.get(el), old.get(anchor))
            else {
                continue;
            };
            let delta = (ab.rect.y - sb.rect.y) - (oab.rect.y - osb.rect.y);
            let ny = (y as f64 + delta).round().max(0.0) as u32;
            if ny != y {
                self.element_scroll.insert(el, (x, ny));
                self.scroll_generation = self.scroll_generation.wrapping_add(1);
                moved.push(ScrollTarget::Container(el));
            }
        }
    }

    /// Elements scrolled away from their origin, by render key
    /// (`snapshot::render_key`), with their offset (CSS px).
    pub fn scrolled_elements(&self) -> Vec<(u64, (f64, f64))> {
        if !self.has_scroll_offsets() {
            return Vec::new();
        }
        self.element_scroll
            .iter()
            .filter(|(_, &o)| o != (0, 0))
            .map(|(&el, &(x, y))| (crate::snapshot::render_key(el), (x as f64, y as f64)))
            .collect()
    }

    pub fn has_scroll_offsets(&self) -> bool {
        self.window_scroll != (0, 0) || self.element_scroll.values().any(|&o| o != (0, 0))
    }

    /// Re-clamps scroll offsets to the current scrollable extents and
    /// returns the targets that moved (in element order, the viewport last).
    /// Chromium clamps a scroll position when layout shrinks the scrollable
    /// overflow below it and fires `scroll` (react-aria's virtualizer reads
    /// its window from that event when measured rows shrink the content).
    /// Flow geometry only; hidden or detached elements keep their offsets.
    pub fn clamp_scroll_offsets(&mut self, dom: &Dom) -> Vec<ScrollTarget> {
        let mut moved = Vec::new();
        if !self.has_scroll_offsets() {
            return moved;
        }
        self.apply_scroll_anchors(dom, &mut moved);
        let mut scrolled: Vec<(NodeId, (u32, u32))> =
            self.element_scroll.iter().filter(|(_, &o)| o != (0, 0)).map(|(&k, &v)| (k, v)).collect();
        scrolled.sort_by_key(|(k, _)| (k.idx, k.gen));
        for (el, (x, y)) in scrolled {
            if !dom.is_connected(el).unwrap_or(false) || !self.is_visible(dom, el) {
                continue;
            }
            let limit = |layout: &Self, axis: Axis, v: u32| {
                if v == 0 {
                    return 0;
                }
                layout.scroll_extent(dom, el, axis).map_or(v, |e| v.min(e.floor().max(0.0) as u32))
            };
            let (nx, ny) = (limit(self, Axis::Width, x), limit(self, Axis::Height, y));
            if (nx, ny) != (x, y) {
                self.element_scroll.insert(el, (nx, ny));
                self.scroll_generation = self.scroll_generation.wrapping_add(1);
                if !moved.contains(&ScrollTarget::Container(el)) {
                    moved.push(ScrollTarget::Container(el));
                }
            }
        }
        let anchored: Vec<NodeId> = self.element_scroll.iter().filter(|(_, &(_, y))| y != 0).map(|(&k, _)| k).collect();
        self.scroll_anchors.retain(|k, _| anchored.contains(k));
        for el in anchored {
            self.capture_scroll_anchor(dom, el);
        }
        if self.window_scroll != (0, 0) {
            let (w, h) = self.document_scroll_size(dom);
            let max = (scroll_limit(w, self.inner_width()), scroll_limit(h, self.inner_height()));
            let clamped = (self.window_scroll.0.min(max.0), self.window_scroll.1.min(max.1));
            if clamped != self.window_scroll {
                self.window_scroll = clamped;
                self.scroll_generation = self.scroll_generation.wrapping_add(1);
                moved.push(ScrollTarget::Viewport);
            }
        }
        moved
    }

    /// `clampScroll` (Element.zig:2261-2270). Zero needs no extent (no
    /// layout): `scrollTop = 0` on a container at the top stays a no-op.
    fn clamp_scroll(&self, dom: &Dom, el: NodeId, axis: Axis, target: i64) -> u32 {
        let mut clamped = target;
        if clamped <= 0 {
            clamped = 0;
        } else if let Some(extent) = self.scroll_extent(dom, el, axis) {
            clamped = clamped.min(extent.floor() as i64);
        }
        clamped.min(u32::MAX as i64) as u32
    }

    /// `scrollContainer` (Element.zig:1788-1801): nearest ancestor-or-self
    /// whose own overflow scrolls on one of `axes`; html/body and detached
    /// chains land on the viewport.
    pub fn scroll_container(&self, dom: &Dom, el: NodeId, axes: ScrollAxes) -> ScrollTarget {
        if !axes.x && !axes.y {
            return ScrollTarget::Viewport;
        }
        if self.owned(dom, el).is_none() {
            return ScrollTarget::Viewport;
        }
        let mut cur = Some(el);
        while let Some(e) = cur {
            if self.is_root_container(dom, e) {
                break;
            }
            let s = self.style.overflow_axes(dom, e);
            if (axes.x && s.x) || (axes.y && s.y) {
                return ScrollTarget::Container(e);
            }
            cur = dom.parent_element(e).ok().flatten();
        }
        ScrollTarget::Viewport
    }

    /// `containsOverscroll` (Element.zig:1805-1809).
    pub fn contains_overscroll(&self, dom: &Dom, el: NodeId, axes: ScrollAxes) -> bool {
        let c = self.style.overscroll_contain_axes(dom, el);
        (axes.x && c.x) || (axes.y && c.y)
    }

    /// The scroll part of a wheel over `target` (frame/user_input.zig:571-615):
    /// each axis' whole delta latches to the nearest container that can still
    /// move, else the viewport. Returns the scrollers that moved. Dispatching
    /// the cancelable `wheel` event first is the caller's job.
    pub fn wheel_scroll(&mut self, dom: &Dom, target: NodeId, delta_x: f64, delta_y: f64) -> Vec<ScrollTarget> {
        let mut moved = Vec::new();
        for (axis, d) in [(Axis::Width, delta_x), (Axis::Height, delta_y)] {
            if let Some(t) = self.scroll_axis(dom, target, axis, delta_to_scroll(d)) {
                moved.push(t);
            }
        }
        moved
    }

    fn scroll_axis(&mut self, dom: &Dom, target: NodeId, axis: Axis, delta: i32) -> Option<ScrollTarget> {
        if delta == 0 {
            return None;
        }
        let axes = match axis {
            Axis::Width => ScrollAxes { x: true, y: false },
            Axis::Height => ScrollAxes { x: false, y: true },
        };
        let mut cur = Some(target);
        while let Some(el) = cur {
            let container = match self.scroll_container(dom, el, axes) {
                ScrollTarget::Container(c) => c,
                ScrollTarget::Viewport => break,
            };
            if self.scroll_by_axis(dom, container, axis, delta) {
                return Some(ScrollTarget::Container(container));
            }
            if self.contains_overscroll(dom, container, axes) {
                return None;
            }
            cur = dom.parent_element(container).ok().flatten();
        }
        let moved = match axis {
            Axis::Width => self.window_scroll_by(dom, Some(delta), None),
            Axis::Height => self.window_scroll_by(dom, None, Some(delta)),
        };
        moved.then_some(ScrollTarget::Viewport)
    }

    /// Reveal a clipped target inside overflow ancestors, innermost
    /// first. Recompute relative geometry after each move so an inner offset is
    /// not counted again by an outer container. Explicitly scrolled containers
    /// retain the previous reveal behavior even without an overflow declaration.
    fn reveal_in_scroll_ancestors(&mut self, dom: &Dom, el: NodeId) -> Vec<ScrollTarget> {
        let mut moved = Vec::new();
        let mut cur = dom.parent_element(el).ok().flatten();
        while let Some(node) = cur {
            if self.is_root_container(dom, node) {
                break;
            }
            let axes = self.style.overflow_axes(dom, node);
            let explicit = self.element_scroll.contains_key(&node);
            if axes.x || axes.y || explicit {
                let target = self.bounding_client_rect(dom, el);
                let container = self.bounding_client_rect(dom, node);
                let left = (axes.x || explicit)
                    .then(|| {
                        nearest_scroll_offset(
                            target.left() - container.left(),
                            target.width,
                            self.client_width(dom, node),
                            self.scroll_left(node),
                        )
                    })
                    .flatten();
                let offset = target.top() - container.top();
                let visible = self.client_height(dom, node);
                let top = ((axes.y || explicit) && (offset < 0.0 || offset + target.height > visible))
                    .then(|| (offset + self.scroll_top(node) as f64).max(0.0).trunc().min(i32::MAX as f64) as i32);
                if self.element_scroll_to(dom, node, left, top) {
                    moved.push(ScrollTarget::Container(node));
                }
            }
            cur = dom.parent_element(node).ok().flatten();
        }
        moved
    }

    /// Reveal a newly focused element before focus events. Center an axis only
    /// when the box is not fully in its visible range; preserve visible axes.
    /// Uses existing synthetic boxes and scroll limits, not fabricated geometry.
    /// Auto/scroll ancestors are processed inside-out, then the viewport.
    pub fn reveal_for_focus(&mut self, dom: &Dom, el: NodeId) -> Vec<ScrollTarget> {
        if !self.is_visible(dom, el) {
            return Vec::new();
        }
        let mut moved = Vec::new();
        let mut parent = dom.parent_element(el).ok().flatten();
        while let Some(node) = parent {
            if self.is_root_container(dom, node) {
                break;
            }
            let axes = self.style.overflow_axes(dom, node);
            if axes.x || axes.y {
                let target = self.bounding_client_rect(dom, el);
                let container = self.bounding_client_rect(dom, node);
                let left = axes
                    .x
                    .then(|| {
                        focus_scroll_offset(
                            target.left() - container.left(),
                            target.width,
                            self.client_width(dom, node),
                            self.scroll_left(node),
                        )
                    })
                    .flatten();
                let top = axes
                    .y
                    .then(|| {
                        focus_scroll_offset(
                            target.top() - container.top(),
                            target.height,
                            self.client_height(dom, node),
                            self.scroll_top(node),
                        )
                    })
                    .flatten();
                if self.element_scroll_to(dom, node, left, top) {
                    moved.push(ScrollTarget::Container(node));
                }
            }
            parent = dom.parent_element(node).ok().flatten();
        }
        let target = self.bounding_client_rect(dom, el);
        let left = focus_scroll_offset(target.left(), target.width, self.inner_width() as f64, self.window_scroll.0);
        let top = focus_scroll_offset(target.top(), target.height, self.inner_height() as f64, self.window_scroll.1);
        if self.window_scroll_to(dom, left, top) {
            moved.push(ScrollTarget::Viewport);
        }
        moved
    }

    /// `scrollIntoView` keeps the existing vertical start alignment for the
    /// viewport, after revealing clipped overflow ancestors inside-out.
    pub fn scroll_into_view(&mut self, dom: &Dom, el: NodeId) -> bool {
        !self.scroll_into_view_targets(dom, el).is_empty()
    }

    /// Changed targets in delivery order. Embedders use this to queue each
    /// container's scroll event followed by the document's viewport event.
    /// Vertical start reveal and horizontal nearest-edge reveal; not full CSS alignment.
    pub fn scroll_into_view_targets(&mut self, dom: &Dom, el: NodeId) -> Vec<ScrollTarget> {
        if !self.is_visible(dom, el) {
            return Vec::new();
        }
        let mut moved = self.reveal_in_scroll_ancestors(dom, el);
        let (_, dy) = self.ancestor_scroll_offset(dom, el);
        let y = match self.layout_box(dom, el) {
            // Fixed boxes do not move with the window.
            Some(b) if b.fixed => return moved,
            Some(b) => b.rect.y - dy,
            None => return moved,
        };
        let top = y.max(0.0).trunc().min(i32::MAX as f64) as i32;
        let target = self.bounding_client_rect(dom, el);
        let left = nearest_scroll_offset(target.left(), target.width, self.inner_width() as f64, self.window_scroll.0);
        if self.window_scroll_to(dom, left, Some(top)) {
            moved.push(ScrollTarget::Viewport);
        }
        moved
    }

    /// No-op while the target fits the viewport and its overflow ancestors.
    pub fn scroll_into_view_if_needed(&mut self, dom: &Dom, el: NodeId) -> bool {
        if !self.is_visible(dom, el) {
            return false;
        }
        let moved = self.reveal_in_scroll_ancestors(dom, el);
        let target = self.bounding_client_rect(dom, el);
        if target.top() >= 0.0
            && target.bottom() <= self.inner_height() as f64
            && nearest_scroll_offset(target.left(), target.width, self.inner_width() as f64, self.window_scroll.0)
                .is_none()
        {
            return !moved.is_empty();
        }
        self.scroll_into_view(dom, el) || !moved.is_empty()
    }

    /// CDP `DOM.scrollIntoViewIfNeeded` (Blink's center-if-needed alignment),
    /// changed targets in delivery order: per axis, each overflow ancestor
    /// inside-out and then the viewport leave a box that is fully visible (or
    /// covers the scrollport) alone, center a box that is entirely hidden and
    /// bring a partly visible one in by its nearest edge. The approximate
    pub fn scroll_into_view_if_needed_targets(&mut self, dom: &Dom, el: NodeId) -> Vec<ScrollTarget> {
        self.scroll_into_view_aligned_targets(dom, el, false)
    }

    pub fn scroll_into_view_centered_targets(&mut self, dom: &Dom, el: NodeId) -> Vec<ScrollTarget> {
        self.scroll_into_view_aligned_targets(dom, el, true)
    }

    fn scroll_into_view_aligned_targets(&mut self, dom: &Dom, el: NodeId, center: bool) -> Vec<ScrollTarget> {
        let offset: fn(f64, f64, f64, u32) -> Option<i32> =
            if center { centered_scroll_offset } else { center_if_needed_offset };
        self.scroll_into_view_offset_targets(dom, el, offset, false)
    }

    /// Script-facing `Element.scrollIntoViewIfNeeded(centerIfNeeded)`.
    /// Hidden overflow is programmatically scrollable; clip/visible is not.
    /// Keep this policy separate from the automation driver's reveal helpers.
    pub fn scroll_into_view_if_needed_script_targets(
        &mut self,
        dom: &Dom,
        el: NodeId,
        center_if_needed: bool,
    ) -> Vec<ScrollTarget> {
        let offset: fn(f64, f64, f64, u32) -> Option<i32> =
            if center_if_needed { center_if_needed_offset } else { nearest_scroll_offset };
        self.scroll_into_view_offset_targets(dom, el, offset, true)
    }

    fn scroll_into_view_offset_targets(
        &mut self,
        dom: &Dom,
        el: NodeId,
        offset: fn(f64, f64, f64, u32) -> Option<i32>,
        script: bool,
    ) -> Vec<ScrollTarget> {
        if !self.is_visible(dom, el) {
            return Vec::new();
        }
        let mut moved = Vec::new();
        let mut cur = dom.parent_element(el).ok().flatten();
        while let Some(node) = cur {
            if self.is_root_container(dom, node) {
                break;
            }
            let axes = if script {
                // A scroll container scrolls on both axes: the other axis's
                // `visible` computes to `auto` and `clip` to `hidden`.
                use style::values::specified::box_::Overflow as O;
                let scroller = |o: O| matches!(o, O::Hidden | O::Scroll | O::Auto);
                let any = self.style.stylo_style(dom, node).is_some_and(|s| {
                    let b = s.get_box();
                    scroller(b.overflow_x) || scroller(b.overflow_y)
                });
                ScrollAxes { x: any, y: any }
            } else {
                self.style.overflow_axes(dom, node)
            };
            let explicit = !script && self.element_scroll.contains_key(&node);
            if axes.x || axes.y || explicit {
                let target = self.bounding_client_rect(dom, el);
                let container = self.bounding_client_rect(dom, node);
                let left = (axes.x || explicit)
                    .then(|| {
                        offset(
                            target.left() - container.left() - if script { self.client_left(dom, node) } else { 0.0 },
                            target.width,
                            self.client_width(dom, node),
                            self.scroll_left(node),
                        )
                    })
                    .flatten();
                let top = (axes.y || explicit)
                    .then(|| {
                        offset(
                            target.top() - container.top() - if script { self.client_top(dom, node) } else { 0.0 },
                            target.height,
                            self.client_height(dom, node),
                            self.scroll_top(node),
                        )
                    })
                    .flatten();
                if self.element_scroll_to(dom, node, left, top) {
                    moved.push(ScrollTarget::Container(node));
                }
            }
            cur = dom.parent_element(node).ok().flatten();
        }
        // Fixed boxes do not move with the window.
        if self.layout_box(dom, el).is_some_and(|b| b.fixed) {
            return moved;
        }
        let target = self.bounding_client_rect(dom, el);
        let left = offset(target.left(), target.width, self.inner_width() as f64, self.window_scroll.0);
        let top = offset(target.top(), target.height, self.inner_height() as f64, self.window_scroll.1);
        if self.window_scroll_to(dom, left, top) {
            moved.push(ScrollTarget::Viewport);
        }
        moved
    }

    // ----- getComputedStyle --------------------------------------------------

    /// `getComputedStyle(el).getPropertyValue(name)`
    /// (CSSStyleDeclaration.zig:76-151): display / visibility /
    /// pointer-events / text-transform from the cascade, custom properties,
    /// font-size in px, other properties from the generic cascade
    /// (`StyleEngine::computed_value`), width/height from a declared absolute
    /// length or the laid-out box, else the initial value.
    pub fn computed_style_value(&self, dom: &Dom, el: NodeId, name: &str) -> String {
        // Chromium computes no style outside a rendered document: an element
        // that is disconnected, or in a document without a browsing context
        // (DOMParser, createHTMLDocument, template contents), reads "" for
        // every property.
        if self.owned(dom, el).is_none() || !dom.is_connected(el).unwrap_or(false) {
            return String::new();
        }
        let custom = name.starts_with("--");
        let name = if custom { name.to_owned() } else { name.to_ascii_lowercase() };
        match name.as_str() {
            "display" if self.style.has_display_none(dom, el) => return "none".into(),
            "display" => return self.style.computed_display(dom, el).unwrap_or_default(),
            "visibility" if self.style.has_visibility_hidden_inherited(dom, el) => return "hidden".into(),
            "pointer-events" if self.style.has_pointer_events_none(dom, el) => return "none".into(),
            "text-transform" => return self.style.text_transform(dom, el).keyword().into(),
            "overflow-x" | "overflow-y" | "overflow" => {
                let x = self.style.used_value(dom, el, "overflow-x");
                let y = self.style.used_value(dom, el, "overflow-y");
                let (x, y) = crate::style::computed_overflow_keywords(x.as_deref(), y.as_deref());
                return match name.as_str() {
                    "overflow-x" => x,
                    "overflow-y" => y,
                    _ if x == y => x,
                    _ => format!("{x} {y}"),
                };
            }
            _ => {}
        }
        if custom {
            return self.style.custom_property_value(dom, el, &name).unwrap_or_default();
        }
        if name == "font-size" {
            // The cascade (stylesheets, em/%/rem, keywords) when any author value applies; else the inline/attribute walk.
            return self
                .style
                .computed_value(dom, el, "font-size")
                .unwrap_or_else(|| format!("{}px", self.style.computed_font_size(dom, Some(el))));
        }
        let declared = self.style.computed_value(dom, el, &name);
        if name == "width" || name == "height" {
            if !self.is_visible(dom, el) {
                return "auto".into();
            }
            // A declared absolute length is the used size the renderless layout cannot derive.
            if let Some(px) = declared.filter(|v| v.ends_with("px")) {
                return px;
            }
            // The used size: content box, or border box under `box-sizing: border-box`.
            let Some(b) = self.layout_box(dom, el) else { return "auto".into() };
            let border_box = self.style.computed_value(dom, el, "box-sizing").as_deref() == Some("border-box");
            let (size, start, end) =
                if name == "width" { (b.rect.width, LEFT, RIGHT) } else { (b.rect.height, TOP, BOTTOM) };
            let inset =
                if border_box { 0.0 } else { b.border[start] + b.border[end] + b.padding[start] + b.padding[end] };
            return format!("{}px", crate::computed::fmt_num((size - inset).max(0.0)));
        }
        if let Some(v) = declared.as_deref().and_then(|v| self.resolved_value(dom, el, &name, v)) {
            return v;
        }
        if let Some(v) = declared {
            return v;
        }
        if let Some(v) = self.style.inline_style_value(dom, el, &name) {
            return v;
        }
        default_property_value(dom, el, &name).to_owned()
    }

    /// `getComputedStyle(el, '::before' | '::after').getPropertyValue(name)`:
    /// Stylo's computed value for the pseudo-element (one without rules
    /// inherits from `el`), "" where `el` has no computed style. Layout-
    /// dependent values (used sizes, insets) are not resolved: they read as
    /// computed.
    pub fn pseudo_computed_style_value(
        &self,
        dom: &Dom,
        el: NodeId,
        pseudo: crate::style::PseudoElement,
        name: &str,
    ) -> String {
        if self.owned(dom, el).is_none() || !dom.is_connected(el).unwrap_or(false) {
            return String::new();
        }
        let name = if name.starts_with("--") { name.to_owned() } else { name.to_ascii_lowercase() };
        self.style.pseudo_value(dom, el, pseudo, &name).unwrap_or_default()
    }
}

impl Layout {
    /// getComputedStyle's resolved value where it differs from Stylo's
    /// computed one (CSSOM §resolved values, as Chromium reports them):
    /// `auto` minimum sizes outside flex and grid items, `auto` insets of
    /// relatively positioned boxes, lengths clamped to Blink's LayoutUnit
    /// range. `None`: the computed value stands.
    fn resolved_value(&self, dom: &Dom, el: NodeId, name: &str, value: &str) -> Option<String> {
        let position = || self.position_of(dom, el);
        match name {
            "min-width" | "min-height" if value == "auto" => {
                // The box's parent: display:contents ancestors generate none.
                let mut parent = dom.parent_element(el).ok().flatten();
                while let Some(p) = parent.filter(|&p| self.computed(dom, p).display_contents) {
                    parent = dom.parent_element(p).ok().flatten();
                }
                let item =
                    parent.is_some_and(|p| matches!(self.computed(dom, p).display, Display::Flex | Display::Grid));
                let out_of_flow = matches!(position(), Position::Absolute | Position::Fixed);
                (!item || out_of_flow).then(|| "0px".into())
            }
            "top" | "bottom" | "left" | "right" if value == "auto" || value.contains('%') => {
                self.resolved_inset(dom, el, name, value, position()).or_else(|| clamp_layout_units(value))
            }
            _ => clamp_layout_units(value),
        }
    }

    /// The resolved value of an inset property that is `auto` or has a
    /// percentage (CSSOM: the used value for a positioned, rendered box).
    /// Relative and sticky boxes resolve percentages against their
    /// containing block's content box; an `auto` side of a relative box
    /// mirrors the other side, and one of a sticky box stays `auto`.
    /// Absolutely positioned boxes report the distance between their margin
    /// box and the padding box of their containing block.
    fn resolved_inset(&self, dom: &Dom, el: NodeId, name: &str, value: &str, position: Position) -> Option<String> {
        let style = &self.style;
        if style.has_display_none(dom, el) || !dom.is_connected(el).unwrap_or(false) {
            return None;
        }
        let horizontal = matches!(name, "left" | "right");
        let px = |v: f64| Some(format!("{}px", crate::computed::fmt_num(v)));
        let length = |el: NodeId, property: &str| -> f64 {
            style
                .computed_value(dom, el, property)
                .and_then(|v| v.strip_suffix("px").and_then(|n| n.parse::<f64>().ok()))
                .unwrap_or(0.0)
        };
        match position {
            Position::Relative | Position::Sticky => {
                // The containing block: the nearest ancestor that generates a box.
                let mut parent = dom.parent_element(el).ok().flatten()?;
                while self.computed(dom, parent).display_contents {
                    parent = dom.parent_element(parent).ok().flatten()?;
                }
                let basis = if horizontal {
                    self.client_width(dom, parent) - length(parent, "padding-left") - length(parent, "padding-right")
                } else {
                    self.client_height(dom, parent) - length(parent, "padding-top") - length(parent, "padding-bottom")
                };
                let resolve = |value: &str| -> Option<f64> {
                    let (px, percent) =
                        crate::computed::resolve_len(value, &crate::computed::LengthContext::fonts(16.0, 16.0))?;
                    Some(px + percent / 100.0 * basis)
                };
                if value != "auto" {
                    return px(resolve(value)?);
                }
                if position == Position::Sticky {
                    return None;
                }
                let opposite = match name {
                    "top" => "bottom",
                    "bottom" => "top",
                    "left" => "right",
                    _ => "left",
                };
                match style.computed_value(dom, el, opposite).as_deref() {
                    Some(o) if o != "auto" => px(-resolve(o)?),
                    _ => Some("0px".into()),
                }
            }
            Position::Absolute | Position::Fixed => {
                // The containing block's padding box, in viewport coordinates.
                let establishes = |a: NodeId| -> bool {
                    let Some(s) = style.stylo_style(dom, a) else { return false };
                    let b = s.get_box();
                    let transformed = !b.transform.0.is_empty()
                        || b.perspective != style::values::generics::box_::GenericPerspective::None
                        || !s.get_effects().filter.0.is_empty();
                    transformed || (position == Position::Absolute && b.position != PositionProperty::Static)
                };
                let mut ancestor = dom.parent_element(el).ok().flatten();
                while let Some(a) = ancestor.filter(|&a| !establishes(a)) {
                    ancestor = dom.parent_element(a).ok().flatten();
                }
                let block = match ancestor {
                    Some(a) => {
                        let r = self.bounding_client_rect(dom, a);
                        Rect {
                            x: r.x + self.client_left(dom, a),
                            y: r.y + self.client_top(dom, a),
                            width: self.client_width(dom, a),
                            height: self.client_height(dom, a),
                        }
                    }
                    None => {
                        Rect { x: 0.0, y: 0.0, width: self.inner_width() as f64, height: self.inner_height() as f64 }
                    }
                };
                let r = self.bounding_client_rect(dom, el);
                px(match name {
                    "top" => r.y - length(el, "margin-top") - block.y,
                    "left" => r.x - length(el, "margin-left") - block.x,
                    "bottom" => block.y + block.height - (r.y + r.height + length(el, "margin-bottom")),
                    _ => block.x + block.width - (r.x + r.width + length(el, "margin-right")),
                })
            }
            Position::Static => None,
        }
    }
}

/// `value` with every px length beyond Blink's LayoutUnit range (±2^25)
/// clamped as Chromium serializes it; `None` when none is.
fn clamp_layout_units(value: &str) -> Option<String> {
    const MAX: f64 = 33_554_431.0;
    if !value.contains("px") {
        return None;
    }
    let mut changed = false;
    let out: Vec<String> = value
        .split(' ')
        .map(|token| match token.strip_suffix("px").and_then(|n| n.parse::<f64>().ok()) {
            Some(n) if n.abs() > MAX => {
                changed = true;
                format!("{}3.35544e+07px", if n < 0.0 { "-" } else { "" })
            }
            _ => token.to_owned(),
        })
        .collect();
    changed.then(|| out.join(" "))
}

// Default inline alignment uses the nearer edge. A box spanning both edges
// stays put; an oversized box outside one edge aligns its opposite edge.
fn nearest_scroll_offset(start: f64, size: f64, viewport: f64, current: u32) -> Option<i32> {
    let end = start + size;
    if (start >= 0.0 && end <= viewport) || (start <= 0.0 && end >= viewport) {
        return None;
    }
    let delta =
        if (start < 0.0 && size <= viewport) || (end > viewport && size > viewport) { start } else { end - viewport };
    Some((current as f64 + delta).round().clamp(0.0, i32::MAX as f64) as i32)
}

fn centered_scroll_offset(start: f64, size: f64, viewport: f64, current: u32) -> Option<i32> {
    Some((current as f64 + start + size / 2.0 - viewport / 2.0).round().clamp(0.0, i32::MAX as f64) as i32)
}

// Blink's center-if-needed on one axis: a fully visible box, or one covering
// the scrollport, stays; a hidden box is centered; a partly visible one is
// brought in by its nearest edge.
fn center_if_needed_offset(start: f64, size: f64, viewport: f64, current: u32) -> Option<i32> {
    let end = start + size;
    if end <= 0.0 || start >= viewport {
        return Some((current as f64 + start + size / 2.0 - viewport / 2.0).round().clamp(0.0, i32::MAX as f64) as i32);
    }
    nearest_scroll_offset(start, size, viewport, current)
}

// Focusing a box already visible on an axis must not perturb that scroll.
fn focus_scroll_offset(start: f64, size: f64, viewport: f64, current: u32) -> Option<i32> {
    if start >= 0.0 && start + size <= viewport {
        return None;
    }
    if start <= 0.0 && start + size >= viewport {
        return None;
    }
    Some((current as f64 + start + size / 2.0 - viewport / 2.0).round().clamp(0.0, i32::MAX as f64) as i32)
}

/// Window.zig:848-851.
fn scroll_limit(size: f64, visible: u32) -> u32 {
    (size - visible as f64).clamp(0.0, u32::MAX as f64) as u32
}

/// frame/user_input.zig:612-615.
fn delta_to_scroll(d: f64) -> i32 {
    if d.is_nan() {
        return 0;
    }
    d.clamp(i32::MIN as f64, i32::MAX as f64).trunc() as i32
}

// ----- text measure (text_measure.zig) ---------------------------------------

fn is_ascii_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// text_measure.zig:40-45.
pub(crate) fn text_width(text: &str, font_size: f64) -> f64 {
    text.chars().map(|c| advance(c, font_size)).sum()
}

/// text_measure.zig:110-117.
fn advance(c: char, font_size: f64) -> f64 {
    let cp = c as u32;
    if is_zero_width(cp) || c == '\n' || c == '\r' {
        return 0.0;
    }
    if c == '\t' {
        return font_size * 1.32;
    }
    if cp < 0x80 && is_ascii_ws(c) {
        return font_size * 0.33;
    }
    if is_wide(cp) {
        return font_size;
    }
    font_size * 0.6
}

/// text_measure.zig:119-128.
fn is_zero_width(cp: u32) -> bool {
    cp == 0x200c
        || cp == 0x200d
        || (0x0300..=0x036f).contains(&cp)
        || (0x1ab0..=0x1aff).contains(&cp)
        || (0x1dc0..=0x1dff).contains(&cp)
        || (0x20d0..=0x20ff).contains(&cp)
        || (0xfe00..=0xfe0f).contains(&cp)
        || (0xfe20..=0xfe2f).contains(&cp)
        || (0xe0100..=0xe01ef).contains(&cp)
}

/// text_measure.zig:130-142.
fn is_wide(cp: u32) -> bool {
    (0x1100..=0x115f).contains(&cp)
        || cp == 0x2329
        || cp == 0x232a
        || (0x2e80..=0xa4cf).contains(&cp)
        || (0xac00..=0xd7a3).contains(&cp)
        || (0xf900..=0xfaff).contains(&cp)
        || (0xfe10..=0xfe19).contains(&cp)
        || (0xfe30..=0xfe6f).contains(&cp)
        || (0xff00..=0xff60).contains(&cp)
        || (0xffe0..=0xffe6).contains(&cp)
        || (0x1f000..=0x1faff).contains(&cp)
        || (0x20000..=0x3fffd).contains(&cp)
}

// ----- computed-style defaults (CSSStyleDeclaration.zig:965-1103) -----------

/// Initial values of properties nothing declares, as Chrome reports them
/// (CSSStyleDeclaration.zig:969-1017).
const INITIAL_VALUES: &[(&str, &str)] = &[
    ("margin", "0px"),
    ("margin-top", "0px"),
    ("margin-right", "0px"),
    ("margin-bottom", "0px"),
    ("margin-left", "0px"),
    ("padding", "0px"),
    ("padding-top", "0px"),
    ("padding-right", "0px"),
    ("padding-bottom", "0px"),
    ("padding-left", "0px"),
    ("border-width", "0px"),
    ("border-top-width", "0px"),
    ("border-right-width", "0px"),
    ("border-bottom-width", "0px"),
    ("border-left-width", "0px"),
    ("border-style", "none"),
    ("border-top-style", "none"),
    ("border-right-style", "none"),
    ("border-bottom-style", "none"),
    ("border-left-style", "none"),
    ("border-radius", "0px"),
    ("border-top-left-radius", "0px"),
    ("border-top-right-radius", "0px"),
    ("border-bottom-right-radius", "0px"),
    ("border-bottom-left-radius", "0px"),
    ("font-weight", "400"),
    ("line-height", "normal"),
    ("letter-spacing", "normal"),
    ("word-spacing", "0px"),
    ("text-indent", "0px"),
    ("top", "auto"),
    ("right", "auto"),
    ("bottom", "auto"),
    ("left", "auto"),
    ("z-index", "auto"),
    ("box-sizing", "content-box"),
    ("min-width", "0px"),
    ("min-height", "0px"),
    ("max-width", "none"),
    ("max-height", "none"),
    ("flex-grow", "0"),
    ("flex-shrink", "1"),
    ("flex-basis", "auto"),
    ("transform", "none"),
    ("gap", "normal"),
    ("row-gap", "normal"),
    ("column-gap", "normal"),
];

/// getDefaultPropertyValue (CSSStyleDeclaration.zig:1019-1058).
fn default_property_value(dom: &Dom, el: NodeId, name: &str) -> &'static str {
    if let Some(&(_, v)) = INITIAL_VALUES.iter().find(|(n, _)| *n == name) {
        return v;
    }
    let element = dom.element(el).ok();
    match name {
        "color" => element.map_or("", |e| {
            if e.is_html() && e.name.local == local_name!("a") {
                "rgb(0, 0, 238)"
            } else {
                "rgb(0, 0, 0)"
            }
        }),
        "opacity" => "1",
        "display" => element.map_or("", default_display),
        "visibility" => "visible",
        "pointer-events" => "auto",
        "background-color" => "rgba(0, 0, 0, 0)",
        "fill" => "rgb(0, 0, 0)",
        "stroke" => "none",
        // Gecko-only in Stylo, so the cascade has no value.
        "resize" => match element {
            Some(e) if e.is_html() && e.name.local == local_name!("textarea") => "both",
            _ => "none",
        },
        _ => "",
    }
}

/// The UA default display: the flow layout's HTML table (`li` list-item,
/// table parts, form controls inline-block, unknown tags inline); SVG and
/// other namespaces `inline`.
fn default_display(e: &crate::node::ElementData) -> &'static str {
    if !e.is_html() {
        return "inline";
    }
    crate::style::ua_display(&e.name.local.to_ascii_lowercase())
}

pub(crate) mod flow_stats {
    use std::cell::RefCell;
    use std::sync::OnceLock;
    use std::time::Duration;

    type Key = ((u64, u64, u32, u32), (u32, u32));

    #[derive(Default)]
    struct Stats {
        passes: u64,
        total: Duration,
        first: u64,
        version: u64,
        generation: u64,
        focus: u64,
        hover: u64,
        viewport: u64,
        // Since the last print.
        window: Duration,
    }

    thread_local! {
        static STATS: RefCell<Stats> = RefCell::new(Stats::default());
    }

    pub(crate) fn enabled() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var("LIGHTNINGPANDA_FLOW_STATS").is_ok_and(|v| v == "1"))
    }

    pub(super) fn record(previous: Option<Key>, now: Key, took: Duration) {
        STATS.with(|s| {
            let mut s = s.borrow_mut();
            s.passes += 1;
            s.total += took;
            s.window += took;
            match previous {
                None => s.first += 1,
                Some((old, old_vp)) => {
                    let (o, n) = (old, now.0);
                    if o.0 != n.0 {
                        s.version += 1;
                    }
                    if o.1 != n.1 {
                        s.generation += 1;
                    }
                    if o.2 != n.2 {
                        s.focus += 1;
                    }
                    if o.3 != n.3 {
                        s.hover += 1;
                    }
                    if old_vp != now.1 {
                        s.viewport += 1;
                    }
                }
            }
            if s.passes % 25 == 0 {
                eprintln!(
                    "flow-stats passes={} total_ms={} avg_ms={:.1} first={} version={} generation={} focus={} hover={} viewport={}",
                    s.passes,
                    s.total.as_millis(),
                    s.total.as_secs_f64() * 1000.0 / s.passes as f64,
                    s.first,
                    s.version,
                    s.generation,
                    s.focus,
                    s.hover,
                    s.viewport
                );
                eprintln!("flow-stats last25 avg_ms={:.1}", s.window.as_secs_f64() * 1000.0 / 25.0);
                s.window = Duration::ZERO;
            }
        });
    }
}

#[cfg(test)]
mod hit_test_geometry_work {
    use super::*;
    use std::cell::Cell;

    struct CountedGeometry {
        data: RefCell<ExternalGeometry>,
        reads: Cell<usize>,
        style: style::servo_arc::Arc<style::properties::ComputedValues>,
    }

    impl GeometrySource for CountedGeometry {
        fn sync(&self, _dom: &Dom, _layout: &Layout) {}
        fn element_box(&self, key: u64) -> Option<ExternalBox> {
            self.reads.set(self.reads.get() + 1);
            self.data.borrow().boxes.iter().find(|(k, _)| *k == key).map(|(_, b)| *b)
        }
        fn geometry(&self) -> Option<ExternalGeometry> {
            Some(self.data.borrow().clone())
        }
        fn document_size(&self) -> Option<(f64, f64)> {
            Some((800.0, 600.0))
        }
        fn has_sticky(&self) -> bool {
            false
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn element_style(&self, _key: u64) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
            Some(self.style.clone())
        }
    }

    #[test]
    fn dense_hit_testing_reports_geometry_reads_and_refreshes_after_mutation() {
        use crate::parse::{parse_document, DomRef};
        let html = format!(
            "<!doctype html><body>{}</body>",
            (0..1000).map(|i| format!("<div id=c{i}></div>")).collect::<String>()
        );
        let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom_ref, "http://test/", &html);
        let mut dom = Rc::try_unwrap(dom_ref).ok().unwrap().into_inner();
        let nodes: Vec<_> = (0..1000).map(|i| dom.get_element_by_id(doc, &format!("c{i}")).unwrap().unwrap()).collect();
        let source = Rc::new(CountedGeometry {
            data: RefCell::new(ExternalGeometry {
                boxes: nodes
                    .iter()
                    .enumerate()
                    .map(|(i, &node)| {
                        (
                            crate::snapshot::render_key(node),
                            ExternalBox {
                                x: (i % 25 * 30) as f64,
                                y: (i / 25 * 12) as f64,
                                width: 30.0,
                                height: 12.0,
                                client_width: 30.0,
                                client_height: 12.0,
                                scroll_width: 30.0,
                                scroll_height: 12.0,
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
                document_width: 800.0,
                document_height: 600.0,
                ..Default::default()
            }),
            reads: Cell::new(0),
            style: style::properties::ComputedValues::initial_values_with_font_override(
                style::properties::style_structs::Font::initial_values(),
            ),
        });
        let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
        layout.set_geometry_source(Some(source.clone()));
        let target = nodes[421];
        for _ in 0..10 {
            assert_eq!(layout.element_from_point(&dom, 635.0, 198.0), Some(target));
        }
        eprintln!("dense-hit-test: 1000 boxes, 10 hits, external element reads={}", source.reads.get());
        // A handler between input events can move a box. No cached client
        // rectangle from the previous hit test may survive the DOM change.
        {
            let mut geometry = source.data.borrow_mut();
            geometry.boxes[421].1.x = 0.0;
            geometry.boxes[421].1.y = 0.0;
        }
        dom.set_attribute(target, "data-moved", "yes").unwrap();
        assert_eq!(layout.element_from_point(&dom, 635.0, 198.0), None);
        assert_eq!(layout.element_from_point(&dom, 5.0, 5.0), Some(target));
    }
}

#[cfg(test)]
mod scroll_to_origin {
    use super::*;
    use std::cell::Cell;

    /// A 2000 px tall document that counts its syncs.
    struct Tall(Cell<usize>);

    impl GeometrySource for Tall {
        fn sync(&self, _dom: &Dom, _layout: &Layout) {
            self.0.set(self.0.get() + 1);
        }
        fn element_box(&self, _key: u64) -> Option<ExternalBox> {
            Some(ExternalBox {
                width: 800.0,
                height: 2000.0,
                scroll_height: 2000.0,
                client_height: 600.0,
                ..Default::default()
            })
        }
        fn geometry(&self) -> Option<ExternalGeometry> {
            Some(ExternalGeometry { document_width: 800.0, document_height: 2000.0, ..Default::default() })
        }
        fn document_size(&self) -> Option<(f64, f64)> {
            Some((800.0, 2000.0))
        }
        fn has_sticky(&self) -> bool {
            false
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[test]
    fn scrolling_to_the_origin_lays_nothing_out() {
        use crate::parse::{parse_document, DomRef};
        let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom_ref, "http://test/", "<!doctype html><body><div id=s></div></body>");
        let dom = Rc::try_unwrap(dom_ref).ok().unwrap().into_inner();
        let div = dom.get_element_by_id(doc, "s").unwrap().unwrap();
        let source = Rc::new(Tall(Cell::new(0)));
        let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
        layout.set_geometry_source(Some(source.clone()));
        // At the origin already, or asked for it: no layout, no move.
        assert!(!layout.window_scroll_to(&dom, Some(0), Some(0)));
        assert!(!layout.window_scroll_to(&dom, Some(-5), None));
        assert!(!layout.window_scroll_by(&dom, Some(0), Some(-10)));
        assert!(!layout.set_scroll_top(&dom, div, 0));
        assert_eq!(source.0.get(), 0, "a scroll to the origin synced the layout");
        // A positive offset needs the document's size.
        assert!(layout.window_scroll_to(&dom, None, Some(500)));
        assert_eq!(layout.scroll_y(), 500);
        assert_eq!(source.0.get(), 1);
        // Back to the origin moves without laying out again.
        assert!(layout.window_scroll_to(&dom, Some(0), Some(0)));
        assert_eq!(layout.scroll_y(), 0);
        assert_eq!(source.0.get(), 1);
    }
}
