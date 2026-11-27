//! The painted-candidate scan shared by `paint`, `lcp`, `element` and `softnav`: what
//! the document shows that those entry types are about (images, text blocks,
//! background images, SVG, canvases, video posters), with the part of it that is
//! visible in the viewport, computed once per frame from layout on demand.
//!
//! # Use
//!
//! ```ignore
//! // in <module>::on_frame(page, frame):
//! let scan = scan::painted_candidates(page, frame);      // Rc<FrameScan>, shared by every module of the frame
//! for c in scan.newly_painted(frame) { ... }             // candidates painted for the first time in this frame
//! for c in &scan.candidates { ... }                      // everything painted now (largest-so-far, re-checks)
//! ```
//!
//! The first caller of a frame pays for the scan, the others get the same `Rc`
//! (memoized by `frame.presented`). Call it only when you will use it: a document
//! nobody observes (`page.timeline().is_observed(..)`, or the first paint not yet
//! reported) must not scan. A scan walks the document; it is skipped (the last
//! result is returned) when layout's `observation_stamp` is unchanged, no image
//! finished loading and the document has no `<canvas>` (a canvas can change
//! without the document changing). [`walks`] counts the walks, for tests that
//! assert work and not time.
//!
//! # What is a candidate
//!
//! A [`Candidate`] is an element (a text block: its block container) that paints
//! something of its own and whose visible rectangle has an area:
//!
//! - `Image`: `<img>` and SVG `<image>` (and `<input type=image>` once the embedder
//!   reports its request like an `<img>`'s) whose request finished and decodes, with its
//!   natural size; `url` is the resolved current source. `VideoPoster`: a
//!   `<video poster>` Resource Timing reports as fetched (no natural size; no frames
//!   are decoded: a poster is painted, a video frame never is).
//! - `BackgroundImage`: an element whose computed `background-image` has a `url()`
//!   (gradients are not candidates, as in the specs). Pyro does not track the load
//!   of CSS images: it is loaded when Resource Timing has a finished entry for the
//!   URL (`load_time` its `responseEnd`); `data:` URLs count as loaded at once.
//! - `Text`: the non-whitespace text nodes of one block container (the nearest
//!   non-inline ancestor): `visible` is the union of their line boxes, each clipped.
//! - `Svg`: an `<svg>` root with a painted shape or text below it. `Canvas`: a
//!   `<canvas>` something was drawn on (2D contexts; there is no WebGL).
//!
//! Not candidates: `display: none`, `visibility: hidden` (the element itself; a
//! child may opt back in), an ancestor with `opacity: 0`, a rectangle clipped away by
//! an ancestor's overflow clip or outside the viewport, zero-area boxes. `visible`
//! is the border box (client coordinates of the document) intersected with the
//! overflow clips of its ancestors and the viewport; transforms are those layout
//! reports (a `scale(0)` box has no area, a translated-away one is outside).
//! Not done: content boxes of replaced elements (`object-fit`), `clip-path`/`mask`
//! clipping, backface visibility, shadow trees (the walk stays in the light tree),
//! subframes (each frame's document scans itself: a frame's content does not count
//! for its embedder's candidates, as for FCP and LCP).
//!
//! # Identity and "first painted"
//!
//! Each candidate has an identity (`node`, kind and `url`): the scan remembers the
//! `presented` time of the first frame in which that identity was painted visible
//! ([`Candidate::first_painted`]). An element that is hidden and reappears is
//! first painted when it reappears; one that is resized or whose text changes
//! keeps its identity (so modified text and resized images are not new, as LCP
//! says); an `<img>` whose `src` changes is a new identity.
//!
//! [`note_image_loaded`] (called from `PageDom::image_loaded`, only while some module
//! needs it) records `load_time`: the document-clock time the image request finished.
//!
//! # Owner
//!
//! `scan.rs` is core's. A lane that needs another fact from it (an LCP rule, an
//! element-timing attribute) adds a field to `Candidate` in a commit of its own and
//! says so in its hand-back; it does not write a second scan.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::layout::Rect;
use pyro_dom::style::CheckVisibilityOptions;
use pyro_dom::{Dom, NodeId, NodeType};
use rustc_hash::FxHashMap;

use crate::page::PageDom;

use super::FrameInfo;

/// Whether some ENABLED module reads scan results (a const: the load-time
/// bookkeeping of [`note_image_loaded`] costs nothing while none does).
pub(crate) const WANTED: bool = super::paint::ENABLED || super::lcp::ENABLED || super::element::ENABLED || super::softnav::ENABLED;

const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// What a candidate paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Text,
    Image,
    VideoPoster,
    BackgroundImage,
    Svg,
    Canvas,
}

/// One painted thing of a frame. See the module docs.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// The element (a text block's container).
    pub node: NodeId,
    pub kind: Kind,
    /// Resolved URL of an image, a poster or a background image.
    pub url: Option<String>,
    /// Natural size in CSS px of an image or poster.
    pub natural: Option<(f64, f64)>,
    /// The border box in client coordinates (text: the union of the line boxes), unclipped.
    pub border_box: Rect,
    /// The visible part of it: clipped by ancestors' overflow and the viewport.
    pub visible: Rect,
    /// `visible.width * visible.height`.
    pub area: f64,
    /// Document-clock time the image finished loading (`None`: unknown, text, svg, canvas).
    pub load_time: Option<f64>,
    /// `presented` of the first frame this identity was painted visible.
    pub first_painted: f64,
}

/// The candidates of one frame, in document order.
#[derive(Debug, Default)]
pub struct FrameScan {
    /// `FrameInfo::presented` of the frame whose scan this is (an unchanged document
    /// returns an earlier one: compare with the frame you are in, see
    /// [`FrameScan::newly_painted`]).
    pub presented: f64,
    /// Viewport size (CSS px) the visible rectangles were clipped to.
    pub viewport: (f64, f64),
    pub candidates: Vec<Candidate>,
}

impl FrameScan {
    /// The candidates first painted in `frame`.
    pub fn newly_painted<'a>(&'a self, frame: &'a FrameInfo) -> impl Iterator<Item = &'a Candidate> + 'a {
        self.candidates.iter().filter(move |c| c.first_painted == frame.presented)
    }
}

type Stamp = ((u64, u64, u32, u32), u64);

#[derive(Default)]
pub(crate) struct ScanState {
    last: Option<Rc<FrameScan>>,
    stamp: Option<Stamp>,
    /// A canvas was seen in the last walk: rescan every frame.
    has_canvas: bool,
    /// The last walk skipped a poster or background image whose load Resource Timing
    /// has not reported yet: rescan next frame.
    waiting: bool,
    /// An image finished loading since the last walk.
    dirty: bool,
    /// `(img, resolved url)` -> time its request finished.
    loaded: FxHashMap<(NodeId, String), f64>,
    /// Identity -> `presented` of the frame it was first painted in.
    first: FxHashMap<(NodeId, Kind, Option<String>), f64>,
    walks: u64,
}

/// An image request of `el` for `src` finished (`PageDom::image_loaded`).
pub(crate) fn note_image_loaded(page: &PageDom, el: NodeId, src: &str) {
    if !WANTED {
        return;
    }
    let time = crate::event::page_now_ms(page);
    let mut state = page.timeline().scan.borrow_mut();
    state.loaded.insert((el, src.to_owned()), time);
    state.dirty = true;
}

/// How many times the document was walked (a counter: tests assert on it, never on time).
pub fn walks(page: &PageDom) -> u64 {
    page.timeline().scan.borrow().walks
}

/// The painted candidates of `page`'s document in `frame`. See the module docs.
pub fn painted_candidates(page: &Rc<PageDom>, frame: &FrameInfo) -> Rc<FrameScan> {
    // Memo: another module already scanned this frame, or nothing changed since.
    let (stamp, force) = {
        let state = page.timeline().scan.borrow();
        if let Some(last) = &state.last {
            if last.presented == frame.presented {
                return last.clone();
            }
        }
        (state.stamp, state.has_canvas || state.dirty || state.waiting)
    };
    let now_stamp = page.layout().observation_stamp(&page.dom.borrow());
    if !force && stamp == Some(now_stamp) {
        let last = page.timeline().scan.borrow().last.clone();
        if let Some(last) = last {
            return last;
        }
    }
    let scan = Rc::new(walk(page, frame));
    let mut state = page.timeline().scan.borrow_mut();
    state.stamp = Some(now_stamp);
    state.dirty = false;
    state.walks += 1;
    state.last = Some(scan.clone());
    scan
}

/// What the first pass of a walk found, before geometry.
enum Found {
    Image(NodeId, Kind),
    Background(NodeId),
    Svg(NodeId),
    Canvas(NodeId),
    Text(NodeId),
}

fn walk(page: &Rc<PageDom>, frame: &FrameInfo) -> FrameScan {
    // Pass 1 (DOM only): which elements and text nodes could paint something.
    let mut found: Vec<Found> = Vec::new();
    {
        let dom = page.dom.borrow();
        let document = page.document();
        let Ok(nodes) = dom.descendants(document) else { return FrameScan { presented: frame.presented, ..Default::default() } };
        for id in nodes {
            match dom.node_type(id) {
                Ok(NodeType::Text) => {
                    if dom.char_data(id).is_ok_and(|d| !d.trim().is_empty()) {
                        found.push(Found::Text(id));
                    }
                }
                Ok(NodeType::Element) => {
                    let Ok(el) = dom.element(id) else { continue };
                    let svg = &*el.name.ns == SVG_NS;
                    match (&*el.name.local, svg) {
                        ("img", false) => found.push(Found::Image(id, Kind::Image)),
                        ("image", true) => found.push(Found::Image(id, Kind::Image)),
                        ("input", false) if el.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("image")) => {
                            found.push(Found::Image(id, Kind::Image))
                        }
                        ("video", false) if el.attr("poster").is_some_and(|p| !p.trim().is_empty()) => {
                            found.push(Found::Image(id, Kind::VideoPoster))
                        }
                        ("canvas", false) => found.push(Found::Canvas(id)),
                        ("svg", true) if dom.parent_element(id).ok().flatten().is_none_or(|p| !is_svg(&dom, p)) => {
                            found.push(Found::Svg(id))
                        }
                        _ => {}
                    }
                    found.push(Found::Background(id));
                }
                _ => {}
            }
        }
    }
    // Pass 2: the sources the loader chose for the images (they read the viewport, so no
    // borrow of ours is held).
    let mut images: FxHashMap<NodeId, ImageInfo> = FxHashMap::default();
    for f in &found {
        if let Found::Image(id, Kind::Image) = f {
            // `complete` first: it adopts an image the embedder never reported (an available one).
            let complete = page.image_complete(*id);
            let request = page.images.borrow().get(id).cloned();
            if let (true, Some((loading, false, true))) = (complete, request) {
                images.insert(*id, ImageInfo { loading, source: crate::image_decode::selected_source(page, *id) });
            }
        }
    }
    // Resource Timing's finished fetches: the load of CSS images and posters, which the
    // loader does not report to the document otherwise.
    page.add_finished_fetches();
    let finished: FxHashMap<String, f64> = page
        .resource_timings()
        .entries()
        .iter()
        .filter(|e| e.response_status < 400 && e.response_end > 0.0)
        .map(|e| (e.name.clone(), e.response_end))
        .collect();
    // Pass 3: geometry.
    let dom = page.dom.borrow();
    let layout = page.layout();
    let viewport = (f64::from(layout.inner_width()), f64::from(layout.inner_height()));
    let view = Rect { x: 0.0, y: 0.0, width: viewport.0, height: viewport.1 };
    let shown = CheckVisibilityOptions { check_visibility: true, check_opacity: true };
    let visible_el = |el: NodeId| layout.check_visibility(&dom, el, shown);
    let mut waiting = false;
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut has_canvas = false;
    let mut texts: Vec<(NodeId, Rect, Rect)> = Vec::new();
    let mut blocks: FxHashMap<NodeId, NodeId> = FxHashMap::default();
    let state = page.timeline().scan.borrow();
    for f in &found {
        match *f {
            Found::Image(el, Kind::VideoPoster) => {
                let Some(poster) = dom.get_attribute(el, "poster").ok().flatten() else { continue };
                let url = crate::html::resolve_url(&dom, el, poster);
                let Some(load_time) = finished.get(&url).copied().or_else(|| url.starts_with("data:").then_some(0.0)) else {
                    waiting = true;
                    continue;
                };
                if !visible_el(el) {
                    continue;
                }
                let border_box = layout.bounding_client_rect(&dom, el);
                let Some(visible) = visible_rect(&dom, &layout, el, border_box, view) else { continue };
                candidates.push(Candidate { node: el, kind: Kind::VideoPoster, url: Some(url), natural: None, border_box, visible, area: visible.width * visible.height, load_time: Some(load_time), first_painted: 0.0 });
            }
            Found::Image(el, kind) => {
                let Some(info) = images.get(&el) else { continue };
                if !visible_el(el) {
                    continue;
                }
                let Some((nw, nh)) = dom.reported_natural_size(el) else { continue };
                let url = info.source.as_ref().map(|s| crate::html::resolve_url(&dom, el, s));
                let load_time = state.loaded.get(&(el, info.loading.clone())).copied();
                let border_box = layout.bounding_client_rect(&dom, el);
                let Some(visible) = visible_rect(&dom, &layout, el, border_box, view) else { continue };
                candidates.push(Candidate { node: el, kind, url, natural: Some((nw, nh)), border_box, visible, area: visible.width * visible.height, load_time, first_painted: 0.0 });
            }
            Found::Background(el) => {
                let value = layout.computed_style_value(&dom, el, "background-image");
                let Some(url) = first_url(&value) else { continue };
                if !visible_el(el) {
                    continue;
                }
                let url = crate::html::resolve_url(&dom, el, &url);
                let load_time = if url.starts_with("data:") { Some(0.0) } else { finished.get(&url).copied() };
                if load_time.is_none() {
                    waiting = true;
                    continue;
                }
                let border_box = layout.bounding_client_rect(&dom, el);
                let Some(visible) = visible_rect(&dom, &layout, el, border_box, view) else { continue };
                candidates.push(Candidate { node: el, kind: Kind::BackgroundImage, url: Some(url), natural: None, border_box, visible, area: visible.width * visible.height, load_time, first_painted: 0.0 });
            }
            Found::Svg(el) => {
                if !visible_el(el) || !has_painted_shape(&dom, el) {
                    continue;
                }
                let border_box = layout.bounding_client_rect(&dom, el);
                let Some(visible) = visible_rect(&dom, &layout, el, border_box, view) else { continue };
                candidates.push(Candidate { node: el, kind: Kind::Svg, url: None, natural: None, border_box, visible, area: visible.width * visible.height, load_time: None, first_painted: 0.0 });
            }
            Found::Canvas(el) => {
                has_canvas = true;
                if !visible_el(el) || !canvas_drawn(page, el) {
                    continue;
                }
                let border_box = layout.bounding_client_rect(&dom, el);
                let Some(visible) = visible_rect(&dom, &layout, el, border_box, view) else { continue };
                candidates.push(Candidate { node: el, kind: Kind::Canvas, url: None, natural: None, border_box, visible, area: visible.width * visible.height, load_time: None, first_painted: 0.0 });
            }
            Found::Text(text) => {
                let Some(parent) = dom.parent_element(text).ok().flatten() else { continue };
                if !visible_el(parent) {
                    continue;
                }
                let Some(line_box) = layout.text_rect(&dom, text) else { continue };
                let Some(visible) = visible_rect_from(&dom, &layout, Some(parent), line_box, view) else { continue };
                let block = *blocks.entry(parent).or_insert_with(|| block_container(&dom, &layout, parent));
                texts.push((block, line_box, visible));
            }
        }
    }
    // Text: one candidate per block container, the union of its text's visible boxes.
    let mut by_block: Vec<(NodeId, Rect, Rect)> = Vec::new();
    let mut slot: FxHashMap<NodeId, usize> = FxHashMap::default();
    for (block, line_box, visible) in texts {
        match slot.get(&block) {
            Some(&i) => {
                by_block[i].1 = union(by_block[i].1, line_box);
                by_block[i].2 = union(by_block[i].2, visible);
            }
            None => {
                slot.insert(block, by_block.len());
                by_block.push((block, line_box, visible));
            }
        }
    }
    for (block, border_box, visible) in by_block {
        candidates.push(Candidate { node: block, kind: Kind::Text, url: None, natural: None, border_box, visible, area: visible.width * visible.height, load_time: None, first_painted: 0.0 });
    }
    // Document order of the elements (text blocks come where their container does).
    drop(state);
    drop(layout);
    let order: FxHashMap<NodeId, usize> = dom
        .descendants(page.document())
        .map(|nodes| nodes.enumerate().map(|(i, n)| (n, i)).collect::<FxHashMap<NodeId, usize>>())
        .unwrap_or_default();
    candidates.sort_by_key(|c| order.get(&c.node).copied().unwrap_or(usize::MAX));
    drop(dom);
    // First painted: remember the frame an identity was first seen painted in.
    let mut state = page.timeline().scan.borrow_mut();
    state.has_canvas = has_canvas;
    state.waiting = waiting;
    state.first.retain(|(node, _, _), _| page.dom.borrow().is_alive(*node));
    state.loaded.retain(|(node, _), _| page.dom.borrow().is_alive(*node));
    for c in &mut candidates {
        let key = (c.node, c.kind, c.url.clone());
        c.first_painted = *state.first.entry(key).or_insert(frame.presented);
    }
    FrameScan { presented: frame.presented, viewport, candidates }
}

/// What pass 2 learned of a loaded `<img>`: its request's `src` and the source the loader chose.
struct ImageInfo {
    loading: String,
    source: Option<String>,
}

/// Whether something was drawn on the 2D canvas `el` (`pyro_canvas::Surface::is_allocated`).
fn canvas_drawn(page: &PageDom, el: NodeId) -> bool {
    let list = page.canvas_surfaces.borrow();
    for (canvas, surface, _, _) in list.iter() {
        if *canvas != el {
            continue;
        }
        let Some(surface) = surface.upgrade() else { continue };
        let drawn = surface.try_borrow().is_ok_and(|s| s.is_allocated());
        if drawn {
            return true;
        }
    }
    false
}

fn is_svg(dom: &Dom, el: NodeId) -> bool {
    dom.element(el).is_ok_and(|e| &*e.name.ns == SVG_NS)
}

/// An `<svg>` with something below it that paints (shapes, text, images, `<use>`).
fn has_painted_shape(dom: &Dom, svg: NodeId) -> bool {
    let Ok(nodes) = dom.descendants(svg) else { return false };
    nodes.skip(1).any(|n| {
        dom.element(n).is_ok_and(|e| {
            &*e.name.ns == SVG_NS
                && matches!(&*e.name.local, "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "text" | "image" | "use" | "foreignObject")
                && e.attr("fill").is_none_or(|f| f != "none")
        })
    })
}

/// The first `url(...)` of a computed `background-image` (gradients have none).
fn first_url(value: &str) -> Option<String> {
    let start = value.find("url(")? + 4;
    let rest = value[start..].trim_start();
    let (quote, rest) = match rest.chars().next()? {
        q @ ('"' | '\'') => (Some(q), &rest[1..]),
        _ => (None, rest),
    };
    let end = match quote {
        Some(q) => rest.find(q)?,
        None => rest.find(')')?,
    };
    Some(rest[..end].trim().to_owned()).filter(|u| !u.is_empty())
}

/// The nearest ancestor-or-self whose `display` is not inline-level flow (text's block container).
fn block_container(dom: &Dom, layout: &pyro_dom::layout::Layout, mut el: NodeId) -> NodeId {
    loop {
        let display = layout.computed_style_value(dom, el, "display");
        if !matches!(display.as_str(), "inline" | "contents") {
            return el;
        }
        match dom.parent_element(el).ok().flatten() {
            Some(parent) => el = parent,
            None => return el,
        }
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let (left, top) = (a.left().min(b.left()), a.top().min(b.top()));
    let (right, bottom) = (a.right().max(b.right()), a.bottom().max(b.bottom()));
    Rect { x: left, y: top, width: right - left, height: bottom - top }
}

/// The intersection with positive area.
fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let (left, top) = (a.left().max(b.left()), a.top().max(b.top()));
    let (right, bottom) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    (right > left && bottom > top).then_some(Rect { x: left, y: top, width: right - left, height: bottom - top })
}

/// `rect` of `el`'s box clipped by the overflow clips of the ancestors of `el` and the viewport.
fn visible_rect(dom: &Dom, layout: &pyro_dom::layout::Layout, el: NodeId, rect: Rect, view: Rect) -> Option<Rect> {
    visible_rect_from(dom, layout, dom.parent_element(el).ok().flatten(), rect, view)
}

/// [`visible_rect`] starting the clip chain at `start` (inclusive).
fn visible_rect_from(dom: &Dom, layout: &pyro_dom::layout::Layout, start: Option<NodeId>, rect: Rect, view: Rect) -> Option<Rect> {
    if !(rect.width > 0.0 && rect.height > 0.0) {
        return None;
    }
    let mut rect = rect;
    let mut ancestor = start;
    while let Some(node) = ancestor {
        // The viewport takes the root containers' overflow.
        if !layout.is_root_container(dom, node) {
            let axes = layout.style().overflow_clip_axes(dom, node);
            if axes.x || axes.y {
                let b = layout.bounding_client_rect(dom, node);
                let clip = Rect {
                    x: if axes.x { b.x } else { f64::MIN / 4.0 },
                    y: if axes.y { b.y } else { f64::MIN / 4.0 },
                    width: if axes.x { b.width } else { f64::MAX / 2.0 },
                    height: if axes.y { b.height } else { f64::MAX / 2.0 },
                };
                rect = intersect(rect, clip)?;
            }
        }
        ancestor = dom.parent_element(node).ok().flatten();
    }
    intersect(rect, view)
}
