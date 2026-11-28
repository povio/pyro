//! Layout Instability: `layout-shift` entries from comparing the document's real layout between
//! rendering updates.
//!
//! # What is compared
//!
//! Each tracked frame the module reads, from layout, the border box (client coordinates) of
//! every element that paints something of its own (a replaced element, or a background colour
//! or image, or a visible border) and the line boxes of every text node, skipping what Layout
//! Instability ignores: `display: none`, `visibility: hidden`, `opacity: 0`, `position: fixed`
//! and `sticky` boxes (and what they hold), and anything under a CSS transform (a transform
//! change is no shift; a shift inside a transformed box is not seen here: Pyro does not give a
//! box's untransformed position in viewport space). Positions are kept in scroll-independent
//! "layout space" (client rect plus the window's and the scroll containers' offsets), so
//! scrolling moves nothing and the previous rect of a node is its old layout position as the
//! viewport shows it now (the scroll offset difference is cancelled).
//!
//! A node shifted when its start edges moved (left and top, or right and top for `direction:
//! rtl`) by more than 1/64 px. For each shift the visible old and new rectangles (clipped by
//! the ancestors' overflow clips and the viewport) join the frame's impact region; the score is
//! `impact area / viewport area * min(1, max distance / viewport's larger side)`, one
//! `layout-shift` entry per frame (the sum of what moved, not more than one per frame), with
//! `sources` the five nodes of largest impact (unclipped previous and current rects in CSS px).
//! `hadRecentInput` is a trusted input within the 500 ms before the frame's start
//! (`timeline().recent_input`), `lastInputTime` the last such input or 0.
//!
//! # When it runs
//!
//! A frame is requested ([`wants_frame`]) after a task only if layout's `observation_stamp`
//! changed since the last tracked frame, and only while an observer watches the type or the
//! document is within its first [`UNOBSERVED_FRAMES`] tracked frames (entries for late
//! `buffered` readers); after that the previous rectangles are dropped and no frame is
//! asked for until an observer appears (its first tracked frame is the new baseline: a shift
//! in that very frame is not seen). The first tracked frame, and a frame after the viewport was
//! resized, only set the baseline.
//!
//! Not done: the visual overflow of a box (a parent's painted area counts its own box only),
//! fragmented (multicol) boxes, vertical writing modes' edges, `contain: paint` and
//! `content-visibility` details, shifts of fixed/sticky boxes.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use pyro_dom::layout::{Layout, Rect};
use pyro_dom::style::{CheckVisibilityOptions, Position};
use pyro_dom::{Dom, NodeId, NodeType};
use pyro_js::{ClassDef, IdentityKey, JsClass, JsResult, NoParent, Persistent, Scope, Value};
use rustc_hash::FxHashMap;

use crate::page::PageDom;
use crate::performance::{PerformanceEntry, PerformanceEntryData};

use super::element::NodeRef;
use super::lcp::transparent;
use super::timeline::{queue_entry, TimelineEntry};
use super::FrameInfo;

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["layout-shift"];
/// Flip to true when the module really works: only then are its types in
/// `supportedEntryTypes`, its classes registered and its hooks called.
pub const ENABLED: bool = true;

/// Tracked frames an unobserved document gets before the module stops comparing.
pub const UNOBSERVED_FRAMES: u32 = 60;

/// Largest number of sources an entry names.
const MAX_SOURCES: usize = 5;
/// Rectangles over this many are not all joined into the impact region (the largest are).
const MAX_REGION_RECTS: usize = 400;
/// A move under this is layout rounding (Chrome's layout unit is 1/64 px).
const EPSILON: f64 = 1.0 / 64.0;

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
pub fn register(engine: &pyro_js::Engine) {
    engine.register::<LayoutShift>();
    engine.register::<LayoutShiftAttribution>();
}

type Stamp = ((u64, u64, u32, u32), u64);

/// What is tracked of a node: it is an element that paints, or a text node.
#[derive(Clone)]
struct Item {
    node: NodeId,
    /// Border box (a text node's line box union) in client coordinates now.
    client: Rect,
    /// `client` plus the scroll offsets in effect: where it is in layout space.
    pos: Rect,
    /// The scroll offsets `pos` adds to `client` now.
    adjust: (f64, f64),
    /// The clip rectangle of its ancestors and the viewport (`None`: clipped away entirely).
    clip: Option<Rect>,
    rtl: bool,
}

struct State {
    page: Weak<PageDom>,
    stamp: Option<Stamp>,
    prev: FxHashMap<NodeId, Item>,
    viewport: (f64, f64),
    /// Tracked frames so far.
    frames: u32,
    /// Walks of the document made so far, and the nodes whose style they read (counters: tests
    /// assert work, never time; see [`work`]).
    walks: u64,
    style_reads: u64,
}

thread_local! {
    /// Per document (`PageDom::id`); entries of dropped documents are pruned on access.
    static STATES: RefCell<FxHashMap<u64, State>> = RefCell::new(FxHashMap::default());
}

fn with_state<R>(page: &Rc<PageDom>, f: impl FnOnce(&mut State) -> R) -> R {
    STATES.with(|states| {
        let mut states = states.borrow_mut();
        states.retain(|_, s| s.page.upgrade().is_some_and(|p| !p.is_disposed()));
        let state = states.entry(page.id()).or_insert_with(|| State {
            page: Rc::downgrade(page),
            stamp: None,
            prev: FxHashMap::default(),
            viewport: (0.0, 0.0),
            frames: 0,
            walks: 0,
            style_reads: 0,
        });
        f(state)
    })
}

/// `(walks of the document, nodes whose computed style they read)` the module made for `page` so
/// far: what tests assert instead of time (a static document with nothing observed reads none).
pub fn work(page: &Rc<PageDom>) -> (u64, u64) {
    with_state(page, |s| (s.walks, s.style_reads))
}

fn tracking(page: &Rc<PageDom>, frames: u32) -> bool {
    frames < UNOBSERVED_FRAMES || page.timeline().is_observed(ENTRY_TYPES[0])
}

/// Whether the document should get a frame after the change a task made: layout changed since
/// the last tracked frame and the module is tracking.
pub fn wants_frame(page: &Rc<PageDom>) -> bool {
    if !ENABLED || page.is_hidden() {
        return false;
    }
    let (frames, stamp) = with_state(page, |s| (s.frames, s.stamp));
    if !tracking(page, frames) {
        return false;
    }
    let (Ok(layout), Ok(dom)) = (page.layout.try_borrow(), page.dom.try_borrow()) else { return true };
    stamp != Some(layout.observation_stamp(&dom))
}

// ----- geometry ------------------------------------------------------------

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let (left, top) = (a.left().max(b.left()), a.top().max(b.top()));
    let (right, bottom) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    (right > left && bottom > top).then_some(Rect { x: left, y: top, width: right - left, height: bottom - top })
}

fn visible(rect: Rect, clip: Option<Rect>) -> Option<Rect> {
    intersect(rect, clip?)
}

/// Area of the union of `rects` (coordinate compression: exact, quadratic in their number).
fn union_area(rects: &[Rect]) -> f64 {
    let mut rects: Vec<Rect> = rects.iter().copied().filter(|r| r.width > 0.0 && r.height > 0.0).collect();
    if rects.len() > MAX_REGION_RECTS {
        rects.sort_by(|a, b| (b.width * b.height).total_cmp(&(a.width * a.height)));
        rects.truncate(MAX_REGION_RECTS);
    }
    let mut xs: Vec<f64> = rects.iter().flat_map(|r| [r.left(), r.right()]).collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    let mut total = 0.0;
    for slab in xs.windows(2) {
        let (x0, x1) = (slab[0], slab[1]);
        let mut spans: Vec<(f64, f64)> =
            rects.iter().filter(|r| r.left() <= x0 && r.right() >= x1).map(|r| (r.top(), r.bottom())).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut covered, mut end) = (0.0, f64::MIN);
        for (top, bottom) in spans {
            let top = top.max(end);
            if bottom > top {
                covered += bottom - top;
            }
            end = end.max(bottom);
        }
        total += covered * (x1 - x0);
    }
    total
}

// ----- collecting the document's items ----------------------------------------

/// The ancestors' state of one pass: exclusion (fixed, sticky, transformed boxes and what they hold)
/// and the clip rectangle, each computed once per element.
struct Walk<'a> {
    dom: &'a Dom,
    layout: &'a Layout,
    view: Rect,
    excluded: FxHashMap<NodeId, bool>,
    clips: FxHashMap<NodeId, Option<Rect>>,
    /// Nodes whose computed style the pass read (a box with an area: nothing else is looked at).
    reads: u64,
}

impl Walk<'_> {
    fn excluded(&mut self, el: NodeId) -> bool {
        if let Some(&known) = self.excluded.get(&el) {
            return known;
        }
        let style = self.layout.style().computed(self.dom, el).unwrap_or_default();
        let own = matches!(style.position, Position::Fixed | Position::Sticky) || style.transformed;
        let result = own || self.dom.parent_element(el).ok().flatten().is_some_and(|p| self.excluded(p));
        self.excluded.insert(el, result);
        result
    }

    /// The clip `el`'s descendants see: the viewport, narrowed by `el`'s own overflow clip and
    /// its ancestors' (padding boxes, client coordinates).
    fn clip_inside(&mut self, el: NodeId) -> Option<Rect> {
        if let Some(&known) = self.clips.get(&el) {
            return known;
        }
        let outer = match self.dom.parent_element(el).ok().flatten() {
            Some(parent) => self.clip_inside(parent),
            None => Some(self.view),
        };
        let mut clip = outer;
        if !self.layout.is_root_container(self.dom, el) {
            let axes = self.layout.style().overflow_clip_axes(self.dom, el);
            if let (true, Some(current)) = (axes.x || axes.y, clip) {
                // (What is laid out: asking a box inside skipped `content-visibility: auto` contents
                // renders them; nothing in it paints.)
                let mut b = self.layout.laid_out_client_rect(self.dom, el);
                if !(b.width > 0.0 || b.height > 0.0) {
                    self.clips.insert(el, None);
                    return None;
                }
                let (width, height) = (self.layout.client_width(self.dom, el), self.layout.client_height(self.dom, el));
                if width > 0.0 && height > 0.0 {
                    b = Rect { x: b.x + self.layout.client_left(self.dom, el), y: b.y + self.layout.client_top(self.dom, el), width, height };
                }
                let own = Rect {
                    x: if axes.x { b.x } else { f64::MIN / 4.0 },
                    y: if axes.y { b.y } else { f64::MIN / 4.0 },
                    width: if axes.x { b.width } else { f64::MAX / 2.0 },
                    height: if axes.y { b.height } else { f64::MAX / 2.0 },
                };
                clip = intersect(current, own);
            }
        }
        self.clips.insert(el, clip);
        clip
    }
}

fn is_replaced(name: &str) -> bool {
    matches!(
        name,
        "img" | "video" | "canvas" | "svg" | "iframe" | "object" | "embed" | "input" | "button" | "select" | "textarea" | "meter" | "progress"
    )
}

/// Whether the element's own box paints: a background colour or image, a visible border.
fn paints_own_box(layout: &Layout, dom: &Dom, el: NodeId) -> bool {
    let style = |name: &str| layout.computed_style_value(dom, el, name);
    if !transparent(&style("background-color")) {
        return true;
    }
    let image = style("background-image");
    if !image.is_empty() && image != "none" {
        return true;
    }
    ["top", "right", "bottom", "left"].iter().any(|side| {
        let border_style = style(&format!("border-{side}-style"));
        if border_style.is_empty() || border_style == "none" || border_style == "hidden" {
            return false;
        }
        let zero = style(&format!("border-{side}-width")).trim().trim_end_matches("px").parse::<f64>().is_ok_and(|w| w <= 0.0);
        !zero && !transparent(&style(&format!("border-{side}-color")))
    })
}

/// Every tracked item of the document now, the viewport and the number of nodes whose style was
/// read. A node's box is asked first: an element without an area (an empty inline, a hidden box)
/// and a text without line boxes are skipped before any style is read, so a document of tens of
/// thousands of boxes that paint nothing costs rectangle lookups, not a computed style each.
fn collect(page: &Rc<PageDom>) -> (FxHashMap<NodeId, Item>, (f64, f64), u64) {
    let dom = page.dom.borrow();
    let layout = page.layout();
    let viewport = (f64::from(layout.inner_width()), f64::from(layout.inner_height()));
    let view = Rect { x: 0.0, y: 0.0, width: viewport.0, height: viewport.1 };
    let scroll = (f64::from(layout.scroll_x()), f64::from(layout.scroll_y()));
    let shown = CheckVisibilityOptions { check_visibility: true, check_opacity: true };
    let mut walk = Walk { dom: &dom, layout: &layout, view, excluded: FxHashMap::default(), clips: FxHashMap::default(), reads: 0 };
    let mut items: FxHashMap<NodeId, Item> = FxHashMap::default();
    let Ok(nodes) = dom.descendants(page.document()) else { return (items, viewport, 0) };
    for id in nodes {
        let (owner, rect) = match dom.node_type(id) {
            Ok(NodeType::Element) => {
                let Ok(el) = dom.element(id) else { continue };
                if &*el.name.ns != "http://www.w3.org/1999/xhtml" && &*el.name.local != "svg" && &*el.name.local != "image" {
                    // (SVG shapes paint through their `svg` root's box.)
                    continue;
                }
                let rect = layout.laid_out_client_rect(&dom, id);
                if !(rect.width > 0.0 && rect.height > 0.0) {
                    continue;
                }
                walk.reads += 1;
                let replaced = is_replaced(&el.name.local);
                if walk.excluded(id) || !layout.check_visibility(&dom, id, shown) {
                    continue;
                }
                if !replaced && !paints_own_box(&layout, &dom, id) {
                    continue;
                }
                (id, rect)
            }
            Ok(NodeType::Text) => {
                if !dom.char_data(id).is_ok_and(|d| !d.trim().is_empty()) {
                    continue;
                }
                let Some(parent) = dom.parent_element(id).ok().flatten() else { continue };
                let Some(rect) = layout.text_rect(&dom, id) else { continue };
                if !(rect.width > 0.0 && rect.height > 0.0) {
                    continue;
                }
                walk.reads += 1;
                if walk.excluded(parent) || !layout.check_visibility(&dom, parent, shown) {
                    continue;
                }
                (parent, rect)
            }
            _ => continue,
        };
        if !(rect.width > 0.0 && rect.height > 0.0) {
            continue;
        }
        let (ax, ay) = layout.ancestor_scroll_offset(&dom, id);
        let adjust = (scroll.0 + ax, scroll.1 + ay);
        let clip = walk.clip_inside(owner);
        let rtl = layout.computed_style_value(&dom, owner, "direction") == "rtl";
        let pos = Rect { x: rect.x + adjust.0, y: rect.y + adjust.1, ..rect };
        items.insert(id, Item { node: id, client: rect, pos, adjust, clip, rtl });
    }
    (items, viewport, walk.reads)
}

// ----- entries ----------------------------------------------------------------

/// One attributed node of a shift.
pub struct Source {
    node: NodeRef,
    previous: Rect,
    current: Rect,
}

pub struct ShiftEntry {
    value: f64,
    had_recent_input: bool,
    last_input_time: f64,
    start: f64,
    sources: Vec<Rc<Source>>,
}

pub struct ShiftData {
    entry: Rc<ShiftEntry>,
    base: PerformanceEntryData,
    sources: RefCell<Option<Persistent>>,
}

pub struct LayoutShift;

impl JsClass for LayoutShift {
    const NAME: &'static str = "LayoutShift";
    type Data = ShiftData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &ShiftData| &d.base);
        def.getter("value", |s, d| Ok(s.number(d.entry.value)));
        def.getter("hadRecentInput", |s, d| Ok(s.boolean(d.entry.had_recent_input)));
        def.getter("lastInputTime", |s, d| Ok(s.number(d.entry.last_input_time)));
        def.getter("sources", |s, d| {
            if let Some(cached) = &*d.sources.borrow() {
                return Ok(cached.get(s));
            }
            let mut values = Vec::new();
            for source in &d.entry.sources {
                let data = AttributionData { source: source.clone(), previous: RefCell::new(None), current: RefCell::new(None) };
                values.push(s.wrap::<LayoutShiftAttribution>(data, None)?.value());
            }
            let list = s.frozen_array(&values)?;
            *d.sources.borrow_mut() = Some(Persistent::new(s, list));
            Ok(list)
        });
        def.method("toJSON", 0, |s, d, _a| {
            let e = &d.entry;
            let o = s.object();
            o.set("name", s.string(""))?;
            o.set("entryType", s.string("layout-shift"))?;
            o.set("startTime", s.number(e.start))?;
            o.set("duration", s.number(0.0))?;
            o.set("value", s.number(e.value))?;
            o.set("hadRecentInput", s.boolean(e.had_recent_input))?;
            o.set("lastInputTime", s.number(e.last_input_time))?;
            Ok(o.value())
        });
    }
}

pub struct AttributionData {
    source: Rc<Source>,
    previous: RefCell<Option<Persistent>>,
    current: RefCell<Option<Persistent>>,
}

pub struct LayoutShiftAttribution;

fn rect_of<'s>(s: &Scope<'s>, d: &AttributionData, cell: &RefCell<Option<Persistent>>, rect: Rect) -> JsResult<Value<'s>> {
    if let Some(cached) = &*cell.borrow() {
        return Ok(cached.get(s));
    }
    let Some(page) = d.source.node.page() else { return Ok(s.null()) };
    let value = crate::canvas::rect_value(s, &page, true, rect)?;
    *cell.borrow_mut() = Some(Persistent::new(s, value));
    Ok(value)
}

impl JsClass for LayoutShiftAttribution {
    const NAME: &'static str = "LayoutShiftAttribution";
    type Data = AttributionData;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("node", |s, d| d.source.node.element(s));
        def.getter("previousRect", |s, d| rect_of(s, d, &d.previous, d.source.previous));
        def.getter("currentRect", |s, d| rect_of(s, d, &d.current, d.source.current));
    }
}

struct ShiftTimingEntry(Rc<ShiftEntry>);

impl TimelineEntry for ShiftTimingEntry {
    fn entry_type(&self) -> &'static str {
        "layout-shift"
    }
    fn start_time(&self) -> f64 {
        self.0.start
    }
    fn wrap<'s>(&self, s: &Scope<'s>, key: IdentityKey) -> JsResult<Value<'s>> {
        let data = ShiftData {
            entry: self.0.clone(),
            base: PerformanceEntryData::new("", "layout-shift", self.0.start, 0.0),
            sources: RefCell::new(None),
        };
        Ok(s.wrap::<LayoutShift>(data, Some(key))?.value())
    }
}

// ----- the frame ----------------------------------------------------------------

/// A node that moved between two tracked frames.
struct Moved {
    node: NodeId,
    previous: Rect,
    current: Rect,
    old_visible: Option<Rect>,
    new_visible: Option<Rect>,
    distance: f64,
}

/// The end of a rendering update of `page`'s document; called when ENABLED.
pub fn on_frame(page: &Rc<PageDom>, frame: &FrameInfo) {
    if page.is_hidden() {
        return;
    }
    let (frames, stamp) = with_state(page, |s| (s.frames, s.stamp));
    if !tracking(page, frames) {
        with_state(page, |s| {
            s.prev = FxHashMap::default();
            s.stamp = None;
        });
        return;
    }
    let now_stamp = match (page.layout.try_borrow(), page.dom.try_borrow()) {
        (Ok(layout), Ok(dom)) => layout.observation_stamp(&dom),
        _ => return,
    };
    if stamp == Some(now_stamp) {
        return;
    }
    let (items, viewport, reads) = collect(page);
    let (previous, old_viewport) = with_state(page, |s| {
        s.walks += 1;
        s.style_reads += reads;
        s.stamp = Some(now_stamp);
        s.frames = s.frames.saturating_add(1);
        let old = s.viewport;
        s.viewport = viewport;
        (std::mem::take(&mut s.prev), old)
    });
    // The first tracked frame and one after a resize only set the baseline.
    let entry = if previous.is_empty() || old_viewport != viewport { None } else { diff(page, frame, &previous, &items, viewport) };
    with_state(page, |s| s.prev = items);
    if let Some(entry) = entry {
        queue_entry(page, Rc::new(ShiftTimingEntry(Rc::new(entry))));
    }
}

/// The shift between two tracked frames, if any (see the module docs).
fn diff(page: &Rc<PageDom>, frame: &FrameInfo, previous: &FxHashMap<NodeId, Item>, items: &FxHashMap<NodeId, Item>, viewport: (f64, f64)) -> Option<ShiftEntry> {
    let mut moved: Vec<Moved> = Vec::new();
    for (id, new) in items {
        let Some(old) = previous.get(id) else { continue };
        // Where the old box is in the viewport now (the scroll difference cancelled).
        let before = Rect { x: old.pos.x - new.adjust.0, y: old.pos.y - new.adjust.1, ..old.pos };
        let dx = if new.rtl { new.client.right() - before.right() } else { new.client.left() - before.left() };
        let dy = new.client.top() - before.top();
        if dx.abs() < EPSILON && dy.abs() < EPSILON {
            continue;
        }
        let (old_visible, new_visible) = (visible(before, new.clip), visible(new.client, new.clip));
        if old_visible.is_none() && new_visible.is_none() {
            continue;
        }
        moved.push(Moved { node: new.node, previous: before, current: new.client, old_visible, new_visible, distance: dx.abs().max(dy.abs()) });
    }
    let view_area = viewport.0 * viewport.1;
    if moved.is_empty() || view_area <= 0.0 {
        return None;
    }
    let mut region: Vec<Rect> = Vec::new();
    for m in &moved {
        region.extend(m.old_visible);
        region.extend(m.new_visible);
    }
    let impact = union_area(&region) / view_area;
    let distance = moved.iter().map(|m| m.distance).fold(0.0, f64::max);
    let value = impact * (distance / viewport.0.max(viewport.1)).min(1.0);
    if value.is_nan() || value <= 0.0 {
        return None;
    }
    // Sources: the nodes of largest impact.
    let mut ranked: Vec<(f64, &Moved)> =
        moved.iter().map(|m| (union_area(&[m.old_visible, m.new_visible].into_iter().flatten().collect::<Vec<_>>()), m)).collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let sources: Vec<Rc<Source>> = ranked
        .into_iter()
        .filter(|(area, _)| *area > 0.0)
        .filter_map(|(_, m)| {
            let node = NodeRef::new(page, m.node)?;
            Some(Rc::new(Source { node, previous: m.previous, current: m.current }))
        })
        .take(MAX_SOURCES)
        .collect();
    let timeline = page.timeline();
    Some(ShiftEntry {
        value,
        had_recent_input: timeline.recent_input(frame.start, 500.0),
        last_input_time: timeline.last_input_time().unwrap_or(0.0),
        start: frame.start,
        sources,
    })
}
