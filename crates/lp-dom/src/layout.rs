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

use crate::dom::Dom;
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
        Layout { style, window_scroll: (0, 0), scroll_generation: 0, element_scroll: FxHashMap::default() }
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
        own.value.max(self.content_axis_idx(dom, idx, axis))
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
        self.viewport_axis(dom, el, axis).unwrap_or_else(|| self.box_axis(dom, el, axis))
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
        let own = self.element_axis(dom, el, axis).value;
        if self.is_root_container(dom, el) {
            return self.root_scroll_size(dom, el, axis).unwrap_or(own);
        }
        own.max(self.content_axis(dom, el, axis))
    }

    /// `scrollExtent` (Element.zig:1845-1858): `None` where no limit can be
    /// proven, which leaves the offset unbounded.
    fn scroll_extent(&self, dom: &Dom, el: NodeId, axis: Axis) -> Option<f64> {
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
        self.box_axis(dom, el, Axis::Width)
    }

    pub fn offset_height(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        self.box_axis(dom, el, Axis::Height)
    }

    /// `offsetTop` (Element.zig:1929-1934).
    pub fn offset_top(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
        }
        self.document_position(dom, el)
    }

    /// `offsetLeft` (Element.zig:1936-1941).
    pub fn offset_left(&self, dom: &Dom, el: NodeId) -> f64 {
        if !self.is_visible(dom, el) {
            return 0.0;
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
        Rect {
            x: self.horizontal_position(dom, el) - self.window_scroll.0 as f64 - dx,
            y: self.document_position(dom, el) - self.window_scroll.1 as f64 - dy,
            width: self.box_axis(dom, el, Axis::Width),
            height: self.box_axis(dom, el, Axis::Height),
        }
    }

    // ----- document ----------------------------------------------------------

    /// `Document.extent` (Document.zig:490-545) as (width, height): enough
    /// height for every synthetic position, the bottom of every element with
    /// an inline height, and body's stacked children; body's widest child.
    pub fn document_extent(&self, dom: &Dom) -> (f64, f64) {
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
        true
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

    /// Brings `el`'s top into view inside every scrolled ancestor container
    /// whose visible range [scrollTop, scrollTop + clientHeight) excludes it,
    /// innermost first. Deviation from upstream (window only): needed once
    /// element scroll offsets move client rects. Containers that were never
    /// scrolled keep upstream behavior. Returns whether anything moved.
    fn reveal_in_scrolled_ancestors(&mut self, dom: &Dom, el: NodeId) -> bool {
        if self.element_scroll.is_empty() {
            return false;
        }
        let y = self.document_position(dom, el);
        let mut moved = false;
        let mut cur = dom.parent_element(el).ok().flatten();
        while let Some(a) = cur {
            if let Some(&(_, sy)) = self.element_scroll.get(&a) {
                let offset = y - self.document_position(dom, a);
                let visible = self.client_height(dom, a);
                if offset < sy as f64 || offset >= sy as f64 + visible {
                    let top = offset.max(0.0).trunc().min(i32::MAX as f64) as i32;
                    moved |= self.set_scroll_top(dom, a, top);
                }
            }
            cur = dom.parent_element(a).ok().flatten();
        }
        moved
    }

    /// `scrollIntoView` (Element.zig:2161-2168): brings the element's top to
    /// the window's scroll offset (x reset to 0), after revealing it inside
    /// scrolled ancestor containers.
    pub fn scroll_into_view(&mut self, dom: &Dom, el: NodeId) -> bool {
        let inner = self.reveal_in_scrolled_ancestors(dom, el);
        let (_, dy) = self.ancestor_scroll_offset(dom, el);
        let y = self.document_position(dom, el) - dy;
        let top = y.max(0.0).trunc().min(i32::MAX as f64) as i32;
        self.window_scroll_to(dom, Some(0), Some(top)) || inner
    }

    /// `scrollIntoViewIfNeeded` (Element.zig:2146-2155): no-op while the top
    /// is within [scrollY, scrollY + innerHeight) and visible inside its
    /// scrolled ancestors.
    pub fn scroll_into_view_if_needed(&mut self, dom: &Dom, el: NodeId) -> bool {
        let inner = self.reveal_in_scrolled_ancestors(dom, el);
        let (_, dy) = self.ancestor_scroll_offset(dom, el);
        let y = self.document_position(dom, el) - dy;
        let sy = self.window_scroll.1 as f64;
        let vh = self.inner_height() as f64;
        if y >= sy && y < sy + vh {
            return inner;
        }
        self.scroll_into_view(dom, el) || inner
    }

    // ----- getComputedStyle --------------------------------------------------

    /// `getComputedStyle(el).getPropertyValue(name)`
    /// (CSSStyleDeclaration.zig:76-151): display / visibility /
    /// pointer-events / text-transform from the cascade, custom properties,
    /// font-size in px, the inline value (unless an `!important` rule beats
    /// it), width/height from the faux layout, else the initial value.
    pub fn computed_style_value(&self, dom: &Dom, el: NodeId, name: &str) -> String {
        let custom = name.starts_with("--");
        let name = if custom { name.to_owned() } else { name.to_ascii_lowercase() };
        let owned = self.owned(dom, el).is_some();
        if owned {
            match name.as_str() {
                "display" if self.style.has_display_none(dom, el) => return "none".into(),
                "visibility" if self.style.has_visibility_hidden_inherited(dom, el) => return "hidden".into(),
                "pointer-events" if self.style.has_pointer_events_none(dom, el) => return "none".into(),
                "text-transform" => return self.style.text_transform(dom, el).keyword().into(),
                _ => {}
            }
            if custom {
                return self.style.custom_property_value(dom, el, &name).unwrap_or_default();
            }
            if name == "font-size" {
                return format!("{}px", self.style.computed_font_size(dom, Some(el)));
            }
            if let Some(v) = self.style.inline_style_value(dom, el, &name) {
                return v;
            }
            if name == "width" || name == "height" {
                if !self.is_visible(dom, el) {
                    return "auto".into();
                }
                let axis = if name == "width" { Axis::Width } else { Axis::Height };
                return format!("{}px", self.box_axis(dom, el, axis));
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
fn text_width(text: &str, font_size: f64) -> f64 {
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
        _ => "",
    }
}

/// getDefaultDisplay + isInlineTag (CSSStyleDeclaration.zig:1060-1091): the
/// HTML types upstream maps to `inline` (anchor, br, span, label, time,
/// font, ins/del, q/blockquote) and the generic inline tags; everything
/// else HTML is `block`; SVG (and other namespaces) `inline`.
fn default_display(e: &crate::node::ElementData) -> &'static str {
    if !e.is_html() {
        return "inline";
    }
    match &*e.name.local {
        "a" | "br" | "span" | "label" | "time" | "font" | "ins" | "del" | "q" | "blockquote" => "inline",
        "abbr" | "b" | "bdi" | "bdo" | "cite" | "code" | "dfn" | "em" | "i" | "kbd" | "mark" | "s" | "samp"
        | "small" | "strong" | "sub" | "sup" | "u" | "var" | "wbr" => "inline",
        _ => "block",
    }
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
