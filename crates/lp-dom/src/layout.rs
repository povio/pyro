//! Deterministic "faux layout": a port of upstream's renderless geometry
//! (lightpanda-io/browser @ 2ad38773). It is an approximation by design:
//! every element is a box stacked 5px per preceding node in tree order and
//! laid out left to right among its visible element siblings. The formulas
//! below are upstream's; each function cites the file:line it ports.
//!
//! Ported:
//! - `isVisible` / `checkVisibility` (webapi/Element.zig:1570-1589) on top
//!   of the cascade in [`crate::style`] (StyleManager.zig).
//! - `getElementAxis` (Element.zig:1602-1634): declared CSS size (inline
//!   only for html/body roots), roots 1920px wide and as tall as the
//!   document extent, `<img>`/`<iframe>` `width`/`height` attributes, else 5px.
//! - `isRootContainer` (1639-1649), `clientWidth/Height` (1655-1668),
//!   `viewportAxis` (1677-1692, quirks-aware), `rootScrollSize` (1671-1675),
//!   `boxAxis` (1695-1715), `contentAxis` (1881-1912) with the greedy
//!   `LineWrap` text measure (text_measure.zig:40-142).
//! - `getBoundingClientRect` / `getClientRects`
//!   (`boundingClientRectValues[ForVisible]`, 1724-1751): zero / empty when
//!   not rendered; viewport-relative (minus window scroll).
//! - `scrollTop/Left` get/set, `scrollTo/By`, `writeScroll`, `clampScroll`,
//!   `scrollExtent`, `scrollContainer`, `containsOverscroll`,
//!   `scrollWidth/Height` (1757-1858, 2194-2270).
//! - `offsetWidth/Height/Top/Left/Parent`, `clientTop/Left` (1915-1997).
//! - `calculateDocumentPosition` / `countSubtreeNodes` (2023-2053) and
//!   `horizontalPosition` (2061-2090) with inline `transform` translateX
//!   (webapi/CSS.zig:31-67).
//! - `scrollIntoView` / `scrollIntoViewIfNeeded` (2146-2168).
//! - `Document.extent` / `scrollSize` (webapi/Document.zig:490-556),
//!   `elementFromPoint`, `elementFromVerticalPoint`, `elementsFromPoint`
//!   (Document.zig:953-1047).
//! - Window `scrollX/Y`, `scrollTo`, `scrollBy`, `scrollLimit`,
//!   `innerWidth/Height` (webapi/Window.zig:758-860).
//! - Wheel latching to a scroll container (`wheel`'s scroll part,
//!   `scrollAxis`, `deltaToScroll`, frame/user_input.zig:553-615).
//! - `getComputedStyle(el).getPropertyValue(name)` resolution
//!   (webapi/css/CSSStyleDeclaration.zig:76-151, 203-212, 965-1103).
//!
//! Deviation: element scroll offsets shift descendants' client rects and hit
//! testing (`ancestor_scroll_offset`), and `scrollIntoView[IfNeeded]` reveals
//! the element inside already-scrolled ancestors; upstream applies only the
//! window scroll.
//! Automatic non-root heights also enclose normal-flow child boxes at their
//! synthetic tree-order offsets. The upstream direct-child sum alone can
//! place nested children outside an automatic ancestor's box. Explicit sizes,
//! positions remain synthetic. Percentage widths resolve through percentage
//! ancestors to a definite parent width; other width approximations remain.
//! This is not a CSS block/flex layout implementation.
//!
//! Not ported (honest omissions):
//! - `scroll` / `scrollend` event scheduling and throttling (Element.zig:
//!   2277-2357, Window.zig:796-845): scroll writers return whether anything
//!   moved and the caller dispatches. Likewise the `wheel` DOM event and
//!   MediaQueryList `change` events.
//! - IntersectionObserver / ResizeObserver scheduling.
//! - Slot assignment in the flat tree (lp-dom keeps no assigned-slot state):
//!   slotted nodes inherit from their DOM parent, not their slot.
//! - Multiple frames: one [`Layout`] per document; elements whose node
//!   document is another one (template contents, DOMParser documents) have no
//!   owner frame and report "not rendered", as upstream does without a frame.
//! - CSSOM value normalization of inline values (`0` -> `0px`, leading
//!   zeros); values are returned as written.
//! - Selectors `crate::select` cannot parse (`:invalid`, `:valid`,
//!   `:popover-open`, `:modal`, ...) drop their rule, where upstream's own
//!   selector engine may accept them.
//!
//! Upstream quirks kept on purpose: the root's width is 1920px whatever the
//! viewport (Element.zig:1619); detached elements are "visible" (their flat
//! tree simply ends); `checkVisibility()` ignores `content-visibility`.

use cssparser::{Parser, Token};
use markup5ever::{local_name, ns};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::rc::Rc;

use crate::dom::Dom;
use crate::flow::{FlowBox, FlowResult};
use crate::media::Viewport;
use crate::node::{DocumentKind, NodeId, NodeKind, NIL};
use crate::style::{CheckVisibilityOptions, ScrollAxes, StyleEngine};

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Axis {
    Width,
    Height,
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

/// Faux layout state of one document: its style engine (which owns the
/// viewport), the window scroll offset and per-element scroll offsets.
pub struct Layout {
    style: StyleEngine,
    window_scroll: (u32, u32),
    scroll_generation: u64,
    element_scroll: FxHashMap<NodeId, (u32, u32)>,
    /// CSS Scroll Anchoring: the layout each vertically scrolled element
    /// was last settled in (shared with the flow cache), to select its
    /// anchor in when the layout next changes.
    scroll_anchors: FxHashMap<NodeId, Rc<FlowResult>>,
    automatic_heights: RefCell<AutomaticHeights>,
    geometry: Geometry,
    flow_cache: RefCell<Option<FlowCache>>,
    measures: RefCell<Measures>,
}

/// Subtree measurements kept across flow passes, with what they are valid
/// for: the DOM version and viewport of the last pass.
#[derive(Default)]
struct Measures {
    cache: crate::flow::MeasureCache,
    version: Option<u64>,
    viewport: (u32, u32),
}

/// Which geometry the layout getters report (docs/dom/FLOW_LAYOUT.md). The
/// flow layout is opt-in until its parity with Chromium is reviewed.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Geometry {
    /// Upstream's renderless "faux layout".
    #[default]
    Approximate,
    /// `crate::flow` block/inline layout with positioning (Stage 1).
    Flow,
}

impl Geometry {
    /// The process-wide opt-in: `LIGHTNINGPANDA_GEOMETRY=flow` selects the flow
    /// layout for new documents; anything else keeps the approximate one.
    pub fn from_env() -> Geometry {
        match std::env::var("LIGHTNINGPANDA_GEOMETRY") {
            Ok(v) if v.eq_ignore_ascii_case("flow") => Geometry::Flow,
            _ => Geometry::Approximate,
        }
    }
}

struct FlowCache {
    stamp: (u64, u64, u32, u32),
    viewport: (u32, u32),
    result: Rc<FlowResult>,
}

#[derive(Default)]
struct AutomaticHeights {
    stamp: Option<(u64, u64, u32, u32)>,
    values: FxHashMap<NodeId, f64>,
}

const NODE_HEIGHT: f64 = 5.0;
const DEFAULT_AXIS: f64 = 5.0;
const ROOT_WIDTH: f64 = 1920.0;

impl Layout {
    pub fn new(doc: NodeId) -> Self {
        Self::with_style(StyleEngine::new(doc))
    }

    pub fn with_viewport(doc: NodeId, viewport: Viewport) -> Self {
        Self::with_style(StyleEngine::with_viewport(doc, viewport))
    }

    pub fn with_style(style: StyleEngine) -> Self {
        Layout {
            style,
            window_scroll: (0, 0),
            scroll_generation: 0,
            element_scroll: FxHashMap::default(),
            scroll_anchors: FxHashMap::default(),
            automatic_heights: RefCell::default(),
            geometry: Geometry::default(),
            flow_cache: RefCell::new(None),
            measures: RefCell::default(),
        }
    }

    /// Selects the geometry the getters report.
    pub fn set_geometry(&mut self, geometry: Geometry) {
        self.geometry = geometry;
        self.flow_cache.borrow_mut().take();
    }

    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    fn flow_mode(&self) -> bool {
        self.geometry == Geometry::Flow
    }

    /// The flow layout of the document, recomputed when the DOM, styles,
    /// focus/hover or viewport change.
    pub fn flow(&self, dom: &Dom) -> Rc<FlowResult> {
        let stamp = self.style.observation_stamp(dom);
        let v = self.viewport();
        let viewport = (v.width, v.height);
        let previous = match self.flow_cache.borrow().as_ref() {
            Some(c) if c.stamp == stamp && c.viewport == viewport => return c.result.clone(),
            Some(c) => Some((c.stamp, c.viewport)),
            None => None,
        };
        let started = flow_stats::enabled().then(std::time::Instant::now);
        let mut measures = self.measures.borrow_mut();
        self.invalidate_measures(dom, &mut measures, viewport);
        let result = Rc::new(crate::style::with_pass_memo(|| {
            crate::flow::layout_cached(
                dom,
                self.document(),
                &self.style,
                v.width as f64,
                v.height as f64,
                &mut measures.cache,
            )
        }));
        measures.version = Some(dom.version());
        measures.viewport = viewport;
        // Entries of removed elements are never served again; bound them.
        if measures.cache.len() > 8 * result.boxes.len().max(64) {
            measures.cache.clear();
        }
        drop(measures);
        if let Some(t) = started {
            flow_stats::record(previous, (stamp, viewport), t.elapsed());
        }
        *self.flow_cache.borrow_mut() = Some(FlowCache { stamp, viewport, result: result.clone() });
        result
    }

    /// Drops the measurements a pass cannot reuse: those of every element
    /// restyled or changed since the last pass (DOM journal: attributes,
    /// form state, image natural sizes, children, text) and of their
    /// ancestors, whose measurements include them; all of them after a full
    /// restyle, a gap in the journal, an unknown change, a viewport change
    /// or with shadow roots (flat-tree ancestors).
    fn invalidate_measures(&self, dom: &Dom, m: &mut Measures, viewport: (u32, u32)) {
        use crate::dom::Change;
        let restyled = self.style.take_restyled(dom);
        let changes = match (m.version, restyled.as_ref()) {
            (Some(v), Some(_)) if m.viewport == viewport && dom.shadow_root_count() == 0 => {
                dom.changes_since(v).map(|c| c.collect::<Vec<_>>())
            }
            _ => None,
        };
        let (Some(restyled), Some(changes)) = (restyled, changes) else {
            m.cache.clear();
            return;
        };
        let mut dirty = rustc_hash::FxHashSet::default();
        let mut mark = |idx: u32| {
            let mut cur = idx;
            while cur != crate::node::NIL && dom.slot_live(cur) {
                if !dirty.insert(dom.id_of(cur)) {
                    break;
                }
                cur = dom.n(cur).parent;
            }
        };
        for n in restyled {
            if let Ok(idx) = dom.check(n) {
                mark(idx);
            }
        }
        for c in changes {
            match c {
                Change::Attr(i, ..) | Change::State(i) => mark(i),
                Change::Inserted(p, n) => {
                    mark(p);
                    mark(n);
                }
                Change::Removed(p, ..) => mark(p),
                Change::Text(t) => {
                    if dom.slot_live(t) {
                        mark(dom.n(t).parent);
                    }
                }
                Change::Pseudo => {}
                Change::Unknown(_) => {
                    m.cache.clear();
                    return;
                }
            }
        }
        m.cache.invalidate(&dirty);
    }

    fn flow_box(&self, dom: &Dom, el: NodeId) -> Option<FlowBox> {
        self.flow(dom).get(el).copied()
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

    /// `getElementAxis` (Element.zig:1602-1634).
    pub fn element_axis(&self, dom: &Dom, el: NodeId, axis: Axis) -> AxisState {
        match dom.check(el) {
            Ok(i) if dom.n(i).is_element() => self.element_axis_idx(dom, i, axis),
            _ => AxisState { value: 0.0, explicit: false },
        }
    }

    fn element_axis_idx(&self, dom: &Dom, idx: u32, axis: Axis) -> AxisState {
        let id = dom.id_of(idx);
        let root = self.is_root_idx(dom, idx);
        let owned = self.owned(dom, id).is_some();
        let width = axis == Axis::Width;
        if owned {
            // Roots take only an inline size: a sheet's `height: 100vh` on
            // body would shrink the box every synthetic position must fit in.
            let size =
                if root { self.style.inline_size(dom, id, width) } else { self.style.declared_size(dom, id, width) };
            if let Some(v) = size {
                return AxisState { value: v, explicit: true };
            }
            if width && !root {
                if let Some(v) = self.percentage_width(dom, idx) {
                    return AxisState { value: v, explicit: true };
                }
            }
        }
        if root {
            let value = match axis {
                Axis::Width => ROOT_WIDTH,
                Axis::Height => {
                    if owned {
                        self.document_extent(dom).1
                    } else {
                        0.0
                    }
                }
            };
            return AxisState { value, explicit: false };
        }
        // Presentational attributes lose to CSS sizes.
        if matches!(self.html_local(dom, idx), Some("img" | "iframe")) {
            let name = if width { "width" } else { "height" };
            if let Some(v) = dom.n(idx).element().and_then(|e| e.attr(name)).and_then(|a| a.parse::<f64>().ok()) {
                return AxisState { value: v, explicit: true };
            }
        }
        AxisState { value: DEFAULT_AXIS, explicit: false }
    }

    /// Resolve a chain of percentage widths against a definite parent width.
    /// Do not derive that basis from children: their percentage size would
    /// make an automatic/intrinsic parent circular. Positioned containing
    /// blocks and percentages through automatic widths remain unsupported.
    fn percentage_width(&self, dom: &Dom, mut idx: u32) -> Option<f64> {
        let own = self.style.computed(dom, dom.id_of(idx))?;
        if own.width.unit != crate::style::LengthUnit::Percentage || own.absolutely_positioned {
            return None;
        }
        let mut factor = own.width.value as f64 / 100.0;
        idx = crate::style::flat_tree_parent_element(dom, idx)?;
        loop {
            let id = dom.id_of(idx);
            if self.is_root_idx(dom, idx) {
                // Preserve the existing root policy: only inline dimensions
                // define a root box, not stylesheet dimensions.
                let result = self.style.inline_size(dom, id, true)? * factor;
                return result.is_finite().then_some(result);
            }
            let style = self.style.computed(dom, id)?;
            if style.absolutely_positioned {
                return None;
            }
            if style.width.unit != crate::style::LengthUnit::Percentage {
                let size = self.style.declared_size(dom, id, true)?;
                let result = size * factor;
                return result.is_finite().then_some(result);
            }
            factor *= style.width.value as f64 / 100.0;
            if !factor.is_finite() {
                return None;
            }
            idx = crate::style::flat_tree_parent_element(dom, idx)?;
        }
    }

    /// `boxAxis` (Element.zig:1695-1715). Callers check visibility.
    pub fn box_axis(&self, dom: &Dom, el: NodeId, axis: Axis) -> f64 {
        match dom.check(el) {
            Ok(i) if dom.n(i).is_element() => self.box_axis_idx(dom, i, axis),
            _ => 0.0,
        }
    }

    fn box_axis_idx(&self, dom: &Dom, idx: u32, axis: Axis) -> f64 {
        let own = self.element_axis_idx(dom, idx, axis);
        if own.explicit || self.is_root_idx(dom, idx) {
            return own.value;
        }
        let id = dom.id_of(idx);
        if axis == Axis::Width && (self.owned(dom, id).is_none() || self.style.width_from_containing_block(dom, id)) {
            return own.value;
        }
        if axis == Axis::Height && self.owned(dom, id).is_some() {
            return self.automatic_height(dom, idx);
        }
        own.value.max(self.content_axis_idx(dom, idx, axis))
    }

    /// Enclose normal-flow children at their actual synthetic positions.
    /// Summing only direct-child fallback sizes can leave a nested child
    /// outside every automatic ancestor. Positions and explicit sizes remain
    /// unchanged; this is not a CSS block/flex layout implementation.
    fn automatic_height(&self, dom: &Dom, root: u32) -> f64 {
        let stamp = self.style.observation_stamp(dom);
        let mut cache = self.automatic_heights.borrow_mut();
        if cache.stamp != Some(stamp) {
            cache.values.clear();
            cache.stamp = Some(stamp);
        }
        let id = dom.id_of(root);
        if let Some(&height) = cache.values.get(&id) {
            return height;
        }
        // One iterative subtree traversal, followed by a bottom-up pass.
        // Count even hidden/text nodes, exactly as document_position does.
        // Cached descendants make subsequent rectangle queries constant-time
        // for height, without rescanning the document or recursive Rust calls.
        let mut nodes = Vec::new();
        let mut positions = FxHashMap::default();
        let mut node = root;
        let mut position = 0.0;
        while node != NIL {
            if dom.n(node).is_element() {
                nodes.push(node);
                positions.insert(node, position);
            }
            position += NODE_HEIGHT;
            node = dom.following_in(node, root);
        }
        for node in nodes.into_iter().rev() {
            let id = dom.id_of(node);
            if cache.values.contains_key(&id) {
                continue;
            }
            let own = self.element_axis_idx(dom, node, Axis::Height);
            let mut height = own.value;
            if !own.explicit && !self.is_root_idx(dom, node) {
                height = height.max(self.content_axis_idx(dom, node, Axis::Height));
                for child in dom.child_idx_iter(node) {
                    let child_id = dom.id_of(child);
                    let Some(style) = self.style.computed(dom, child_id) else { continue };
                    if style.in_display_none || style.absolutely_positioned || style.floated {
                        continue;
                    }
                    if let Some(&child_height) = cache.values.get(&child_id) {
                        height = height.max(positions[&child] - positions[&node] + child_height);
                    }
                }
            }
            cache.values.insert(id, height);
        }
        cache.values[&dom.id_of(root)]
    }

    /// `contentAxis` (Element.zig:1881-1912): direct child elements laid end
    /// to end (width) or stacked (height); text only adds height, wrapped at
    /// an explicit width.
    pub fn content_axis(&self, dom: &Dom, el: NodeId, axis: Axis) -> f64 {
        match dom.check(el) {
            Ok(i) if dom.n(i).is_element() => self.content_axis_idx(dom, i, axis),
            _ => 0.0,
        }
    }

    fn content_axis_idx(&self, dom: &Dom, idx: u32, axis: Axis) -> f64 {
        let id = dom.id_of(idx);
        if self.owned(dom, id).is_none() {
            return 0.0;
        }
        let mut wrap = None;
        if axis == Axis::Height {
            let width = self.element_axis_idx(dom, idx, Axis::Width);
            if width.explicit {
                wrap = Some(LineWrap::new(width.value, self.style.computed_font_size(dom, Some(id))));
            }
        }
        let mut total = 0.0;
        for c in dom.child_idx_iter(idx) {
            match &dom.n(c).kind {
                NodeKind::Element(_) => {
                    if !self.style.has_display_none(dom, dom.id_of(c)) {
                        total += self.element_axis_idx(dom, c, axis).value;
                    }
                }
                NodeKind::Text(t) => {
                    if let Some(w) = wrap.as_mut() {
                        w.add(t);
                    }
                }
                _ => {}
            }
        }
        if let Some(w) = wrap {
            // Whole pixels, like scroll offsets
            total += w.height().ceil();
        }
        total
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
        if self.flow_mode() {
            return self.flow_box(dom, el).map_or(0.0, |b| match axis {
                Axis::Width => b.client_width,
                Axis::Height => b.client_height,
            });
        }
        self.box_axis(dom, el, axis)
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
        if self.flow_mode() {
            if let Some(size) = self.root_scroll_size(dom, el, axis) {
                return size;
            }
            return self.flow_box(dom, el).map_or(0.0, |b| match axis {
                Axis::Width => b.scroll_width.max(b.client_width),
                Axis::Height => b.scroll_height.max(b.client_height),
            });
        }
        let own = self.element_axis(dom, el, axis).value;
        if self.is_root_container(dom, el) {
            return self.root_scroll_size(dom, el, axis).unwrap_or(own);
        }
        own.max(self.content_axis(dom, el, axis)).max(self.box_axis(dom, el, axis))
    }

    /// `scrollExtent` (Element.zig:1845-1858): `None` where no limit can be
    /// proven, which leaves the offset unbounded.
    fn scroll_extent(&self, dom: &Dom, el: NodeId, axis: Axis) -> Option<f64> {
        if self.flow_mode() && !self.is_root_container(dom, el) {
            // Only scroll containers scroll; others stay at 0 (as in Chrome).
            let clips = self.style.overflow_clip_axes(dom, el);
            if !(if axis == Axis::Width { clips.x } else { clips.y }) {
                return Some(0.0);
            }
            let client = self.client_axis(dom, el, axis);
            return Some((self.scroll_axis_size(dom, el, axis) - client).max(0.0));
        }
        if self.is_root_container(dom, el) || !self.element_axis(dom, el, axis).explicit {
            return None;
        }
        let client = self.client_axis(dom, el, axis);
        let content = self.scroll_axis_size(dom, el, axis);
        if content <= client {
            return None;
        }
        Some(content - client)
    }

    /// `offsetWidth` / `offsetHeight` (Element.zig:1915-1927).
    pub fn offset_width(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        if self.flow_mode() {
            return self.flow_box(dom, el).map_or(0.0, |b| b.rect.width);
        }
        self.box_axis(dom, el, Axis::Width)
    }

    pub fn offset_height(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        if self.flow_mode() {
            return self.flow_box(dom, el).map_or(0.0, |b| b.rect.height);
        }
        self.box_axis(dom, el, Axis::Height)
    }

    /// Flow mode `offsetTop`/`offsetLeft`: border-box offset from the offset
    /// parent's border box (document coordinates for the body/none).
    fn flow_offset(&self, dom: &Dom, el: NodeId) -> (f64, f64) {
        let Some(b) = self.flow_box(dom, el) else { return (0.0, 0.0) };
        let parent = self
            .offset_parent(dom, el)
            .filter(|&p| !self.is_root_container(dom, p))
            .and_then(|p| self.flow_box(dom, p));
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
        if self.flow_mode() {
            return self.flow_offset(dom, el).1;
        }
        self.document_position(dom, el)
    }

    /// `offsetLeft` (Element.zig:1936-1941).
    pub fn offset_left(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        if self.flow_mode() {
            return self.flow_offset(dom, el).0;
        }
        self.horizontal_position(dom, el)
    }

    /// `clientTop` / `clientLeft`: no borders in the faux layout
    /// (Element.zig:1989-1997).
    pub fn client_top(&self) -> f64 {
        0.0
    }

    pub fn client_left(&self) -> f64 {
        0.0
    }

    /// `offsetParent` (Element.zig:1943-1987). `position` is read from the
    /// inline style only, like upstream's `positionStyle`.
    pub fn offset_parent(&self, dom: &Dom, el: NodeId) -> Option<NodeId> {
        if !dom.is_connected(el).ok()? || !self.is_visible(dom, el) {
            return None;
        }
        let idx = dom.check(el).ok()?;
        if matches!(self.html_local(dom, idx), Some("html" | "body")) {
            return None;
        }
        let self_position = self.position_style(dom, el);
        if self_position == "fixed" {
            return None;
        }
        let self_static = self_position.is_empty() || self_position == "static";
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
            let position = self.position_style(dom, dom.id_of(node));
            if !position.is_empty() && position != "static" {
                return Some(dom.id_of(node));
            }
            if self_static && matches!(tag, Some("td" | "th" | "table")) {
                return Some(dom.id_of(node));
            }
            node = dom.n(node).parent;
        }
        None
    }

    fn position_style(&self, dom: &Dom, el: NodeId) -> String {
        self.style.inline_declared_value(dom, el, "position").map(|(v, _)| v).unwrap_or_default()
    }

    // ----- positions ---------------------------------------------------------

    /// `calculateDocumentPosition` (Element.zig:2023-2043): 5px per node
    /// preceding `node` in tree order (left side of the tree only).
    pub fn document_position(&self, dom: &Dom, node: NodeId) -> f64 {
        let Ok(mut cur) = dom.check(node) else { return 0.0 };
        let mut position = 0.0;
        loop {
            let parent = dom.n(cur).parent;
            if parent == NIL {
                break;
            }
            for s in dom.child_idx_iter(parent) {
                if s == cur {
                    break;
                }
                position += count_subtree_nodes(dom, s);
            }
            position += 1.0;
            cur = parent;
        }
        position * NODE_HEIGHT
    }

    /// `horizontalPosition` (Element.zig:2061-2090): to the right of the
    /// visible element siblings before it at every level, shifted by inline
    /// `transform` translateX of itself and its ancestors.
    pub fn horizontal_position(&self, dom: &Dom, el: NodeId) -> f64 {
        let Some(mut cur) = self.owned(dom, el) else { return 0.0 };
        let mut x = self.translate_x(dom, el);
        loop {
            let parent = dom.n(cur).parent;
            if parent == NIL {
                break;
            }
            if dom.n(parent).is_element() {
                x += self.translate_x(dom, dom.id_of(parent));
            }
            for s in dom.child_idx_iter(parent) {
                if s == cur {
                    break;
                }
                if dom.n(s).is_element() && !self.style.has_display_none(dom, dom.id_of(s)) {
                    x += self.element_axis_idx(dom, s, Axis::Width).value;
                }
            }
            cur = parent;
        }
        x
    }

    fn translate_x(&self, dom: &Dom, el: NodeId) -> f64 {
        match self.style.inline_declared_value(dom, el, "transform") {
            Some((v, _)) => parse_translate_x(&v),
            None => 0.0,
        }
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

    /// A text node's line-box rect (the union of its fragments) in client
    /// coordinates, in flow geometry; `None` otherwise or when it is not laid
    /// out. Range geometry over text uses it (Chromium reports text boxes,
    /// even inside a `display: contents` parent that has no box).
    pub fn text_rect(&self, dom: &Dom, text: NodeId) -> Option<Rect> {
        if !self.flow_mode() {
            return None;
        }
        let flow = self.flow(dom);
        let &(r, fixed) = flow.texts.get(&text)?;
        let (dx, dy) = self.ancestor_scroll_offset(dom, text);
        let (kx, ky) = self.sticky_shift(dom, &flow, text);
        let (sx, sy) = if fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
        Some(Rect { x: r.x - sx - dx + kx, y: r.y - sy - dy + ky, width: r.width, height: r.height })
    }

    /// The caret position (text node, UTF-16 offset) under a viewport
    /// point, in flow geometry, within `within` (the element under the
    /// point, as Chromium resolves positions in the hit element): the
    /// character boundary nearest to the point in the visible word nearest
    /// to it on its line (else on the nearest line), and whether the point
    /// is on that word. Words clipped away by an overflow ancestor are not
    /// candidates; `selectable` filters the text nodes considered.
    pub fn caret_from_point(
        &self,
        dom: &Dom,
        within: NodeId,
        x: f64,
        y: f64,
        selectable: &dyn Fn(NodeId) -> bool,
    ) -> Option<(NodeId, u32, bool)> {
        if !self.flow_mode() {
            return None;
        }
        let flow = self.flow(dom);
        let (wx, wy) = (self.window_scroll.0 as f64, self.window_scroll.1 as f64);
        let mut best: Option<((f64, f64), u32, NodeId, f64)> = None;
        for (i, run) in flow.text_runs.iter().enumerate() {
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
            let (kx, ky) = self.sticky_shift(dom, &flow, run.node);
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
        best.map(|(key, i, node, left)| (node, flow.text_runs[i as usize].offset_at(x - left), key == (0.0, 0.0)))
    }

    /// Whether a text box (viewport coordinates) lies entirely outside the
    /// visible area of an overflow-clipping ancestor of `node`.
    fn text_clipped(&self, dom: &Dom, node: NodeId, x: f64, y: f64, w: f64, h: f64) -> bool {
        let mut cur = dom.parent_element(node).ok().flatten();
        while let Some(a) = cur {
            if self.is_root_container(dom, a) {
                break;
            }
            let clips = self.style.overflow_clip_axes(dom, a);
            if clips.x || clips.y {
                let r = self.bounding_client_rect_for_visible(dom, a);
                if (clips.x && (x + w <= r.x || x >= r.x + r.width))
                    || (clips.y && (y + h <= r.y || y >= r.y + r.height))
                {
                    return true;
                }
            }
            cur = dom.parent_element(a).ok().flatten();
        }
        false
    }

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
            let v = self
                .style
                .computed_value(dom, e, "user-select")
                .or_else(|| self.style.computed_value(dom, e, "-webkit-user-select"));
            match v.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
                Some("none") => return true,
                Some("text" | "all" | "contain") => return false,
                _ => {}
            }
            cur = dom.parent_element(e).ok().flatten();
        }
        false
    }

    /// The bounding rect, or `None` when the element isn't rendered.
    pub fn client_rect(&self, dom: &Dom, el: NodeId) -> Option<Rect> {
        self.is_visible(dom, el).then(|| self.bounding_client_rect_for_visible(dom, el))
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
        if self.flow_mode() {
            let flow = self.flow(dom);
            let Some(b) = flow.get(el).copied() else { return Rect::default() };
            let (kx, ky) = self.sticky_shift(dom, &flow, el);
            let (sx, sy) =
                if b.fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
            return Rect {
                x: b.rect.x - sx - dx + kx,
                y: b.rect.y - sy - dy + ky,
                width: b.rect.width,
                height: b.rect.height,
            };
        }
        Rect {
            x: self.horizontal_position(dom, el) - self.window_scroll.0 as f64 - dx,
            y: self.document_position(dom, el) - self.window_scroll.1 as f64 - dy,
            width: self.box_axis(dom, el, Axis::Width),
            height: self.box_axis(dom, el, Axis::Height),
        }
    }

    /// The scroll-dependent offset of `node`'s sticky ancestors (and itself):
    /// each sticky box moves with its descendants.
    fn sticky_shift(&self, dom: &Dom, flow: &FlowResult, node: NodeId) -> (f64, f64) {
        if flow.sticky.is_empty() {
            return (0.0, 0.0);
        }
        let (mut x, mut y) = (0.0, 0.0);
        let mut cur = Some(node);
        while let Some(n) = cur {
            if flow.sticky.contains(&n) {
                let (dx, dy) = self.sticky_offset(dom, flow, n);
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
    fn sticky_offset(&self, dom: &Dom, flow: &FlowResult, el: NodeId) -> (f64, f64) {
        use crate::flow::{FlowStyleSource, InnerDisplay, TablePart, BOTTOM, LEFT, RIGHT, TOP};
        let Some(b) = flow.get(el) else { return (0.0, 0.0) };
        let Some(style) = self.style.flow_style(dom, el) else { return (0.0, 0.0) };
        // Scrollport in document coordinates of the scrolled content.
        let mut port = None;
        let mut a = dom.parent_element(el).ok().flatten();
        while let Some(p) = a {
            if self.is_root_container(dom, p) {
                break;
            }
            let axes = self.style.overflow_clip_axes(dom, p);
            if axes.x || axes.y {
                if let (Some(pb), Some(ps)) = (flow.get(p), self.style.flow_style(dom, p)) {
                    port = Some(Rect {
                        x: pb.rect.x + ps.border[LEFT] + self.scroll_left(p) as f64,
                        y: pb.rect.y + ps.border[TOP] + self.scroll_top(p) as f64,
                        width: pb.client_width,
                        height: pb.client_height,
                    });
                }
                break;
            }
            a = dom.parent_element(p).ok().flatten();
        }
        let (wx, wy) = if b.fixed { (0.0, 0.0) } else { (self.window_scroll.0 as f64, self.window_scroll.1 as f64) };
        let port =
            port.unwrap_or(Rect { x: wx, y: wy, width: self.inner_width() as f64, height: self.inner_height() as f64 });
        // Containing block: the table for table parts, else the parent.
        let mut cb = dom.parent_element(el).ok().flatten();
        if style.table_part != TablePart::None {
            while let Some(c) = cb {
                if self.style.flow_style(dom, c).is_some_and(|s| s.inner == InnerDisplay::Table) {
                    break;
                }
                cb = dom.parent_element(c).ok().flatten();
            }
        }
        let limit = cb.and_then(|c| {
            let cbox = flow.get(c)?;
            let cs = self.style.flow_style(dom, c)?;
            let w = cbox.rect.width;
            let pad = |i: usize| cs.padding[i].resolve(w);
            Some(Rect {
                x: cbox.rect.x + cs.border[LEFT] + pad(LEFT),
                y: cbox.rect.y + cs.border[TOP] + pad(TOP),
                width: (cbox.rect.width - cs.border[LEFT] - cs.border[RIGHT] - pad(LEFT) - pad(RIGHT)).max(0.0),
                height: (cbox.rect.height - cs.border[TOP] - cs.border[BOTTOM] - pad(TOP) - pad(BOTTOM)).max(0.0),
            })
        });
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
        let dy = axis(
            style.inset[TOP].resolve(Some(port.height)),
            style.inset[BOTTOM].resolve(Some(port.height)),
            r.y,
            r.height,
            port.y,
            port.height,
            lo_y,
            hi_y,
        );
        let dx = axis(
            style.inset[LEFT].resolve(Some(port.width)),
            style.inset[RIGHT].resolve(Some(port.width)),
            r.x,
            r.width,
            port.x,
            port.width,
            lo_x,
            hi_x,
        );
        (dx, dy)
    }

    // ----- document ----------------------------------------------------------

    /// `Document.extent` (Document.zig:490-545) as (width, height): enough
    /// height for every synthetic position, the bottom of every element with
    /// an inline height, and body's stacked children; body's widest child.
    pub fn document_extent(&self, dom: &Dom) -> (f64, f64) {
        if self.flow_mode() {
            let f = self.flow(dom);
            return (f.document_width, f.document_height);
        }
        if let Some(e) = self.style.cached_extent(dom) {
            return e;
        }
        let Ok(d) = dom.check(self.document()) else { return (0.0, 0.0) };
        let mut height: f64 = 0.0;
        let mut width: f64 = 0.0;
        let mut index = 0.0;
        let mut cur = d;
        while cur != NIL {
            if dom.n(cur).is_element() {
                let parent = dom.n(cur).parent;
                if parent != NIL && dom.n(parent).is_element() && !self.is_root_idx(dom, parent) {
                    if let Some(h) = self.style.inline_size(dom, dom.id_of(cur), false) {
                        height = height.max(index * NODE_HEIGHT + h);
                    }
                }
            }
            index += 1.0;
            cur = dom.following_in(cur, d);
        }
        height = height.max(index * NODE_HEIGHT);

        let is_html = matches!(&dom.n(d).kind, NodeKind::Document(dd) if dd.kind == DocumentKind::Html);
        if is_html {
            let body = dom
                .body(self.document())
                .ok()
                .flatten()
                .filter(|&b| dom.element(b).is_ok_and(|e| e.name.local == local_name!("body")));
            if let Some(body) = body {
                let bi = body.idx;
                height = height
                    .max(self.content_axis_idx(dom, bi, Axis::Height))
                    .max(self.style.inline_size(dom, body, false).unwrap_or(0.0));
                width = self.style.inline_size(dom, body, true).unwrap_or(0.0);
                for c in dom.child_idx_iter(bi) {
                    if dom.n(c).is_element() && !self.style.has_display_none(dom, dom.id_of(c)) {
                        width = width.max(self.element_axis_idx(dom, c, Axis::Width).value);
                    }
                }
            }
        }
        // Whole pixels, like scroll offsets
        let e = (width.ceil(), height.ceil());
        self.style.store_extent(e);
        e
    }

    /// `Document.scrollSize` (Document.zig:548-556): the document, at least
    /// viewport-sized.
    pub fn document_scroll_size(&self, dom: &Dom) -> (f64, f64) {
        let (w, h) = self.document_extent(dom);
        let v = self.viewport();
        (w.max(v.width as f64), h.max(v.height as f64))
    }

    // ----- hit testing -------------------------------------------------------

    /// Flow mode hit test in viewport coordinates: the topmost hit-testable
    /// box (positioned above in-flow, later above earlier) whose rect and
    /// every overflow-clipping ancestor's rect contain the point.
    fn flow_element_from_point(&self, dom: &Dom, x: f64, y: f64, ignore_x: bool) -> Option<NodeId> {
        let flow = self.flow(dom);
        let inside = |r: &Rect| (ignore_x || (x >= r.x && x <= r.x + r.width)) && y >= r.y && y <= r.y + r.height;
        let mut best: Option<(bool, u32, NodeId)> = None;
        for (&el, b) in &flow.boxes {
            if b.rect.width <= 0.0 && b.rect.height <= 0.0 {
                continue;
            }
            if best.is_some_and(|k| (k.0, k.1) > (b.positioned, b.order)) {
                continue;
            }
            let r = self.bounding_client_rect_for_visible(dom, el);
            if !inside(&r) || !self.is_visible(dom, el) || !self.style.is_hit_testable(dom, el) {
                continue;
            }
            // Clipped by an overflow ancestor (fixed boxes escape non-fixed clips).
            let mut clipped = false;
            let mut cur = dom.parent_element(el).ok().flatten();
            while let Some(a) = cur {
                if self.is_root_container(dom, a) {
                    break;
                }
                let clips = self.style.overflow_clip_axes(dom, a);
                if clips.x || clips.y {
                    if let Some(ab) = flow.get(a) {
                        if !(b.fixed && !ab.fixed) {
                            let ar = self.bounding_client_rect_for_visible(dom, a);
                            let out_x = clips.x && !ignore_x && (x < ar.x || x > ar.x + ar.width);
                            let out_y = clips.y && (y < ar.y || y > ar.y + ar.height);
                            if out_x || out_y {
                                clipped = true;
                                break;
                            }
                        }
                    }
                }
                cur = dom.parent_element(a).ok().flatten();
            }
            if !clipped {
                best = Some((b.positioned, b.order, el));
            }
        }
        best.map(|k| k.2)
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

    fn element_from_point_impl(&self, dom: &Dom, viewport_x: f64, viewport_y: f64, ignore_x: bool) -> Option<NodeId> {
        if self.flow_mode() {
            return self.flow_element_from_point(dom, viewport_x, viewport_y, ignore_x);
        }
        // DFS in document order; topmost = last visited element whose rect
        // contains (x, y). Tops increase monotonically in document order, so
        // the walk stops once the running position passes y. Hidden subtrees
        // still count towards the index so positions agree with
        // getBoundingClientRect.
        //
        // Element scroll offsets shift descendants up/left exactly as in
        // `bounding_client_rect`; tops then decrease by at most the total of
        // all element scroll offsets, which bounds the early exit.
        let root = dom.check(self.document()).ok()?;
        let x = viewport_x + self.window_scroll.0 as f64;
        let y = viewport_y + self.window_scroll.1 as f64;
        let max_dy: f64 = self.element_scroll.values().map(|&(_, sy)| sy as f64).sum();
        let mut topmost = None;
        // (node, hidden, ancestor scroll x, ancestor scroll y)
        let mut stack: Vec<(u32, bool, f64, f64)> = vec![(root, false, 0.0, 0.0)];
        let mut preorder_index = 0.0;
        while let Some((node, mut hidden, dx, dy)) = stack.pop() {
            let pos = preorder_index * NODE_HEIGHT;
            if pos - max_dy > y {
                return topmost;
            }
            preorder_index += 1.0;
            if dom.n(node).is_element() {
                let id = dom.id_of(node);
                hidden = hidden || self.style.has_display_none(dom, id);
                // Unlike display:none, a child can opt back in, so the
                // subtree is still walked.
                let top = pos - dy;
                if !hidden
                    && self.style.is_hit_testable(dom, id)
                    && y >= top
                    && y <= top + self.box_axis_idx(dom, node, Axis::Height)
                {
                    if ignore_x {
                        topmost = Some(id);
                    } else {
                        let left = self.horizontal_position(dom, id) - dx;
                        if x >= left && x <= left + self.box_axis_idx(dom, node, Axis::Width) {
                            topmost = Some(id);
                        }
                    }
                }
            }
            let (cdx, cdy) = match self.element_scroll.get(&dom.id_of(node)) {
                Some(&(sx, sy)) => (dx + sx as f64, dy + sy as f64),
                None => (dx, dy),
            };
            let mut c = dom.n(node).last_child;
            while c != NIL {
                stack.push((c, hidden, cdx, cdy));
                c = dom.n(c).prev;
            }
        }
        topmost
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
    pub fn window_scroll_to(&mut self, dom: &Dom, left: Option<i32>, top: Option<i32>) -> bool {
        let (w, h) = self.document_scroll_size(dom);
        let max_x = scroll_limit(w, self.inner_width());
        let max_y = scroll_limit(h, self.inner_height());
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
        if !self.flow_mode() || self.scroll_top(scroller) == 0 {
            self.scroll_anchors.remove(&scroller);
            return;
        }
        let flow = self.flow(dom);
        self.scroll_anchors.insert(scroller, flow);
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
        flow: &FlowResult,
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
        flow: &FlowResult,
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
            if b.fixed || !keep(child) || self.style.computed(dom, child).is_some_and(|c| c.absolutely_positioned) {
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
        let flow = self.flow(dom);
        let mut anchors: Vec<(NodeId, Rc<FlowResult>)> =
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
        if !self.flow_mode() || !self.has_scroll_offsets() {
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

    /// `clampScroll` (Element.zig:2261-2270).
    fn clamp_scroll(&self, dom: &Dom, el: NodeId, axis: Axis, target: i64) -> u32 {
        let mut clamped = target;
        if clamped < 0 {
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
        let y = if self.flow_mode() {
            match self.flow_box(dom, el) {
                // Fixed boxes do not move with the window.
                Some(b) if b.fixed => return moved,
                Some(b) => b.rect.y - dy,
                None => return moved,
            }
        } else {
            self.document_position(dom, el) - dy
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
    /// geometry's client rects cannot tell whether a box is in view, so it
    /// keeps the `scrollIntoView` start reveal.
    pub fn scroll_into_view_if_needed_targets(&mut self, dom: &Dom, el: NodeId) -> Vec<ScrollTarget> {
        if !self.flow_mode() {
            return self.scroll_into_view_targets(dom, el);
        }
        if !self.is_visible(dom, el) {
            return Vec::new();
        }
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
                        center_if_needed_offset(
                            target.left() - container.left(),
                            target.width,
                            self.client_width(dom, node),
                            self.scroll_left(node),
                        )
                    })
                    .flatten();
                let top = (axes.y || explicit)
                    .then(|| {
                        center_if_needed_offset(
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
            cur = dom.parent_element(node).ok().flatten();
        }
        // Fixed boxes do not move with the window.
        if self.flow_mode() && self.flow_box(dom, el).is_some_and(|b| b.fixed) {
            return moved;
        }
        let target = self.bounding_client_rect(dom, el);
        let left =
            center_if_needed_offset(target.left(), target.width, self.inner_width() as f64, self.window_scroll.0);
        let top =
            center_if_needed_offset(target.top(), target.height, self.inner_height() as f64, self.window_scroll.1);
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
    /// length or the faux layout, else the initial value.
    pub fn computed_style_value(&self, dom: &Dom, el: NodeId, name: &str) -> String {
        let custom = name.starts_with("--");
        let name = if custom { name.to_owned() } else { name.to_ascii_lowercase() };
        let owned = self.owned(dom, el).is_some();
        if owned {
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
                if let Some(v) = self.style.inline_style_value(dom, el, &name) {
                    return v;
                }
                let axis = if name == "width" { Axis::Width } else { Axis::Height };
                return format!("{}px", self.box_axis(dom, el, axis));
            }
            if let Some(v) = declared {
                return v;
            }
            if let Some(v) = self.style.inline_style_value(dom, el, &name) {
                return v;
            }
        }
        default_property_value(dom, el, &name).to_owned()
    }
}

/// `countSubtreeNodes` (Element.zig:2046-2053): the node and its light-tree
/// descendants.
fn count_subtree_nodes(dom: &Dom, idx: u32) -> f64 {
    let mut count = 0.0;
    let mut cur = idx;
    while cur != NIL {
        count += 1.0;
        cur = dom.following_in(cur, idx);
    }
    count
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

/// `CSS.parseTranslateX` (webapi/CSS.zig:31-67): the summed X of every
/// `translateX` / `translate` / `translate3d` (first argument) and `matrix`
/// (fifth argument) in a transform, unitless or `px` only.
pub fn parse_translate_x(value: &str) -> f64 {
    let mut total = 0.0;
    let mut p = Parser::new(value);
    loop {
        let name = match p.next() {
            Ok(Token::Function(name)) => name.clone(),
            Ok(_) => continue,
            Err(_) => break,
        };
        let x_arg = if name.eq_ignore_ascii_case("translatex")
            || name.eq_ignore_ascii_case("translate")
            || name.eq_ignore_ascii_case("translate3d")
        {
            0
        } else if name.eq_ignore_ascii_case("matrix") {
            4
        } else {
            continue;
        };
        let _: Result<(), cssparser::ParseError<()>> = p.parse_nested_block(|n| {
            let mut arg = 0;
            loop {
                let start = n.position().byte_index();
                let tok = match n.next() {
                    Ok(t) => t.clone(),
                    Err(_) => break,
                };
                let text = value.get(start..n.position().byte_index()).unwrap_or("").trim();
                match tok {
                    Token::Comma => arg += 1,
                    Token::Number { value: v, .. } if arg == x_arg => {
                        total += text.parse::<f64>().unwrap_or(v as f64);
                    }
                    Token::Dimension { value: v, ref unit, .. } if arg == x_arg && unit.eq_ignore_ascii_case("px") => {
                        let num = text.get(..text.len().saturating_sub(2)).and_then(|t| t.parse::<f64>().ok());
                        total += num.unwrap_or(v as f64);
                    }
                    _ => {}
                }
            }
            Ok(())
        });
    }
    total
}

// ----- text measure (text_measure.zig) ---------------------------------------

/// Line height for `line-height: normal`, in em (text_measure.zig:67).
const LINE_HEIGHT: f64 = 1.2;

/// Greedy line breaking under `white-space: normal` (text_measure.zig:69-108).
struct LineWrap {
    line_width: f64,
    font_size: f64,
    lines: u32,
    x: f64,
    space: bool,
}

fn is_ascii_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

impl LineWrap {
    fn new(line_width: f64, font_size: f64) -> Self {
        LineWrap { line_width, font_size, lines: 0, x: 0.0, space: false }
    }

    fn add(&mut self, text: &str) {
        let Some(first) = text.chars().next() else { return };
        if is_ascii_ws(first) {
            self.space = true;
        }
        for word in text.split(is_ascii_ws).filter(|w| !w.is_empty()) {
            let w = text_width(word, self.font_size);
            let gap = if self.space { advance(' ', self.font_size) } else { 0.0 };
            if self.lines == 0 {
                self.lines = 1;
                self.x = w;
            } else if self.x + gap + w > self.line_width {
                self.lines += 1;
                self.x = w;
            } else {
                self.x += gap + w;
            }
            self.space = true;
        }
        self.space = text.chars().next_back().is_some_and(is_ascii_ws);
    }

    fn height(&self) -> f64 {
        self.lines as f64 * self.font_size * LINE_HEIGHT
    }
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
    crate::style_flow::ua_display(&e.name.local.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_x() {
        assert_eq!(parse_translate_x("translateX(-12.5px)"), -12.5);
        assert_eq!(parse_translate_x("translate3d(2px, 100px, 0) rotate(45deg) matrix(1, 0, 0, 1, 3, 9)"), 5.0);
        assert_eq!(parse_translate_x("translateX(-100%)"), 0.0);
    }

    #[test]
    fn line_wrap() {
        // text_measure.zig:157-175 "LineWrap: collapses whitespace and breaks between words"
        let mut wrap = LineWrap::new(60.0, 10.0);
        assert_eq!(wrap.height(), 0.0);
        wrap.add("  aaaa \n\t  aaaa   aaaa ");
        assert_eq!(wrap.lines, 2);
        wrap.add("aaaa");
        assert_eq!(wrap.lines, 2);
        wrap.add("aaaa");
        assert_eq!(wrap.lines, 3);
        assert!((wrap.height() - 36.0).abs() < 1e-4);
        let mut narrow = LineWrap::new(10.0, 10.0);
        narrow.add("aaaa aaaa");
        assert_eq!(narrow.lines, 2);
    }
}

/// `LIGHTNINGPANDA_FLOW_STATS=1`: counts flow passes and why each ran (the
/// first pass, DOM version, style generation, focus, hover, viewport),
/// printed to stderr every 25 passes, with how the style cache was
/// invalidated and how many flow styles were recomputed in those 25 passes.
/// Diagnostic only; off by default.
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
        selective: u64,
        dropped: u64,
        full: [u64; Full::COUNT],
        misses: u64,
        miss_time: Duration,
        unknown: std::collections::BTreeMap<String, u64>,
        /// The largest invalidation roots: (descendants dropped, root).
        roots: Vec<(usize, String)>,
    }

    /// Why the style cache was cleared entirely.
    #[derive(Clone, Copy)]
    pub(crate) enum Full {
        /// The rule set was rebuilt (stylesheet change).
        Rules,
        /// Style generation, focus or hover changed.
        Stamp,
        /// Unbounded or sibling-relative `:has()` in the stylesheet.
        Relative,
        ShadowRoots,
        /// The journal no longer covers the cached version.
        Journal,
        /// An unjournaled change (or a document-level root).
        Unknown,
        /// Selective roots covered more than half the cache.
        Large,
    }

    impl Full {
        const COUNT: usize = 7;
        const NAMES: [&str; Full::COUNT] = ["rules", "stamp", "has", "shadow", "journal", "unknown", "large"];
    }

    pub(crate) fn note_full(reason: Full) {
        if enabled() {
            STATS.with(|s| s.borrow_mut().full[reason as usize] += 1);
        }
    }

    pub(crate) fn note_selective(dropped: usize) {
        if enabled() {
            STATS.with(|s| {
                let mut s = s.borrow_mut();
                s.selective += 1;
                s.dropped += dropped as u64;
            });
        }
    }

    /// Where an unjournaled change came from (`file:line`, or `document`).
    pub(crate) fn note_unknown(label: impl FnOnce() -> String) {
        if enabled() {
            STATS.with(|s| *s.borrow_mut().unknown.entry(label()).or_default() += 1);
        }
    }

    /// One selective invalidation root and the descendants it dropped; the
    /// three largest per window are printed.
    pub(crate) fn note_root(size: usize, label: impl FnOnce() -> String) {
        if !enabled() {
            return;
        }
        STATS.with(|s| {
            let roots = &mut s.borrow_mut().roots;
            if roots.len() < 3 || roots.last().is_some_and(|(n, _)| size > *n) {
                roots.push((size, label()));
                roots.sort_by_key(|r| std::cmp::Reverse(r.0));
                roots.truncate(3);
            }
        });
    }

    pub(crate) fn note_style_miss(took: Duration) {
        STATS.with(|s| {
            let mut s = s.borrow_mut();
            s.misses += 1;
            s.miss_time += took;
        });
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
                let full: Vec<String> = Full::NAMES
                    .iter()
                    .zip(s.full)
                    .filter(|(_, n)| *n > 0)
                    .map(|(name, n)| format!("{name}:{n}"))
                    .collect();
                eprintln!(
                    "flow-stats last25 avg_ms={:.1} selective={} dropped={} full=[{}] style_misses={} style_ms={}",
                    s.window.as_secs_f64() * 1000.0 / 25.0,
                    s.selective,
                    s.dropped,
                    full.join(" "),
                    s.misses,
                    s.miss_time.as_millis()
                );
                let unknown: Vec<String> = s.unknown.iter().map(|(at, n)| format!("{at}:{n}")).collect();
                let roots: Vec<String> = s.roots.iter().map(|(n, r)| format!("{r}:{n}")).collect();
                eprintln!("flow-stats last25 unknown=[{}] largest_roots=[{}]", unknown.join(" "), roots.join(" "));
                s.unknown.clear();
                s.roots.clear();
                s.window = Duration::ZERO;
                s.selective = 0;
                s.dropped = 0;
                s.full = [0; Full::COUNT];
                s.misses = 0;
                s.miss_time = Duration::ZERO;
            }
        });
    }
}
