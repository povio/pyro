//! Layout queries (CSSOM View: `getBoundingClientRect`, `offset*`,
//! `client*`, `scroll*`, `elementFromPoint`, scrolling) over the geometry an
//! external engine laid the document out as ([`GeometrySource`]: Blitz, in
//! pyro-render). pyro-dom lays nothing out itself; without a source (builds
//! without `render`) every element reports an empty box.
//!
//! What pyro-dom adds on top of the engine's boxes: the window and element
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
use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use crate::dom::Dom;
use crate::geometry::{node_of, LayoutBox, LayoutResult, PaintChain, PaintEntry, TextResult, BOTTOM, LEFT, RIGHT, TOP};
pub use crate::geometry::{
    ExternalBox, ExternalGeometry, ExternalQuads, ExternalSticky, ExternalWord, GeometrySource, HitProbe, HitVisit,
    LayoutBody, LayoutBodyKey, SubresourceBodies, SubresourceKind, SubresourceLoader, SubresourceOptions,
};
/// Overflow clips an out-of-flow box escapes while walking up to its
/// containing block (`Layout::clip_escape`).
#[derive(Copy, Clone, PartialEq, Eq)]
enum ClipEscape {
    None,
    /// `position: fixed`: no ancestor clips up to the nearest one that is
    /// a fixed containing block (a transform, filter, `contain: paint`, ...;
    /// `ComputedStyle::contains_fixed`), if any.
    Viewport,
    /// `position: absolute`: no ancestor clips up to the nearest containing
    /// block (positioned, or a fixed containing block;
    /// `ComputedStyle::contains_absolute`).
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
    /// The engine may split boxes into fragments (columns): a box is hit only in one of its
    /// fragments ([`GeometrySource::element_fragments`]), not anywhere in their union.
    has_fragments: bool,
    /// Boxes' fragments as the engine reports them (`None`: whole), by element.
    fragments: FxHashMap<NodeId, Option<Rc<[Rect]>>>,
}

use crate::media::Viewport;
use crate::node::{DocumentKind, NodeId, NodeKind, NIL};
use crate::style::{CheckVisibilityOptions, ComputedStyle, Display, Position, ScrollAxes, StyleEngine, UserSelect};
use crate::viewport_meta::{MetaViewport, MobileMetrics, MobileViewport};
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

/// pyro: a box a scroller snapped to: an element, or the column of a multi-column scroll
/// container at an index (`::column` snap areas).
type SnapBox = (NodeId, Option<usize>);

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
    fn target(self, axis: Axis, current: i32) -> Option<i64> {
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

/// What `resize` events report a change of ([`Layout::viewport_metrics`]):
/// the layout viewport, the window (`innerWidth`, `innerHeight`) and the
/// visual viewport (width, height, scale). Without mobile emulation all
/// three are the emulated viewport, at scale 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportMetrics {
    pub layout: (u32, u32),
    pub window: (u32, u32),
    pub visual: (f64, f64, f64),
}

impl ViewportMetrics {
    /// The window's `resize`: the layout viewport or the window changed size.
    pub fn window_resized(&self, before: &ViewportMetrics) -> bool {
        self.layout != before.layout || self.window != before.window
    }

    /// The visual viewport's `resize`: its size or scale changed.
    pub fn visual_resized(&self, before: &ViewportMetrics) -> bool {
        self.visual != before.visual
    }
}

/// Layout state of one document: its style engine (which owns the
/// viewport), its geometry source, the window scroll offset and
/// per-element scroll offsets.
pub struct Layout {
    style: StyleEngine,
    /// The viewport as emulated ([`Layout::set_viewport`]); `style` has the
    /// layout viewport made of it, the same one unless it is `mobile`.
    device: Viewport,
    /// Under mobile emulation: the document's viewport `<meta>` on the
    /// device.
    mobile: Option<MobileViewport>,
    /// The document's viewport `<meta>` ([`Layout::set_viewport_meta`]).
    meta: Option<MetaViewport>,
    /// Under mobile emulation: the document's width (CSS px, whole) as of
    /// the last sync of the geometry source, which decides the minimum
    /// page scale (`None` before one).
    contents_width: std::cell::Cell<Option<f32>>,
    /// pyro: signed: a root whose inline or block axis runs right to left (`direction: rtl`,
    /// `vertical-rl`) scrolls from 0 at its start to negative offsets (CSSOM View 1 § 4).
    window_scroll: (i32, i32),
    scroll_generation: u64,
    /// The `scroll_generation` last told to the geometry source for scroll timelines, and when
    /// (page clock, ms) ([`Self::push_scroll`]).
    scroll_pushed: std::cell::Cell<Option<u64>>,
    scroll_pushed_at: std::cell::Cell<f64>,
    /// Each scrolled element's `scrollLeft` and `scrollTop`: negative on an axis whose scroll
    /// origin is at its end (right-to-left, `row-reverse`, `column-reverse`, ...), as CSSOM View
    /// and Blitz's scroll offsets have them.
    element_scroll: FxHashMap<NodeId, (i32, i32)>,
    /// CSS Overflow 5 § 2.1: each scroll container's current scroll target, the element a script
    /// scroll into view last revealed in it, until it scrolls otherwise (the viewport's under
    /// `None`). Scroll marker selection reads it (`scroll_targets`).
    scroll_targets: FxHashMap<Option<NodeId>, NodeId>,
    /// CSS Scroll Anchoring: the layout each vertically scrolled element
    /// was last settled in, to select its anchor in when the layout next
    /// changes.
    /// pyro: with the anchor selected in it and the suppression-trigger values of the anchor and
    /// its ancestors then (`anchor_fingerprint`).
    scroll_anchors: FxHashMap<NodeId, (Rc<LayoutResult>, Option<(NodeId, String)>)>,
    /// pyro: the box each scroller (`None`: the viewport) last snapped to on each axis (x, y), to
    /// snap to again after layout changes (CSS Scroll Snap 1 § 6.2.3).
    snapped: FxHashMap<Option<NodeId>, [Option<SnapBox>; 2]>,
    /// pyro: the mandatory snap containers whose initial scroll position snapped
    /// ([`Self::snap_initial_positions`]), and the DOM version they were last looked for at.
    initially_snapped: rustc_hash::FxHashSet<NodeId>,
    /// pyro: the scroll offsets of scroll containers that are `::before` (1) or `::after` (2)
    /// boxes of an element, which they keep across relayouts (WPT css-overflow
    /// `before-after-pseudo-element-scrolling`).
    pseudo_scroll: FxHashMap<(NodeId, u8), (i32, i32)>,
    /// pyro: whether the viewport took its initial snap position.
    viewport_initially_snapped: bool,
    /// pyro: the scroll offset script last gave an element on each axis in its CSS px (x, y), with
    /// the whole layout px it was kept as: read back while the offset is still that one, as a
    /// zoomed element's fractional offset ([`Self::css_scroll`]).
    css_scroll: FxHashMap<NodeId, [Option<(f64, i32, f64)>; 2]>,
    initial_snap_checked: std::cell::Cell<Option<u64>>,
    /// The engine laying the document out.
    external: Option<Rc<dyn GeometrySource>>,
    /// The observation stamp and viewport the engine was last synced at.
    external_synced: std::cell::Cell<Option<SyncedAt>>,
    /// Syncs so far (the whole document's geometry is read again after one).
    syncs: std::cell::Cell<u64>,
    /// The whole document's boxes, with the sync they were read at.
    result: RefCell<Option<(u64, Rc<LayoutResult>)>>,
    /// The whole document's words, with the sync they were read at.
    text: RefCell<Option<(u64, Rc<TextResult>)>>,
    /// The `position: sticky` elements' inputs, with the sync they were
    /// read at.
    sticky: RefCell<Option<(u64, Rc<FxHashMap<NodeId, ExternalSticky>>)>>,
    /// Sticky entries looked up one element at a time since the last sync
    /// ([`Layout::sticky_shift`]), `None` for elements that are not sticky.
    sticky_lazy: RefCell<Option<(u64, FxHashMap<NodeId, Option<ExternalSticky>>)>>,
    anchor_sticky_deltas: RefCell<((u64,u64), FxHashMap<NodeId,(f64,f64)>)>,
    /// The hit-test index of the current layout and scroll position, and
    /// what decides when one is built (see [`HitIndex`]).
    hit_index: RefCell<HitIndexState>,
}

/// The hit-testable boxes of one layout at one scroll position, in paint
/// input order, under a spatial index: answers "the topmost box at this
/// point" from the few boxes around the point, for as long as neither the
/// layout (DOM, styles, focus/hover, viewport, a loaded resource: one more
/// sync of the engine) nor a scroll offset changes.
///
/// `els`/`rects` list every box with an area in tree order, with the client
/// rect hit testing gives it; `grid` maps a point to the boxes whose rect
/// reaches the point's cell. Paint order (stacking contexts, z-index, top
/// layer), overflow clips and the style predicates of the boxes found are
/// those of `result`, resolved once per box when a test first meets it
/// (`LayoutResult::hit`, `geometry`): the selection is the scan's, over
/// fewer boxes.
struct HitIndex {
    result: Rc<LayoutResult>,
    geometry: RefCell<HitTestGeometry>,
    els: Vec<NodeId>,
    rects: Vec<Rect>,
    grid: HitGrid,
}

impl HitIndex {
    /// `hit_select`'s test of a point against a half-open box.
    fn holds(r: &Rect, x: f64, y: f64) -> bool {
        x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
    }
}

/// A uniform grid over the bounds of a list of rects: each cell lists, in
/// ascending order, the rects reaching it (compressed rows: `starts[c]..
/// starts[c + 1]` of `items`). A rect holding a point is listed in the
/// point's cell: the cell of a coordinate never decreases as the coordinate
/// grows, and a rect is listed from its near edge's cell to its far edge's.
#[derive(Default)]
struct HitGrid {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    /// Cells per CSS pixel.
    sx: f64,
    sy: f64,
    nx: usize,
    ny: usize,
    starts: Vec<u32>,
    items: Vec<u32>,
    /// Rects with an edge at infinity, in every cell's list by hand.
    unbounded: Vec<u32>,
}

impl HitGrid {
    /// At most this many list entries per rect on average: a coarser grid
    /// is taken when large boxes (a page's wrappers span every cell) would
    /// need more.
    const FILL: usize = 12;

    fn new(rects: &[Rect]) -> HitGrid {
        let mut g = HitGrid {
            x0: f64::INFINITY,
            y0: f64::INFINITY,
            x1: f64::NEG_INFINITY,
            y1: f64::NEG_INFINITY,
            ..Default::default()
        };
        let finite = |r: &Rect| r.x.is_finite() && r.y.is_finite() && r.width.is_finite() && r.height.is_finite();
        let mut bounded = 0usize;
        for (i, r) in rects.iter().enumerate() {
            if !finite(r) {
                g.unbounded.push(i as u32);
                continue;
            }
            bounded += 1;
            g.x0 = g.x0.min(r.x);
            g.y0 = g.y0.min(r.y);
            g.x1 = g.x1.max(r.x + r.width);
            g.y1 = g.y1.max(r.y + r.height);
        }
        if bounded == 0 {
            // No cell: `cell` answers `unbounded` for every point.
            (g.x0, g.y0, g.x1, g.y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            return g;
        }
        // About one cell per rect, in the bounds' proportions.
        let (w, h) = ((g.x1 - g.x0).max(1.0), (g.y1 - g.y0).max(1.0));
        let cells = bounded.min(1 << 16) as f64;
        let mut nx = ((cells * w / h).sqrt().round() as usize).clamp(1, 1 << 10);
        let mut ny = ((cells / nx as f64).round() as usize).clamp(1, 1 << 12);
        loop {
            g.nx = nx;
            g.ny = ny;
            g.sx = if g.x1 > g.x0 { nx as f64 / (g.x1 - g.x0) } else { 0.0 };
            g.sy = if g.y1 > g.y0 { ny as f64 / (g.y1 - g.y0) } else { 0.0 };
            let limit = Self::FILL * bounded + nx * ny;
            let mut counts = vec![0u32; nx * ny + 1];
            let mut total = 0usize;
            for r in rects.iter().filter(|r| finite(r)) {
                let (cx0, cx1, cy0, cy1) = g.span(r);
                total += (cx1 - cx0 + 1) * (cy1 - cy0 + 1);
                if total > limit && nx * ny > 1 {
                    break;
                }
                for cy in cy0..=cy1 {
                    for c in &mut counts[cy * nx + cx0..=cy * nx + cx1] {
                        *c += 1;
                    }
                }
            }
            if total > limit && nx * ny > 1 {
                nx = nx.div_ceil(2);
                ny = ny.div_ceil(2);
                continue;
            }
            // Counts to starts, then fill in rect order (ascending lists).
            let mut sum = 0u32;
            for c in counts.iter_mut() {
                let n = *c;
                *c = sum;
                sum += n;
            }
            let mut next = counts.clone();
            let mut items = vec![0u32; sum as usize];
            for (i, r) in rects.iter().enumerate().filter(|(_, r)| finite(r)) {
                let (cx0, cx1, cy0, cy1) = g.span(r);
                for cy in cy0..=cy1 {
                    for cx in cx0..=cx1 {
                        let at = &mut next[cy * nx + cx];
                        items[*at as usize] = i as u32;
                        *at += 1;
                    }
                }
            }
            if !g.unbounded.is_empty() {
                // Rare (a box at infinity): merged into every list.
                let mut merged = Vec::with_capacity(items.len() + g.unbounded.len() * nx * ny);
                let mut starts = Vec::with_capacity(counts.len());
                for c in 0..nx * ny {
                    starts.push(merged.len() as u32);
                    let mut list: Vec<u32> = items[counts[c] as usize..counts[c + 1] as usize].to_vec();
                    list.extend_from_slice(&g.unbounded);
                    list.sort_unstable();
                    merged.extend(list);
                }
                starts.push(merged.len() as u32);
                (counts, items) = (starts, merged);
            }
            g.starts = counts;
            g.items = items;
            return g;
        }
    }

    fn col(&self, x: f64) -> usize {
        (((x - self.x0) * self.sx).floor().max(0.0) as usize).min(self.nx - 1)
    }

    fn row(&self, y: f64) -> usize {
        (((y - self.y0) * self.sy).floor().max(0.0) as usize).min(self.ny - 1)
    }

    /// The cells a (finite) rect reaches: columns, then rows, inclusive.
    fn span(&self, r: &Rect) -> (usize, usize, usize, usize) {
        (self.col(r.x), self.col(r.x + r.width), self.row(r.y), self.row(r.y + r.height))
    }

    /// The rects that may hold the point, ascending.
    fn cell(&self, x: f64, y: f64) -> &[u32] {
        if x.is_nan() || y.is_nan() {
            return &[];
        }
        if !(x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1) {
            // Outside every bounded rect.
            return &self.unbounded;
        }
        let c = self.row(y) * self.nx + self.col(x);
        &self.items[self.starts[c] as usize..self.starts[c + 1] as usize]
    }
}

/// [`Layout`]'s hit-test index and the account that decides when to build
/// one (see `Layout::hit_index_for`).
#[derive(Default)]
struct HitIndexState {
    /// The sync count and scroll generation the rest is of.
    epoch: Option<(u64, u64)>,
    index: Option<Rc<HitIndex>>,
    /// Tests of this epoch answered without an index, and what they took.
    tests: u32,
    rented: std::time::Duration,
    /// What a descent takes and what an index takes to build (nothing
    /// before the first of each).
    descent_cost: CostEstimate,
    build_cost: CostEstimate,
    /// What the descents since the last build took, over every layout.
    rented_since_build: std::time::Duration,
    /// Layouts indexed at their first test since a descent was last timed.
    bought_unseen: u32,
    builds: u64,
}

/// What one of the two hit-test paths costs, from the clock: the lesser of
/// its last two measurements. A measurement is the work plus whatever the
/// thread waited inside it (descheduled on a busy machine, a cold first
/// run), so one alone can be orders of magnitude too high, and the choice
/// made from it stops the path being measured again; two in a row that
/// high are a cost that rose.
#[derive(Default)]
struct CostEstimate([Option<std::time::Duration>; 2]);

impl CostEstimate {
    fn record(&mut self, took: std::time::Duration) {
        self.0 = [self.0[1], Some(took)];
    }

    fn get(&self) -> Option<std::time::Duration> {
        self.0.iter().flatten().min().copied()
    }
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
            device: style.viewport().clone(),
            style,
            mobile: None,
            meta: None,
            contents_width: std::cell::Cell::new(None),
            window_scroll: (0, 0),
            scroll_generation: 0,
            scroll_pushed: std::cell::Cell::new(None),
            scroll_pushed_at: std::cell::Cell::new(f64::NEG_INFINITY),
            element_scroll: FxHashMap::default(),
            scroll_targets: FxHashMap::default(),
            scroll_anchors: FxHashMap::default(),
            snapped: FxHashMap::default(),
            initially_snapped: Default::default(),
            pseudo_scroll: FxHashMap::default(),
            viewport_initially_snapped: false,
            css_scroll: FxHashMap::default(),
            initial_snap_checked: Default::default(),
            external,
            external_synced: std::cell::Cell::new(None),
            syncs: std::cell::Cell::new(0),
            result: RefCell::new(None),
            text: RefCell::new(None),
            sticky: RefCell::new(None),
            sticky_lazy: RefCell::new(None),
            anchor_sticky_deltas: Default::default(),
            hit_index: RefCell::default(),
        }
    }

    /// Takes the geometry from `source` (see [`GeometrySource`]); `None`:
    /// nothing is laid out.
    /// pyro: a rendering frame starts (`content-visibility: auto` relevance is determined anew).
    pub fn content_visibility_frame(&self) {
        if let Some(source) = self.external.as_ref().filter(|s| s.active()) {
            source.content_visibility_frame();
        }
    }

    pub fn set_geometry_source(&mut self, source: Option<Rc<dyn GeometrySource>>) {
        self.style.set_style_engine(source.clone());
        self.external = source;
        self.external_synced.set(None);
        self.result.borrow_mut().take();
        self.text.borrow_mut().take();
        self.sticky.borrow_mut().take();
        *self.hit_index.borrow_mut() = HitIndexState::default();
    }

    /// The geometry source, brought up to the document (synced when
    /// anything it lays out changed since the last query).
    pub fn synced_geometry_source(&self, dom: &Dom) -> Option<&Rc<dyn GeometrySource>> {
        let source = self.external.as_ref().filter(|s| s.active())?;
        let key = self.sync_key(dom);
        // pyro: changes made only to nodes outside the document (detached clones styled one
        // property at a time between computed-style reads) need no sync.
        if let Some(old) = self.external_synced.get() {
            let ((v0, g0, f0, h0), vp0) = old;
            let ((v1, g1, f1, h1), vp1) = key;
            if v0 != v1
                && (g0, f0, h0, vp0) == (g1, f1, h1, vp1)
                && !source.stale()
                && !self.style.animations_changed_for_layout()
                && dom.only_detached_changes_since(v0)
            {
                self.external_synced.set(Some(key));
            }
        }
        if self.external_synced.get() != Some(key) || source.stale() || self.style.animations_changed_for_layout() {
            self.style.before_recalc(dom);
            source.sync(dom, self);
            self.external_synced.set(Some(key));
            self.syncs.set(self.syncs.get() + 1);
            self.style.engine_synced(dom);
            self.style.animations_laid_out();
            if self.mobile.is_some() {
                // Blink's contents size is whole px (`LayoutView::DocumentRect`).
                self.contents_width.set(source.document_size().map(|(w, _)| w.round() as f32));
            }
        }
        Some(source)
    }

    /// pyro: whether the document's sheets declare a web font (`@font-face`).
    pub fn declares_web_fonts(&self, dom: &Dom) -> bool {
        const FONT_FACE: &str = "@font-face";
        // Loaded sheets (`<link>`, `@import`ed texts), then `<style>` elements by their text.
        if self.style.sheet_owners().into_iter().any(|owner| self.style.sheet_text(dom, owner).is_some_and(|t| t.contains(FONT_FACE))) {
            return true;
        }
        let Ok(mut nodes) = dom.descendants(self.style.document()) else { return false };
        nodes.any(|id| {
            dom.element(id).is_ok_and(|e| e.is_html() && &*e.name.local == "style")
                && dom.child_text_content(id).contains(FONT_FACE)
        })
    }

    /// pyro: starts the geometry engine's web font fetches when the document's sheets declare
    /// web fonts: it lays the document out once, which asks for the faces its styles use (as a
    /// browser starts loading a face when a style first uses it), so that measurements after
    /// they arrive see them. Nothing without web fonts or a geometry source.
    pub fn start_font_loads(&self, dom: &Dom) {
        if self.external.as_ref().is_some_and(|s| s.active()) && self.declares_web_fonts(dom) {
            self.synced_geometry_source(dom);
        }
    }

    /// pyro: the geometry engine's state of this document's font request for `url`
    /// ([`GeometrySource::font_load`]).
    pub fn font_load(&self, url: &str) -> Option<crate::geometry::FontLoad> {
        self.external.as_ref().and_then(|s| s.font_load(url))
    }

    /// pyro: whether web fonts the geometry engine uses are still loading (it starts them
    /// first, see [`Self::start_font_loads`]).
    pub fn fonts_pending(&self, dom: &Dom) -> bool {
        self.start_font_loads(dom);
        self.external.as_ref().is_some_and(|s| s.fonts_pending())
    }

    /// pyro: whether the CSS images the last completed layout asked for are still loading
    /// (`GeometrySource::images_pending`), never running layout.
    pub fn images_pending_as_laid_out(&self) -> bool {
        self.external.as_ref().is_some_and(|s| s.images_pending())
    }

    /// [`Self::fonts_pending`] as of the last completed layout, never running layout: the loads
    /// that layout started (a face is loaded when layout first needs it). For readiness checks
    /// made at every microtask checkpoint (the load event).
    pub fn fonts_pending_as_laid_out(&self, dom: &Dom) -> bool {
        if self.style.engine_current(dom) {
            return self.fonts_pending(dom);
        }
        self.external.as_ref().is_some_and(|s| s.fonts_pending())
    }

    /// A rect query lays the document out anyway: sync the engine before
    /// the visibility check, so that check reads the styles of that one
    /// sync (it stamps the style engine) instead of running a style-only
    /// pass first and the full sync right after. Without an active
    /// geometry source this does nothing.
    fn sync_before_rect_query(&self, dom: &Dom) {
        self.synced_geometry_source(dom);
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

    /// The document's network, for the geometry source's sub-resources
    /// (see [`SubresourceLoader`]).
    pub fn set_subresource_loader(&self, loader: std::sync::Arc<dyn SubresourceLoader>) {
        if let Some(source) = &self.external {
            source.set_subresource_loader(loader);
        }
    }

    /// The whole document's boxes, read again when the DOM, styles,
    /// focus/hover or viewport changed (scroll anchoring, and hit testing
    /// when the engine offers no descent: see [`Layout::element_from_point`]).
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

    /// The whole document's words, read again when the DOM, styles,
    /// focus/hover or viewport changed (text ranges, carets, selection).
    pub fn text_result(&self, dom: &Dom) -> Rc<TextResult> {
        let Some(source) = self.synced_geometry_source(dom) else { return Rc::default() };
        let key = self.syncs.get();
        if let Some((k, r)) = self.text.borrow().as_ref() {
            if *k == key {
                return r.clone();
            }
        }
        let started = flow_stats::enabled().then(std::time::Instant::now);
        let result = Rc::new(source.text_words().map(|w| TextResult::from_words(dom, w)).unwrap_or_default());
        if let Some(t) = started {
            flow_stats::record(None, self.sync_key(dom), t.elapsed());
        }
        *self.text.borrow_mut() = Some((key, result.clone()));
        result
    }

    /// The words of the text nodes below `within` (with others), for one
    /// query: the document's when they are read already since the last
    /// change ([`Self::text_result`]), else only those of the inline
    /// formatting contexts in and around `within`
    /// ([`GeometrySource::subtree_words`]).
    fn text_result_within(&self, dom: &Dom, within: NodeId) -> Rc<TextResult> {
        let Some(source) = self.synced_geometry_source(dom) else { return Rc::default() };
        let key = self.syncs.get();
        if let Some((k, r)) = self.text.borrow().as_ref() {
            if *k == key {
                return r.clone();
            }
        }
        match source.subtree_words(crate::snapshot::render_key(within)) {
            Some(words) => Rc::new(TextResult::from_words(dom, words)),
            None => self.text_result(dom),
        }
    }

    /// The `position: sticky` elements' inputs, read again when the DOM,
    /// styles, focus/hover or viewport changed; empty while the engine lays
    /// out no sticky element (one flag check: no read).
    ///
    /// pyro: whether any element is sticky is known from styles alone (the
    /// engine's style traversal keeps the set, `GeometrySource::has_sticky`):
    /// a document without one is restyled, not laid out, for the question.
    /// Every render snapshot asks it (each screenshot and screencast frame),
    /// and a layout of the whole document for it after each DOM change cost
    /// a capture about half a millisecond.
    fn sticky_map(&self, dom: &Dom) -> Rc<FxHashMap<NodeId, ExternalSticky>> {
        let styled_without_sticky = self.external.as_ref().filter(|s| s.active()).is_some_and(|source| {
            // (`None`: no styles to tell by now; the sync below decides.)
            self.style.engine_styled(dom).is_some() && !source.has_sticky()
        });
        if styled_without_sticky {
            return Rc::default();
        }
        let Some(source) = self.synced_geometry_source(dom).filter(|s| s.has_sticky()) else { return Rc::default() };
        let key = self.syncs.get();
        if let Some((k, m)) = self.sticky.borrow().as_ref() {
            if *k == key {
                return m.clone();
            }
        }
        let map: FxHashMap<NodeId, ExternalSticky> =
            source.sticky_elements().into_iter().filter_map(|(k, s)| Some((node_of(dom, k)?, s))).collect();
        let map = Rc::new(map);
        *self.sticky.borrow_mut() = Some((key, map.clone()));
        map
    }

    /// One element's box, synced only when something the engine lays out
    /// changed: no whole-document work per query.
    fn layout_box(&self, dom: &Dom, el: NodeId) -> Option<LayoutBox> {
        let source = self.synced_geometry_source(dom)?;
        let key = crate::snapshot::render_key(el);
        let b = source.element_box(key);
        // pyro: no box, or an empty one: perhaps in contents `content-visibility: auto` skips, which
        // a query renders (CSS Contain 2 § 4.1, as Chromium): read again once rendered.
        let b = match b {
            Some(b) if b.width > 0.0 || b.height > 0.0 => Some(b),
            b if source.force_content_visibility(key) => source.element_box(key).or(b),
            b => b,
        };
        b.map(|b| LayoutBox::from_external(&b, 0))
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

    /// The layout viewport: what layout, media queries, viewport units and
    /// the root's `clientWidth`/`clientHeight` see. The emulated viewport,
    /// unless it is `mobile` ([`Layout::set_viewport_meta`]).
    pub fn viewport(&self) -> &Viewport {
        self.style.viewport()
    }

    /// The viewport as emulated (`screen`, `outerWidth`/`outerHeight`).
    pub fn device_viewport(&self) -> &Viewport {
        &self.device
    }

    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.device = viewport;
        self.apply_viewport();
    }

    /// The viewport `<meta>` the document processed last: under mobile
    /// emulation it decides the layout viewport and the page scale.
    pub fn set_viewport_meta(&mut self, meta: Option<MetaViewport>) {
        if self.meta != meta {
            self.meta = meta;
            self.apply_viewport();
        }
    }

    fn apply_viewport(&mut self) {
        if !self.device.mobile {
            self.mobile = None;
            self.style.set_viewport(self.device.clone());
            return;
        }
        let mobile = MobileViewport::new(self.meta.as_ref(), self.device.width, self.device.height);
        let mut layout = self.device.clone();
        layout.screen_width.get_or_insert(self.device.width);
        layout.screen_height.get_or_insert(self.device.height);
        (layout.width, layout.height) = mobile.layout_size();
        if self.mobile.is_none() || (layout.width, layout.height) != (self.viewport().width, self.viewport().height) {
            // Laid out again at another width: until then, as wide as the layout viewport.
            self.contents_width.set(None);
        }
        self.mobile = Some(mobile);
        self.style.set_viewport(layout);
    }

    /// The viewport metrics `resize` events report on.
    pub fn viewport_metrics(&self) -> ViewportMetrics {
        let v = self.viewport();
        ViewportMetrics { layout: (v.width, v.height), window: self.window_size(), visual: self.visual_viewport() }
    }

    /// Whether the geometry source laid the document out at least once
    /// (Blink's `DidFirstLayout`).
    pub fn has_laid_out(&self) -> bool {
        self.syncs.get() > 0
    }

    /// Under mobile emulation, the frame, visual viewport and page scale of
    /// the document as last laid out (`None` without mobile emulation).
    pub fn mobile_metrics(&self) -> Option<MobileMetrics> {
        let mobile = self.mobile.as_ref()?;
        let layout_width = self.viewport().width as f32;
        Some(mobile.metrics(self.contents_width.get().map_or(layout_width, |w| w.max(layout_width))))
    }

    /// `window.innerWidth`/`innerHeight` and the viewport scrolling, hit
    /// testing and the implicit intersection root use: the layout viewport,
    /// or under mobile emulation the frame (the device at the minimum page
    /// scale, wider than the layout viewport when the document is).
    pub fn window_size(&self) -> (u32, u32) {
        match self.mobile_metrics() {
            Some(m) => m.frame,
            None => (self.viewport().width, self.viewport().height),
        }
    }

    /// `window.visualViewport`: width and height (CSS px) and scale. The
    /// layout viewport at scale 1, or under mobile emulation the device at
    /// the page scale.
    pub fn visual_viewport(&self) -> (f64, f64, f64) {
        match self.mobile_metrics() {
            Some(m) => (m.visual.0, m.visual.1, m.scale),
            None => (self.viewport().width as f64, self.viewport().height as f64, 1.0),
        }
    }

    /// `window.innerWidth` (Window.zig:766-768).
    pub fn inner_width(&self) -> u32 {
        self.window_size().0
    }

    /// `window.innerHeight` (Window.zig:772-774).
    pub fn inner_height(&self) -> u32 {
        self.window_size().1
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
            // pyro: not a body with layout or paint containment (nor under a root with it), whose
            // overflow does not go to the viewport (CSS Contain 2 § 3.2, § 3.4): it scrolls as an
            // element does (WPT css-contain `contain-body-overflow-002`).
            Some("body") => {
                self.html_local(dom, parent) == Some("html")
                    && self.is_root_idx(dom, parent)
                    && !self.contains_layout_or_paint(dom, idx)
                    && !self.contains_layout_or_paint(dom, parent)
            }
            _ => false,
        }
    }

    /// Whether the element at `idx` has layout or paint containment (`contain: layout`, `paint`,
    /// `content` or `strict`).
    fn contains_layout_or_paint(&self, dom: &Dom, idx: u32) -> bool {
        let contain = self.style.computed_value(dom, dom.id_of(idx), "contain").unwrap_or_default();
        contain.split_whitespace().any(|w| matches!(w, "layout" | "paint" | "content" | "strict"))
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
        // The visual bounding rect includes CSS transforms. A child browsing
        // context keeps its layout viewport regardless of its owner's scale
        // or rotation; use the untransformed border-box dimensions.
        let (width, height) = self.box_quads(dom, el).map_or((b.rect.width, b.rect.height), |q| (q.width, q.height));
        Some(((width - edges(LEFT, RIGHT)).max(0.0), (height - edges(TOP, BOTTOM)).max(0.0)))
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
        let viewport = self.viewport();
        let (width, height) = self.without_scrollbars(dom, (viewport.width, viewport.height));
        Some(match axis {
            Axis::Width => width,
            Axis::Height => height,
        })
    }

    fn without_scrollbars(&self, dom: &Dom, (width, height): (u32, u32)) -> (f64, f64) {
        let (bar_width, bar_height) = self.synced_geometry_source(dom).map_or((0.0, 0.0), |s| s.viewport_scrollbar_sizes());
        ((width as f64 - bar_width).max(0.0), (height as f64 - bar_height).max(0.0))
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
        // A `<textarea>` scrolls its text whatever its `overflow` computes to here (the UA
        // sheet's `overflow: auto` in Chrome): lines that do not wrap scroll sideways.
        let textarea = dom.element(el).is_ok_and(|e| e.is_html() && &*e.name.local == "textarea");
        if !(if axis == Axis::Width { clips.x } else { clips.y }) && !text_control && !textarea {
            return Some(0.0);
        }
        // `overflow: clip` clips without making a scroll container: the axis does not scroll.
        let clip_only = self.style.stylo_style(dom, el).is_some_and(|s| {
            use style::values::computed::Overflow;
            let b = s.get_box();
            (if axis == Axis::Width { b.overflow_x } else { b.overflow_y }) == Overflow::Clip
        });
        if clip_only && !text_control && !textarea {
            return Some(0.0);
        }
        // Chromium's maximum offset is its pixel-snapped scroll size less its
        // pixel-snapped client size (`PaintLayerScrollableArea::MaximumScrollOffset`),
        // the sizes script reads: content ending a layout unit short of a
        // pixel still scrolls to that pixel.
        let (scroll, client) = match axis {
            Axis::Width => (Metric::ScrollWidth, Metric::ClientWidth),
            Axis::Height => (Metric::ScrollHeight, Metric::ClientHeight),
        };
        Some((self.metric(dom, el, scroll) - self.metric(dom, el, client)).max(0.0))
    }

    /// `offsetWidth` / `offsetHeight` (Element.zig:1915-1927).
    /// Layout sizes, as Chromium's: a CSS transform on the element or an
    /// ancestor scales its client rect, not its `offsetWidth`.
    pub fn offset_width(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        self.untransformed_size(dom, el).0
    }

    pub fn offset_height(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        self.untransformed_size(dom, el).1
    }

    /// The border box's size before transforms (the engine's quads keep it);
    /// the laid-out rect's for boxes without quads (SVG content).
    fn untransformed_size(&self, dom: &Dom, el: NodeId) -> (f64, f64) {
        let Some(b) = self.layout_box(dom, el) else { return (0.0, 0.0) };
        match self.box_quads(dom, el) {
            Some(q) if q.transformed => (q.width, q.height),
            _ => (b.rect.width, b.rect.height),
        }
    }

    /// `offsetTop`/`offsetLeft`: border-box offset from the offset parent's
    /// padding edge (document coordinates for the body/none).
    fn box_offset(&self, dom: &Dom, el: NodeId) -> (f64, f64) {
        let Some(b) = self.layout_box(dom, el) else { return (0.0, 0.0) };
        let parent = self.offset_parent(dom, el).filter(|&p| !self.is_root_container(dom, p));
        // A sticky box is where scrolling holds it (and its descendants with it).
        let sticky = self.sticky_map(dom);
        let (kx, ky) = self.sticky_shift_in(dom, &sticky, el);
        // Transforms move neither box (CSSOM View: offsets are in layout coordinates).
        let origin = |b: &LayoutBox| b.layout_origin.map_or((b.rect.x, b.rect.y), |[x, y]| (x, y));
        let (bx, by) = origin(&b);
        match parent.and_then(|p| Some((p, self.layout_box(dom, p)?))) {
            Some((parent, p)) => {
                let (px, py) = self.sticky_shift_in(dom, &sticky, parent);
                let (ox, oy) = origin(&p);
                (bx + kx - ox - px - p.border[LEFT], by + ky - oy - py - p.border[TOP])
            }
            None => (bx + kx, by + ky),
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
    /// a fractional width). Offsets and offset dimensions round independently;
    /// client and scroll sizes snap at the padding edge.
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
            Metric::OffsetTop | Metric::OffsetLeft | Metric::OffsetWidth | Metric::OffsetHeight => round_layout_unit(raw),
            Metric::ScrollWidth | Metric::ClientWidth => {
                snap_size_to_pixel(raw, self.offset_left(dom, el) + border(LEFT))
            }
            Metric::ScrollHeight | Metric::ClientHeight => {
                snap_size_to_pixel(raw, self.offset_top(dom, el) + border(TOP))
            }
        }
    }

    /// The content box size of a rendered element (HTML `img.width`: "the rendered width"),
    /// `None` when it is not being rendered.
    pub fn rendered_content_size(&self, dom: &Dom, el: NodeId) -> Option<(f64, f64)> {
        if !self.is_visible(dom, el) {
            return None;
        }
        self.content_size(dom, el)
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
        self.sync_before_rect_query(dom);
        // (Its box first: it renders skipped `content-visibility: auto` contents it is in.)
        let _ = self.layout_box(dom, el);
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
        let result = self.text_result(dom);
        let &(r, fixed) = result.texts.get(&text)?;
        Some(self.text_client_rect(dom, text, r, fixed))
    }

    pub fn text_range_rect(&self, dom: &Dom, text: NodeId, start: u32, end: u32) -> Option<Rect> {
        let result = self.text_result(dom);
        let mut bounds = None::<Rect>;
        for run in result.text_runs.iter().filter(|run| run.node == text && run.start <= end && start <= run.end) {
            for pair in run.edges().windows(2) {
                let [(first, left), (last, right)] = [pair[0], pair[1]];
                let (lo, hi) = (first.min(last), first.max(last));
                let (left, right) = if start == end && lo <= start && start <= hi {
                    let caret = if start == first || (lo < start && start < hi && first == lo) { left } else { right };
                    (caret, caret)
                } else if lo < end && start < hi {
                    (left.min(right), left.max(right))
                } else {
                    continue;
                };
                let rect = if run.vertical_edges {
                    Rect { y: left, height: right - left, ..run.rect }
                } else {
                    Rect { x: left, width: right - left, ..run.rect }
                };
                if start == end {
                    return Some(self.text_client_rect(dom, text, rect, run.fixed));
                }
                bounds = Some(match bounds {
                    None => rect,
                    Some(prev) => {
                        let x = prev.left().min(rect.left());
                        let y = prev.top().min(rect.top());
                        Rect { x, y, width: prev.right().max(rect.right()) - x, height: prev.bottom().max(rect.bottom()) - y }
                    }
                });
            }
        }
        bounds.map(|rect| self.text_client_rect(dom, text, rect, result.texts[&text].1))
    }

    fn text_client_rect(&self, dom: &Dom, text: NodeId, r: Rect, fixed: bool) -> Rect {
        let (dx, dy) = self.ancestor_scroll_offset(dom, text);
        let (kx, ky) = self.sticky_shift_lazy(dom, text);
        let (sx, sy) = if fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
        Rect { x: r.x - sx - dx + kx, y: r.y - sy - dy + ky, width: r.width, height: r.height }
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
        let result = self.text_result_within(dom, within);
        let sticky = self.sticky_map(dom);
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
            let (kx, ky) = self.sticky_shift_in(dom, &sticky, run.node);
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
            let inline_gap = gap(x, left, run.rect.width);
            let block_gap = gap(y, top, run.rect.height);
            let key = if run.vertical_edges { (inline_gap, block_gap) } else { (block_gap, inline_gap) };
            if best.is_none_or(|b| key < b.0) {
                best = Some((key, i as u32, run.node, if run.vertical_edges { y - top } else { x - left }));
            }
        }
        // The flag: the point is on the word itself.
        best.map(|(key, i, node, distance)| (node, result.text_runs[i as usize].offset_at(distance), key == (0.0, 0.0)))
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
    /// block (CSS 2.1 10.1, CSS Transforms 1 3): for `position: fixed`, every
    /// ancestor below the nearest one that a transform, filter, containment
    /// (...) makes its containing block, if any, else the viewport; for
    /// `position: absolute`, every ancestor below the nearest positioned one
    /// or such a one. Blitz lays them out in the same containing blocks. In-
    /// flow content escapes with its out-of-flow ancestors.
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
            // The containing block of the out-of-flow box escaping: its clip
            // applies, and above it the escape is its own.
            let applies = match escape {
                ClipEscape::None => true,
                ClipEscape::Viewport => c.contains_fixed,
                ClipEscape::Positioned => c.contains_absolute,
            };
            if applies {
                let clips = ScrollAxes { x: c.overflow_x_clips, y: c.overflow_y_clips };
                if (clips.x || clips.y) && out(a, clips) {
                    return true;
                }
                escape = self.clip_escape(dom, a);
            }
            cur = dom.parent_element(a).ok().flatten();
        }
        false
    }

    /// Which overflow clips above `el` do not apply to it, from its
    /// `position` (the containing blocks are found by `for_each_clip`).
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
        self.sync_before_rect_query(dom);
        self.is_visible(dom, el).then(|| self.bounding_client_rect_for_visible(dom, el))
    }

    /// `el`'s CSS boxes as quads in client coordinates (CDP
    /// `DOM.getBoxModel`/`getContentQuads`): the engine's quads moved by the
    /// same scroll and sticky offsets as its client rect. `None` when it is
    /// not rendered, has no CSS box (SVG content) or the engine reports none.
    pub fn box_quads(&self, dom: &Dom, el: NodeId) -> Option<ExternalQuads> {
        self.sync_before_rect_query(dom);
        if !self.is_visible(dom, el) || self.svg_client_rect(dom, el).is_some() {
            return None;
        }
        let source = self.synced_geometry_source(dom)?;
        let mut q = source.element_quads(crate::snapshot::render_key(el))?;
        let (dx, dy) = self.ancestor_scroll_offset(dom, el);
        let (kx, ky) = self.sticky_shift_lazy(dom, el);
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

    /// `getClientRects`: none when not rendered; an inline box over several
    /// lines has one border box per line (the engine's fragments), any other
    /// element one rect.
    pub fn client_rects(&self, dom: &Dom, el: NodeId) -> Vec<Rect> {
        let Some(rect) = self.client_rect(dom, el) else { return Vec::new() };
        match self.box_quads(dom, el) {
            Some(q) if q.fragments.len() > 1 => q
                .fragments
                .iter()
                .map(|f| {
                    let (xs, ys) = ([f[0], f[2], f[4], f[6]], [f[1], f[3], f[5], f[7]]);
                    let (x0, y0) = (xs.into_iter().fold(f64::MAX, f64::min), ys.into_iter().fold(f64::MAX, f64::min));
                    let (x1, y1) = (xs.into_iter().fold(f64::MIN, f64::max), ys.into_iter().fold(f64::MIN, f64::max));
                    Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }
                })
                .collect(),
            _ => vec![rect],
        }
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
                let (kx, ky) = self.scroller_scale(dom, dom.id_of(parent), (x, y));
                dx += x as f64 * kx;
                dy += y as f64 * ky;
            }
            cur = parent;
        }
    }

    /// How much one CSS px of `scroller`'s scroll offset moves its content
    /// in client px, per axis: 1 unless a transform on the scroller or an
    /// ancestor scales it (a scroller inside a `scale(0.5)` stage moves its
    /// content half as far as its `scrollTop`, as in Chromium).
    fn scroller_scale(&self, dom: &Dom, scroller: NodeId, offset: (i32, i32)) -> (f64, f64) {
        if offset == (0, 0) {
            return (1.0, 1.0);
        }
        let Some(source) = self.synced_geometry_source(dom) else { return (1.0, 1.0) };
        let Some(q) = source.element_quads(crate::snapshot::render_key(scroller)) else { return (1.0, 1.0) };
        if !q.transformed {
            return (1.0, 1.0);
        }
        let b = q.border;
        let span = |i: usize| {
            let v = [b[i], b[i + 2], b[i + 4], b[i + 6]];
            v.iter().copied().fold(f64::MIN, f64::max) - v.iter().copied().fold(f64::MAX, f64::min)
        };
        let ratio = |client: f64, local: f64| if client > 0.0 && local > 0.0 { client / local } else { 1.0 };
        (ratio(span(0), q.width), ratio(span(1), q.height))
    }

    fn bounding_client_rect_for_visible(&self, dom: &Dom, el: NodeId) -> Rect {
        let (dx, dy) = self.ancestor_scroll_offset(dom, el);
        // SVG content (anything in an `<svg>` but a nested `<svg>`'s own box
        // aside) has no CSS box: its geometry is its bbox.
        if let Some(r) = self.svg_client_rect(dom, el) {
            return r;
        }
        let Some(b) = self.layout_box(dom, el) else { return Rect::default() };
        let (kx, ky) = self.sticky_shift_lazy(dom, el);
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
            let (ax, ay) = self.anchor_shift(dom, el);
            let (sx, sy) =
                if b.fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
            Rect { x: b.rect.x - sx - dx + kx + ax, y: b.rect.y - sy - dy + ky + ay, width: b.rect.width, height: b.rect.height }
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
                    let (kx, ky) = self.scroller_scale(dom, dom.id_of(parent), (x, y));
                    offset.0 += x as f64 * kx;
                    offset.1 += y as f64 * ky;
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
    /// each sticky box moves with its descendants. The boxes are `result`'s.
    fn sticky_shift(&self, dom: &Dom, result: &LayoutResult, node: NodeId) -> (f64, f64) {
        if result.sticky.is_empty() {
            return (0.0, 0.0);
        }
        self.sticky_shift_with(dom, node, &|n| result.sticky.get(&n).copied(), &|n| result.get(n).copied())
    }

    /// [`Layout::sticky_shift`] with the sticky elements `sticky`
    /// ([`Layout::sticky_map`]) and the engine's boxes as they are.
    /// [`Self::sticky_shift_in`] for one rect query: when the whole sticky
    /// map is not built for this sync, only the sticky entries of `node`'s
    /// ancestors (and their containing blocks) are looked up, and kept until
    /// the next sync. A text probe written and read in a loop on a page with
    /// many sticky table heads no longer gets every sticky box after each
    /// write.
    fn sticky_shift_lazy(&self, dom: &Dom, node: NodeId) -> (f64, f64) {
        let (x,y) = self.anchor_shift(dom,node);
        let (sx,sy) = self.sticky_only_shift_lazy(dom,node);
        (x+sx,y+sy)
    }

    fn anchor_shift(&self, dom: &Dom, node: NodeId) -> (f64, f64) {
        if self.element_scroll.is_empty() && self.window_scroll == (0,0) { return (0.0,0.0); }
        self.external.as_ref().map_or((0.0,0.0),|source| source.anchor_scroll_shift(crate::snapshot::render_key(node), &|key| {
            let value = match key { Some(key) => node_of(dom,key).and_then(|el| self.element_scroll.get(&el).copied()).unwrap_or_default(), None => self.window_scroll };
            (f64::from(value.0),f64::from(value.1))
        }, &|key| {
            let Some(el) = node_of(dom,key) else { return (0.,0.) };
            let epoch = (self.syncs.get(), self.scroll_generation);
            let mut cache = self.anchor_sticky_deltas.borrow_mut();
            if cache.0 != epoch { cache.1.clear(); cache.0 = epoch; }
            self.sticky_delta(dom, el, &|n| source.sticky_element(crate::snapshot::render_key(n)),
                &|n| source.element_box(crate::snapshot::render_key(n)).map(|b| LayoutBox::from_external(&b,0)), &mut cache.1)
        }))
    }

    fn sticky_only_shift_lazy(&self, dom: &Dom, node: NodeId) -> (f64, f64) {
        let styled_without_sticky = self.external.as_ref().filter(|s| s.active()).is_some_and(|source| {
            self.style.engine_styled(dom).is_some() && !source.has_sticky()
        });
        if styled_without_sticky {
            return (0.0, 0.0);
        }
        let Some(source) = self.synced_geometry_source(dom).filter(|s| s.has_sticky()) else { return (0.0, 0.0) };
        let key = self.syncs.get();
        if let Some((k, m)) = self.sticky.borrow().as_ref() {
            if *k == key {
                let m = m.clone();
                return self.sticky_only_shift_in(dom, &m, node);
            }
        }
        {
            let mut lazy = self.sticky_lazy.borrow_mut();
            if lazy.as_ref().is_none_or(|(k, _)| *k != key) {
                *lazy = Some((key, FxHashMap::default()));
            }
        }
        let sticky_of = |n: NodeId| -> Option<ExternalSticky> {
            if let Ok(lazy) = self.sticky_lazy.try_borrow() {
                if let Some(hit) = lazy.as_ref().and_then(|(_, cache)| cache.get(&n)) {
                    return *hit;
                }
            }
            let found = source.sticky_element(crate::snapshot::render_key(n));
            if let Ok(mut lazy) = self.sticky_lazy.try_borrow_mut() {
                if let Some((_, cache)) = lazy.as_mut() {
                    cache.insert(n, found);
                }
            }
            found
        };
        self.sticky_shift_with(dom, node, &sticky_of, &|n| self.layout_box(dom, n))
    }

    fn sticky_shift_in(&self, dom: &Dom, sticky: &FxHashMap<NodeId, ExternalSticky>, node: NodeId) -> (f64, f64) {
        let (x,y) = self.anchor_shift(dom,node);
        let (sx,sy) = self.sticky_only_shift_in(dom,sticky,node);
        (x+sx,y+sy)
    }

    fn sticky_only_shift_in(&self, dom: &Dom, sticky: &FxHashMap<NodeId, ExternalSticky>, node: NodeId) -> (f64, f64) {
        if sticky.is_empty() {
            return (0.0, 0.0);
        }
        self.sticky_shift_with(dom, node, &|n| sticky.get(&n).copied(), &|n| self.layout_box(dom, n))
    }

    fn sticky_shift_with(
        &self,
        dom: &Dom,
        node: NodeId,
        sticky_of: &dyn Fn(NodeId) -> Option<ExternalSticky>,
        box_of: &dyn Fn(NodeId) -> Option<LayoutBox>,
    ) -> (f64, f64) {
        let (mut x, mut y) = (0.0, 0.0);
        let mut memo = FxHashMap::default();
        let mut cur = Some(node);
        while let Some(n) = cur {
            if sticky_of(n).is_some() {
                let (dx, dy) = self.sticky_delta(dom, n, sticky_of, box_of, &mut memo);
                x += dx;
                y += dy;
            }
            cur = dom.parent_element(n).ok().flatten();
        }
        (x, y)
    }

    /// The offset `el`'s own stickiness adds to its sticky ancestors' (the box moves by
    /// all of them): computed with its box moved by theirs, and its containing block by
    /// the ones that hold that block, as Blink's sticky constraints do (a sticky cell in
    /// a sticky row is where the row holds it before it holds itself; WPT
    /// `position-sticky-table-parts`). `memo` keeps the ones computed.
    fn sticky_delta(
        &self,
        dom: &Dom,
        el: NodeId,
        sticky_of: &dyn Fn(NodeId) -> Option<ExternalSticky>,
        box_of: &dyn Fn(NodeId) -> Option<LayoutBox>,
        memo: &mut FxHashMap<NodeId, (f64, f64)>,
    ) -> (f64, f64) {
        if let Some(&delta) = memo.get(&el) {
            return delta;
        }
        let Some(sticky) = sticky_of(el) else { return (0.0, 0.0) };
        let cb = sticky.cb.and_then(|key| node_of(dom, key));
        let (mut box_shift, mut cb_shift) = ((0.0, 0.0), (0.0, 0.0));
        let mut holds_cb = cb.is_none();
        let mut cur = dom.parent_element(el).ok().flatten();
        while let Some(n) = cur {
            holds_cb |= Some(n) == cb;
            if sticky_of(n).is_some() {
                let (dx, dy) = self.sticky_delta(dom, n, sticky_of, box_of, memo);
                box_shift.0 += dx;
                box_shift.1 += dy;
                if holds_cb {
                    cb_shift.0 += dx;
                    cb_shift.1 += dy;
                }
            }
            cur = dom.parent_element(n).ok().flatten();
        }
        let delta = self.sticky_offset(dom, el, &sticky, box_of, box_shift, cb_shift);
        memo.insert(el, delta);
        delta
    }

    /// CSS Positioned Layout 3 sticky positioning for one box laid out in
    /// place: the offset that keeps its border box within its nearest scroll
    /// container's scrollport (the viewport when none) inset by its
    /// `top`/`bottom`/`left`/`right`, without leaving its containing block
    /// (the table for table parts). `top` wins over `bottom`, `left` over
    /// `right`. Inside `position: fixed` the window scroll moves nothing, so
    /// with no scroll container in between the scrollport is the viewport at
    /// the origin.
    fn sticky_offset(
        &self,
        dom: &Dom,
        el: NodeId,
        sticky: &ExternalSticky,
        box_of: &dyn Fn(NodeId) -> Option<LayoutBox>,
        box_shift: (f64, f64),
        cb_shift: (f64, f64),
    ) -> (f64, f64) {
        let Some(b) = box_of(el) else { return (0.0, 0.0) };
        // Scrollport in document coordinates of the scrolled content.
        //
        // The scroll container is the nearest one on the box's containing
        // block chain: an ancestor that is not the containing block of an
        // out-of-flow box on the way up does not hold it (a `position:
        // absolute` box's needs a positioned one, a `position: fixed` box's
        // one that contains fixed boxes), and `overflow: clip` makes none.
        let mut port = None;
        let mut a = dom.parent_element(el).ok().flatten();
        // The out-of-flow position of the box whose containing block is looked for.
        let mut escaping = None;
        while let Some(p) = a {
            if self.is_root_container(dom, p) {
                break;
            }
            let Some(c) = self.style.computed(dom, p) else {
                a = dom.parent_element(p).ok().flatten();
                continue;
            };
            let holds = match escaping {
                None => true,
                Some(Position::Fixed) => c.contains_fixed,
                Some(_) => c.contains_absolute,
            };
            if !holds {
                a = dom.parent_element(p).ok().flatten();
                continue;
            }
            escaping = matches!(c.position, Position::Absolute | Position::Fixed).then_some(c.position);
            if c.scroll_container {
                if let Some(pb) = box_of(p) {
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
        let fixed = b.fixed || escaping == Some(Position::Fixed);
        let (wx, wy) = if fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
        let port =
            port.unwrap_or(Rect { x: wx, y: wy, width: self.inner_width() as f64, height: self.inner_height() as f64 });
        let limit =
            sticky.limit.map(|(x, y, width, height)| Rect { x: x + cb_shift.0, y: y + cb_shift.1, width, height });
        let inset = |side: usize, basis: f64| sticky.insets[side].map(|(px, fraction)| px + fraction * basis);
        let r = Rect { x: b.rect.x + box_shift.0, y: b.rect.y + box_shift.1, ..b.rect };
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
        let (vw, vh) = self.without_scrollbars(dom, self.window_size());
        (w.max(vw), h.max(vh))
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
        // pyro: an in-flow box in the inline-content phase (`PaintChain::flow_phase`) paints over
        // the block backgrounds of its layer, after the boxes of the earlier phase.
        key.push((if own { 0 } else { 2 }, if own { 0 } else { chain.flow_phase }, if own { 0 } else { result.get(el).map_or(0, |b| b.order) }));
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
            // A shadow tree's top-level elements paint within their host.
            cur =
                dom.check(node).ok().and_then(|i| crate::style::flat_tree_parent_element(dom, i)).map(|i| dom.id_of(i));
        };
        let reached_root = cur.is_none();
        for (i, &node) in pending.iter().enumerate().rev() {
            // The chain's first element (no parent element) always starts a context.
            let is_root = reached_root && i == pending.len() - 1;
            if let Some(bounds) = result.get(node) {
                // pyro: the items of a container whose items paint with the inline content.
                let in_items_phase = dom
                    .check(node)
                    .ok()
                    .and_then(|i| crate::style::flat_tree_parent_element(dom, i))
                    .map(|i| dom.id_of(i))
                    .and_then(|parent| result.get(parent))
                    .is_some_and(|parent| parent.items_in_phases);
                state = self.paint_step(dom, node, bounds.order, is_root, state);
                if in_items_phase && !state.own_layer {
                    state.flow_phase = 1;
                }
            }
            result.hit.borrow_mut().chains.insert(node, state.clone());
        }
        state
    }

    /// [`Self::paint_chain`]'s step for one box.
    fn paint_step(&self, dom: &Dom, node: NodeId, order: u32, is_root: bool, mut state: PaintChain) -> PaintChain {
        // A top-layer element (modal dialog, showing popover) paints above
        // the whole document in its own root context, by its rank in the
        // top layer, whatever its ancestors' stacking contexts. One empty
        // check when the top layer is empty.
        if let Some(rank) = dom.top_layer_rank(node) {
            state.contexts = Rc::from([(5, rank as i32, 0)].as_slice());
            state.positioned_layer = None;
            state.own_layer = true;
            return state;
        }
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
            state.flow_phase = 0;
        } else if positioned {
            // z-auto positioned boxes group their normal content, but their
            // positioned descendants belong to the enclosing real context.
            state.positioned_layer = Some((3, 0, order));
            state.own_layer = true;
            state.flow_phase = 0;
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
                if let ClipPath::Shape(shape, reference) = style.slow_clone_clip_path() {
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

    /// The element under viewport point (`x`, `y`): the topmost in paint
    /// order of the hit-testable boxes holding the point, as the engine
    /// reports its candidates ([`GeometrySource::hit_candidates`]: a
    /// descent of its tree reaching the boxes that may hold the point and
    /// their ancestors) or, without that, among every box of the document.
    fn element_from_point_impl(&self, dom: &Dom, x: f64, y: f64, ignore_x: bool) -> Option<NodeId> {
        if ignore_x {
            // A whole row of the page: nothing a point index narrows.
            return self.hit_test_descent(dom, x, y, true);
        }
        // pyro: classic scrollbars: a point in the viewport's scrollbar gutters (outside its
        // scrollport) hits the root element (Chrome).
        if let Some([left, top, right, bottom]) = self.synced_geometry_source(dom).and_then(|s| s.viewport_gutters()) {
            let (vw, vh) = (self.viewport().width as f64, self.viewport().height as f64);
            let in_viewport = x >= 0.0 && y >= 0.0 && x < vw && y < vh;
            if in_viewport && (x < left || y < top || x >= vw - right || y >= vh - bottom) {
                return dom.document_element(self.document()).ok().flatten();
            }
        }
        if let Some(index) = self.hit_index_for(dom, false) {
            return self.hit_test_indexed(dom, &index, x, y);
        }
        let started = std::time::Instant::now();
        let hit = self.hit_test_descent(dom, x, y, false);
        let took = started.elapsed();
        let mut state = self.hit_index.borrow_mut();
        state.rented += took;
        state.rented_since_build += took;
        state.descent_cost.record(took);
        state.bought_unseen = 0;
        hit
    }

    /// The index of the current layout and scroll position, when there is
    /// one or it is time to build one (`force`: now).
    ///
    /// An index answers any point but reads every box of the document
    /// once, and lives until the next change of the layout or of a scroll
    /// offset; a descent of the engine's tree answers one point and reads
    /// the boxes on its way, with a fixed cost of its own (its candidates'
    /// boxes, paint order and clips are worked out per test). Which is
    /// cheaper depends on the size of the document and on how many tests
    /// come before the next change, which is not known. So, by what each
    /// last took on this document:
    ///
    /// - an index that builds in no more time than one descent takes is
    ///   built at the first test of every layout (a small document: a
    ///   click handler writing to the DOM between clicks costs a build per
    ///   click, less than the descents it replaces);
    /// - otherwise the descents answer until those of this layout have
    ///   cost as much as an index, and then it is built: never more than
    ///   twice the best choice, whatever the page does. A large page that
    ///   changes between pointer events keeps descending; one clicked or
    ///   moved over as it stands is indexed after a few tests.
    ///
    /// The first index of a document is built at the second test of a
    /// layout (its price is not known before). The answer is the same by
    /// either path: only the cost depends on the clock.
    ///
    /// Each choice stops the other path being timed, so a price that was
    /// wrong or has changed would stand for the life of the document. Both
    /// are therefore taken again at a bounded cost: a descent answers the
    /// first test of every [`Self::HIT_DESCENT_EVERY`]th layout in a row
    /// that would be indexed at once, and an index is built when the
    /// descents since the last one have cost [`Self::HIT_BUILD_AFTER`]
    /// times its price (see also [`CostEstimate`]).
    const HIT_DESCENT_EVERY: u32 = 32;
    const HIT_BUILD_AFTER: u32 = 16;

    fn hit_index_for(&self, dom: &Dom, force: bool) -> Option<Rc<HitIndex>> {
        self.synced_geometry_source(dom)?;
        let epoch = (self.syncs.get(), self.scroll_generation);
        {
            let mut state = self.hit_index.borrow_mut();
            if state.epoch != Some(epoch) {
                state.epoch = Some(epoch);
                state.index = None;
                state.tests = 0;
                state.rented = std::time::Duration::ZERO;
            }
            if let Some(index) = &state.index {
                return Some(index.clone());
            }
            state.tests += 1;
            let buy = match state.build_cost.get() {
                None => state.tests >= 2,
                Some(build) if state.tests == 1 && state.descent_cost.get().is_some_and(|descent| build <= descent) => {
                    state.bought_unseen += 1;
                    state.bought_unseen < Self::HIT_DESCENT_EVERY
                }
                Some(build) => state.rented >= build || state.rented_since_build >= build * Self::HIT_BUILD_AFTER,
            };
            if !force && !buy {
                return None;
            }
        }
        let started = std::time::Instant::now();
        let index = Rc::new(self.build_hit_index(dom));
        let mut state = self.hit_index.borrow_mut();
        state.build_cost.record(started.elapsed());
        state.rented_since_build = std::time::Duration::ZERO;
        state.builds += 1;
        state.index = Some(index.clone());
        Some(index)
    }

    /// Reads every box of the document into a [`HitIndex`].
    fn build_hit_index(&self, dom: &Dom) -> HitIndex {
        let result = self.layout_result(dom);
        // As `hit_test_scan` sets up, for the life of the index.
        let mut geometry = HitTestGeometry {
            has_element_scroll: self.element_scroll.values().any(|&(x, y)| x != 0 || y != 0),
            has_sticky: self.external.as_ref().is_some_and(|source| source.has_sticky()),
            has_fragments: self.external.as_ref().is_some_and(|source| source.has_fragmented_boxes()),
            ..Default::default()
        };
        let mut order: Vec<(u32, NodeId)> = result
            .boxes
            .iter()
            .filter(|(_, b)| !(b.rect.width <= 0.0 && b.rect.height <= 0.0))
            .map(|(&el, b)| (b.order, el))
            .collect();
        order.sort_unstable_by_key(|&(order, _)| order);
        let mut els = Vec::with_capacity(order.len());
        let mut rects = Vec::with_capacity(order.len());
        for (_, el) in order {
            let r = self.hit_test_client_rect(dom, &result, el, &mut geometry);
            // A rect no point is inside of (see `HitIndex::holds`).
            if r.x.is_nan() || r.y.is_nan() || r.width.is_nan() || r.height.is_nan() || r.width < 0.0 || r.height < 0.0
            {
                continue;
            }
            els.push(el);
            rects.push(r);
        }
        let grid = HitGrid::new(&rects);
        HitIndex { result, geometry: RefCell::new(geometry), els, rects, grid }
    }

    /// [`Layout::element_from_point_impl`] from `index`: the boxes of the
    /// point's cell holding the point, in tree order, are the candidates
    /// of the same selection as the scan's and the descent's.
    fn hit_test_indexed(&self, dom: &Dom, index: &HitIndex, x: f64, y: f64) -> Option<NodeId> {
        let candidates = index
            .grid
            .cell(x, y)
            .iter()
            .filter(|&&i| HitIndex::holds(&index.rects[i as usize], x, y))
            .map(|&i| index.els[i as usize]);
        self.hit_select(dom, &index.result, candidates, x, y, false, &mut index.geometry.borrow_mut())
    }

    /// `document.elementFromPoint(x, y)` answered from the index, built
    /// now if this layout has none yet (tests).
    #[doc(hidden)]
    pub fn element_from_point_indexed(&self, dom: &Dom, x: f64, y: f64) -> Option<NodeId> {
        let index = self.hit_index_for(dom, true)?;
        self.hit_test_indexed(dom, &index, x, y)
    }

    /// `document.elementFromPoint(x, y)` answered by a descent of the
    /// engine's tree, whether or not there is an index (tests).
    #[doc(hidden)]
    pub fn element_from_point_descent(&self, dom: &Dom, x: f64, y: f64) -> Option<NodeId> {
        self.hit_test_descent(dom, x, y, false)
    }

    /// How many hit-test indexes were built so far, and whether the current
    /// layout has one (tests).
    #[doc(hidden)]
    pub fn hit_index_stats(&self, dom: &Dom) -> (u64, bool) {
        self.synced_geometry_source(dom);
        let state = self.hit_index.borrow();
        let current = state.epoch == Some((self.syncs.get(), self.scroll_generation)) && state.index.is_some();
        (state.builds, current)
    }

    /// The hit test by the engine's candidates (see
    /// [`Layout::element_from_point_impl`]).
    fn hit_test_descent(&self, dom: &Dom, x: f64, y: f64, ignore_x: bool) -> Option<NodeId> {
        let source = self.synced_geometry_source(dom)?.clone();
        let has_element_scroll = self.element_scroll.values().any(|&(x, y)| x != 0 || y != 0);
        let sticky = self.sticky_map(dom);
        let has_sticky = !sticky.is_empty();
        let has_fragments = source.has_fragmented_boxes();
        let geometry =
            RefCell::new(HitTestGeometry { has_element_scroll, has_sticky, has_fragments, ..Default::default() });
        let probe =
            HitProbe { x, y, ignore_x, window_scroll: (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
        // How far this document moves an element's box from where the
        // engine laid it out: its scrolled ancestors' offsets, less its
        // sticky ancestors' (the same as `hit_test_client_rect` applies).
        let offset = |key: u64| -> (f64, f64) {
            let Some(el) = node_of(dom, key) else { return (0.0, 0.0) };
            let (dx, dy) = match has_element_scroll {
                true => self.hit_test_scroll_offset(dom, el, &mut geometry.borrow_mut().scroll_offsets),
                false => (0.0, 0.0),
            };
            let (kx, ky) = self.sticky_shift_in(dom, &sticky, el);
            (dx - kx, dy - ky)
        };
        let mut shifted: Vec<u64> = Vec::new();
        if has_element_scroll {
            shifted.extend(self.scrolled_elements().into_iter().map(|(key, _)| key));
        }
        shifted.extend(sticky.keys().map(|&el| crate::snapshot::render_key(el)));
        let Some(visits) = source.hit_candidates(&probe, &offset, &shifted) else {
            return self.hit_test_scan(dom, x, y, ignore_x);
        };
        let (document_width, document_height) = source.document_size().unwrap_or((0.0, 0.0));
        let mut result =
            LayoutResult { document_width, document_height, sticky: (*sticky).clone(), ..LayoutResult::default() };
        // Tree order: an element's ancestors are in before it.
        for v in &visits {
            if let Some(el) = node_of(dom, v.key) {
                result.insert_box(dom, el, LayoutBox::from_external(&v.bounds, v.order));
            }
        }
        // The topmost modal dialog's box, for the fallback below.
        if let Some(m) = dom.topmost_modal() {
            if !result.boxes.contains_key(&m) {
                if let Some(b) = self.layout_box(dom, m) {
                    result.insert_box(dom, m, b);
                }
            }
        }
        let candidates: Vec<NodeId> = visits.iter().filter(|v| v.hit).filter_map(|v| node_of(dom, v.key)).collect();
        let mut geometry = geometry.into_inner();
        self.hit_select(dom, &result, candidates.into_iter(), x, y, ignore_x, &mut geometry)
    }

    /// [`Layout::element_from_point_impl`] over every box of the document
    /// (the whole document's geometry is read).
    fn hit_test_scan(&self, dom: &Dom, x: f64, y: f64, ignore_x: bool) -> Option<NodeId> {
        let result = self.layout_result(dom);
        let mut geometry = HitTestGeometry {
            has_element_scroll: self.element_scroll.values().any(|&(x, y)| x != 0 || y != 0),
            has_sticky: self.external.as_ref().is_some_and(|source| source.has_sticky()),
            has_fragments: self.external.as_ref().is_some_and(|source| source.has_fragmented_boxes()),
            ..Default::default()
        };
        let mut all: Vec<NodeId> = result.boxes.keys().copied().collect();
        all.sort_by_key(|el| result.boxes[el].order);
        self.hit_select(dom, &result, all.into_iter(), x, y, ignore_x, &mut geometry)
    }

    /// `document.elementFromPoint(x, y)` answered from every box of the
    /// document instead of the engine's candidates: the reference the
    /// descent is checked against (tests).
    #[doc(hidden)]
    pub fn element_from_point_scan(&self, dom: &Dom, x: f64, y: f64) -> Option<NodeId> {
        self.hit_test_scan(dom, x, y, false)
    }

    /// [`Layout::element_from_vertical_point`] answered from every box of
    /// the document (tests).
    #[doc(hidden)]
    pub fn element_from_vertical_point_scan(&self, dom: &Dom, y: f64) -> Option<NodeId> {
        self.hit_test_scan(dom, 0.0, y, true)
    }

    /// The topmost hit-testable box among `candidates` (in tree order)
    /// holding the point, in paint order; else the topmost modal dialog
    /// (everything outside it is inert: the point hits its `::backdrop`).
    #[allow(clippy::too_many_arguments)]
    fn hit_select(
        &self,
        dom: &Dom,
        result: &LayoutResult,
        candidates: impl Iterator<Item = NodeId>,
        x: f64,
        y: f64,
        ignore_x: bool,
        geometry: &mut HitTestGeometry,
    ) -> Option<NodeId> {
        let inside = |r: &Rect| (ignore_x || (x >= r.x && x < r.x + r.width)) && y >= r.y && y < r.y + r.height;
        let mut best: Option<(Rc<[PaintEntry]>, NodeId)> = None;
        for el in candidates {
            let Some(b) = result.get(el) else { continue };
            if b.rect.width <= 0.0 && b.rect.height <= 0.0 {
                continue;
            }
            let r = self.hit_test_client_rect(dom, result, el, geometry);
            // (Nor is a box a line clamp discarded: `ExternalBox::hidden`.)
            if b.hidden || !inside(&r) || !self.is_visible(dom, el) {
                continue;
            }
            let svg_viewport = !ignore_x && dom.element(el).is_ok_and(|e| {
                e.name.ns == markup5ever::ns!(svg) && e.name.local == local_name!("svg")
            });
            let self_hit = self.style.is_hit_testable(dom, el);
            if !self_hit && (!svg_viewport || self.computed(dom, el).inert) {
                continue;
            }
            // A box split into fragments (columns) is hit in one of them: its rect is their union.
            if geometry.has_fragments {
                let fragments = geometry
                    .fragments
                    .entry(el)
                    .or_insert_with(|| {
                        let source = self.external.as_ref()?;
                        let pieces = source.element_fragments(crate::snapshot::render_key(el))?;
                        // Moved as the union is (scroll and sticky offsets, the window scroll).
                        let (dx, dy) = (r.x - b.rect.x, r.y - b.rect.y);
                        Some(
                            pieces
                                .iter()
                                .map(|&[x, y, w, h]| Rect { x: x + dx, y: y + dy, width: w, height: h })
                                .collect(),
                        )
                    })
                    .clone();
                if fragments.is_some_and(|f| !f.iter().any(|f| inside(f))) {
                    continue;
                }
            }
            if self.is_table_row_box(dom, el) {
                continue;
            }
            if self.outside_inset_clip_path(dom, el, x, y, ignore_x, |node| {
                self.hit_test_client_rect(dom, result, node, geometry)
            }) {
                continue;
            }
            let key = self.paint_key(dom, result, el);
            if let Some((top_key, top_node)) = &best {
                let same_layer = key.len() == top_key.len()
                    && key[..key.len() - 1] == top_key[..top_key.len() - 1]
                    && key.last().map(|value| (value.0, value.1)) == top_key.last().map(|value| (value.0, value.1));
                // Shadow-including: a host's shadow content paints over it.
                if same_layer && dom.shadow_including_contains(el, *top_node) {
                    continue;
                }
                if *top_key > key && !(same_layer && dom.shadow_including_contains(*top_node, el)) {
                    continue;
                }
            }
            let clipped = self.hit_test_clips(dom, result, el).iter().any(|&(a, clips)| {
                if result.get(a).is_none() {
                    return false;
                }
                let ar = self.hit_test_client_rect(dom, result, a, geometry);
                let out_x = clips.x && !ignore_x && (x < ar.x || x > ar.x + ar.width);
                let out_y = clips.y && (y < ar.y || y > ar.y + ar.height);
                out_x || out_y
            });
            if !clipped {
                let target = if svg_viewport {
                    // SVG graphics are painted inside the viewport's CSS box and
                    // have no CSS boxes of their own. Hit them in local user space.
                    let left = b.border[LEFT] + b.padding[LEFT];
                    let top = b.border[TOP] + b.padding[TOP];
                    let viewport = (
                        b.client_width - b.padding[LEFT] - b.padding[RIGHT],
                        b.client_height - b.padding[TOP] - b.padding[BOTTOM],
                    );
                    crate::svg_geometry::hit_test(dom, &self.style, el, viewport, x - r.x - left, y - r.y - top)
                } else {
                    None
                };
                if let Some(target) = target.or_else(|| self_hit.then_some(el)) {
                    best = Some((key, target));
                }
            }
        }
        // Outside the topmost modal dialog everything is inert: the point
        // hits the dialog's ::backdrop, whose events go to the dialog.
        best.map(|(_, node)| node).or_else(|| {
            dom.topmost_modal().filter(|&m| {
                dom.node_document(m).ok() == Some(self.document()) && result.get(m).is_some() && self.is_visible(dom, m)
            })
        })
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
            // Flat-tree ancestors: a slottable's slot (boxless, so not
            // listed), a shadow root's host.
            let parent = match dom.assigned_slot_any(el) {
                Some(slot) => Some(slot),
                None => match dom.parent(el).ok().flatten() {
                    Some(p) if dom.is_shadow_root(p).unwrap_or(false) => dom.fragment_host(p).ok().flatten(),
                    _ => dom.parent_element(el).ok().flatten(),
                },
            };
            let slot = dom.element(el).is_ok_and(|e| e.is_html() && &*e.name.local == "slot");
            if parent.is_none() || (self.style.is_hit_testable(dom, el) && !slot) {
                out.push(el);
            }
            cur = parent;
        }
        out
    }

    // ----- window scroll -----------------------------------------------------

    /// `window.scrollX` / `scrollY` (Window.zig:758-764).
    pub fn scroll_x(&self) -> i32 {
        self.window_scroll.0
    }

    pub fn scroll_y(&self) -> i32 {
        self.window_scroll.1
    }

    /// pyro: the `content-visibility: auto` state the layout engine determined (see
    /// `GeometrySource::content_visibility_state`).
    pub fn content_visibility_state(&self, dom: &Dom) -> Vec<(u64, bool, Option<(f32, f32)>)> {
        self.synced_geometry_source(dom).map(|s| s.content_visibility_state()).unwrap_or_default()
    }

    /// pyro: the contents `content-visibility: auto` skips around `el` are rendered for a query
    /// that needs its layout (`scrollIntoView`): the renderer lays them out (CSS Contain 2 § 4.1).
    fn force_content_visibility(&self, dom: &Dom, el: NodeId) {
        if let Some(source) = self.synced_geometry_source(dom) {
            source.force_content_visibility(crate::snapshot::render_key(el));
        }
    }

    /// pyro: whether the viewport's horizontal scroll origin is at its right edge: the root
    /// element's writing mode is `vertical-rl`/`sideways-rl`, or horizontal with `direction: rtl`
    /// (its scroll offsets then run from 0 to negative values).
    fn viewport_x_reversed(&self, dom: &Dom) -> bool {
        use style::properties::longhands::writing_mode::computed_value::T as WritingMode;
        // The layout engine's answer when its document is in sync (blitz-dom
        // `viewport_scrolls_leftward`, which its scroll timelines use too), never laying out for it
        // (a scroll to the origin lays nothing out); else the same rule from the styles here.
        let in_sync = self
            .external
            .as_ref()
            .filter(|s| s.active() && !s.stale() && self.external_synced.get() == Some(self.sync_key(dom)));
        if let Some(leftward) = in_sync.and_then(|s| s.viewport_scrolls_leftward()) {
            return leftward;
        }
        let Some(root) = dom.document_element(self.document()).ok().flatten() else { return false };
        // The principal writing mode: an HTML root's `<body>` child's, else its own (CSS Writing
        // Modes 3 § 8; WPT css-writing-modes `wm-propagation-body-scroll-offset-*`).
        let body = dom
            .children(root)
            .ok()
            .and_then(|children| {
                children.into_iter().find(|&child| dom.element(child).is_ok_and(|e| &*e.name.local == "body"))
            });
        let principal = body.filter(|_| dom.element(root).is_ok_and(|e| &*e.name.local == "html")).unwrap_or(root);
        self.style.stylo_style(dom, principal).is_some_and(|s| match s.slow_clone_writing_mode() {
            WritingMode::VerticalRl | WritingMode::SidewaysRl => true,
            WritingMode::HorizontalTb => s.slow_clone_direction() == style::computed_values::direction::T::Rtl,
            _ => false,
        })
    }

    /// pyro: the viewport's scroll range on each axis ([`Self::viewport_x_reversed`]).
    fn viewport_scroll_range(&self, dom: &Dom) -> ((i32, i32), (i32, i32)) {
        let (w, h) = self.document_extent(dom);
        let (vw, vh) = self.without_scrollbars(dom, self.window_size());
        // (Saturated: an extent past i32::MAX px stays a valid range, min <= max.)
        let limit = |size: f64, visible: f64| (round_layout_unit(size) - round_layout_unit(visible)).clamp(0.0, i32::MAX as f64) as i32;
        let (mx, my) = (limit(w, vw), limit(h, vh));
        let x = if self.viewport_x_reversed(dom) { (-mx, 0) } else { (0, mx) };
        (x, (0, my))
    }

    /// `window.scrollTo` (Window.zig:780-794): each given axis is clamped to
    /// [0, scrollSize - viewport]. Returns whether the offset moved (the
    /// caller then schedules `scroll`/`scrollend` on the document).
    ///
    /// Only a positive coordinate needs the limit: zero or less lands at the
    /// origin whatever the document's size, so `scrollTo(0, 0)` (a router's
    /// scroll restoration on every navigation) lays nothing out.
    pub fn window_scroll_to(&mut self, dom: &Dom, left: Option<i32>, top: Option<i32>) -> bool {
        // A scroll without a target (CSS Overflow 5 § 2.1).
        self.scroll_targets.remove(&None);
        // pyro: the viewport's snap positions (CSS Scroll Snap 1 § 6.2).
        let current = self.window_scroll;
        let (left, top) = {
            let (l, t) = self.snap_scroll(dom, None, current, left.map(i64::from), top.map(i64::from));
            (l.map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32), t.map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32))
        };
        // (Only an offset past the origin, in the direction the viewport scrolls, needs the range,
        // which needs a layout: one at or before the origin lands there, so `scrollTo(0, 0)` lays
        // nothing out.)
        let leftward = left.is_some_and(|l| l != 0) && self.viewport_x_reversed(dom);
        let past = |l: i32| if leftward { l < 0 } else { l > 0 };
        let (rx, ry) = if left.is_some_and(past) || top.is_some_and(|t| t > 0) {
            self.viewport_scroll_range(dom)
        } else {
            ((0, 0), (0, 0))
        };
        let new_x = left.map_or(self.window_scroll.0, |l| l.clamp(rx.0, rx.1));
        let new_y = top.map_or(self.window_scroll.1, |t| t.clamp(ry.0, ry.1));
        if (new_x, new_y) == self.window_scroll {
            return false;
        }
        self.window_scroll = (new_x, new_y);
        self.scroll_generation = self.scroll_generation.wrapping_add(1);
        true
    }

    /// `window.scrollBy` (Window.zig:853-860): relative, both axes written.
    pub fn window_scroll_by(&mut self, dom: &Dom, left: Option<i32>, top: Option<i32>) -> bool {
        let abs = |cur: i32, d: Option<i32>| (cur as i64 + d.unwrap_or(0) as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let x = abs(self.window_scroll.0, left);
        let y = abs(self.window_scroll.1, top);
        self.window_scroll_to(dom, Some(x), Some(y))
    }

    // ----- element scroll ----------------------------------------------------

    /// Changes at every scroll (of the window or an element).
    pub fn scroll_generation(&self) -> u64 {
        self.scroll_generation as u64
    }

    /// Tells the geometry source the scroll offsets now, when they changed since it was last told:
    /// scroll-driven animations follow them. Called at the end of each animation frame (and when a
    /// frame's time passed without one, `push_scroll_if_due`), not at each scroll: a scroll moves
    /// scroll timelines at the next frame, as in browsers (their times hold still until then).
    /// `true`: told.
    pub fn push_scroll(&self) -> bool {
        if self.scroll_pushed.get() == Some(self.scroll_generation as u64) {
            return false;
        }
        self.scroll_pushed.set(Some(self.scroll_generation as u64));
        if let Some(source) = self.external.as_ref() {
            let window = (self.window_scroll.0 as f64, self.window_scroll.1 as f64);
            source.set_scroll(self.scrolled_elements(), window);
        }
        true
    }

    /// [`Self::push_scroll`] when `now_ms` (the page clock) is a frame (16 ms) or more past the last
    /// push: without frames (nothing requested one) scroll timelines still follow within one.
    pub fn push_scroll_if_due(&self, now_ms: f64) {
        let last = self.scroll_pushed_at.get();
        if now_ms - last >= 16.0 {
            if self.push_scroll() {
                self.scroll_pushed_at.set(now_ms);
            }
        }
    }

    /// Records when the scroll offsets were last told ([`Self::push_scroll`]).
    pub fn set_scroll_pushed_at(&self, now_ms: f64) {
        self.scroll_pushed_at.set(now_ms);
    }

    /// Changes whenever the scroll offsets scroll timelines follow change ([`Self::push_scroll`]).
    pub fn timeline_scroll_generation(&self) -> u64 {
        self.scroll_pushed.get().unwrap_or(0)
    }

    /// pyro: records the offsets script asked for on `el` (CSS px of the element; `zoom` its
    /// effective zoom) after a write, on each axis whose kept offset is the asked one rounded (a
    /// clamped one reads as kept). Unzoomed, an offset reads as kept: whole px, as Chromium rounds
    /// it (`scrollTop = 10.5` reads 11).
    pub fn note_css_scroll(&mut self, el: NodeId, x: Option<f64>, y: Option<f64>, zoom: f64) {
        let kept = [self.scroll_left(el), self.scroll_top(el)];
        let entry = self.css_scroll.entry(el).or_insert([None, None]);
        for (axis, asked) in [x, y].into_iter().enumerate() {
            if let Some(v) = asked {
                let rounded = (v * zoom + 0.5).floor();
                entry[axis] = (zoom != 1.0 && rounded == kept[axis] as f64).then_some((v, kept[axis], zoom));
            }
        }
    }

    /// pyro: `el`'s scroll offset on an axis (`top`: vertical) in its CSS px, for an element of
    /// effective zoom `zoom`: what script asked for while the kept offset is still the one it
    /// became at the zoom it is at ([`Self::note_css_scroll`]), else the kept offset over the zoom.
    pub fn css_scroll(&self, el: NodeId, top: bool, zoom: f64) -> f64 {
        let kept = if top { self.scroll_top(el) } else { self.scroll_left(el) };
        match self.css_scroll.get(&el).and_then(|entry| entry[top as usize]) {
            Some((value, at, asked_zoom)) if at == kept && asked_zoom == zoom => value,
            _ => kept as f64 / zoom,
        }
    }

    /// `scrollTop` (Element.zig:1757-1761).
    pub fn scroll_top(&self, el: NodeId) -> i32 {
        self.element_scroll.get(&el).map_or(0, |p| p.1)
    }

    /// `scrollLeft` (Element.zig:1767-1771).
    pub fn scroll_left(&self, el: NodeId) -> i32 {
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
        // A scroll without a target (CSS Overflow 5 § 2.1).
        self.scroll_targets.remove(&Some(el));
        let current = self.element_scroll.get(&el).copied().unwrap_or((0, 0));
        let mut x = current.0;
        let mut y = current.1;
        // pyro: the scroll lands on a snap position (CSS Scroll Snap 1 § 6.2), as Chromium snaps
        // programmatic scrolls.
        let tx = write.target(Axis::Width, current.0);
        let ty = write.target(Axis::Height, current.1);
        let (tx, ty) = self.snap_scroll(dom, Some(el), current, tx, ty);
        if let Some(t) = tx {
            x = self.clamp_scroll(dom, el, Axis::Width, t);
        }
        if let Some(t) = ty {
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

    /// pyro: the scroll offsets `scroller` (`None`: the viewport) lands on for the targets `tx`,
    /// `ty` (`None`: that axis is not written) when it has `scroll-snap-type`: on each snapping
    /// axis, the snap position nearest the target among those of the boxes whose nearest scroll
    /// container it is (`scroll-snap-align` in the snapport, the scroll container's padding box
    /// less `scroll-padding`, their boxes grown by `scroll-margin`). `mandatory` always snaps;
    /// `proximity` within a third of the snapport, as Chromium. The targets as given otherwise.
    fn snap_scroll(
        &mut self,
        dom: &Dom,
        scroller: Option<NodeId>,
        current: (i32, i32),
        tx: Option<i64>,
        ty: Option<i64>,
    ) -> (Option<i64>, Option<i64>) {
        let (x, y, chosen) = self.snap_positions(dom, scroller, current, tx, ty, None);
        if chosen.iter().any(Option::is_some) {
            self.snapped.insert(scroller, chosen);
        } else {
            self.snapped.remove(&scroller);
        }
        (x, y)
    }

    /// [`Self::snap_scroll`]'s positions and the boxes they snap to on each axis; `only` limits
    /// an axis's candidates to one box (re-snapping to it).
    fn snap_positions(
        &self,
        dom: &Dom,
        scroller: Option<NodeId>,
        current: (i32, i32),
        tx: Option<i64>,
        ty: Option<i64>,
        only: Option<[Option<SnapBox>; 2]>,
    ) -> (Option<i64>, Option<i64>, [Option<SnapBox>; 2]) {
        let none = [None, None];
        let unchanged = (tx, ty, none);
        if tx.is_none() && ty.is_none() {
            return unchanged;
        }
        let doc = self.style.document();
        let Some(styled) = scroller.or_else(|| dom.document_element(doc).ok().flatten()) else { return unchanged };
        let value = |el: NodeId, name: &str| self.style.computed_value(dom, el, name).unwrap_or_default();
        let snap_type = value(styled, "scroll-snap-type");
        if snap_type.is_empty() || snap_type == "none" {
            return unchanged;
        }
        let words: Vec<&str> = snap_type.split_whitespace().collect();
        let (snap_x, snap_y) = match words.first().copied() {
            Some("x") | Some("inline") => (true, false),
            Some("y") | Some("block") => (false, true),
            Some("both") => (true, true),
            _ => return unchanged,
        };
        let mandatory = words.get(1).copied() == Some("mandatory");
        // The scroller's padding box in client coordinates and its size.
        let px = |v: &str, basis: f64| -> f64 {
            let v = v.trim();
            if let Some(p) = v.strip_suffix('%') {
                p.parse::<f64>().map_or(0.0, |p| p * basis / 100.0)
            } else {
                v.trim_end_matches("px").parse::<f64>().unwrap_or(0.0)
            }
        };
        // pyro: the scrollport: past a left or top classic scrollbar gutter, the viewport's less
        // its gutters (Chrome: WPT css-scroll-snap `scroll-snap-type-on-root-element`).
        let (origin_x, origin_y, width, height) = match scroller {
            Some(el) => {
                let r = self.bounding_client_rect(dom, el);
                let gutters = self.layout_box(dom, el).map(|b| b.border);
                let bl = px(&value(el, "border-left-width"), 0.0);
                let bt = px(&value(el, "border-top-width"), 0.0);
                let (bl, bt) = gutters.map_or((bl, bt), |b| (b[LEFT].max(bl), b[TOP].max(bt)));
                (r.x + bl - current.0 as f64, r.y + bt - current.1 as f64, self.client_width(dom, el), self.client_height(dom, el))
            }
            None => {
                let [gl, gt, gr, gb] =
                    self.synced_geometry_source(dom).and_then(|s| s.viewport_gutters()).unwrap_or([0.0; 4]);
                (
                    gl - current.0 as f64,
                    gt - current.1 as f64,
                    self.inner_width() as f64 - gl - gr,
                    self.inner_height() as f64 - gt - gb,
                )
            }
        };
        let pad = |side: &str, basis: f64| {
            let v = value(styled, &format!("scroll-padding-{side}"));
            if v == "auto" { 0.0 } else { px(&v, basis) }
        };
        let (pt, pr, pb, pl) = (pad("top", height), pad("right", width), pad("bottom", height), pad("left", width));
        let mut best: [Option<(f64, SnapBox)>; 2] = [None, None];
        // Every box's snap position on each axis, to break ties (below).
        let mut all: Vec<(NodeId, [Option<f64>; 2])> = Vec::new();
        let target = [tx.map(|v| v as f64), ty.map(|v| v as f64)];
        let Ok(nodes) = dom.descendants(scroller.unwrap_or(doc)) else { return unchanged };
        for id in nodes {
            if dom.element(id).is_err() || Some(id) == scroller {
                continue;
            }
            let align = value(id, "scroll-snap-align");
            if align.is_empty() || align == "none" {
                continue;
            }
            // Its nearest scroll container: as user scrolls find it (which handles the root and
            // body), or, `overflow: hidden` being one too, its nearest ancestor that clips.
            let by_user = match dom.parent_element(id).ok().flatten() {
                Some(p) => self.scroll_container(dom, p, crate::style::ScrollAxes { x: true, y: true }),
                None => ScrollTarget::Viewport,
            };
            let by_user = match (by_user, scroller) {
                (ScrollTarget::Container(c), Some(s)) => c == s,
                (ScrollTarget::Viewport, None) => true,
                _ => false,
            };
            let owned = by_user || {
                let mut cur = dom.parent_element(id).ok().flatten();
                loop {
                    match cur {
                        None => break scroller.is_none(),
                        Some(p) if Some(p) == scroller => break true,
                        Some(p) if dom.parent_element(p).ok().flatten().is_none() => break scroller.is_none(),
                        Some(p) => {
                            let scrolls = |v: String| !matches!(v.as_str(), "" | "visible" | "clip");
                            if scrolls(value(p, "overflow-x")) || scrolls(value(p, "overflow-y")) {
                                break false;
                            }
                            cur = dom.parent_element(p).ok().flatten();
                        }
                    }
                }
            };
            if !owned {
                continue;
            }
            let r = self.bounding_client_rect(dom, id);
            let margin = |side: &str| px(&value(id, &format!("scroll-margin-{side}")), 0.0);
            let (top, bottom) = (r.y - origin_y - margin("top"), r.y + r.height - origin_y + margin("bottom"));
            let (left, right) = (r.x - origin_x - margin("left"), r.x + r.width - origin_x + margin("right"));
            let mut parts = align.split_whitespace();
            let block = parts.next().unwrap_or("none");
            let inline = parts.next().unwrap_or(block);
            let position = |a: &str, start: f64, end: f64, pad_start: f64, pad_end: f64, size: f64| match a {
                "start" => Some(start - pad_start),
                "end" => Some(end - (size - pad_end)),
                "center" => Some((start + end) / 2.0 - (pad_start + (size - pad_start - pad_end) / 2.0)),
                _ => None,
            };
            let candidates = [
                snap_x.then(|| position(inline, left, right, pl, pr, width)).flatten(),
                snap_y.then(|| position(block, top, bottom, pt, pb, height)).flatten(),
            ];
            // A snap area larger than the snapport on an axis can be scrolled within: any target
            // that keeps the snapport inside it is a snap position (CSS Scroll Snap 1 § 6.2.1).
            let covering = [
                snap_x && right - left > width - pl - pr,
                snap_y && bottom - top > height - pt - pb,
            ];
            let spans = [(left - pl, right - (width - pr)), (top - pt, bottom - (height - pb))];
            let mut mine = [None, None];
            for axis in 0..2 {
                if only.is_some_and(|o| o[axis].is_some_and(|b| b != (id, None))) {
                    continue;
                }
                let (Some(c), Some(t)) = (candidates[axis], target[axis]) else { continue };
                let c = if covering[axis] && t >= spans[axis].0 && t <= spans[axis].1 { t } else { c };
                let c = c.max(0.0);
                mine[axis] = Some(c);
                if best[axis].is_none_or(|(b, _)| (c - t).abs() < (b - t).abs()) {
                    best[axis] = Some((c, (id, None)));
                }
            }
            all.push((id, mine));
        }
        // pyro: the columns of a multi-column scroll container are snap areas when its `::column`
        // has a `scroll-snap-align` (CSS Multicol 2 § 4; WPT css-overflow
        // `column-style-change-triggers-relayout`).
        if let (Some(container), Some(source)) = (scroller, self.external.as_ref()) {
            let columns = source.column_boxes(crate::snapshot::render_key(container));
            let align = if columns.is_empty() {
                String::new()
            } else {
                self.computed_pseudo_style_value(dom, container, &style::selector_parser::PseudoElement::Column, "scroll-snap-align")
            };
            if !align.is_empty() && align != "none" {
                let mut parts = align.split_whitespace();
                let block = parts.next().unwrap_or("none").to_owned();
                let inline = parts.next().map(str::to_owned).unwrap_or_else(|| block.clone());
                let (bl, bt) = (px(&value(container, "border-left-width"), 0.0), px(&value(container, "border-top-width"), 0.0));
                let position = |a: &str, start: f64, end: f64, pad_start: f64, pad_end: f64, size: f64| match a {
                    "start" => Some(start - pad_start),
                    "end" => Some(end - (size - pad_end)),
                    "center" => Some((start + end) / 2.0 - (pad_start + (size - pad_start - pad_end) / 2.0)),
                    _ => None,
                };
                // (Each column by its index, which a re-snap follows: review of round 6.)
                for (index, [cx, cy, cw, ch]) in columns.into_iter().enumerate() {
                    let (left, right, top, bottom) = (cx - bl, cx - bl + cw, cy - bt, cy - bt + ch);
                    let candidates = [
                        snap_x.then(|| position(&inline, left, right, pl, pr, width)).flatten(),
                        snap_y.then(|| position(&block, top, bottom, pt, pb, height)).flatten(),
                    ];
                    for axis in 0..2 {
                        if only.is_some_and(|o| o[axis].is_some_and(|b| b != (container, Some(index)))) {
                            continue;
                        }
                        let (Some(c), Some(t)) = (candidates[axis], target[axis]) else { continue };
                        let c = c.max(0.0);
                        if best[axis].is_none_or(|(b, _)| (c - t).abs() < (b - t).abs()) {
                            best[axis] = Some((c, (container, Some(index))));
                        }
                    }
                }
            }
        }
        // Several boxes at the same position on an axis: the box at the snap position on both axes
        // wins (Chromium picks a box aligned on both before one aligned on a single axis).
        let picked = [best[0].map(|b| b.0), best[1].map(|b| b.0)];
        for axis in 0..2 {
            let other = 1 - axis;
            let (Some(p), Some(q)) = (picked[axis], picked[other]) else { continue };
            let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 0.5);
            if let Some((id, _)) = all.iter().find(|(_, c)| close(c[axis], p) && close(c[other], q)) {
                best[axis] = Some((p, (*id, None)));
            }
        }
        let mut chosen = none;
        let mut snap = |axis: usize, t: Option<i64>, size: f64| match (t, best[axis]) {
            (Some(t), Some((b, id))) if mandatory || only.is_some() || (b - t as f64).abs() <= size / 3.0 => {
                chosen[axis] = Some(id);
                Some(b.round() as i64)
            }
            _ => t,
        };
        let x = snap(0, tx, width);
        let y = snap(1, ty, height);
        (x, y, chosen)
    }

    /// pyro: after a layout change, each scroller that snapped snaps again to the box it snapped
    /// to (or, when that box no longer has a snap position, to the nearest one), as Chromium
    /// re-snaps (CSS Scroll Snap 1 § 6.2.3). Adds the scrollers that moved to `moved`.
    fn resnap(&mut self, dom: &Dom, moved: &mut Vec<ScrollTarget>) {
        let mut snapped: Vec<(Option<NodeId>, [Option<SnapBox>; 2])> = self.snapped.iter().map(|(k, v)| (*k, *v)).collect();
        snapped.sort_by_key(|(k, _)| k.map(|k| (k.idx, k.gen)));
        for (scroller, targets) in snapped {
            if let Some(el) = scroller {
                if !dom.is_connected(el).unwrap_or(false) || !self.is_visible(dom, el) {
                    self.snapped.remove(&scroller);
                    continue;
                }
            }
            let current = match scroller {
                Some(el) => self.element_scroll.get(&el).copied().unwrap_or((0, 0)),
                None => self.window_scroll,
            };
            let (tx, ty) = (Some(current.0 as i64), Some(current.1 as i64));
            let alive = targets.map(|t| t.filter(|(el, _)| dom.is_connected(*el).unwrap_or(false)));
            let (mut x, mut y, mut chosen) = self.snap_positions(dom, scroller, current, tx, ty, Some(alive));
            // The box it snapped to has no snap position any more: the nearest one.
            if chosen[0].is_none() && chosen[1].is_none() {
                (x, y, chosen) = self.snap_positions(dom, scroller, current, tx, ty, None);
            }
            if chosen.iter().all(Option::is_none) {
                self.snapped.remove(&scroller);
                continue;
            }
            self.snapped.insert(scroller, chosen);
            let (x, y) = (x.unwrap_or(current.0 as i64), y.unwrap_or(current.1 as i64));
            match scroller {
                Some(el) => {
                    let next = (self.clamp_scroll(dom, el, Axis::Width, x), self.clamp_scroll(dom, el, Axis::Height, y));
                    if next != current {
                        self.element_scroll.insert(el, next);
                        self.scroll_generation = self.scroll_generation.wrapping_add(1);
                        if !moved.contains(&ScrollTarget::Container(el)) {
                            moved.push(ScrollTarget::Container(el));
                        }
                    }
                }
                None => {
                    let (rx, ry) = self.viewport_scroll_range(dom);
                    let next = (
                        x.clamp(rx.0 as i64, rx.1 as i64) as i32,
                        y.clamp(ry.0 as i64, ry.1 as i64) as i32,
                    );
                    if next != self.window_scroll {
                        self.window_scroll = next;
                        self.scroll_generation = self.scroll_generation.wrapping_add(1);
                        if !moved.contains(&ScrollTarget::Viewport) {
                            moved.push(ScrollTarget::Viewport);
                        }
                    }
                }
            }
        }
    }

    /// Whether any element or the window is scrolled away from the origin.
    /// Records the layout `scroller` is settled in (nothing at the top).
    fn capture_scroll_anchor(&mut self, dom: &Dom, scroller: NodeId) {
        if self.scroll_top(scroller) <= 0 {
            self.scroll_anchors.remove(&scroller);
            return;
        }
        let result = self.layout_result(dom);
        let anchor = self
            .scroll_anchor(dom, &result, scroller, &|_| true)
            .map(|anchor| (anchor, self.anchor_fingerprint(dom, scroller, anchor)));
        self.scroll_anchors.insert(scroller, (result, anchor));
    }

    /// pyro: the computed values of the properties whose change is a suppression trigger (CSS
    /// Scroll Anchoring 1 § 2.4: position, insets, margin, padding, sizes, transforms) of `anchor`
    /// and its ancestors up to `scroller`. A change between selecting the anchor and adjusting for
    /// it suppresses the adjustment (WPT scroll-animations `view-timeline-subject-bounds-update`:
    /// the anchor's own height and margin change).
    fn anchor_fingerprint(&self, dom: &Dom, scroller: NodeId, anchor: NodeId) -> String {
        const PROPERTIES: &[&str] = &[
            "position", "top", "right", "bottom", "left", "margin-top", "margin-right", "margin-bottom",
            "margin-left", "padding-top", "padding-right", "padding-bottom", "padding-left", "width", "height",
            "min-width", "min-height", "max-width", "max-height", "transform", "translate", "rotate", "scale",
        ];
        let mut out = String::new();
        let mut current = Some(anchor);
        while let Some(el) = current.filter(|&el| el != scroller) {
            for property in PROPERTIES {
                out.push_str(&self.style.computed_value(dom, el, property).unwrap_or_default());
                out.push('|');
            }
            current = dom.parent_element(el).ok().flatten();
        }
        out
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
        let mut anchors: Vec<(NodeId, Rc<LayoutResult>, Option<(NodeId, String)>)> =
            self.scroll_anchors.iter().map(|(&k, (v, a))| (k, v.clone(), a.clone())).collect();
        anchors.sort_by_key(|(k, _, _)| (k.idx, k.gen));
        for (el, old, captured) in anchors {
            if Rc::ptr_eq(&old, &flow) {
                continue;
            }
            let Some(&(x, y)) = self.element_scroll.get(&el) else { continue };
            if y <= 0 || !dom.is_connected(el).unwrap_or(false) {
                continue;
            }
            let keep = |n: NodeId| flow.get(n).is_some();
            let Some(anchor) = self.scroll_anchor(dom, &old, el, &keep) else { continue };
            // A suppression trigger on the anchor or an ancestor since: no adjustment.
            let suppressed = captured.as_ref().is_some_and(|(captured, fingerprint)| {
                *captured == anchor && self.anchor_fingerprint(dom, el, anchor) != *fingerprint
            });
            if suppressed {
                continue;
            }
            let (Some(sb), Some(ab), Some(osb), Some(oab)) =
                (flow.get(el), flow.get(anchor), old.get(el), old.get(anchor))
            else {
                continue;
            };
            let delta = (ab.rect.y - sb.rect.y) - (oab.rect.y - osb.rect.y);
            let ny = (y as f64 + delta).round().clamp(0.0, i32::MAX as f64) as i32;
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
            .chain(
                self.pseudo_scroll
                    .iter()
                    .filter(|(_, &o)| o != (0, 0))
                    .map(|(&(el, p), &(x, y))| (crate::snapshot::pseudo_render_key(el, p), (x as f64, y as f64))),
            )
            .collect()
    }

    /// The `position: sticky` elements that scrolling holds away from where
    /// they are laid out, by render key, each with its own offset (CSS px;
    /// its sticky ancestors' are theirs): what painting moves them by.
    pub fn sticky_offsets(&self, dom: &Dom) -> Vec<(u64, (f64, f64))> {
        let sticky = self.sticky_map(dom);
        if sticky.is_empty() {
            return Vec::new();
        }
        let mut memo = FxHashMap::default();
        let mut out: Vec<(u64, (f64, f64))> = sticky
            .keys()
            .filter_map(|&el| {
                let offset =
                    self.sticky_delta(dom, el, &|n| sticky.get(&n).copied(), &|n| self.layout_box(dom, n), &mut memo);
                (offset != (0.0, 0.0)).then(|| (crate::snapshot::render_key(el), offset))
            })
            .collect();
        out.sort_by_key(|&(key, _)| key);
        out
    }

    /// pyro: CSS Scroll Snap 1 § 6.3: a scroll container that snaps mandatorily and was not
    /// scrolled starts at a snap position (Chromium snaps it at its first layout). Looked for once
    /// per DOM version. Whether a scroll offset changed.
    pub fn snap_initial_positions(&mut self, dom: &Dom) -> bool {
        // pyro: free for a document no element of which was styled with a `scroll-snap-type` as
        // of the last sync (looked for again after the next: the version is not marked checked).
        if !self.external.as_ref().filter(|s| s.active()).is_some_and(|s| s.uses_scroll_snap()) {
            return false;
        }
        let version = dom.version();
        if self.initial_snap_checked.replace(Some(version)) == Some(version) {
            return false;
        }
        let Some(source) = self.synced_geometry_source(dom).cloned() else { return false };
        let mut moved = false;
        for key in source.mandatory_snap_containers() {
            let Some(el) = crate::geometry::node_of(dom, key) else { continue };
            // (An offset of zero is no scroll: a script read or a reveal that found nothing to do.)
            if self.element_scroll.get(&el).is_some_and(|&o| o != (0, 0)) || !self.initially_snapped.insert(el) {
                continue;
            }
            let (tx, ty) = self.snap_scroll(dom, Some(el), (0, 0), Some(0), Some(0));
            let x = tx.map_or(0, |t| self.clamp_scroll(dom, el, Axis::Width, t));
            let y = ty.map_or(0, |t| self.clamp_scroll(dom, el, Axis::Height, t));
            if (x, y) != (0, 0) {
                self.element_scroll.insert(el, (x, y));
                self.scroll_generation = self.scroll_generation.wrapping_add(1);
                moved = true;
            }
        }
        // pyro: the viewport too, when the root's `scroll-snap-type` snaps mandatorily.
        if self.window_scroll == (0, 0) && !self.viewport_initially_snapped {
            let doc = self.style.document();
            let mandatory = dom.document_element(doc).ok().flatten().is_some_and(|root| {
                self.style.computed_value(dom, root, "scroll-snap-type").unwrap_or_default().ends_with("mandatory")
            });
            if mandatory {
                self.viewport_initially_snapped = true;
                moved |= self.window_scroll_to(dom, Some(0), Some(0));
            }
        }
        moved
    }

    pub fn has_scroll_offsets(&self) -> bool {
        self.window_scroll != (0, 0)
            || self.element_scroll.values().any(|&o| o != (0, 0))
            || !self.snapped.is_empty()
            || self.pseudo_scroll.values().any(|&o| o != (0, 0))
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
        if !self.snapped.is_empty() {
            self.resnap(dom, &mut moved);
        }
        let mut scrolled: Vec<(NodeId, (i32, i32))> =
            self.element_scroll.iter().filter(|(_, &o)| o != (0, 0)).map(|(&k, &v)| (k, v)).collect();
        scrolled.sort_by_key(|(k, _)| (k.idx, k.gen));
        for (el, (x, y)) in scrolled {
            if !dom.is_connected(el).unwrap_or(false) || !self.is_visible(dom, el) {
                continue;
            }
            let limit = |layout: &Self, axis: Axis, v: i32| {
                if v == 0 {
                    return 0;
                }
                layout.clamp_scroll(dom, el, axis, v as i64)
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
        let anchored: Vec<NodeId> = self.element_scroll.iter().filter(|(_, &(_, y))| y > 0).map(|(&k, _)| k).collect();
        self.scroll_anchors.retain(|k, _| anchored.contains(k));
        for el in anchored {
            self.capture_scroll_anchor(dom, el);
        }
        if self.window_scroll != (0, 0) {
            let (rx, ry) = self.viewport_scroll_range(dom);
            let clamped = (self.window_scroll.0.clamp(rx.0, rx.1), self.window_scroll.1.clamp(ry.0, ry.1));
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
    ///
    /// pyro: an axis whose scroll origin is at its end (`direction: rtl`, `row-reverse`,
    /// `column-reverse`, `wrap-reverse`) scrolls from 0 down to minus its extent, as in CSSOM
    /// View (it stayed at 0, its overflow on the start side unreachable).
    fn clamp_scroll(&self, dom: &Dom, el: NodeId, axis: Axis, target: i64) -> i32 {
        let clamped = if target == 0 {
            0
        } else if self.scroll_reversed(dom, el, axis) {
            if target >= 0 {
                0
            } else {
                let extent = self.scroll_extent(dom, el, axis).map_or(0, |e| e.floor() as i64);
                target.max(-extent)
            }
        } else if target < 0 {
            0
        } else if let Some(extent) = self.scroll_extent(dom, el, axis) {
            target.min(extent.floor() as i64)
        } else {
            target
        };
        clamped.clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    /// pyro: whether `el` scrolls from the end of `axis` (its scroll origin there).
    fn scroll_reversed(&self, dom: &Dom, el: NodeId, axis: Axis) -> bool {
        self.layout_box(dom, el).is_some_and(|b| match axis {
            Axis::Width => b.scroll_reversed[0],
            Axis::Height => b.scroll_reversed[1],
        })
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
    /// [`Self::wheel_scroll`] at the client point (`x`, `y`) over `target`: a `::before` or
    /// `::after` box of `target` that is a scroll container under the point scrolls first, on each
    /// axis it can still move on (WPT css-overflow `before-after-pseudo-element-scrolling`).
    pub fn wheel_scroll_at(
        &mut self,
        dom: &Dom,
        target: NodeId,
        point: (f64, f64),
        delta_x: f64,
        delta_y: f64,
    ) -> Vec<ScrollTarget> {
        let (mut dx, mut dy) = (delta_to_scroll(delta_x), delta_to_scroll(delta_y));
        let mut pseudo_moved = false;
        if let Some(source) = self.synced_geometry_source(dom).cloned() {
            for (pseudo, which) in [
                (1u8, style::selector_parser::PseudoElement::Before),
                (2u8, style::selector_parser::PseudoElement::After),
            ] {
                if dx == 0 && dy == 0 {
                    break;
                }
                let scrolls = |name: &str| {
                    matches!(self.computed_pseudo_style_value(dom, target, &which, name).as_str(), "hidden" | "scroll" | "auto")
                };
                if !scrolls("overflow-x") && !scrolls("overflow-y") {
                    continue;
                }
                let Some(b) = source.element_box(crate::snapshot::pseudo_render_key(target, pseudo)) else { continue };
                let (px, py) = if b.fixed {
                    point
                } else {
                    (point.0 + self.window_scroll.0 as f64, point.1 + self.window_scroll.1 as f64)
                };
                if px < b.x || py < b.y || px >= b.x + b.width || py >= b.y + b.height {
                    continue;
                }
                let max_x = (b.scroll_width - b.client_width).round().max(0.0) as i32;
                let max_y = (b.scroll_height - b.client_height).round().max(0.0) as i32;
                let current = self.pseudo_scroll.get(&(target, pseudo)).copied().unwrap_or((0, 0));
                let next = (
                    current.0.saturating_add(dx).clamp(0, max_x),
                    current.1.saturating_add(dy).clamp(0, max_y),
                );
                if next.0 != current.0 {
                    dx = 0;
                }
                if next.1 != current.1 {
                    dy = 0;
                }
                if next != current {
                    self.pseudo_scroll.insert((target, pseudo), next);
                    self.scroll_generation = self.scroll_generation.wrapping_add(1);
                    pseudo_moved = true;
                }
            }
        }
        // (A pseudo-element's scroll events go to its originating element, as Chromium sends them.)
        let mut moved = if pseudo_moved { vec![ScrollTarget::Container(target)] } else { Vec::new() };
        for (axis, d) in [(Axis::Width, dx), (Axis::Height, dy)] {
            if let Some(t) = self.scroll_axis(dom, target, axis, d) {
                if !moved.contains(&t) {
                    moved.push(t);
                }
            }
        }
        moved
    }

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
    /// `target` (a client rect) inside `node`'s scrollport, in `node`'s own
    /// CSS px: (left, width, top, height) from the padding box's top-left.
    /// A transformed (scaled) container or ancestor shows its scroll offsets
    /// scaled, so client distances divide by the container's scale, as
    /// Blink's scroll into view maps the rect into the scroller's space.
    fn rect_in_scrollport(&self, dom: &Dom, node: NodeId, target: Rect) -> (f64, f64, f64, f64) {
        let container = self.bounding_client_rect(dom, node);
        let scale = |client: f64, local: f64| if client > 0.0 && local > 0.0 { client / local } else { 1.0 };
        let (width, height) = self.untransformed_size(dom, node);
        let sx = scale(container.width, width);
        let sy = scale(container.height, height);
        (
            (target.left() - container.left()) / sx - self.client_left(dom, node),
            target.width / sx,
            (target.top() - container.top()) / sy - self.client_top(dom, node),
            target.height / sy,
        )
    }

    /// Whether `node` is a scroll container on either axis (`hidden`,
    /// `scroll` or `auto`): scroll into view scrolls every one, `hidden`
    /// included, as Chromium does for script and for an action's reveal.
    fn is_scroll_container(&self, dom: &Dom, node: NodeId) -> bool {
        use style::values::specified::box_::Overflow as O;
        let scroller = |o: O| matches!(o, O::Hidden | O::Scroll | O::Auto);
        self.style.stylo_style(dom, node).is_some_and(|s| {
            let b = s.get_box();
            scroller(b.overflow_x) || scroller(b.overflow_y)
        })
    }

    fn reveal_in_scroll_ancestors(&mut self, dom: &Dom, el: NodeId) -> Vec<ScrollTarget> {
        let mut moved = Vec::new();
        let mut cur = dom.parent_element(el).ok().flatten();
        while let Some(node) = cur {
            if self.is_root_container(dom, node) {
                break;
            }
            let any = self.is_scroll_container(dom, node);
            let axes = ScrollAxes { x: any, y: any };
            let explicit = self.element_scroll.contains_key(&node);
            if axes.x || axes.y || explicit {
                let target = self.bounding_client_rect(dom, el);
                let (x, width, offset, height) = self.rect_in_scrollport(dom, node, target);
                let left = (axes.x || explicit)
                    .then(|| nearest_scroll_offset(x, width, self.client_width(dom, node), self.scroll_left(node) as i64))
                    .flatten();
                let visible = self.client_height(dom, node);
                let top = ((axes.y || explicit) && (offset < 0.0 || offset + height > visible)).then(|| {
                    (offset + self.scroll_top(node) as f64).trunc().clamp(i32::MIN as f64, i32::MAX as f64) as i32
                });
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
            // pyro: an `overflow: hidden` axis too (Chromium's focus scrolls it; WPT css-contain
            // `contain-body-overflow-002`), on which the element fits: a visually hidden wrapper
            // (a 1px overflow: hidden box around a native control) is left alone, as an action's
            // reveal leaves it (`scroll_into_view_offset_targets`).
            let mut axes = self.style.overflow_axes(dom, node);
            if self.is_scroll_container(dom, node) {
                let target = self.bounding_client_rect(dom, el);
                axes.x = axes.x || target.width.abs() <= self.client_width(dom, node);
                axes.y = axes.y || target.height.abs() <= self.client_height(dom, node);
            }
            if axes.x || axes.y {
                let target = self.bounding_client_rect(dom, el);
                let (x, width, y, height) = self.rect_in_scrollport(dom, node, target);
                let x = x + self.client_left(dom, node);
                let y = y + self.client_top(dom, node);
                let left = axes
                    .x
                    .then(|| focus_scroll_offset(x, width, self.client_width(dom, node), self.scroll_left(node) as i64))
                    .flatten();
                let top = axes
                    .y
                    .then(|| focus_scroll_offset(y, height, self.client_height(dom, node), self.scroll_top(node) as i64))
                    .flatten();
                if self.element_scroll_to(dom, node, left, top) {
                    moved.push(ScrollTarget::Container(node));
                }
            }
            parent = dom.parent_element(node).ok().flatten();
        }
        let target = self.bounding_client_rect(dom, el);
        let left =
            focus_scroll_offset(target.left(), target.width, self.inner_width() as f64, self.window_scroll.0 as i64);
        let top =
            focus_scroll_offset(target.top(), target.height, self.inner_height() as f64, self.window_scroll.1 as i64);
        if self.window_scroll_to(dom, left, top) {
            moved.push(ScrollTarget::Viewport);
        }
        moved
    }

    /// `scrollIntoView` keeps the existing vertical start alignment for the
    /// viewport, after revealing clipped overflow ancestors inside-out.
    /// pyro: a click's default action on a `::scroll-marker` at the client point: scrolls to its
    /// target (CSS Overflow 5 § 3.2). A column's container scrolls the column into its
    /// scrollport as its `::column`'s `scroll-snap-align` aligns it (start without one); an
    /// element scrolls into view. Returns the scrollers that moved (WPT css-overflow
    /// `column-scroll-marker-002`, `column-scroll-marker-activation-target-current`).
    pub fn activate_scroll_marker_at(&mut self, dom: &Dom, point: (f64, f64)) -> Vec<ScrollTarget> {
        let Some(source) = self.synced_geometry_source(dom).cloned() else { return Vec::new() };
        let doc_point = (point.0 + self.window_scroll.0 as f64, point.1 + self.window_scroll.1 as f64);
        let Some((key, column)) = source.scroll_marker_at(doc_point.0, doc_point.1) else { return Vec::new() };
        let Some(target) = crate::geometry::node_of(dom, key) else { return Vec::new() };
        let Some(index) = column else {
            return self.scroll_into_view_targets(dom, target);
        };
        let Some(&[cx, _cy, cw, _ch]) = source.column_boxes(key).get(index as usize) else { return Vec::new() };
        let value = |name: &str| self.style.computed_value(dom, target, name).unwrap_or_default();
        let border_left = value("border-left-width").trim_end_matches("px").parse::<f64>().unwrap_or(0.0);
        let align = self.computed_pseudo_style_value(dom, target, &style::selector_parser::PseudoElement::Column, "scroll-snap-align");
        let mut parts = align.split_whitespace();
        let block = parts.next().unwrap_or("none").to_owned();
        let inline = parts.next().map(str::to_owned).unwrap_or(block);
        let width = self.client_width(dom, target) as f64;
        let left = cx - border_left;
        let x = match inline.as_str() {
            "center" => left + cw / 2.0 - width / 2.0,
            "end" => left + cw - width,
            _ => left,
        };
        if self.element_scroll_to(dom, target, Some(x.round().max(0.0) as i32), None) {
            vec![ScrollTarget::Container(target)]
        } else {
            Vec::new()
        }
    }

    pub fn scroll_into_view(&mut self, dom: &Dom, el: NodeId) -> bool {
        !self.scroll_into_view_targets(dom, el).is_empty()
    }

    /// Changed targets in delivery order. Embedders use this to queue each
    /// container's scroll event followed by the document's viewport event.
    /// Vertical start reveal and horizontal nearest-edge reveal; not full CSS alignment.
    pub fn scroll_into_view_targets(&mut self, dom: &Dom, el: NodeId) -> Vec<ScrollTarget> {
        self.force_content_visibility(dom, el);
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
        let left =
            nearest_scroll_offset(target.left(), target.width, self.inner_width() as f64, self.window_scroll.0 as i64);
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
            && nearest_scroll_offset(
                target.left(),
                target.width,
                self.inner_width() as f64,
                self.window_scroll.0 as i64,
            )
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
        let offset: fn(f64, f64, f64, i64) -> Option<i32> =
            if center { centered_scroll_offset } else { center_if_needed_offset };
        self.scroll_into_view_offset_targets(dom, el, offset, offset, false)
    }

    /// Script-facing `Element.scrollIntoView(options)` with an explicit
    /// alignment per axis (`block` vertical, `inline` horizontal; CSSOM View
    /// "scroll an element into view"), changed targets in delivery order.
    /// The default `{block: start, inline: nearest}` aligns in every scroll
    /// container on the way too, as Chromium does.
    pub fn scroll_into_view_aligned_script_targets(
        &mut self,
        dom: &Dom,
        el: NodeId,
        block: ScrollAlign,
        inline: ScrollAlign,
    ) -> Vec<ScrollTarget> {
        let moved = self.scroll_into_view_offset_targets(dom, el, inline.offset_fn(), block.offset_fn(), true);
        self.set_scroll_target(dom, el);
        moved
    }

    /// CSS Overflow 5 § 2.1: `el` is the current scroll target of each scroll container it was
    /// scrolled into view in (and of the viewport).
    fn set_scroll_target(&mut self, dom: &Dom, el: NodeId) {
        if !self.is_visible(dom, el) {
            return;
        }
        use style::values::specified::box_::Overflow as O;
        let scroller = |o: O| matches!(o, O::Hidden | O::Scroll | O::Auto);
        let mut cur = dom.parent_element(el).ok().flatten();
        while let Some(node) = cur {
            if self.is_root_container(dom, node) {
                break;
            }
            let scrolls = self.style.stylo_style(dom, node).is_some_and(|s| {
                let b = s.get_box();
                scroller(b.overflow_x) || scroller(b.overflow_y)
            });
            if scrolls {
                self.scroll_targets.insert(Some(node), el);
            }
            cur = dom.parent_element(node).ok().flatten();
        }
        self.scroll_targets.insert(None, el);
    }

    /// The current scroll targets (CSS Overflow 5 § 2.1) by render key: each scroll container's
    /// (the viewport's under `u64::MAX`) and its target's. Elements no longer in the document drop.
    pub fn scroll_targets(&self, dom: &Dom) -> Vec<(u64, u64)> {
        let mut out: Vec<(u64, u64)> = self
            .scroll_targets
            .iter()
            .filter(|&(scroller, &target)| {
                dom.check(target).is_ok() && scroller.is_none_or(|scroller| dom.check(scroller).is_ok())
            })
            .map(|(scroller, &target)| {
                (scroller.map_or(u64::MAX, crate::snapshot::render_key), crate::snapshot::render_key(target))
            })
            .collect();
        out.sort_unstable();
        out
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
        let offset: fn(f64, f64, f64, i64) -> Option<i32> =
            if center_if_needed { center_if_needed_offset } else { nearest_scroll_offset };
        let moved = self.scroll_into_view_offset_targets(dom, el, offset, offset, true);
        self.set_scroll_target(dom, el);
        moved
    }

    /// The box that scrolling `el` into view brings in: its border box grown by its
    /// `scroll-margin` (CSS Scroll Snap 1 § 4.1, the scroll snap area, which CSSOM View's
    /// "scroll an element into view" aligns).
    fn scroll_margin_rect(&self, dom: &Dom, el: NodeId) -> Rect {
        let r = self.bounding_client_rect(dom, el);
        let margin = |side: &str| {
            let v = self.style.computed_value(dom, el, &format!("scroll-margin-{side}")).unwrap_or_default();
            v.trim().trim_end_matches("px").parse::<f64>().unwrap_or(0.0)
        };
        let (top, right, bottom, left) = (margin("top"), margin("right"), margin("bottom"), margin("left"));
        if top == 0.0 && right == 0.0 && bottom == 0.0 && left == 0.0 {
            return r;
        }
        Rect { x: r.left() - left, y: r.top() - top, width: r.width.abs() + left + right, height: r.height.abs() + top + bottom }
    }

        /// `offset_x` aligns the horizontal axis, `offset_y` the vertical one.
    /// pyro: the alignment functions of `scrollIntoView` for the target `el` in the scroll container
    /// `container`: its `scroll-snap-align` on an axis the container snaps, else the ones asked for.
    #[allow(clippy::type_complexity)]
    fn snap_aligned_offsets(
        &self,
        dom: &Dom,
        container: NodeId,
        el: NodeId,
        offset_x: fn(f64, f64, f64, i64) -> Option<i32>,
        offset_y: fn(f64, f64, f64, i64) -> Option<i32>,
    ) -> (fn(f64, f64, f64, i64) -> Option<i32>, fn(f64, f64, f64, i64) -> Option<i32>) {
        let snap_type = self.style.computed_value(dom, container, "scroll-snap-type").unwrap_or_default();
        let (snap_x, snap_y) = match snap_type.split_whitespace().next() {
            Some("x") | Some("inline") => (true, false),
            Some("y") | Some("block") => (false, true),
            Some("both") => (true, true),
            _ => return (offset_x, offset_y),
        };
        let align = self.style.computed_value(dom, el, "scroll-snap-align").unwrap_or_default();
        let mut words = align.split_whitespace();
        let block = words.next().unwrap_or("none").to_owned();
        let inline = words.next().map(str::to_owned).unwrap_or_else(|| block.clone());
        let pick = |keyword: &str, asked: fn(f64, f64, f64, i64) -> Option<i32>| -> fn(f64, f64, f64, i64) -> Option<i32> {
            match keyword {
                "start" => ScrollAlign::Start.offset_fn(),
                "center" => ScrollAlign::Center.offset_fn(),
                "end" => ScrollAlign::End.offset_fn(),
                _ => asked,
            }
        };
        (
            if snap_x { pick(&inline, offset_x) } else { offset_x },
            if snap_y { pick(&block, offset_y) } else { offset_y },
        )
    }

    fn scroll_into_view_offset_targets(
        &mut self,
        dom: &Dom,
        el: NodeId,
        offset_x: fn(f64, f64, f64, i64) -> Option<i32>,
        offset_y: fn(f64, f64, f64, i64) -> Option<i32>,
        script: bool,
    ) -> Vec<ScrollTarget> {
        self.force_content_visibility(dom, el);
        if !self.is_visible(dom, el) {
            return Vec::new();
        }
        let mut moved = Vec::new();
        // pyro: up the flat tree: a slotted element's slot (and a shadow root's host) are
        // ancestors whose scroll containers scroll it (WPT cssom-view
        // `scrollIntoView-should-treat-slot-as-scroll-container`).
        let flat_parent = |node: NodeId| -> Option<NodeId> {
            // (A closed shadow root's slot too: script's `assignedSlot` hides it.)
            if let Some(slot) = dom.flat_tree_slot(node) {
                return Some(slot);
            }
            match dom.parent_element(node).ok().flatten() {
                Some(parent) => Some(parent),
                None => dom
                    .parent(node)
                    .ok()
                    .flatten()
                    .filter(|&frag| dom.is_shadow_root(frag).unwrap_or(false))
                    .and_then(|root| dom.fragment_host(root).ok().flatten()),
            }
        };
        let mut cur = flat_parent(el);
        while let Some(node) = cur {
            if self.is_root_container(dom, node) {
                break;
            }
            // A scroll container scrolls on both axes: the other axis's
            // `visible` computes to `auto` and `clip` to `hidden`. `hidden`
            // scrolls for an action's reveal too (Chromium's
            // DOM.scrollIntoViewIfNeeded brings a box past an overflow:
            // hidden stage's edge in).
            let any = self.is_scroll_container(dom, node);
            let mut axes = ScrollAxes { x: any, y: any };
            let explicit = !script && self.element_scroll.contains_key(&node);
            let target = self.scroll_margin_rect(dom, el);
            // From the scrollport (the padding box) in the container's own px, as
            // Blink's scroll into view measures for script and for an action's reveal alike.
            let (x, width, y, height) = self.rect_in_scrollport(dom, node, target);
            if !script && any {
                // An action's reveal leaves a visually hidden wrapper (a 1px
                // overflow: hidden box around a native control) alone: on an
                // axis that does not scroll for the user it scrolls only a box
                // the target fits in, so a clipped control still falls back to
                // its label instead of showing a sliver.
                let user = self.style.overflow_axes(dom, node);
                axes.x = user.x || width <= self.client_width(dom, node);
                axes.y = user.y || height <= self.client_height(dom, node);
            }
            if axes.x || axes.y || explicit {
                // pyro: a snap area of a snapping container is scrolled to its snap position (its
                // `scroll-snap-align` in place of the alignment asked for), as Chromium does (CSS
                // Scroll Snap 1 § 4; WPT css-overflow `targeted-column-scroll-marker-selection-001`).
                let (offset_x, offset_y) = if script { self.snap_aligned_offsets(dom, node, el, offset_x, offset_y) } else { (offset_x, offset_y) };
                let left = (axes.x || explicit)
                    .then(|| offset_x(x, width, self.client_width(dom, node), self.scroll_left(node) as i64))
                    .flatten();
                let top = (axes.y || explicit)
                    .then(|| offset_y(y, height, self.client_height(dom, node), self.scroll_top(node) as i64))
                    .flatten();
                if self.element_scroll_to(dom, node, left, top) {
                    moved.push(ScrollTarget::Container(node));
                }
            }
            cur = flat_parent(node);
        }
        // Fixed boxes do not move with the window.
        if self.layout_box(dom, el).is_some_and(|b| b.fixed) {
            return moved;
        }
        let target = self.scroll_margin_rect(dom, el);
        // pyro: into the viewport's scrollport: less its classic scrollbar gutters (Chrome: WPT
        // css-overflow `scrollbar-gutter-scroll-into-view`).
        let [gl, gt, gr, gb] =
            self.synced_geometry_source(dom).and_then(|s| s.viewport_gutters()).unwrap_or([0.0; 4]);
        let left = offset_x(target.left() - gl, target.width, self.inner_width() as f64 - gl - gr, self.window_scroll.0 as i64);
        let top = offset_y(target.top() - gt, target.height, self.inner_height() as f64 - gt - gb, self.window_scroll.1 as i64);
        if self.window_scroll_to(dom, left, top) {
            moved.push(ScrollTarget::Viewport);
        }
        moved
    }

    // ----- getComputedStyle --------------------------------------------------

    /// The text a generated `::before` / `::after` of `el` adds to its
    /// accessible name (Playwright's `getPseudoContent`): see
    /// [`crate::style::StyleEngine::css_content`]; "" when there is none.
    pub fn pseudo_name_content(&self, dom: &Dom, el: NodeId, pseudo: &style::selector_parser::PseudoElement) -> String {
        // Playwright's `getCSSContent`: alternative text after `/` wins.
        let pseudo = match pseudo {
            style::selector_parser::PseudoElement::Before => crate::style::PseudoElement::Before,
            style::selector_parser::PseudoElement::After => crate::style::PseudoElement::After,
            _ => return String::new(),
        };
        self.style.css_content(dom, el, Some(pseudo)).unwrap_or_default()
    }

    /// `getComputedStyle(el, pseudo).getPropertyValue(name)`: the computed
    /// value in the pseudo-element's style (Stylo's, from the layout
    /// engine). A `::before` / `::after` no rule generates reads `content:
    /// none`, as in Chromium. Lengths are the computed values: pseudo boxes
    /// have no resolved geometry here. "" where the element has no style.
    pub fn computed_pseudo_style_value(
        &self,
        dom: &Dom,
        el: NodeId,
        pseudo: &style::selector_parser::PseudoElement,
        name: &str,
    ) -> String {
        if self.owned(dom, el).is_none() || !dom.is_connected(el).unwrap_or(false) {
            return String::new();
        }
        let Some(style) = self.style.stylo_pseudo_style(dom, el, pseudo, false) else { return String::new() };
        let name = resolved_property_name(name);
        // The animation and transition properties of a `::before`/`::after`, from the page's own
        // timing cascade (the engine's has them all `none`), as the element's are.
        let timing_pseudo = match pseudo {
            style::selector_parser::PseudoElement::Before => Some(crate::style::PseudoElement::Before),
            style::selector_parser::PseudoElement::After => Some(crate::style::PseudoElement::After),
            _ => None,
        };
        if let Some(value) = timing_pseudo.and_then(|p| self.style.timing_value_for(dom, el, Some(p), &name)) {
            return value;
        }
        // A running CSS transition of the pseudo-element: its value now (the layout engine samples
        // none), in the after-change style.
        if let Some(p) = timing_pseudo {
            match self.style.pseudo_transition_value(el, p, &name) {
                Some(crate::animation::TransitionValue::Text(v)) => return v,
                Some(crate::animation::TransitionValue::Style(style)) => {
                    return crate::stylo::serialize(&style, &name).unwrap_or_default();
                }
                None => {}
            }
        }
        let value = crate::stylo::serialize(&style, &name).unwrap_or_default();
        if name == "content" && pseudo.is_before_or_after() && value == "normal" {
            return "none".into();
        }
        value
    }

    pub fn typed_computed_value(&self, dom: &Dom, el: NodeId, name: &str) -> Option<String> {
        if !self.style.style_overridden() {
            match self.style.transition_value(el, name) {
                Some(crate::animation::TransitionValue::Text(value)) => return Some(value),
                Some(crate::animation::TransitionValue::Style(sample)) => {
                    return self.style.with_style_override(el, sample, || self.style.computed_value(dom, el, name)).flatten();
                }
                None => {}
            }
        }
        self.style.computed_value(dom, el, name)
    }

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
        let name = resolved_property_name(name);
        let name: &str = &name;
        if matches!(name, "page-orientation" | "size") { return String::new(); }
        // pyro: a running CSS transition's value now (the layout engine samples none), serialized
        // as this function serializes the after-change style with that value in it
        if !self.style.style_overridden() {
            match self.style.transition_value(el, name) {
                Some(crate::animation::TransitionValue::Text(v)) => return v,
                Some(crate::animation::TransitionValue::Style(style)) => {
                    if let Some(v) = self.style.with_style_override(el, style, || self.computed_style_value(dom, el, name)) {
                        return v;
                    }
                }
                None => {}
            }
        }
        let name = if matches!(name, "inline-size" | "block-size") {
            let horizontal = self.style.stylo_style(dom, el).is_none_or(|style| style.writing_mode.is_horizontal());
            if (name == "inline-size") == horizontal { "width" } else { "height" }
        } else {
            name
        };
        match name {
            "display" if self.style.has_display_none(dom, el) => return "none".into(),
            "display" => return self.style.computed_display(dom, el).unwrap_or_default(),
            "visibility" if self.style.has_visibility_hidden_inherited(dom, el) => return "hidden".into(),
            "pointer-events" if self.style.has_pointer_events_none(dom, el) => return "none".into(),
            "text-transform" => return self.style.text_transform(dom, el).keyword().into(),
            "overflow-x" | "overflow-y" | "overflow" => {
                let x = self.style.used_value(dom, el, "overflow-x");
                let y = self.style.used_value(dom, el, "overflow-y");
                let (x, y) = crate::style::computed_overflow_keywords(x.as_deref(), y.as_deref());
                return match name {
                    "overflow-x" => x,
                    "overflow-y" => y,
                    _ if x == y => x,
                    _ => format!("{x} {y}"),
                };
            }
            _ => {}
        }
        if custom {
            // A registered property no declaration sets has its initial value.
            return self
                .style
                .custom_property_value(dom, el, name)
                .or_else(|| self.style.registered_property(name)?.initial_value.clone())
                .unwrap_or_default();
        }
        if name == "font-size" {
            // The cascade (stylesheets, em/%/rem, keywords) when any author value applies; else the inline/attribute walk.
            return self
                .style
                .computed_value(dom, el, "font-size")
                .unwrap_or_else(|| format!("{}px", self.style.computed_font_size(dom, Some(el))));
        }
        if let Some(v) = self.style.timing_value(dom, el, name) {
            return v;
        }
        let declared = self.style.computed_value(dom, el, name);
        // Resolve percentages against the renderer's actual reference box, also
        // for a sampled transition. Keep the sampled style read-only: the live
        // document must not be restyled to an intermediate animation value.
        if matches!(name, "transform-origin" | "perspective-origin") || (name == "transform" && declared.as_deref().is_some_and(|value| value != "none")) {
            if let Some(value) = self.synced_geometry_source(dom).and_then(|source| {
                let style = self.style.stylo_style(dom, el)?;
                source.resolved_transform_value(crate::snapshot::render_key(el), name, &style)
            }) {
                return value;
            }
        }
        // pyro: a list keeps no function that does nothing (`perspective(none)`, an identity matrix: what
        // composing and interpolating lists leave), as Chromium's one resolved matrix shows none of them
        if name == "transform" {
            if let Some(value) = declared.as_deref().and_then(without_identity_transforms) {
                return value;
            }
        }
        // pyro: a grid container's template resolves to its used tracks, each listed (css-grid-1 §7.2.6), and a
        // subgrid's to `subgrid` with each of its lines' names (css-grid-2 §7.2.6.2): only its layout knows
        // them (`repeat(auto-fill, …)` counts, implicit tracks, how many tracks a subgrid spans)
        if matches!(name, "grid-template-columns" | "grid-template-rows")
            && (declared.as_deref().is_some_and(|value| value.starts_with("subgrid"))
                || self.style.computed_display(dom, el).is_some_and(|display| display.ends_with("grid")))
        {
            if let Some(value) = self
                .synced_geometry_source(dom)
                .and_then(|source| source.resolved_style_value(crate::snapshot::render_key(el), name))
                .filter(|value| !value.is_empty())
            {
                return value;
            }
        }
        if name == "width" || name == "height" {
            if !self.is_visible(dom, el) {
                // No box (display: none, or inside one): the computed value,
                // as Chromium reports it (`7px`, `50%`, `auto`).
                return declared.unwrap_or_else(|| "auto".into());
            }
            if self.style.style_overridden() && declared.as_deref().is_some_and(|v| v.starts_with("calc-size(")) {
                if let Some(size) = self.synced_geometry_source(dom).and_then(|source| {
                    let sampled = self.style.stylo_style(dom, el)?;
                    source.sampled_calc_size(crate::snapshot::render_key(el), name == "width", &sampled)
                }) {
                    return format!("{}px", crate::computed::fmt_num(f64::from(size)));
                }
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
        // pyro: an `auto` or percentage margin of a rendered box resolves to its used value (Blink
        // reports the used margin then; WPT css-flexbox `auto-margins-001`)
        if let (Some(side), Some(value)) = (margin_side(name), declared.as_deref()) {
            if value == "auto" || value.contains('%') || value.contains('(') {
                if let Some(q) = self.box_quads(dom, el) {
                    return format!("{}px", crate::computed::fmt_num(q.margin[side]));
                }
            }
        }
        if let Some(v) = declared.as_deref().and_then(|v| self.resolved_value(dom, el, name, v)) {
            return v;
        }
        if let Some(v) = declared {
            return v;
        }
        if let Some(v) = self.style.inline_style_value(dom, el, name) {
            return v;
        }
        default_property_value(dom, el, name).to_owned()
    }
}

/// The side index ([`TOP`], [`RIGHT`], [`BOTTOM`], [`LEFT`]) of a physical margin property.
fn margin_side(name: &str) -> Option<usize> {
    match name {
        "margin-top" => Some(TOP),
        "margin-right" => Some(RIGHT),
        "margin-bottom" => Some(BOTTOM),
        "margin-left" => Some(LEFT),
        _ => None,
    }
}

/// A computed transform list without its functions that do nothing (`perspective(none)`, an
/// identity `matrix()`/`matrix3d()`), when it has others; `None`: nothing to leave out.
fn without_identity_transforms(value: &str) -> Option<String> {
    const NOOPS: [&str; 3] = [
        "perspective(none)",
        "matrix(1, 0, 0, 1, 0, 0)",
        "matrix3d(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1)",
    ];
    if !NOOPS.iter().any(|n| value.contains(n)) {
        return None;
    }
    // The list's functions, split at the spaces outside parentheses.
    let mut items = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, b) in value.bytes().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b' ' if depth == 0 => {
                items.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    items.push(&value[start..]);
    let kept: Vec<&str> = items.iter().copied().filter(|item| !NOOPS.contains(item)).collect();
    (!kept.is_empty() && kept.len() < items.len()).then(|| kept.join(" "))
}

fn resolved_property_name(name: &str) -> Cow<'_, str> {
    if name.starts_with("--") {
        return Cow::Borrowed(name);
    }
    if name.starts_with('-') {
        if let Some(style::properties::PropertyId::NonCustom(property)) = crate::stylo::property_id(name) {
            return Cow::Borrowed(match property.unaliased().name() {
                "-webkit-transform" => "transform",
                "-webkit-perspective" => "perspective",
                canonical => canonical,
            });
        }
    }
    if name.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(name.to_ascii_lowercase())
    } else {
        Cow::Borrowed(name)
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
            "min-width" | "min-height" | "max-width" | "max-height" if value.contains("anchor-size(") => {
                self.synced_geometry_source(dom)?.resolved_style_value(crate::snapshot::render_key(el),name)
            }
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
            "top" | "bottom" | "left" | "right" if value == "auto" || value.contains('%') || value.contains("anchor(") || value.contains("anchor-size(") => {
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
                // A sticky box's percentages resolve against the content box of its
                // nearest scroll container, if it has one (as Chrome does).
                let mut scroller = None;
                if position == Position::Sticky {
                    let mut a = Some(parent);
                    while let Some(e) = a {
                        let scrolls = style.computed_value(dom, e, "overflow-x").is_some_and(|o| !matches!(o.as_str(), "visible" | "clip"))
                            || style.computed_value(dom, e, "overflow-y").is_some_and(|o| !matches!(o.as_str(), "visible" | "clip"));
                        if scrolls {
                            scroller = Some(e);
                            break;
                        }
                        a = dom.parent_element(e).ok().flatten();
                    }
                }
                let b = scroller.unwrap_or(parent);
                let basis = if horizontal {
                    self.client_width(dom, b) - length(b, "padding-left") - length(b, "padding-right")
                } else {
                    self.client_height(dom, b) - length(b, "padding-top") - length(b, "padding-bottom")
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
                // A specified inset (percentage or calc()) is the computed value
                // absolutized against the containing block, also when the box is
                // over-constrained; only `auto` reports the box's used position.
                if value != "auto" && !value.contains("anchor(") && !value.contains("anchor-size(") {
                    let basis = if horizontal { block.width } else { block.height };
                    let (len, percent) =
                        crate::computed::resolve_len(value, &crate::computed::LengthContext::fonts(16.0, 16.0))?;
                    return px(len + percent / 100.0 * basis);
                }
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
fn nearest_scroll_offset(start: f64, size: f64, viewport: f64, current: i64) -> Option<i32> {
    let end = start + size;
    if (start >= 0.0 && end <= viewport) || (start <= 0.0 && end >= viewport) {
        return None;
    }
    let delta =
        if (start < 0.0 && size <= viewport) || (end > viewport && size > viewport) { start } else { end - viewport };
    Some((current as f64 + delta).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
}

/// A `ScrollLogicalPosition` (`scrollIntoView` `block` / `inline`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScrollAlign {
    Start,
    Center,
    End,
    Nearest,
}

impl ScrollAlign {
    /// The `ScrollLogicalPosition` enum value `s`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "start" => ScrollAlign::Start,
            "center" => ScrollAlign::Center,
            "end" => ScrollAlign::End,
            "nearest" => ScrollAlign::Nearest,
            _ => return None,
        })
    }

    fn offset_fn(self) -> fn(f64, f64, f64, i64) -> Option<i32> {
        match self {
            ScrollAlign::Start => start_scroll_offset,
            ScrollAlign::Center => centered_scroll_offset,
            ScrollAlign::End => end_scroll_offset,
            ScrollAlign::Nearest => nearest_scroll_offset,
        }
    }
}

fn start_scroll_offset(start: f64, _size: f64, _viewport: f64, current: i64) -> Option<i32> {
    Some((current as f64 + start).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
}

fn end_scroll_offset(start: f64, size: f64, viewport: f64, current: i64) -> Option<i32> {
    Some((current as f64 + start + size - viewport).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
}

fn centered_scroll_offset(start: f64, size: f64, viewport: f64, current: i64) -> Option<i32> {
    Some((current as f64 + start + size / 2.0 - viewport / 2.0).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
}

// Blink's center-if-needed on one axis: a fully visible box, or one covering
// the scrollport, stays; a hidden box is centered; a partly visible one is
// brought in by its nearest edge.
fn center_if_needed_offset(start: f64, size: f64, viewport: f64, current: i64) -> Option<i32> {
    let end = start + size;
    if end <= 0.0 || start >= viewport {
        return Some(
            (current as f64 + start + size / 2.0 - viewport / 2.0).round().clamp(i32::MIN as f64, i32::MAX as f64)
                as i32,
        );
    }
    nearest_scroll_offset(start, size, viewport, current)
}

// Focusing a box already visible on an axis must not perturb that scroll.
fn focus_scroll_offset(start: f64, size: f64, viewport: f64, current: i64) -> Option<i32> {
    if start >= 0.0 && start + size <= viewport {
        return None;
    }
    if start <= 0.0 && start + size >= viewport {
        return None;
    }
    Some((current as f64 + start + size / 2.0 - viewport / 2.0).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
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
        *ON.get_or_init(|| std::env::var("PYRO_FLOW_STATS").is_ok_and(|v| v == "1"))
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

#[cfg(test)]
mod hit_index_policy {
    use super::*;
    use crate::geometry::HitVisit;
    use std::cell::Cell;
    use std::time::Duration;

    /// Boxes of 30 x 12 px in rows of 25, with a descent that reads only
    /// the box at the point. Each stall is served once: the thread sleeps
    /// inside the next descent or the next read of every box, as one
    /// descheduled there would.
    struct Tiles {
        boxes: Vec<(u64, ExternalBox)>,
        stall_descent: Cell<Option<Duration>>,
        stall_geometry: Cell<Option<Duration>>,
        style: style::servo_arc::Arc<style::properties::ComputedValues>,
    }

    impl GeometrySource for Tiles {
        fn sync(&self, _dom: &Dom, _layout: &Layout) {}
        fn element_box(&self, key: u64) -> Option<ExternalBox> {
            self.boxes.iter().find(|(k, _)| *k == key).map(|(_, b)| *b)
        }
        fn geometry(&self) -> Option<ExternalGeometry> {
            if let Some(stall) = self.stall_geometry.take() {
                std::thread::sleep(stall);
            }
            Some(ExternalGeometry {
                boxes: self.boxes.clone(),
                document_width: 800.0,
                document_height: 600.0,
                ..Default::default()
            })
        }
        fn hit_candidates(
            &self,
            probe: &HitProbe,
            _offset: &dyn Fn(u64) -> (f64, f64),
            _shifted: &[u64],
        ) -> Option<Vec<HitVisit>> {
            if let Some(stall) = self.stall_descent.take() {
                std::thread::sleep(stall);
            }
            let at = (probe.y / 12.0) as usize * 25 + (probe.x / 30.0) as usize;
            Some(
                self.boxes
                    .get(at)
                    .map(|&(key, bounds)| HitVisit { key, order: at as u32, bounds, hit: true })
                    .into_iter()
                    .collect(),
            )
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

    fn tiles(count: usize) -> (Dom, Vec<NodeId>, Rc<Tiles>, Layout) {
        use crate::parse::{parse_document, DomRef};
        let html = format!(
            "<!doctype html><body>{}</body>",
            (0..count).map(|i| format!("<div id=c{i}></div>")).collect::<String>()
        );
        let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom_ref, "http://test/", &html);
        let dom = Rc::try_unwrap(dom_ref).ok().unwrap().into_inner();
        let nodes: Vec<_> =
            (0..count).map(|i| dom.get_element_by_id(doc, &format!("c{i}")).unwrap().unwrap()).collect();
        let boxes = nodes
            .iter()
            .enumerate()
            .map(|(i, &node)| {
                let bounds = ExternalBox {
                    x: (i % 25 * 30) as f64,
                    y: (i / 25 * 12) as f64,
                    width: 30.0,
                    height: 12.0,
                    ..Default::default()
                };
                (crate::snapshot::render_key(node), bounds)
            })
            .collect();
        let source = Rc::new(Tiles {
            boxes,
            stall_descent: Cell::new(None),
            stall_geometry: Cell::new(None),
            style: style::properties::ComputedValues::initial_values_with_font_override(
                style::properties::style_structs::Font::initial_values(),
            ),
        });
        let mut layout = Layout::with_viewport(doc, Viewport::new(800, 600));
        layout.set_geometry_source(Some(source.clone()));
        (dom, nodes, source, layout)
    }

    /// A large page that changes between pointer events is answered by
    /// descents. One descent that took long for a reason of its own (the
    /// thread was descheduled inside it) must not make every later pointer
    /// event read all the boxes of the page.
    #[test]
    fn one_slow_descent_does_not_index_a_large_changing_page_at_every_event() {
        let (mut dom, nodes, source, layout) = tiles(20_000);
        let target = nodes[421];
        let hit = |dom: &Dom| layout.element_from_point(dom, 635.0, 198.0);
        // The first index: two tests of one layout.
        assert_eq!(hit(&dom), Some(target));
        assert_eq!(hit(&dom), Some(target));
        assert_eq!(layout.hit_index_stats(&dom).0, 1);
        let mut round = 0;
        let mut change_and_hit = |dom: &mut Dom| {
            round += 1;
            dom.set_attribute(target, "data-round", &round.to_string()).unwrap();
            assert_eq!(layout.element_from_point(dom, 635.0, 198.0), Some(target));
        };
        for _ in 0..10 {
            change_and_hit(&mut dom);
        }
        let before = layout.hit_index_stats(&dom).0;
        source.stall_descent.set(Some(Duration::from_millis(1500)));
        for _ in 0..12 {
            change_and_hit(&mut dom);
        }
        let built = layout.hit_index_stats(&dom).0 - before;
        assert!(built <= 2, "{built} indexes of 20000 boxes built for 12 pointer events after one slow descent");
    }

    /// A small page whose click handler writes to the DOM is indexed at
    /// each click. One index that took long to build (the first: cold, or
    /// the thread was descheduled) must not leave the page to descents for
    /// good.
    #[test]
    fn one_slow_build_does_not_leave_a_small_page_without_an_index_for_good() {
        let (mut dom, nodes, source, layout) = tiles(8);
        let target = nodes[3];
        source.stall_geometry.set(Some(Duration::from_millis(4)));
        let mut builds = 0;
        for round in 0..200_000 {
            dom.set_attribute(target, "data-round", &round.to_string()).unwrap();
            assert_eq!(layout.element_from_point(&dom, 95.0, 5.0), Some(target));
            assert_eq!(layout.element_from_point(&dom, 95.0, 5.0), Some(target));
            builds = layout.hit_index_stats(&dom).0;
            if builds >= 3 {
                break;
            }
        }
        assert!(builds >= 3, "{builds} index(es) built: the slow one's price was never measured again");
    }
}
